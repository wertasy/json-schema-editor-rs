//! Ports of internal/validate/validate_test.go. Library-specific error
//! wording is adapted to jsonschema-rs's messages; paths, structure and
//! behavior assertions mirror the Go tests.

use crate::doc::Doc;
use crate::doc::jsondoc;
use crate::validate::new;
use std::sync::atomic::Ordering;

const SCHEMA_SRC: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "server": {
      "type": "object",
      "additionalProperties": false,
      "required": ["port"],
      "properties": {
        "port": {"type": "integer", "minimum": 1024},
        "host": {"type": "string"}
      }
    }
  },
  "required": ["server"]
}"#;

fn write_schema() -> String {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("schema.json");
    std::fs::write(&p, SCHEMA_SRC).unwrap();
    let p = p.canonicalize().unwrap();
    // the schema must outlive the test's load; leak the temp dir
    std::mem::forget(dir);
    p.to_string_lossy().to_string()
}

fn cfg(src: &str) -> Doc {
    jsondoc::parse(src).expect("config must parse")
}

/// Serializes tests that touch JSE_CACHE_DIR / the fetch memo.
static ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn isolate_cache() -> tempfile::TempDir {
    let cache = tempfile::TempDir::new().unwrap();
    // SAFETY: test-only env manipulation, serialized by ENV_LOCK
    unsafe {
        std::env::set_var("JSE_CACHE_DIR", cache.path());
    }
    crate::fetch::reset_memo();
    cache
}

#[test]
fn valid_passes() {
    let v = new(&write_schema()).unwrap();
    assert!(
        v.check(&cfg(r#"{"server": {"port": 8080, "host": "x"}}"#))
            .is_empty()
    );
}

#[test]
fn constraint_violations() {
    let v = new(&write_schema()).unwrap();
    let errs = v.check(&cfg(r#"{"server": {"port": 80}}"#));
    let joined = errs.join("\n");
    assert!(
        joined.contains("at '$.server.port':") && joined.contains("minimum"),
        "minimum error missing or wrong path: {errs:?}"
    );

    let errs = v.check(&cfg(r#"{"server": {"host": "x"}}"#));
    let joined = errs.join("\n");
    assert!(
        joined.contains("at '$.server':") && joined.contains("port"),
        "required error missing or wrong path: {errs:?}"
    );

    let errs = v.check(&cfg(r#"{"server": {"port": 8080, "extra": 1}}"#));
    let joined = errs.join("\n");
    assert!(
        joined.contains("at '$.server':") && joined.contains("extra"),
        "additionalProperties error missing or wrong path: {errs:?}"
    );

    let errs = v.check(&cfg("{}"));
    assert_eq!(errs.len(), 1);
    assert!(
        errs[0].contains("at '$':") && errs[0].contains("server"),
        "root required error = {:?}, want the document-root path",
        errs
    );
}

#[test]
fn path_shapes() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("shapes.schema.json");
    let src = r#"{"type":"object","properties":{"items":{"type":"array","items":{"type":"object","properties":{"port":{"type":"integer","minimum":1}}}},"123":{"type":"integer","minimum":10}}}"#;
    std::fs::write(&p, src).unwrap();
    let v = new(p.to_str().unwrap()).unwrap();
    let joined = v
        .check(&cfg(r#"{"items":[{"port":1},{"port":0}],"123":1}"#))
        .join("\n");
    assert!(
        joined.contains("at '$.items[1].port':"),
        "array index path missing: {joined:?}"
    );
    assert!(
        joined.contains("at '$.123':"),
        "numeric object key must stay a field: {joined:?}"
    );
}

#[test]
fn remote_schema() {
    let _g = ENV_LOCK.lock();
    // Ephemeral ports get reused; a shared cache can then serve another
    // test's schema for the same URL.
    let _cache = isolate_cache();
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        loop {
            let Ok(request) = server.recv() else { break };
            let _ = request.respond(tiny_http::Response::from_string(SCHEMA_SRC));
        }
    });

    let v = new(&format!("{base}/schema.json")).unwrap();
    assert!(
        v.check(&cfg(r#"{"server": {"port": 8080}}"#)).is_empty(),
        "remote valid"
    );
    assert!(
        !v.check(&cfg(r#"{"server": {"port": 1}}"#)).is_empty(),
        "remote invalid should fail"
    );
}

#[test]
fn remote_ref_inside_schema() {
    let _g = ENV_LOCK.lock();
    let _cache = isolate_cache();
    let ref_server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let ref_base = format!("http://{}", ref_server.server_addr());
    std::thread::spawn(move || {
        loop {
            let Ok(request) = ref_server.recv() else {
                break;
            };
            let _ = request.respond(tiny_http::Response::from_string(
                r#"{"type": "integer", "minimum": 1024}"#,
            ));
        }
    });

    let root_schema = format!(
        r#"{{"type": "object", "properties": {{"port": {{"$ref": "{ref_base}/port.json"}}}}}}"#
    );
    let root = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let root_base = format!("http://{}", root.server_addr());
    std::thread::spawn(move || {
        loop {
            let Ok(request) = root.recv() else { break };
            let _ = request.respond(tiny_http::Response::from_string(root_schema.clone()));
        }
    });

    let v = new(&format!("{root_base}/schema.json")).unwrap();
    assert!(
        v.check(&cfg(r#"{"port": 8080}"#)).is_empty(),
        "remote ref valid"
    );
    assert!(
        !v.check(&cfg(r#"{"port": 80}"#)).is_empty(),
        "remote ref violation should fail"
    );
}

#[test]
fn compile_failure() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("bad.json");
    std::fs::write(&p, br#"{"required": "not-an-array"}"#).unwrap();
    assert!(
        new(p.to_str().unwrap()).is_err(),
        "invalid schema should fail compile"
    );
}

#[test]
fn load_failure() {
    let dir = tempfile::TempDir::new().unwrap();
    assert!(new(dir.path().join("nope.json").to_str().unwrap()).is_err());
}

#[test]
fn cache_ttl_is_restored() {
    // guards against a test leaking the shrunken TTL into later phases
    assert_eq!(
        crate::fetch::CACHE_TTL_SECS.load(Ordering::Relaxed),
        24 * 60 * 60
    );
}
