//! Uniform loading of local paths, http(s):// and file:// URLs with a
//! timeout, a size cap and an on-disk cache for remote documents.
//!
//! Rust port of `internal/fetch/fetch.go`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// MaxBytes caps http response bodies (and thus schema documents).
/// Exported so tests can shrink it.
pub static MAX_BYTES: AtomicU64 = AtomicU64::new(32 << 20);

/// CacheTTL is how long a cached remote document is served without asking
/// the origin again (seconds). An expired entry is revalidated with a
/// conditional request. Exported so tests can shrink it.
pub static CACHE_TTL_SECS: AtomicU64 = AtomicU64::new(24 * 60 * 60);

/// CacheDir overrides the cache root; empty means $JSE_CACHE_DIR if set,
/// else <user cache dir>/jse. Exported so tests can isolate it.
#[doc(hidden)]
pub(crate) static CACHE_DIR_OVERRIDE: parking_lot::Mutex<Option<String>> =
    parking_lot::Mutex::new(None);

/// memo keeps the documents loaded in this process: the same URL is fetched
/// once even though the schema is loaded for the form model and compiled for
/// validation.
static MEMO: LazyLock<parking_lot::Mutex<HashMap<String, MemoEntry>>> =
    LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

struct MemoEntry {
    data: Vec<u8>,
    note: String,
    /// A failed load is memoized too: a multi-file startup must not retry the
    /// same unreachable/rate-limited URL once per file.
    error: Option<String>,
}

/// URLs with a stale-while-revalidate refresh in flight, so several session
/// files or picker opens share one background request.
static REFRESHING: LazyLock<parking_lot::Mutex<HashSet<String>>> =
    LazyLock::new(|| parking_lot::Mutex::new(HashSet::new()));

fn memo() -> &'static parking_lot::Mutex<HashMap<String, MemoEntry>> {
    &MEMO
}

/// Drops the per-process memo, simulating a fresh jse run (test surface).
#[cfg(test)]
pub fn reset_memo() {
    memo().lock().clear();
}

/// Sets the cache root override (test surface); empty restores the default
/// resolution order.
#[cfg(test)]
pub fn set_cache_dir(dir: Option<String>) {
    *CACHE_DIR_OVERRIDE.lock() = dir;
}

/// TEST_LOCK serializes tests that touch the process-global cache root and
/// memo: fetch_tests and the remote-$ref tests in smodel both use it.
#[cfg(test)]
pub(crate) static TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Note returns the user-facing note about how the last Load of loc was
/// served (a "⚠ using the cached schema" note when the origin was
/// unreachable and the cached copy was used), or "" for a live fetch, a
/// cache hit or a local file.
pub fn note(loc: &str) -> String {
    memo()
        .lock()
        .get(loc)
        .map(|e| e.note.clone())
        .unwrap_or_default()
}

/// Agents are cached per proxy URI ("" = direct): the proxy decision is
/// per-request (NO_PROXY), and rebuilding an agent per request would drop
/// the connection pool.
static AGENTS: LazyLock<parking_lot::Mutex<HashMap<String, ureq::Agent>>> =
    LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

/// hostOf returns the host part of an absolute http(s) URL.
#[doc(hidden)]
pub(crate) fn host_of(loc: &str) -> &str {
    let rest = loc.split_once("://").map(|(_, r)| r).unwrap_or(loc);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    // strip userinfo and port
    let after_at = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    // a bracketed IPv6 host keeps its colons; the port follows the ']'
    if let Some(close) = after_at.find(']') {
        return &after_at[..=close];
    }
    match after_at.rsplit_once(':') {
        Some((h, _)) => h,
        None => after_at,
    }
}

/// noProxyMatch reports whether host is exempted by NO_PROXY/no_proxy.
/// Mirrors Go's ProxyFromEnvironment matching: "*" matches everything, a
/// leading dot or "*.", a bare suffix matches subdomains, else exact host
/// (case-insensitive); entries may carry a port, which is ignored here.
fn no_proxy_match(host: &str) -> bool {
    let Ok(list) = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    list.split(',').any(|raw| {
        let pat = raw.trim().to_ascii_lowercase();
        if pat.is_empty() {
            return false;
        }
        if pat == "*" {
            return true;
        }
        let pat = pat
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(&pat)
            .trim_start_matches('.')
            .to_string();
        let pat = pat.strip_prefix("*.").map(|p| p.to_string()).unwrap_or(pat);
        host == pat || host.ends_with(&format!(".{pat}"))
    })
}

