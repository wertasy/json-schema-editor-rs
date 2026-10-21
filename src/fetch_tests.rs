//! Ports of internal/fetch/cache_test.go and fetch_test.go (tiny_http stands
//! in for Go's httptest). The global cache knobs are shared across tests, so
//! every cache test holds TEST_LOCK to run serially.

use crate::fetch::{
    CACHE_DIR_OVERRIDE, CACHE_TTL_SECS, MAX_BYTES, cache_path, load, note, reset_memo,
    set_cache_dir,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use crate::fetch::TEST_LOCK;

struct CacheTest {
    old_dir: Option<String>,
    _dir: tempfile::TempDir,
    _guard: parking_lot::MutexGuard<'static, ()>,
}

impl CacheTest {
    fn new() -> CacheTest {
        let guard = TEST_LOCK.lock();
        let old_dir = CACHE_DIR_OVERRIDE.lock().clone();
        let dir = tempfile::TempDir::new().unwrap();
        set_cache_dir(Some(dir.path().to_string_lossy().to_string()));
        CACHE_TTL_SECS.store(24 * 60 * 60, Ordering::Relaxed);
        MAX_BYTES.store(32 << 20, Ordering::Relaxed);
        reset_memo();
        CacheTest {
            old_dir,
            _dir: dir,
            _guard: guard,
        }
    }

    /// newProcess drops the per-process memo, simulating a fresh jse run.
    fn new_process(&self) {
        reset_memo();
    }
}

impl Drop for CacheTest {
    fn drop(&mut self) {
        set_cache_dir(self.old_dir.clone());
        reset_memo();
    }
}

/// schemaServer serves body with an ETag, counting hits and conditionals.
fn schema_server(
    body: &'static str,
    etag: &'static str,
    hits: Arc<AtomicI32>,
    conds: Arc<AtomicI32>,
) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        loop {
            let Ok(request) = server.recv() else { break };
            hits.fetch_add(1, Ordering::SeqCst);
            let inm = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("If-None-Match"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            let mut response: tiny_http::Response<std::io::Cursor<Vec<u8>>> = if inm == etag {
                conds.fetch_add(1, Ordering::SeqCst);
                tiny_http::Response::from_data(Vec::new()).with_status_code(304)
            } else {
                tiny_http::Response::from_string(body)
            };
            response
                .add_header(tiny_http::Header::from_bytes(&b"ETag"[..], etag.as_bytes()).unwrap());
            let _ = request.respond(response);
        }
    });
    url
}

fn counter() -> Arc<AtomicI32> {
    Arc::new(AtomicI32::new(0))
}

