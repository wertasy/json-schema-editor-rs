//! SchemaStore catalog integration (www.schemastore.org): it loads the
//! public schema directory and matches a config file against the directory's
//! fileMatch patterns, the way VSCode suggests schemas for otherwise
//! unschemed JSON files.
//!
//! Rust port of `internal/catalog/catalog.go` and `glob.go`.

use crate::fetch;

/// DefaultURL is the SchemaStore catalog endpoint.
pub const DEFAULT_URL: &str = "https://www.schemastore.org/api/json/catalog.json";

/// Entry is one schema directory listing.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub description: String,
    pub url: String,
    pub file_match: Vec<String>,
}

/// Loc returns $JSE_CATALOG_URL when set, else DefaultURL.
pub fn loc() -> String {
    match std::env::var("JSE_CATALOG_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => DEFAULT_URL.to_string(),
    }
}

/// Load fetches and parses the catalog at loc — anything fetch::load takes
/// (https URL or local path). HTTP catalogs use stale-while-revalidate: a
/// cached copy is served immediately even after its TTL and refreshed in the
/// background, so a restricted network never stalls startup. Entries without
/// a URL are skipped; per-version listings are ignored (the latest URL is the
/// entry's url).
pub fn load(loc: &str) -> Result<Vec<Entry>, String> {
    let data = fetch::load_stale_first(loc)?;
    let w: Wire = serde_json::from_slice(&data).map_err(|e| format!("parse catalog {loc}: {e}"))?;
    Ok(w.schemas
        .into_iter()
        .filter(|s| !s.url.is_empty())
        .map(|s| Entry {
            name: s.name,
            description: s.description,
            url: s.url,
            file_match: s.file_match,
        })
        .collect())
}

#[derive(serde::Deserialize)]
struct Wire {
    #[serde(default)]
    schemas: Vec<WireSchema>,
}

#[derive(serde::Deserialize)]
struct WireSchema {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    url: String,
    #[serde(default, rename = "fileMatch")]
    file_match: Vec<String>,
}

/// Match returns the entries whose fileMatch patterns match the file, in
/// catalog order, deduplicated by URL (the catalog lists some schemas under
/// aliasing names sharing one URL). path is the config's absolute path.
pub fn match_entries(entries: &[Entry], path: &str) -> Vec<Entry> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let full = path.replace('\\', "/");
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for e in entries {
        if seen.contains(&e.url) {
            continue;
        }
        for pattern in &e.file_match {
            if match_glob(pattern, base, &full) {
                seen.insert(e.url.clone());
                out.push(e.clone());
                break;
            }
        }
    }
    out
}

/// matchGlob implements the catalog fileMatch semantics (aligned with
/// VSCode's schema associations), case-insensitively:
///
///   - patterns without "/" match the file NAME only ("package.json",
///     "tsconfig*.json", "*.srv.json");
///   - patterns with "/" match the FULL path segment-wise, anchored to the
///     tail of the path at a segment boundary: "**" spans zero or more
///     directories ("**/.cdf/config.yaml"), while "*" and "?" stay within
///     one segment (".platform/applications.yml" requires that directory
///     right above the file).
///
/// Within-segment wildcards follow path.Match.
fn match_glob(pattern: &str, base: &str, full: &str) -> bool {
    let pattern = pattern.to_lowercase();
    if !pattern.contains('/') {
        return path_match(&pattern, &base.to_lowercase());
    }
    let seg_pat: Vec<&str> = pattern.split('/').collect();
    let lowered = full.to_lowercase();
    let trimmed = lowered.trim_start_matches('/');
    let seg_path: Vec<&str> = trimmed.split('/').collect();
    match_segs(&seg_pat, &seg_path)
}

/// matchSegs matches pat against a tail of p (segment lists): the pattern
/// must anchor at a segment boundary, anything before it is free. "**"
/// consumes zero or more segments.
fn match_segs(pat: &[&str], p: &[&str]) -> bool {
    if pat.is_empty() {
        return true;
    }
    for start in 0..p.len() {
        if match_anchored(pat, &p[start..]) {
            return true;
        }
    }
    false
}

/// matchAnchored matches pat against exactly p: "**" consumes zero or more
/// segments, every other segment must path.Match the same-position segment.
fn match_anchored(pat: &[&str], p: &[&str]) -> bool {
    let mut i = 0usize;
    for (pi, seg) in pat.iter().enumerate() {
        if *seg == "**" {
            let rest = &pat[pi + 1..];
            for skip in 0..=p.len().saturating_sub(i) {
                if match_anchored(rest, &p[i + skip..]) {
                    return true;
                }
            }
            return false;
        }
        if i >= p.len() {
            return false;
        }
        if !path_match(seg, p[i]) {
            return false;
        }
        i += 1;
    }
    i == p.len()
}

/// path.Match within one segment: `*` matches any sequence of non-separator
/// characters, `?` a single one, `[...]` a character class, `\\` escapes.
/// Port of Go's path.Match for separator-free patterns, with star
/// backtracking.
fn path_match(pattern: &str, name: &str) -> bool {
    let (pat, nam): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0usize, 0usize);
    // the last star: (pattern index after it, name index it started at)
    let mut star: Option<(usize, usize)> = None;
    while ni < nam.len() {
        let mut matched = false;
        if pi < pat.len() {
            match pat[pi] {
                '*' => {
                    star = Some((pi + 1, ni));
                    pi += 1;
                    continue;
                }
                '?' => {
                    pi += 1;
                    ni += 1;
                    matched = true;
                }
                '\\' if pi + 1 < pat.len() => {
                    if nam[ni] == pat[pi + 1] {
                        pi += 2;
                        ni += 1;
                        matched = true;
                    }
                }
                '[' => {
                    if let Some(next_pi) = match_class(&pat, pi, nam[ni]) {
                        pi = next_pi;
                        ni += 1;
                        matched = true;
                    }
                }
                c => {
                    if nam[ni] == c {
                        pi += 1;
                        ni += 1;
                        matched = true;
                    }
                }
            }
        }
        if !matched {
            // mismatch: let the last star absorb one more character
            match star {
                Some((sp, sn)) if sn < nam.len() => {
                    star = Some((sp, sn + 1));
                    pi = sp;
                    ni = sn + 1;
                }
                _ => return false,
            }
        }
    }
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}

/// match_class matches a `[...]` class at pattern[i] against c, returning the
/// pattern index after the class, or None when c does not match (including
/// malformed classes, like path.Match's ErrBadPattern → false).
fn match_class(pattern: &[char], i: usize, c: char) -> Option<usize> {
    // path.Match: a valid class has at least [x] or [^x]; malformed → error
    let mut j = i + 1;
    let mut negate = false;
    if j < pattern.len() && pattern[j] == '^' {
        negate = true;
        j += 1;
    }
    let mut matched = false;
    let mut first = true;
    while j < pattern.len() && (first || pattern[j] != ']') {
        first = false;
        let lo = if pattern[j] == '\\' && j + 1 < pattern.len() {
            j += 1;
            pattern[j]
        } else {
            pattern[j]
        };
        j += 1;
        let hi = if j + 1 < pattern.len() && pattern[j] == '-' {
            let h = if pattern[j + 1] == '\\' && j + 2 < pattern.len() {
                j += 1;
                pattern[j + 1]
            } else {
                pattern[j + 1]
            };
            j += 2;
            h
        } else {
            lo
        };
        if lo <= c && c <= hi {
            matched = true;
        }
    }
    if j >= pattern.len() {
        return None; // malformed
    }
    j += 1; // consume ']'
    if matched != negate { Some(j) } else { None }
}