/// proxyFor returns the proxy URI for loc: HTTPS_PROXY/https_proxy, then
/// HTTP_PROXY/http_proxy (Go's ProxyFromEnvironment order), then the
/// curl-style ALL_PROXY/all_proxy as a last resort, honoring NO_PROXY.
#[doc(hidden)]
pub(crate) fn proxy_for(loc: &str) -> Option<String> {
    if no_proxy_match(host_of(loc)) {
        return None;
    }
    const VARS: &[&str] = &[
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    for var in VARS {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// agentFor returns the cached client for the request's proxy setting.
fn agent_for(loc: &str) -> ureq::Agent {
    let key = proxy_for(loc).unwrap_or_default();
    if let Some(a) = AGENTS.lock().get(&key) {
        return a.clone();
    }
    let mut config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(15)));
    if !key.is_empty() {
        if let Ok(proxy) = ureq::Proxy::new(&key) {
            config = config.proxy(Some(proxy));
        }
    }
    let agent = config.build().new_agent();
    AGENTS.lock().insert(key, agent.clone());
    agent
}

/// CanonicalLoc normalizes a schema location to an absolute URL:
/// http(s):// and file:// pass through, bare paths become file://<abs>.
pub fn canonical_loc(loc: &str) -> Result<String, String> {
    if is_http(loc) {
        return Ok(loc.to_string());
    }
    let (scheme, rest) = match loc.split_once(':') {
        Some((s, r)) if !r.is_empty() && !s.contains(['/', '\\']) => (s.to_string(), r.to_string()),
        _ => (String::new(), loc.to_string()),
    };
    match scheme.as_str() {
        "http" | "https" | "file" => {
            // URL form validation mirrors Go's url.Parse + String round-trip:
            // the passthrough keeps the location as written.
            if scheme == "file" && !loc.starts_with("file://") {
                return Err(format!("unsupported schema location {loc:?}"));
            }
            Ok(loc.to_string())
        }
        "" => {
            let abs = std::fs::canonicalize(&rest)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .to_string();
            Ok(format!("file://{abs}"))
        }
        other => {
            let _ = other;
            Err(format!("unsupported schema location {loc:?}"))
        }
    }
}

fn is_http(loc: &str) -> bool {
    loc.starts_with("http://") || loc.starts_with("https://")
}

/// Load returns the bytes at loc:
///   - http(s):// → GET through the cache (fresh entries skip the request, a
///     stale one is revalidated and an unreachable origin falls back to the
///     cached copy); non-2xx → "GET <loc>: status <code>"
///   - file:// → path taken from the URL
///   - anything else → treated as a local filesystem path
///
/// HTTP bodies larger than MaxBytes fail with "<loc>: response exceeds size
/// limit".
pub fn load(loc: &str) -> Result<Vec<u8>, String> {
    if is_http(loc) {
        if let Some(e) = memo().lock().get(loc) {
            if let Some(err) = &e.error {
                return Err(err.clone());
            }
            return Ok(e.data.clone());
        }
        match load_http(loc) {
            Ok((data, note)) => {
                memo().lock().insert(
                    loc.to_string(),
                    MemoEntry {
                        data: data.clone(),
                        note,
                        error: None,
                    },
                );
                Ok(data)
            }
            Err(err) => {
                memo().lock().insert(
                    loc.to_string(),
                    MemoEntry {
                        data: Vec::new(),
                        note: String::new(),
                        error: Some(err.clone()),
                    },
                );
                Err(err)
            }
        }
    } else {
        let path = if let Some(rest) = loc.strip_prefix("file://") {
            rest
        } else {
            loc
        };
        std::fs::read(path).map_err(|e| e.to_string())
    }
}

/// LoadStaleFirst is the catalog's loader: a cached copy is returned
/// immediately even after the 24h TTL, and the origin is revalidated in the
/// background for the next read. Without any cached copy it falls back to the
/// synchronous [load] (there is nothing to show while the network hangs).
///
/// Use this only for data where a slightly old copy is harmless (the
/// SchemaStore catalog). Schemas themselves keep the normal fresh/ revalidate
/// / fallback behavior.
pub fn load_stale_first(loc: &str) -> Result<Vec<u8>, String> {
    if !is_http(loc) {
        return load(loc);
    }
    if let Some(e) = memo().lock().get(loc) {
        if let Some(err) = &e.error {
            return Err(err.clone());
        }
        return Ok(e.data.clone());
    }
    let (cached, body) = read_cache(loc);
    let Some(entry) = cached else {
        return load(loc); // cold cache: the only data must come from the origin
    };
    let fresh = SystemTime::now()
        .duration_since(entry.fetched_at)
        .map(|age| age.as_secs() < CACHE_TTL_SECS.load(Ordering::Relaxed))
        .unwrap_or(false);
    memo().lock().insert(
        loc.to_string(),
        MemoEntry {
            data: body.clone(),
            note: String::new(),
            error: None,
        },
    );
    if !fresh {
        refresh_async(loc);
    }
    Ok(body)
}

/// refreshAsync revalidates loc on a detached thread; the cache and the
/// process memo are updated when it finishes, so the next catalog read (a
/// second file, the S picker, or the next run) sees the new copy.
fn refresh_async(loc: &str) {
    if !REFRESHING.lock().insert(loc.to_string()) {
        return;
    }
    let url = loc.to_string();
    let spawned = std::thread::Builder::new()
        .name("jse-catalog-refresh".to_string())
        .spawn({
            let url = url.clone();
            move || {
                if let Ok((data, note)) = load_http(&url) {
                    memo().lock().insert(
                        url.clone(),
                        MemoEntry {
                            data,
                            note,
                            error: None,
                        },
                    );
                }
                REFRESHING.lock().remove(&url);
            }
        });
    if spawned.is_err() {
        REFRESHING.lock().remove(&url);
    }
}

/// loadHTTP serves loc from the cache when fresh, else revalidates it.
fn load_http(loc: &str) -> Result<(Vec<u8>, String), String> {
    let (cached, body) = read_cache(loc);
    let max_ttl = CACHE_TTL_SECS.load(Ordering::Relaxed);
    if let Some(c) = &cached {
        if let Ok(age) = SystemTime::now().duration_since(c.fetched_at) {
            if age.as_secs() < max_ttl {
                return Ok((body, String::new())); // fresh: no request at all
            }
        }
    }

    // A 429 cooldown persists across runs: keep serving the stale catalog
    // instead of hammering the rate-limited origin on every startup.
    if let Some(until) = read_cooldown(loc)
        && let Ok(wait) = until.duration_since(SystemTime::now())
    {
        let secs = wait.as_secs().max(1);
        if cached.is_some() {
            let note = format!(
                "{} using the cached schema: rate limited, retry in {secs}s",
                crate::icons::WARN
            );
            return Ok((body, note));
        }
        return Err(format!("{loc}: rate limited; retry in {secs}s"));
    }

    let mut req = agent_for(loc).get(loc);
    if let Some(c) = &cached {
        if !c.etag.is_empty() {
            req = req.header("If-None-Match", &c.etag);
        }
        if !c.last_modified.is_empty() {
            req = req.header("If-Modified-Since", &c.last_modified);
        }
    }
    let mut resp = match req.call() {
        Ok(r) => r,
        Err(e) => return stale_or_err(loc, &body, cached.is_some(), &e.to_string()),
    };
    let status = resp.status().as_u16();

    if status == 304 && cached.is_some() {
        let mut c = cached.unwrap();
        if let Some(etag) = resp.headers().get("ETag").and_then(|v| v.to_str().ok()) {
            c.etag = etag.to_string();
        }
        if let Some(lm) = resp
            .headers()
            .get("Last-Modified")
            .and_then(|v| v.to_str().ok())
        {
            c.last_modified = lm.to_string();
        }
        c.fetched_at = SystemTime::now();
        write_entry(loc, &c);
        return Ok((body, String::new()));
    }
    if !(200..=299).contains(&status) {
        if status == 429 {
            // Rate limiting is expected when the catalog is refreshed by
            // several starts in a row: back off for Retry-After (or a
            // minute) and keep using the cached copy.
            let secs = retry_after_secs(
                resp.headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok()),
            );
            write_cooldown(loc, SystemTime::now() + Duration::from_secs(secs));
            if cached.is_some() {
                let note = format!(
                    "{} using the cached schema: GET {loc}: status 429",
                    crate::icons::WARN
                );
                return Ok((body, note));
            }
            return Err(format!(
                "GET {loc}: rate limited (status 429); retry in {secs}s"
            ));
        }
        // Origin-side failures fall back to the cached copy; other statuses
        // (404 and friends) stay hard errors so a wrong URL is not masked.
        if cached.is_some() && status >= 500 {
            let note = format!(
                "{} using the cached schema: GET {loc}: status {status}",
                crate::icons::WARN
            );
            return Ok((body, note));
        }
        return Err(format!("GET {loc}: status {status}"));
    }

    let max = MAX_BYTES.load(Ordering::Relaxed);
    let data = match resp.body_mut().with_config().limit(max + 1).read_to_vec() {
        Ok(d) => d,
        Err(e) => {
            // a body over the limit is a size error, not a network error
            let msg = e.to_string();
            if msg.contains("larger than request limit") {
                return Err(format!("{loc}: response exceeds size limit"));
            }
            return stale_or_err(loc, &body, cached.is_some(), &msg);
        }
    };
    if data.len() as u64 > max {
        return Err(format!("{loc}: response exceeds size limit"));
    }
    store_body(loc, &data);
    let etag = resp
        .headers()
        .get("ETag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let last_modified = resp
        .headers()
        .get("Last-Modified")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    write_entry(
        loc,
        &CacheEntry {
            url: loc.to_string(),
            etag,
            last_modified,
            fetched_at: SystemTime::now(),
        },
    );
    Ok((data, String::new()))
}

/// defaultRateLimitCooldown is used when a 429 carries no usable Retry-After.
const DEFAULT_RATE_LIMIT_COOLDOWN_SECS: u64 = 60;

/// retryAfterSecs reads Retry-After's delta-seconds form, clamped to a
/// sensible range; HTTP-date values fall back to the default.
fn retry_after_secs(value: Option<&str>) -> u64 {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|s| s.clamp(1, 24 * 60 * 60))
        .unwrap_or(DEFAULT_RATE_LIMIT_COOLDOWN_SECS)
}