#[test]
fn load_http_unchanged() {
    let ct = CacheTest::new();
    let (hits, conds) = (counter(), counter());
    let url = format!(
        "{}/ok",
        schema_server(r#"{"a":1}"#, "\"v1\"", hits.clone(), conds.clone())
    );

    let data = load(&url).unwrap();
    assert_eq!(data, br#"{"a":1}"#);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(conds.load(Ordering::SeqCst), 0);
    assert_eq!(note(&url), "");
    drop(ct);
}

#[test]
fn http_memo_per_process() {
    let ct = CacheTest::new();
    let (hits, conds) = (counter(), counter());
    let url = format!(
        "{}/ok",
        schema_server(r#"{"a":1}"#, "\"v1\"", hits.clone(), conds.clone())
    );
    for _ in 0..3 {
        load(&url).unwrap();
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1, "memoized");
    drop(ct);
}

#[test]
fn cache_fresh_skips_network() {
    let ct = CacheTest::new();
    let (hits, conds) = (counter(), counter());
    let url = format!(
        "{}/ok",
        schema_server(r#"{"a":1}"#, "\"v1\"", hits.clone(), conds.clone())
    );

    load(&url).unwrap();
    ct.new_process(); // next jse run
    let data = load(&url).unwrap();
    assert_eq!(data, br#"{"a":1}"#);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "fresh cache must not hit the origin"
    );
    drop(ct);
}

#[test]
fn cache_revalidates() {
    let ct = CacheTest::new();
    let (hits, conds) = (counter(), counter());
    let url = format!(
        "{}/ok",
        schema_server(r#"{"a":1}"#, "\"v1\"", hits.clone(), conds.clone())
    );

    load(&url).unwrap();
    ct.new_process();
    CACHE_TTL_SECS.store(0, Ordering::Relaxed); // force revalidation
    let data = load(&url).unwrap();
    assert_eq!(data, br#"{"a":1}"#);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(conds.load(Ordering::SeqCst), 1);

    // A 304 refreshes the entry: with a TTL again, no further request.
    ct.new_process();
    CACHE_TTL_SECS.store(24 * 60 * 60, Ordering::Relaxed);
    load(&url).unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2, "304 must refresh the entry");
    drop(ct);
}

#[test]
fn cache_updates_on_change() {
    let ct = CacheTest::new();
    let hits = counter();
    let hits2 = hits.clone();
    let saw_inm: Arc<parking_lot::Mutex<String>> = Arc::new(parking_lot::Mutex::new(String::new()));
    let saw2 = saw_inm.clone();
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/ok", server.server_addr());
    std::thread::spawn(move || {
        let mut body = r#"{"a":1}"#;
        let mut etag = "\"v1\"";
        loop {
            let Ok(request) = server.recv() else { break };
            hits2.fetch_add(1, Ordering::SeqCst);
            let inm = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("If-None-Match"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            *saw2.lock() = inm.clone();
            let mut response: tiny_http::Response<std::io::Cursor<Vec<u8>>> = if inm == etag {
                tiny_http::Response::from_data(Vec::new()).with_status_code(304)
            } else {
                tiny_http::Response::from_string(body)
            };
            response
                .add_header(tiny_http::Header::from_bytes(&b"ETag"[..], etag.as_bytes()).unwrap());
            let _ = request.respond(response);
            // The test flips these after the first round; observe via memo
            // count instead of shared mutable body.
            if hits2.load(Ordering::SeqCst) == 1 {
                body = r#"{"a":2}"#;
                etag = "\"v2\"";
            }
        }
    });

    load(&url).unwrap();
    ct.new_process();
    CACHE_TTL_SECS.store(0, Ordering::Relaxed);
    let data = load(&url).unwrap();
    assert_eq!(data, br#"{"a":2}"#);
    assert_eq!(
        *saw_inm.lock(),
        "\"v1\"",
        "If-None-Match must be the cached etag"
    );
    // The new body is cached: a fresh read must serve it without a request.
    ct.new_process();
    CACHE_TTL_SECS.store(24 * 60 * 60, Ordering::Relaxed);
    let data = load(&url).unwrap();
    assert_eq!(data, br#"{"a":2}"#);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    drop(ct);
}

/// A server that always answers `status` with an empty body.
fn status_server(status: Arc<AtomicU32>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/ok", server.server_addr());
    std::thread::spawn(move || {
        loop {
            let Ok(request) = server.recv() else { break };
            let response = tiny_http::Response::empty(status.load(Ordering::SeqCst) as u16);
            let _ = request.respond(response);
        }
    });
    url
}

#[test]
fn cache_offline_fallback() {
    let ct = CacheTest::new();
    let (hits, conds) = (counter(), counter());
    let live = schema_server(r#"{"cached":true}"#, "\"v1\"", hits.clone(), conds.clone());
    load(&live).unwrap();

    // a port that refuses connections (dropped listener)
    let dead = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let dead_addr = dead.server_addr().to_string();
    drop(dead);
    let dead_url = format!("http://{dead_addr}/gone");

    // move the cached entry to the dead URL: same bytes, unreachable origin
    let live_stem = cache_path(&live).unwrap();
    let dead_stem = cache_path(&dead_url).unwrap();
    std::fs::create_dir_all(dead_stem.parent().unwrap()).unwrap();
    std::fs::copy(
        live_stem.with_extension("body"),
        dead_stem.with_extension("body"),
    )
    .unwrap();
    let sidecar = std::fs::read_to_string(live_stem.with_extension("json")).unwrap();
    std::fs::write(
        dead_stem.with_extension("json"),
        sidecar.replace(&live, &dead_url),
    )
    .unwrap();
    ct.new_process();
    CACHE_TTL_SECS.store(0, Ordering::Relaxed); // force a revalidation attempt

    let data = load(&dead_url).unwrap();
    assert_eq!(data, br#"{"cached":true}"#);
    assert!(
        note(&dead_url).contains("cached schema"),
        "note = {:?}",
        note(&dead_url)
    );

    // A different (uncached) URL still errors out.
    let err = load(&format!("http://{dead_addr}/other")).unwrap_err();
    assert!(!err.is_empty());
    drop(ct);
}

/// The Go implementation stored fetched_at as an RFC3339 string. A cache
/// written before the Rust rewrite must still satisfy the fresh-cache path,
/// otherwise an unreachable SchemaStore origin aborts startup and the schema
/// picker never opens.
#[test]
fn cache_reads_go_timestamp_sidecar() {
    let ct = CacheTest::new();
    let dead = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = dead.server_addr().to_string();
    drop(dead); // port now refuses connections
    let url = format!("http://{addr}/catalog.json");

    let stem = cache_path(&url).unwrap();
    std::fs::create_dir_all(stem.parent().unwrap()).unwrap();
    std::fs::write(stem.with_extension("body"), br#"{"schemas":[]}"#).unwrap();
    let fetched = crate::fetch::timestamp::format_rfc3339(std::time::SystemTime::now());
    std::fs::write(
        stem.with_extension("json"),
        format!(
            r#"{{"url":"{url}","etag":"W/\"6ad7f6a2-8594c\"","last_modified":"Tue, 20 Oct 2026 23:17:54 GMT","fetched_at":"{fetched}"}}"#
        ),
    )
    .unwrap();

    let data = load(&url).unwrap();
    assert_eq!(data, br#"{"schemas":[]}"#);
    assert_eq!(note(&url), "", "fresh Go cache must not hit the origin");
    drop(ct);
}

/// Writes stay Go-compatible so a cache created here can be read by an older
/// Go binary (and by this version's string parser).
#[test]
fn cache_sidecar_timestamp_is_rfc3339() {
    let ct = CacheTest::new();
    let (hits, conds) = (counter(), counter());
    let url = format!(
        "{}/ok",
        schema_server(r#"{"a":1}"#, "\"v1\"", hits.clone(), conds.clone())
    );
    load(&url).unwrap();

    let sidecar =
        std::fs::read_to_string(cache_path(&url).unwrap().with_extension("json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&sidecar).unwrap();
    let stamp = v["fetched_at"]
        .as_str()
        .unwrap_or_else(|| panic!("sidecar: {sidecar}"));
    assert!(
        crate::fetch::timestamp::parse_rfc3339(stamp).is_ok(),
        "stamp: {stamp}"
    );
    drop(ct);
}

#[test]
fn cache_timestamp_parses_go_offsets() {
    let local =
        crate::fetch::timestamp::parse_rfc3339("2026-10-21T11:40:06.403520564+08:00").unwrap();
    let utc = crate::fetch::timestamp::parse_rfc3339("2026-10-21T03:40:06.403520564Z").unwrap();
    assert_eq!(local, utc);
    // Legacy serde struct written by earlier Rust builds.
    #[derive(serde::Deserialize)]
    struct LegacySidecar {
        #[serde(with = "crate::fetch::timestamp")]
        fetched_at: std::time::SystemTime,
    }
    let d = utc.duration_since(std::time::UNIX_EPOCH).unwrap();
    let legacy: LegacySidecar = serde_json::from_str(&format!(
        r#"{{"fetched_at":{{"secs_since_epoch":{},"nanos_since_epoch":{}}}}}"#,
        d.as_secs(),
        d.subsec_nanos()
    ))
    .unwrap();
    assert_eq!(legacy.fetched_at, utc);
}

/// A fixed-status server counting requests, optionally with Retry-After.
fn counted_status_server(status: u16, hits: Arc<AtomicI32>, retry: Option<&'static str>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/doc", server.server_addr());
    std::thread::spawn(move || {
        loop {
            let Ok(request) = server.recv() else { break };
            hits.fetch_add(1, Ordering::SeqCst);
            let mut response =
                tiny_http::Response::from_string(String::new()).with_status_code(status);
            if let Some(v) = retry {
                response.add_header(
                    tiny_http::Header::from_bytes(&b"Retry-After"[..], v.as_bytes()).unwrap(),
                );
            }
            let _ = request.respond(response);
        }
    });
    url
}

/// seed_cache writes a cache entry older than the TTL, forcing a request.
fn seed_cache(url: &str, body: &[u8], age: std::time::Duration) {
    let stem = cache_path(url).unwrap();
    std::fs::create_dir_all(stem.parent().unwrap()).unwrap();
    std::fs::write(stem.with_extension("body"), body).unwrap();
    let fetched = std::time::SystemTime::now() - age;
    std::fs::write(
        stem.with_extension("json"),
        format!(
            r#"{{"url":"{url}","etag":"","last_modified":"","fetched_at":"{}"}}"#,
            crate::fetch::timestamp::format_rfc3339(fetched)
        ),
    )
    .unwrap();
}

#[test]
fn rate_limit_falls_back_to_cache_and_cools_down() {
    let ct = CacheTest::new();
    let hits = counter();
    let url = counted_status_server(429, hits.clone(), Some("120"));
    seed_cache(
        &url,
        br#"{"schemas":[]}"#,
        std::time::Duration::from_secs(2 * 24 * 3600),
    );

    let data = load(&url).unwrap();
    assert_eq!(data, br#"{"schemas":[]}"#);
    assert!(note(&url).contains("429"), "note = {:?}", note(&url));
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // The next jse run inside Retry-After must not contact the origin.
    ct.new_process();
    let data = load(&url).unwrap();
    assert_eq!(data, br#"{"schemas":[]}"#);
    assert!(
        note(&url).contains("rate limited"),
        "note = {:?}",
        note(&url)
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "cooldown must suppress the request"
    );
    drop(ct);
}

#[test]
fn rate_limit_without_cache_backs_off_across_runs() {
    let ct = CacheTest::new();
    let hits = counter();
    let url = counted_status_server(429, hits.clone(), Some("120"));

    let err = load(&url).unwrap_err();
    assert!(err.contains("rate limited"), "err = {err}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    ct.new_process();
    let err = load(&url).unwrap_err();
    assert!(err.contains("rate limited"), "err = {err}");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "cooldown must suppress the retry"
    );
    drop(ct);
}

#[test]
fn load_errors_memoized_per_process() {
    let ct = CacheTest::new();
    let hits = counter();
    let url = counted_status_server(404, hits.clone(), None);

    let first = load(&url).unwrap_err();
    let second = load(&url).unwrap_err();
    assert_eq!(first, second);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "one failed request per process"
    );
    drop(ct);
}

#[test]
fn cache_server_error_fallback() {
    let ct = CacheTest::new();
    let status = Arc::new(AtomicU32::new(200));
    let url = status_server(status.clone());

    load(&url).unwrap(); // 200 with an empty body
    ct.new_process();
    CACHE_TTL_SECS.store(0, Ordering::Relaxed);
    status.store(500, Ordering::SeqCst);
    load(&url).unwrap();
    assert!(!note(&url).is_empty(), "5xx should fall back to the cache");
    status.store(404, Ordering::SeqCst);
    ct.new_process();
    let err = load(&url).unwrap_err();
    assert!(err.contains("status 404"), "err = {err}");
    drop(ct);
}

#[test]
fn cache_unwritable_degrades() {
    let ct = CacheTest::new();
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    set_cache_dir(Some(file.to_string_lossy().to_string())); // MkdirAll under it fails

    let (hits, conds) = (counter(), counter());
    let url = format!(
        "{}/ok",
        schema_server(r#"{"a":1}"#, "\"v1\"", hits.clone(), conds.clone())
    );
    for i in 0..2 {
        load(&url).unwrap_or_else(|e| panic!("load {i}: {e}"));
        ct.new_process();
    }
    assert_eq!(hits.load(Ordering::SeqCst), 2, "no cache available");
    drop(ct);
}

// --- fetch_test.go -------------------------------------------------------------

#[test]
fn load_local_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("s.json");
    std::fs::write(&p, br#"{"ok":true}"#).unwrap();
    assert_eq!(load(p.to_str().unwrap()).unwrap(), br#"{"ok":true}"#);
}

#[test]
fn load_file_url() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("s.json");
    std::fs::write(&p, b"v").unwrap();
    assert_eq!(load(&format!("file://{}", p.display())).unwrap(), b"v");
}

#[test]
fn load_missing_file() {
    let dir = tempfile::TempDir::new().unwrap();
    assert!(load(dir.path().join("nope.json").to_str().unwrap()).is_err());
}

#[test]
fn load_http_redirects_and_404() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        loop {
            let Ok(request) = server.recv() else { break };
            let path = request.url().to_string();
            let response: tiny_http::Response<std::io::Cursor<Vec<u8>>> = match path.as_str() {
                "/ok" => tiny_http::Response::from_string(r#"{"a":1}"#),
                "/redirect" => {
                    let mut r = tiny_http::Response::empty(302);
                    r.add_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], &b"/ok"[..]).unwrap(),
                    );
                    request.respond(r).ok();
                    continue;
                }
                "/moved" => {
                    let mut r = tiny_http::Response::empty(301);
                    r.add_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], &b"/ok"[..]).unwrap(),
                    );
                    request.respond(r).ok();
                    continue;
                }
                _ => tiny_http::Response::from_data(Vec::new()).with_status_code(404),
            };
            let _ = request.respond(response);
        }
    });

    for path in ["/ok", "/redirect", "/moved"] {
        let data = load(&format!("{base}{path}")).unwrap();
        assert_eq!(data, br#"{"a":1}"#, "{path}");
    }
    let err = load(&format!("{base}/404")).unwrap_err();
    assert!(err.contains("status 404"), "err = {err}");
}

#[test]
fn load_size_limit() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/big", server.server_addr());
    std::thread::spawn(move || {
        loop {
            let Ok(request) = server.recv() else { break };
            let body = vec![0u8; 1000];
            let _ = request.respond(tiny_http::Response::from_data(body));
        }
    });

    let _guard = TEST_LOCK.lock();
    let old = MAX_BYTES.load(Ordering::Relaxed);
    MAX_BYTES.store(100, Ordering::Relaxed);
    let err = load(&url).unwrap_err();
    assert!(
        err.contains("response exceeds size limit"),
        "limit err = {err}"
    );
    MAX_BYTES.store(2000, Ordering::Relaxed);
    crate::fetch::reset_memo(); // the failed load is memoized per process
    let data = load(&url).unwrap();
    assert_eq!(data.len(), 1000);
    MAX_BYTES.store(old, Ordering::Relaxed);
}

// --- regression: proxy selection (bug: Proxy::try_from_env preferred the
// socks5 ALL_PROXY which ureq could not use, so every fetch went direct and
// timed out on a proxied network) -------------------------------------------

#[test]
fn proxy_selection_follows_go_order_and_no_proxy() {
    let _g = TEST_LOCK.lock();
    let saved: Vec<(String, Option<String>)> = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ]
    .iter()
    .map(|k| (k.to_string(), std::env::var(k).ok()))
    .collect();
    // SAFETY: test-only env manipulation, serialized by TEST_LOCK
    unsafe {
        for k in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            std::env::remove_var(k);
        }
        std::env::set_var("all_proxy", "socks5://127.0.0.1:1080");
        std::env::set_var("http_proxy", "http://127.0.0.1:8080");
    }
    // Go's order: http_proxy wins over all_proxy
    assert_eq!(
        crate::fetch::proxy_for("https://example.com/s.json").as_deref(),
        Some("http://127.0.0.1:8080")
    );
    // SAFETY: as above
    unsafe {
        std::env::set_var("https_proxy", "http://127.0.0.1:9090");
    }
    assert_eq!(
        crate::fetch::proxy_for("https://example.com/s.json").as_deref(),
        Some("http://127.0.0.1:9090")
    );
    // NO_PROXY exempts the host
    // SAFETY: as above
    unsafe {
        std::env::set_var("NO_PROXY", "internal.example, .corp.example");
    }
    assert!(crate::fetch::proxy_for("https://internal.example/s.json").is_none());
    assert!(crate::fetch::proxy_for("https://a.corp.example/s.json").is_none());
    assert!(crate::fetch::proxy_for("https://other.example/s.json").is_some());
    // SAFETY: as above
    unsafe {
        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
    }
}

#[test]
fn host_of_shapes() {
    assert_eq!(
        crate::fetch::host_of("https://example.com/a/b?q=1"),
        "example.com"
    );
    assert_eq!(
        crate::fetch::host_of("http://user:pw@h.example:8080/x"),
        "h.example"
    );
    assert_eq!(crate::fetch::host_of("http://[::1]:80/x"), "[::1]");
}
