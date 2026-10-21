//! Ports of internal/catalog/match_test.go and load_test.go.

use crate::catalog::{DEFAULT_URL, Entry, load, loc, match_entries};
use std::sync::atomic::Ordering;

fn fixture_path() -> String {
    format!("{}/testdata/catalog.json", env!("CARGO_MANIFEST_DIR"))
}

fn test_entries() -> Vec<Entry> {
    load(&fixture_path()).expect("load fixture")
}

fn names(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

/// Serializes tests that mutate JSE_CATALOG_URL.
static ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[test]
fn match_exact_name() {
    let entries = test_entries();
    let got = match_entries(&entries, "/home/u/app/package.json");
    assert_eq!(names(&got), vec!["Package"]);
}

#[test]
fn match_case_insensitive() {
    let entries = test_entries();
    let got = match_entries(&entries, "/home/u/app/PACKAGE.JSON");
    assert_eq!(names(&got), vec!["Package"]);
}

#[test]
fn match_star_within_segment() {
    let entries = test_entries();
    for (path, want) in [
        ("/x/tsconfig.json", "Tsconfig"),
        ("/x/tsconfig.build.json", "Tsconfig"),
    ] {
        let got = match_entries(&entries, path);
        assert_eq!(names(&got), vec![want], "{path}");
    }
    // "*" stays within one segment: a leading character is not spanned.
    assert_eq!(
        names(&match_entries(&entries, "/x/mytsconfig.json")),
        Vec::<&str>::new()
    );
}

#[test]
fn match_suffix_glob() {
    let entries = test_entries();
    let got = match_entries(&entries, "/srv/app/odd.srv.json");
    assert_eq!(names(&got), vec!["Srv"]);
    // "*.srv.json" does not match plain srv.json.
    assert_eq!(
        names(&match_entries(&entries, "/srv/app/srv.json")),
        Vec::<&str>::new()
    );
}

#[test]
fn match_double_star_spans_directories() {
    let entries = test_entries();
    for p in [
        "/.cdf/config.yaml",
        "/a/.cdf/config.yaml",
        "/a/b/c/.cdf/config.yaml",
    ] {
        let got = match_entries(&entries, p);
        assert_eq!(names(&got), vec!["CDF"], "{p}");
    }
    assert_eq!(
        names(&match_entries(&entries, "/a/config.yaml")),
        Vec::<&str>::new()
    );
    assert_eq!(
        names(&match_entries(&entries, "/a/config.yaml/.cdf/config.yaml")),
        vec!["CDF"]
    );
}

#[test]
fn match_slash_pattern_anchors_full_path() {
    let entries = test_entries();
    let got = match_entries(&entries, "/srv/app/.platform/applications.yml");
    assert_eq!(names(&got), vec!["Platform"]);
    for p in [
        "/srv/app/applications.yml",
        "/srv/app/platform/applications.yml",
        "/srv/app/my.platform/applications.yml",
    ] {
        assert_eq!(
            names(&match_entries(&entries, p)),
            Vec::<&str>::new(),
            "{p}"
        );
    }
}

#[test]
fn match_name_pattern_ignores_path() {
    let entries = test_entries();
    let got = match_entries(&entries, "/deep/nested/dir/package.json");
    assert_eq!(names(&got), vec!["Package"]);
    assert_eq!(
        names(&match_entries(&entries, "/x/package.json/README")),
        Vec::<&str>::new()
    );
}

#[test]
fn match_deduplicates_by_url() {
    let entries = test_entries();
    let got = match_entries(&entries, "/x/one.json");
    assert_eq!(
        names(&got),
        vec!["AliasA"],
        "same-URL aliases not deduplicated"
    );
    let got = match_entries(&entries, "/x/two.json");
    assert_eq!(names(&got), vec!["AliasB"]);
}

#[test]
fn match_catalog_order() {
    let cat = vec![
        Entry {
            name: "B".into(),
            description: String::new(),
            url: "https://b".into(),
            file_match: vec!["*.json".into()],
        },
        Entry {
            name: "A".into(),
            description: String::new(),
            url: "https://a".into(),
            file_match: vec!["a.json".into()],
        },
    ];
    let got = match_entries(&cat, "/x/a.json");
    assert_eq!(names(&got), vec!["B", "A"], "catalog order lost");
}

#[test]
fn match_no_file_match_entries() {
    let entries = test_entries();
    assert_eq!(
        names(&match_entries(&entries, "/x/nourl.json")),
        Vec::<&str>::new()
    );
    assert_eq!(
        names(&match_entries(&entries, "/x/nomatch.json")),
        Vec::<&str>::new()
    );
}

#[test]
fn loc_default() {
    let _g = ENV_LOCK.lock();
    let saved = std::env::var("JSE_CATALOG_URL").ok();
    // SAFETY: test-only env manipulation, serialized by ENV_LOCK
    unsafe {
        std::env::remove_var("JSE_CATALOG_URL");
    }
    assert_eq!(loc(), DEFAULT_URL);
    if let Some(s) = saved {
        // SAFETY: test-only env manipulation, serialized by ENV_LOCK
        unsafe {
            std::env::set_var("JSE_CATALOG_URL", s);
        }
    }
}

#[test]
fn loc_env_override() {
    let _g = ENV_LOCK.lock();
    let saved = std::env::var("JSE_CATALOG_URL").ok();
    // SAFETY: test-only env manipulation, serialized by ENV_LOCK
    unsafe {
        std::env::set_var("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json");
    }
    assert_eq!(loc(), "http://127.0.0.1:1/catalog.json");
    // SAFETY: test-only env manipulation, serialized by ENV_LOCK
    unsafe {
        match saved {
            Some(s) => std::env::set_var("JSE_CATALOG_URL", s),
            None => std::env::remove_var("JSE_CATALOG_URL"),
        }
    }
}

#[test]
fn load_fixture() {
    let entries = load(&fixture_path()).unwrap();
    assert_eq!(entries.len(), 9, "NoURL skipped: {:?}", names(&entries));
    let e = entries.iter().find(|e| e.name == "Versioned").unwrap();
    assert_eq!(
        e.url, "https://example.org/versioned.json",
        "versions field not ignored"
    );
    assert_eq!(e.file_match[0], "versioned.json");
    let tsconfig = entries.iter().find(|e| e.name == "Tsconfig").unwrap();
    assert_eq!(tsconfig.description, "TypeScript compiler configuration");
    let alias_a = entries.iter().find(|e| e.name == "AliasA").unwrap();
    let alias_b = entries.iter().find(|e| e.name == "AliasB").unwrap();
    assert_eq!(alias_a.url, alias_b.url, "aliased entries diverged");
}

#[test]
fn load_local_file_url_and_path() {
    let _g = ENV_LOCK.lock();
    let saved = std::env::var("JSE_CATALOG_URL").ok();
    // SAFETY: test-only env manipulation, serialized by ENV_LOCK
    unsafe {
        std::env::remove_var("JSE_CATALOG_URL");
    }
    // keep the fetch cache away from the user's real cache dir
    let cache = tempfile::TempDir::new().unwrap();
    // SAFETY: test-only env manipulation, serialized by ENV_LOCK
    unsafe {
        std::env::set_var("JSE_CACHE_DIR", cache.path());
    }
    let saved_cache_ttl = crate::fetch::CACHE_TTL_SECS.load(Ordering::Relaxed);
    crate::fetch::reset_memo();
    let abs = std::fs::canonicalize(&fixture_path()).unwrap();
    for loc in [
        abs.to_string_lossy().to_string(),
        format!("file://{}", abs.display()),
    ] {
        let entries = load(&loc).unwrap_or_else(|e| panic!("load {loc}: {e}"));
        assert_eq!(entries.len(), 9, "{loc}");
    }
    // SAFETY: test-only env manipulation, serialized by ENV_LOCK
    unsafe {
        match saved {
            Some(s) => std::env::set_var("JSE_CATALOG_URL", s),
            None => std::env::remove_var("JSE_CATALOG_URL"),
        }
        std::env::remove_var("JSE_CACHE_DIR");
    }
    crate::fetch::CACHE_TTL_SECS.store(saved_cache_ttl, Ordering::Relaxed);
    crate::fetch::reset_memo();
}

#[test]
fn load_errors() {
    let _g = ENV_LOCK.lock();
    let saved = std::env::var("JSE_CATALOG_URL").ok();
    // SAFETY: test-only env manipulation, serialized by ENV_LOCK
    unsafe {
        std::env::remove_var("JSE_CATALOG_URL");
    }
    let dir = tempfile::TempDir::new().unwrap();
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, b"not json").unwrap();
    let err = load(bad.to_str().unwrap()).unwrap_err();
    assert!(
        err.contains("parse catalog"),
        "malformed catalog: err = {err}"
    );
    assert!(load(dir.path().join("missing.json").to_str().unwrap()).is_err());
    // SAFETY: test-only env manipulation, serialized by ENV_LOCK
    unsafe {
        match saved {
            Some(s) => std::env::set_var("JSE_CATALOG_URL", s),
            None => std::env::remove_var("JSE_CATALOG_URL"),
        }
    }
}