/// cooldownPath is the rate-limit stamp beside the cache entry.
fn cooldown_path(loc: &str) -> Option<PathBuf> {
    cache_path(loc).map(|p| p.with_extension("cooldown"))
}

/// readCooldown returns the instant before which loc must not be requested.
fn read_cooldown(loc: &str) -> Option<SystemTime> {
    let p = cooldown_path(loc)?;
    let secs: u64 = std::fs::read_to_string(p).ok()?.trim().parse().ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// writeCooldown persists the no-request window (best effort, like the cache).
fn write_cooldown(loc: &str, until: SystemTime) {
    let Some(p) = cooldown_path(loc) else { return };
    let Ok(until) = until.duration_since(UNIX_EPOCH) else {
        return;
    };
    write_file_atomic(&p, until.as_secs().to_string().as_bytes());
}

/// staleOrErr yields the cached copy when there is one, else the error.
fn stale_or_err(
    loc: &str,
    body: &[u8],
    has_cache: bool,
    err: &str,
) -> Result<(Vec<u8>, String), String> {
    if has_cache {
        let note = format!("{} using the cached schema: {err}", crate::icons::WARN);
        return Ok((body.to_vec(), note));
    }
    let _ = loc;
    Err(err.to_string())
}

// --- cache files ---------------------------------------------------------

/// entry is the cache sidecar of one remote document.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    url: String,
    #[serde(default)]
    etag: String,
    #[serde(default)]
    last_modified: String,
    #[serde(with = "timestamp")]
    fetched_at: SystemTime,
}

