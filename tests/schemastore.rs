//! Port of `e2e/schemastore_test.go`: SchemaStore catalog integration —
//! automatic fileMatch resolution, the accessible search picker and the
//! pty-driven TUI picker.

mod common;

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use serde_json::json;
use tiny_http::{Header, Response, Server};

const MINI_SCHEMA: &str =
    r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#;

/// Serves a two-entry catalog plus the mini schema both entries point at
/// (startCatalogServer in the Go suite).
struct CatalogServer {
    url: String,
    server: Arc<Server>,
}

impl CatalogServer {
    fn start() -> CatalogServer {
        let server =
            Arc::new(Server::http("127.0.0.1:0").unwrap_or_else(|e| panic!("start catalog: {e}")));
        let url = format!("http://{}", server.server_addr());
        let s = Arc::clone(&server);
        let base = url.clone();
        thread::spawn(move || {
            for request in s.incoming_requests() {
                let path = request.url().split('?').next().unwrap_or("").to_string();
                let body = if path == "/catalog.json" {
                    // Decoy only matches other.json: never auto-applied to the
                    // configs below, never listed for a "serv" search.
                    format!(
                        concat!(
                            r#"{{"schemas":["#,
                            r#"{{"name":"Server Config","description":"server settings","url":"{base}/server.schema.json","fileMatch":["server.json","*.srv.json"]}},"#,
                            r#"{{"name":"Decoy","description":"something else","url":"{base}/other.schema.json","fileMatch":["other.json"]}}]}}"#,
                        ),
                        base = base
                    )
                    .into_bytes()
                } else {
                    MINI_SCHEMA.as_bytes().to_vec()
                };
                let header =
                    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                let _ = request.respond(Response::from_data(body).with_header(header));
            }
        });
        CatalogServer { url, server }
    }
}

impl Drop for CatalogServer {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

/// Run accessible against a fresh cache with the catalog endpoint set.
fn run_with_catalog(ts: &CatalogServer, stdin: &str, args: &[&str]) -> RunOutput {
    let cache = tempfile::tempdir().expect("cache");
    let cache_dir = cache.path().to_string_lossy().into_owned();
    let catalog = ts.url.clone() + "/catalog.json";
    run_jse_env(
        repo_root(),
        stdin,
        args,
        &[
            ("JSE_CATALOG_URL", catalog.as_str()),
            ("JSE_CACHE_DIR", cache_dir.as_str()),
        ],
        Duration::from_secs(60),
    )
}

#[test]
fn schema_store_auto_match() {
    let ts = CatalogServer::start();
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("server.json");
    write_file(&cfg, r#"{"name":"x"}"#);
    let cfg_arg = cfg.to_string_lossy().into_owned();
    let r = run_with_catalog(&ts, "\n\n", &[cfg_arg.as_str(), "--accessible"]);
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
    assert_eq!(
        read_json(&cfg)["$schema"],
        json!(ts.url.clone() + "/server.schema.json"),
        "matched URL not saved as $schema"
    );
    assert!(
        r.stdout.contains("schema from SchemaStore: Server Config"),
        "catalog source not reported:\n{}",
        r.stdout
    );
}

#[test]
fn schema_store_glob_match() {
    let ts = CatalogServer::start();
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("odd.srv.json");
    write_file(&cfg, r#"{"name":"x"}"#);
    let cfg_arg = cfg.to_string_lossy().into_owned();
    let r = run_with_catalog(&ts, "\n\n", &[cfg_arg.as_str(), "--accessible"]);
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
    assert_eq!(
        read_json(&cfg)["$schema"],
        json!(ts.url.clone() + "/server.schema.json"),
        "glob match URL not saved"
    );
}

#[test]
fn schema_store_manual_search() {
    let ts = CatalogServer::start();
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("mystery.json");
    write_file(&cfg, r#"{"name":"y"}"#);
    let cfg_arg = cfg.to_string_lossy().into_owned();
    // search "serv" → numbered list → 1 → form accepts defaults, saves
    let r = run_with_catalog(&ts, "serv\n1\n\n\n", &[cfg_arg.as_str(), "--accessible"]);
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
    assert_eq!(
        read_json(&cfg)["$schema"],
        json!(ts.url.clone() + "/server.schema.json"),
        "picked URL not saved"
    );
}

#[test]
fn schema_store_abort() {
    let ts = CatalogServer::start();
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("mystery.json");
    write_file(&cfg, r#"{"name":"z"}"#);
    let cfg_arg = cfg.to_string_lossy().into_owned();
    // default query "mystery" matches nothing; empty lines keep it, then
    // EOF aborts the picker
    let r = run_with_catalog(&ts, "\n\n", &[cfg_arg.as_str(), "--accessible"]);
    assert_eq!(r.code, 2, "exit; stderr:\n{}", r.stderr);
    assert!(
        r.stderr.contains("no schema found"),
        "stderr:\n{}",
        r.stderr
    );
    assert!(
        read_json(&cfg).get("$schema").is_none(),
        "aborted session still wrote $schema"
    );
}

#[test]
fn pty_schema_picker() {
    let ts = CatalogServer::start();
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("mystery.json");
    write_file(&cfg, r#"{"name":"z"}"#);
    let cfg_arg = cfg.to_string_lossy().into_owned();
    let cache = tempfile::tempdir().unwrap();
    let cache_dir = cache.path().to_string_lossy().into_owned();
    let catalog = ts.url.clone() + "/catalog.json";

    let mut pty = match PtySession::spawn(
        dir.path(),
        &[cfg_arg.as_str()],
        &[
            ("JSE_CATALOG_URL", catalog.as_str()),
            ("JSE_CACHE_DIR", cache_dir.as_str()),
        ],
    ) {
        PtySpawn::Ok(p) => p,
        PtySpawn::Unavailable(e) => {
            eprintln!("SKIP: no pty available: {e}");
            return;
        }
    };

    let (_, mut off) = pty.wait_frame("picker opens", 0, &["Select a schema"]);
    pty.send(b"e"); // replace the filename-stem preset; both entries match
    let (_, off2) = pty.wait_frame("filtered list", off, &["Server Config", "Decoy"]);
    off = off2;
    pty.send(b"\x1b[B"); // ↓ to Decoy
    let (_, off3) = pty.wait_frame("cursor moved", off, &["> Decoy"]);
    off = off3;
    // The search input's caret blinks every ~530ms. A blink must not re-filter
    // and snap the cursor back to the first row (the old bug).
    let blink_until = Instant::now() + Duration::from_millis(1200);
    loop {
        let snap = pty.snapshot();
        if snap.get(off..).unwrap_or("").contains("> Server Config") {
            panic!("blink reset the picker cursor:\n{snap}");
        }
        if Instant::now() >= blink_until {
            break;
        }
        pty.pause(Duration::from_millis(25));
    }
    pty.send(b"\x0b"); // ctrl+k: up to Server Config
    let (_, off4) = pty.wait_frame("ctrl+k moves up", off, &["> Server Config"]);
    off = off4;
    pty.send(b"\n"); // ctrl+j: down to Decoy
    let (_, off5) = pty.wait_frame("ctrl+j moves down", off, &["> Decoy"]);
    off = off5;
    pty.send(b"\r");
    pty.wait_frame("form after pick", off, &["Save and exit"]);
    pty.send(b"\x03"); // ctrl+c abort

    pty.wait_exit(130);
    assert_eq!(
        std::fs::read(&cfg).unwrap(),
        br#"{"name":"z"}"#.to_vec(),
        "abort must not rewrite the config"
    );
}