/// timestamp keeps `fetched_at` readable across implementations: the Go
/// version stored a `time.Time` as an RFC3339 string, while serde's
/// `SystemTime` form is a `{secs_since_epoch, nanos_since_epoch}` struct.
/// Both are accepted on read; writes use RFC3339 so a Go build can still
/// consume a cache written by this one. Without this, upgrading from the Go
/// binary silently orphaned the cached SchemaStore catalog: an unreachable
/// origin then aborted startup instead of opening the schema picker.
pub(crate) mod timestamp {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format_rfc3339(*t))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Rfc3339(String),
            Legacy {
                secs_since_epoch: u64,
                nanos_since_epoch: u32,
            },
        }
        match Raw::deserialize(d)? {
            Raw::Rfc3339(s) => parse_rfc3339(&s).map_err(serde::de::Error::custom),
            Raw::Legacy {
                secs_since_epoch,
                nanos_since_epoch,
            } => Ok(UNIX_EPOCH + Duration::new(secs_since_epoch, nanos_since_epoch)),
        }
    }

    /// formatRFC3339 renders UTC with up to nanosecond precision (Go's
    /// RFC3339Nano layout omits trailing zero fractions).
    pub(crate) fn format_rfc3339(t: SystemTime) -> String {
        let (secs, nanos) = unix_parts(t);
        let days = secs.div_euclid(86_400);
        let sod = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        let (h, mi, s) = (sod / 3_600, (sod % 3_600) / 60, sod % 60);
        let mut out = format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}");
        if nanos != 0 {
            out.push('.');
            let frac = format!("{nanos:09}");
            out.push_str(frac.trim_end_matches('0'));
        }
        out.push('Z');
        out
    }

    /// parseRFC3339 accepts Go's `time.Time` JSON form: an offset (`Z` or
    /// ±HH:MM) and an optional fractional second.
    pub(crate) fn parse_rfc3339(s: &str) -> Result<SystemTime, String> {
        let bad = || format!("invalid timestamp {s:?}");
        let (date, rest) = s.split_once('T').ok_or_else(bad)?;
        let mut dp = date.split('-');
        let y: i64 = dp.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let mo: i64 = dp.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let da: i64 = dp.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        if dp.next().is_some() || !(1..=12).contains(&mo) || !(1..=31).contains(&da) {
            return Err(bad());
        }
        let (time, offset_min) = match rest.strip_suffix(['Z', 'z']) {
            Some(t) => (t, 0i64),
            None => {
                let at = rest.rfind(['+', '-']).ok_or_else(bad)?;
                let (t, off) = rest.split_at(at);
                if off.len() != 6 || off.as_bytes()[3] != b':' {
                    return Err(bad());
                }
                let sign = if off.starts_with('-') { -1 } else { 1 };
                let oh: i64 = off[1..3].parse().map_err(|_| bad())?;
                let om: i64 = off[4..6].parse().map_err(|_| bad())?;
                (t, sign * (oh * 60 + om))
            }
        };
        let (hms, frac) = match time.split_once('.') {
            Some((a, b)) => (a, b),
            None => (time, ""),
        };
        let mut tp = hms.split(':');
        let h: i64 = tp.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let mi: i64 = tp.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let se: i64 = tp.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        if tp.next().is_some()
            || !(0..24).contains(&h)
            || !(0..60).contains(&mi)
            || !(0..=60).contains(&se)
            || !frac.chars().all(|c| c.is_ascii_digit())
        {
            return Err(bad());
        }
        let mut nanos = 0u32;
        for (i, c) in frac.chars().take(9).enumerate() {
            nanos += (c as u32 - b'0' as u32) * 10u32.pow(8 - i as u32);
        }
        let se = se.min(59); // a leap second is not representable in SystemTime
        let secs = days_from_civil(y, mo, da) * 86_400 + h * 3_600 + mi * 60 + se - offset_min * 60;
        Ok(if secs >= 0 {
            UNIX_EPOCH + Duration::new(secs as u64, nanos)
        } else {
            UNIX_EPOCH - Duration::new((-secs) as u64, 0) + Duration::new(0, nanos)
        })
    }

    /// unixParts splits a SystemTime into Unix seconds and nanoseconds.
    fn unix_parts(t: SystemTime) -> (i64, u32) {
        match t.duration_since(UNIX_EPOCH) {
            Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
            Err(e) => {
                let d = e.duration();
                if d.subsec_nanos() == 0 {
                    (-(d.as_secs() as i64), 0)
                } else {
                    (-(d.as_secs() as i64) - 1, 1_000_000_000 - d.subsec_nanos())
                }
            }
        }
    }

    /// daysFromCivil returns days since 1970-01-01 (Hinnant's algorithm).
    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// civilFromDays is the inverse of daysFromCivil.
    fn civil_from_days(z: i64) -> (i64, i64, i64) {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (if m <= 2 { y + 1 } else { y }, m, d)
    }
}

/// cacheDir resolves the cache root, None when caching is unavailable.
fn cache_dir() -> Option<PathBuf> {
    if let Some(d) = CACHE_DIR_OVERRIDE.lock().as_deref() {
        if d.is_empty() {
            return None;
        }
        return Some(PathBuf::from(d));
    }
    if let Ok(d) = std::env::var("JSE_CACHE_DIR") {
        if !d.is_empty() {
            return Some(PathBuf::from(d));
        }
    }
    if let Some(d) = dirs::cache_dir() {
        return Some(d.join("jse"));
    }
    None
}

/// cachePath returns the entry stem for loc, None when caching is unavailable.
#[doc(hidden)]
pub(crate) fn cache_path(loc: &str) -> Option<PathBuf> {
    let dir = cache_dir()?;
    let sum = sha256(loc.as_bytes());
    Some(dir.join("fetch").join(hex(&sum)))
}

/// readCache returns the cached entry and body of loc (None when absent,
/// unreadable or not matching loc).
fn read_cache(loc: &str) -> (Option<CacheEntry>, Vec<u8>) {
    let Some(p) = cache_path(loc) else {
        return (None, Vec::new());
    };
    let Ok(mb) = std::fs::read(p.with_extension("json")) else {
        return (None, Vec::new());
    };
    let Ok(e) = serde_json::from_slice::<CacheEntry>(&mb) else {
        return (None, Vec::new());
    };
    if e.url != loc {
        return (None, Vec::new());
    }
    match std::fs::read(p.with_extension("body")) {
        Ok(body) => (Some(e), body),
        Err(_) => (None, Vec::new()),
    }
}

/// storeBody writes the document body; the sidecar is written by writeEntry
/// afterwards, so a body without a sidecar counts as a miss.
fn store_body(loc: &str, body: &[u8]) {
    if let Some(p) = cache_path(loc) {
        write_file_atomic(&p.with_extension("body"), body);
    }
}

/// writeEntry writes the cache sidecar of loc.
fn write_entry(loc: &str, e: &CacheEntry) {
    let Some(p) = cache_path(loc) else { return };
    let Ok(data) = serde_json::to_vec(e) else {
        return;
    };
    write_file_atomic(&p.with_extension("json"), &data);
}

/// writeFileAtomic writes data to path via a temp file + rename (concurrent
/// jse instances), best-effort: any failure leaves the cache untouched.
fn write_file_atomic(path: &Path, data: &[u8]) {
    let dir = match path.parent() {
        Some(d) => d,
        None => return,
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if std::fs::write(&tmp, data).is_err() {
        return;
    }
    let _ = set_mode_0600(&tmp);
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(unix)]
fn set_mode_0600(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_mode_0600(_p: &Path) -> std::io::Result<()> {
    Ok(())
}

/// SHA-256 (FIPS 180-4) — small, dependency-free implementation for cache
/// keys.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bitlen = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        // echo -n "" | sha256sum
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // echo -n "abc" | sha256sum
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn canonical_loc_shapes() {
        assert_eq!(canonical_loc("http://x/y.json").unwrap(), "http://x/y.json");
        assert_eq!(
            canonical_loc("https://x/y.json").unwrap(),
            "https://x/y.json"
        );
        assert!(canonical_loc("ftp://x").is_err());
        let abs = canonical_loc("Cargo.toml").unwrap();
        assert!(abs.starts_with("file:///"), "{abs}");
        assert!(abs.ends_with("Cargo.toml"));
    }
}
