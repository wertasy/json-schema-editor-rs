//! Port of `e2e/e2e_test.go`: the built `jse` binary driven end to end —
//! deterministic accessible-mode line protocols plus the full-screen TUI over
//! a real pty.
//!
//! Assertion wording follows the Rust binary where it is library-dependent
//! (validation error text); structure, exit codes, saved documents, key
//! order, comments/anchors/aliases and navigation behavior are asserted like
//! the Go suite.
#![allow(clippy::needless_range_loop)]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use serde_json::json;

use yaml_rust2::{Yaml, YamlLoader};

const CREATE_SCRIPT: &str = concat!(
    "demo\n",                         // Name
    "2\n",                            // Mode → http
    "1\n",                            // Navigate → Server
    "8080\nlocalhost\ny\n\n\n",       // Port, Host, Debug=y, Ratio=omit, Done
    "demo\n\n2\n",                    // root rerun: Name, Mode=http, Navigate → Logging
    "2\n0\n",                         // Levels: toggle info, confirm
    "2\n",                            // Logging navigate → tags items
    "1\nprod\n6\n",                   // mgr: add item, value, done
    "0\n1\n",                         // Logging rerun: confirm levels, Done
    "demo\n\n3\n",                    // root: → Mounts
    "1\ndemos/ROOT\n/var/www\n\n6\n", // mgr: add mount {src,dst}, done
    "demo\n\n4\n",                    // root: → Auth
    "2\nsecret123\n\n",               // variant Token, token, done
    "demo\n\n5\n",                    // root: save
);

fn arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn mktemp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// The binary needs an isolated schema cache; every pty session gets one.
fn cache_env(cache: &tempfile::TempDir) -> (String, String) {
    (
        "JSE_CACHE_DIR".to_string(),
        cache.path().to_string_lossy().into_owned(),
    )
}

/// Start a pty session or skip (return false) when no pty is available,
/// mirroring Go's `t.Skipf("no pty available")`.
fn pty_or_skip(
    args: &[&str],
    extra_envs: &[(&str, &str)],
    cache: &tempfile::TempDir,
) -> Option<PtySession> {
    let (k, v) = cache_env(cache);
    let mut envs: Vec<(&str, &str)> = vec![(k.as_str(), v.as_str())];
    envs.extend_from_slice(extra_envs);
    match PtySession::spawn(repo_root(), args, &envs) {
        PtySpawn::Ok(s) => Some(s),
        PtySpawn::Unavailable(e) => {
            eprintln!("SKIP: no pty available: {e}");
            None
        }
    }
}

fn yaml_doc(text: &str) -> Yaml {
    let docs = YamlLoader::load_from_str(text)
        .unwrap_or_else(|e| panic!("saved YAML parses: {e}\n{text}"));
    docs.into_iter().next().expect("one YAML document")
}

fn yaml_get<'a>(node: &'a Yaml, key: &str) -> Option<&'a Yaml> {
    node.as_hash()?.get(&Yaml::String(key.to_string()))
}

// --- CLI flag tests ---------------------------------------------------------

#[test]
fn cli_version_short_and_long() {
    for flag in ["-v", "--version"] {
        let r = run_jse("", &[flag]);
        assert_eq!(r.code, 0, "{flag}: stderr:\n{}", r.stderr);
        assert!(
            r.stdout.starts_with("jse v"),
            "{flag}: stdout:\n{}",
            r.stdout
        );
        assert!(r.stderr.is_empty(), "{flag}: stderr:\n{}", r.stderr);
    }
}

#[test]
fn cli_help_short_and_long() {
    for flag in ["-h", "--help"] {
        let r = run_jse("", &[flag]);
        assert_eq!(r.code, 0, "{flag}: stderr:\n{}", r.stderr);
        assert!(
            r.stderr.contains("Usage: jse"),
            "{flag}: usage missing:\n{}",
            r.stderr
        );
        assert!(
            r.stderr.contains("-v, --version"),
            "{flag}: flags missing:\n{}",
            r.stderr
        );
    }
}

#[test]
fn cli_bad_flags() {
    let r = run_jse("", &["--bogus"]);
    assert_eq!(r.code, 2, "stderr:\n{}", r.stderr);
    assert!(
        r.stderr.contains("unrecognized option '--bogus'"),
        "stderr:\n{}",
        r.stderr
    );

    let r = run_jse("", &["-z"]);
    assert_eq!(r.code, 2, "stderr:\n{}", r.stderr);
    assert!(
        r.stderr.contains("invalid option -- 'z'"),
        "stderr:\n{}",
        r.stderr
    );

    let r = run_jse("", &["--force=false", "cfg.json"]);
    assert_eq!(r.code, 2, "stderr:\n{}", r.stderr);
    assert!(
        r.stderr
            .contains("option '--force' doesn't allow an argument"),
        "stderr:\n{}",
        r.stderr
    );
}

#[test]
fn cli_long_schema() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    write_file(&schema, MINI_SCHEMA);
    let cfg = dir.path().join("cfg.json");
    write_file(&cfg, r#"{"name": "x"}"#);
    let cfg_arg = arg(&cfg);
    let schema_arg = arg(&schema);
    let schema_flag = format!("--schema={schema_arg}");

    let r = run_jse("\n\n", &[schema_flag.as_str(), cfg_arg.as_str(), "-a"]);
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
    assert_eq!(read_json(&cfg)["$schema"], json!(schema_arg));
}

#[test]
fn cli_double_dash_operand() {
    let dir = mktemp();
    let cache = mktemp();
    write_file(&dir.path().join("mini.schema.json"), MINI_SCHEMA);
    write_file(
        &dir.path().join("-dash.json"),
        r#"{"$schema": "mini.schema.json", "name": "x"}"#,
    );
    let (k, v) = cache_env(&cache);

    let r = run_jse_env(
        dir.path(),
        "\n\n",
        &["-a", "--", "-dash.json"],
        &[(k.as_str(), v.as_str())],
        Duration::from_secs(60),
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
}

// --- accessible-mode tests --------------------------------------------------

#[test]
fn create_accessible() {
    let out = mktemp();
    let out_file = out.path().join("out.json");
    let out_arg = arg(&out_file);
    let r = run_jse(
        CREATE_SCRIPT,
        &[
            "-s",
            "examples/server.schema.json",
            out_arg.as_str(),
            "--accessible",
        ],
    );
    assert_eq!(r.code, 0, "exit; stdout:\n{}", r.stdout);
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);

    let want = json!({
        "$schema": "examples/server.schema.json", // -s auto-fills the field
        "name": "demo",
        "mode": "http",
        "server": {"port": 8080, "host": "localhost", "debug": true},
        "logging": {"levels": ["info"], "tags": ["prod"]},
        "mounts": [{"src": "demos/ROOT", "dst": "/var/www"}],
        "auth": {"token": "secret123"},
    });
    assert_eq!(read_json(&out_file), want);
    assert_eq!(
        top_level_keys(&out_file).join(","),
        "$schema,name,mode,server,logging,mounts,auth"
    );
}

#[test]
fn prefill_accessible() {
    let dir = mktemp();
    let cfg = dir.path().join("edit.json");
    let src = std::fs::read_to_string(repo_root().join("examples/server.config.json")).unwrap();
    write_file(&cfg, &src);

    // Re-type required Name, replace the illegal enum value with grpc, save.
    let cfg_arg = arg(&cfg);
    let r = run_jse(
        "demo\n3\n5\n",
        &[
            "-s",
            "examples/server.schema.json",
            cfg_arg.as_str(),
            "--accessible",
        ],
    );
    assert_eq!(r.code, 0, "exit; stdout:\n{}", r.stdout);
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);

    let got = read_json(&cfg);
    assert_eq!(got["mode"], json!("grpc"), "illegal enum not replaced");
    assert_eq!(
        got["extra_unknown_key"],
        json!(42),
        "schema-external key lost"
    );
    assert_eq!(got["name"], json!("demo"));
    assert_eq!(got["server"]["port"], json!(8080));
    assert_eq!(
        top_level_keys(&cfg).join(","),
        "$schema,name,mode,server,logging,mounts,extra_unknown_key",
        "key order not preserved"
    );
}

#[test]
fn move_accessible() {
    let dir = mktemp();
    let cfg = dir.path().join("move.json");
    write_file(
        &cfg,
        r#"{"$schema": "server.schema.json", "name": "demo", "server": {"port": 8080}, "logging": {"tags": ["alpha", "beta", "gamma"]}}"#,
    );
    let script = concat!(
        "demo\n",      // Name re-typed (required)
        "2\n2\n",      // Mode → http, Navigate → Logging
        "2\n0\n",      // Levels: toggle info, confirm
        "2\n",         // Navigate → Tags items (n=3)
        "5\n2\n",      // "Move item up…" → pick 1: beta
        "8\n0\n1\n",   // items manager Done, levels confirm, Logging Done
        "demo\n\n5\n", // Name, Mode default, Save and exit
    );
    let cfg_arg = arg(&cfg);
    let r = run_jse(
        script,
        &[
            "-s",
            "examples/server.schema.json",
            cfg_arg.as_str(),
            "--accessible",
        ],
    );
    assert_eq!(r.code, 0, "exit; stdout:\n{}", r.stdout);
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
    let got = read_json(&cfg);
    assert_eq!(got["logging"]["tags"], json!(["beta", "alpha", "gamma"]));
}

#[test]
fn validation_block() {
    let dir = mktemp();
    let schema = dir.path().join("strict.schema.json");
    let cfg = dir.path().join("strict.json");
    write_file(
        &schema,
        r#"{
  "type": "object",
  "properties": {"port": {"type": "integer", "title": "Port", "minimum": 1024}},
  "required": ["port"],
  "additionalProperties": false
}"#,
    );
    write_file(&cfg, r#"{"port": 80, "rogue": 1}"#);

    // empty → required error; 1 → minimum error; 8080 → accepted.
    // Save → save-time errors (rogue key) → decline; fix via raw editor; save.
    let script = concat!(
        "\n1\n8080\n\n", // Port attempts, navigate=save
        "n\n",           // decline save-anyway
        "8080\n2\n",     // Port, navigate → raw editor
        r#"{"port":8080}"#,
        "\n",        // raw JSON replaces doc
        "8080\n1\n", // Port, navigate=save → valid
    );
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let r = run_jse(
        script,
        &["-s", schema_arg.as_str(), cfg_arg.as_str(), "--accessible"],
    );
    assert_eq!(r.code, 0, "exit; stdout:\n{}", r.stdout);
    assert!(
        r.stdout.contains("required"),
        "required error not shown:\n{}",
        r.stdout
    );
    // The Rust validator renders the same "must be at least N" wording the
    // Go suite asserts (the library detail differs, the structure does not).
    assert!(
        r.stdout.contains("must be at least 1024"),
        "minimum error not shown:\n{}",
        r.stdout
    );
    assert!(r.stdout.contains("Saved to"), "not saved:\n{}", r.stdout);
    assert_eq!(read_json(&cfg), json!({"port": 8080}));
}

const MINI_SCHEMA: &str =
    r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#;

#[test]
fn schema_resolution_relative() {
    let dir = mktemp();
    write_file(&dir.path().join("mini.schema.json"), MINI_SCHEMA);
    let cfg = dir.path().join("cfg.json");
    write_file(&cfg, r#"{"$schema": "mini.schema.json", "name": "x"}"#);
    let cfg_arg = arg(&cfg);
    let r = run_jse("\n\n", &[cfg_arg.as_str(), "--accessible"]);
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
}

#[test]
fn schema_resolution_relative_falls_back_to_cwd() {
    let dir = mktemp();
    let work = mktemp();
    write_file(&work.path().join("mini.schema.json"), MINI_SCHEMA);
    let cfg = dir.path().join("cfg.json");
    write_file(&cfg, r#"{"$schema": "mini.schema.json", "name": "x"}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let cache_dir = arg(cache.path());
    let r = run_jse_env(
        work.path(),
        "\n\n",
        &[cfg_arg.as_str(), "--accessible"],
        &[("JSE_CACHE_DIR", cache_dir.as_str())],
        Duration::from_secs(60),
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
}

#[test]
fn schema_resolution_url() {
    let http = HttpServer::start(MINI_SCHEMA.as_bytes().to_vec());
    let dir = mktemp();
    let cfg = dir.path().join("cfg.json");
    write_file(
        &cfg,
        &format!(
            r#"{{"$schema": "{}", "name": "y"}}"#,
            http.url.clone() + "/mini.json"
        ),
    );
    let cfg_arg = arg(&cfg);
    let r = run_jse("\n\n", &[cfg_arg.as_str(), "--accessible"]);
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
}

#[test]
fn schema_resolution_none_falls_back_to_schema_less() {
    let dir = mktemp();
    let cfg = dir.path().join("cfg.json");
    write_file(&cfg, r#"{"name": "z"}"#);
    let cache = mktemp();
    let cache_dir = arg(cache.path());
    let cfg_arg = arg(&cfg);
    // closed port: the catalog lookup fails instantly, no network. The file
    // still opens; raw JSON edits it with no save-time validation.
    let r = run_jse_env(
        repo_root(),
        "2\n{\"name\":\"w\"}\n\n",
        &[cfg_arg.as_str(), "--accessible"],
        &[
            ("JSE_CACHE_DIR", cache_dir.as_str()),
            ("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json"),
        ],
        Duration::from_secs(10),
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("no schema found"),
        "stdout:\n{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("schema catalog unavailable"),
        "stdout:\n{}",
        r.stdout
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
    assert_eq!(read_json(&cfg)["name"], json!("w"));
}

#[test]
fn pty_schema_less_structured_editing() {
    let dir = mktemp();
    let cfg = dir.path().join("plain.json");
    write_file(&cfg, r#"{"name":"z","nested":{"port":8080}}"#);
    let cache = mktemp();
    let cfg_arg = arg(&cfg);
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };

    let (snap, mut off) = pty.wait_frame(
        "schema-less root",
        0,
        &["no schema found", "name", "nested", "Save and exit"],
    );
    assert!(
        !snap.contains("(not in schema)"),
        "schema-less keys must not be decorated as unknown:\n{snap}"
    );
    // The first key is a known string value: enter edits it in place.
    pty.send(b"\r");
    (_, off) = pty.wait_frame("inline scalar editor", off, &["> name", "z"]);
    pty.send(b"\x15"); // clear the prefilled value
    pty.send(b"web\r");
    (_, off) = pty.wait_frame("scalar edited in place", off, &["name", "web"]);
    pty.send(b"\x13"); // ctrl+s
    pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let got = read_json(&cfg);
    assert_eq!(got["name"], json!("web"), "saved doc = {got}");
}

#[test]
fn pty_schema_less_whole_doc_edit_rebuilds_root() {
    let dir = mktemp();
    let cfg = dir.path().join("shape.json");
    let editor = dir.path().join("editor.sh");
    write_file(&cfg, r#"{"name":"z"}"#);
    write_file(
        &editor,
        "#!/bin/sh\nprintf '%s\\n' '[1, \"two\"]' > \"$1\"\n",
    );
    set_executable(&editor);
    let cache = mktemp();
    let cfg_arg = arg(&cfg);
    let editor_arg = arg(&editor);
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[
            ("EDITOR", editor_arg.as_str()),
            ("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json"),
        ],
        &cache,
    ) else {
        return;
    };

    pty.wait_frame("schema-less object root", 0, &["name", "Save and exit"]);
    pty.send(b"G\r"); // the last row: < Edit raw JSON >
    pty.wait_frame("raw edit applied", 0, &["Raw JSON edited"]);
    // The document is now an array: the root menu follows the new shape.
    let (_, off) = pty.wait_frame("array root", 0, &["< Add item >", "$[0]", "$[1]"]);
    pty.send(b"\x13"); // ctrl+s
    pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    assert_eq!(read_json(&cfg), json!([1, "two"]));
}

#[test]
fn pty_json_and_yaml_titles_match_toml_behavior() {
    let schema_src = r#"{
      "type": "object",
      "properties": {
        "server": {
          "type": "object",
          "title": "Server Title",
          "properties": {"port": {"type": "integer"}}
        }
      }
    }"#;
    for (name, config) in [
        ("config.json", "{\"server\":{\"port\":8080}}\n"),
        ("config.yaml", "server:\n  port: 8080\n"),
    ] {
        let dir = mktemp();
        let schema = dir.path().join("schema.json");
        let cfg = dir.path().join(name);
        write_file(&schema, schema_src);
        write_file(&cfg, config);
        let schema_arg = arg(&schema);
        let cfg_arg = arg(&cfg);
        let cache = mktemp();
        let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
            return;
        };

        let (_, off) = pty.wait_frame("root title is the file name", 0, &[name, "Server Title"]);
        pty.send(b"\r");
        pty.wait_frame(
            "submenu keeps the schema title",
            off,
            &["Server Title", "port"],
        );
        pty.send(b"\x03");
        pty.wait_exit(130);
    }
}

#[test]
fn pty_duplicate_child_titles_show_their_keys() {
    let dir = mktemp();
    let schema = dir.path().join("profiles.schema.json");
    let cfg = dir.path().join("Cargo.toml");
    let src = json!({
        "type": "object",
        "properties": {"profile": {"$ref": "#/definitions/Profiles"}},
        "definitions": {
            "Profile": {
                "title": "Profile with Build Override",
                "type": "object",
                "properties": {"opt-level": {"type": "string"}}
            },
            "Profiles": {
                "title": "Profiles",
                "type": "object",
                "properties": {
                    "bench": {"$ref": "#/definitions/Profile"},
                    "dev": {"$ref": "#/definitions/Profile"},
                    "release": {"$ref": "#/definitions/Profile"},
                    "test": {"$ref": "#/definitions/Profile"}
                },
                "additionalProperties": {"$ref": "#/definitions/Profile"}
            }
        }
    });
    write_file(&schema, &src.to_string());
    write_file(&cfg, "[profile.release]\nopt-level = \"z\"\n");
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root", 0, &["profile", "Save and exit"]);
    pty.send(b"\r");
    let (snap, _) = pty.wait_frame(
        "profiles menu",
        0,
        &["Profiles", "bench", "dev", "release", "test"],
    );
    assert!(
        !snap.contains("Profile with Build Override"),
        "duplicate titles hid the profile keys:\n{snap}"
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_long_comment_page_scrolls_and_closes() {
    let dir = mktemp();
    let schema = dir.path().join("long-desc.schema.json");
    let cfg = dir.path().join("cfg.json");
    let desc = (1..=80)
        .map(|i| format!("COMMENT LINE {i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let src = json!({
        "type": "object",
        "properties": {"name": {"type": "string", "title": "Name", "description": desc}}
    });
    write_file(&schema, &src.to_string());
    write_file(&cfg, r#"{"name":"x"}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root with description", 0, &["Name", "Save and exit"]);
    pty.send(b"c");
    let (snap, off) = pty.wait_frame(
        "long comment page",
        0,
        &["Comment: Name", "COMMENT LINE 01", "c/q/esc close"],
    );
    assert!(
        !snap.contains("COMMENT LINE 80"),
        "first page shows the tail:\n{snap}"
    );

    let down = "j".repeat(35);
    pty.send(down.as_bytes());
    pty.wait_frame("scrolled comment page", off, &["COMMENT LINE 36"]);

    // PgDn (CSI 6~) and ctrl+f page forward; ctrl+b and PgUp (CSI 5~) page
    // back. Four screens run past both ends and clamp there.
    pty.send(b"\x1b[6~\x1b[6~\x06\x06");
    let (_, off) = pty.wait_frame("paged to the comment tail", off, &["COMMENT LINE 80"]);
    pty.send(b"\x02\x02\x1b[5~\x1b[5~");
    pty.wait_frame("paged back to the top", off, &["COMMENT LINE 01"]);

    pty.send(b"c");
    pty.wait_frame("menu after comment page", 0, &["Name", "Save and exit"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_yaml_schema_less_editing_keeps_comments() {
    let dir = mktemp();
    let cfg = dir.path().join("plain.yaml");
    write_file(&cfg, "# keep me\nname: z # inline\n");
    let cache = mktemp();
    let cfg_arg = arg(&cfg);
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };

    let (_, mut off) = pty.wait_frame(
        "schema-less YAML root",
        0,
        &["no schema found", "name", "Edit raw YAML"],
    );
    pty.send(b"\r"); // the only key: edit the string in place
    (_, off) = pty.wait_frame("inline YAML scalar editor", off, &["> name", "z"]);
    pty.send(b"\x15");
    pty.send(b"web\r");
    (_, off) = pty.wait_frame("YAML scalar edited", off, &["name", "web"]);
    pty.send(b"\x13");
    pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&cfg);
    assert!(text.contains("# keep me"), "header comment lost:\n{text}");
    assert!(
        text.contains("name: web # inline"),
        "value or inline comment lost:\n{text}"
    );
    assert!(
        !text.contains("$schema:"),
        "schema-less YAML gained a schema key:\n{text}"
    );
}

#[test]
fn yaml_schema_member_records_location_when_declared() {
    let dir = mktemp();
    let schema = dir.path().join("with-member.schema.json");
    let cfg = dir.path().join("config.yaml");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"$schema":{"type":"string","title":"Schema"},"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&cfg, "name: demo\n");
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    // blank keeps the prefilled $schema; then edit Name; blank saves.
    let r = run_jse(
        "\nnewname\n\n",
        &["-s", schema_arg.as_str(), cfg_arg.as_str(), "--accessible"],
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    let text = read_text(&cfg);
    assert!(
        text.contains(&format!("$schema: {schema_arg}")),
        "schema location not written to the declared field:\n{text}"
    );
    assert!(
        !text.contains("yaml-language-server"),
        "directive comment written despite the declared field:\n{text}"
    );
    assert!(text.contains("name: newname"), "edit lost:\n{text}");
}

#[test]
fn yaml_editing_accessible() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let cfg = dir.path().join("config.yaml");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}},"required":["name"]}"#,
    );
    write_file(&cfg, "# config header\nname: demo # keep me\n");

    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let r = run_jse(
        "newname\n\n",
        &["-s", schema_arg.as_str(), cfg_arg.as_str(), "--accessible"],
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);

    let text = read_text(&cfg);
    let want_directive = format!("# yaml-language-server: $schema={schema_arg}");
    assert!(
        text.contains(&want_directive),
        "yaml schema directive missing:\n{text}"
    );
    assert!(
        text.contains("# config header"),
        "header comment lost:\n{text}"
    );
    assert!(
        text.contains("name: newname # keep me"),
        "value or line comment lost:\n{text}"
    );
    assert!(!text.contains("name: demo"), "old value survived:\n{text}");
    assert!(
        !text.contains("$schema:"),
        "JSON-style $schema key written into YAML:\n{text}"
    );
    // Structural check via the crate's YAML dependency (Go used yaml.Unmarshal).
    let doc = yaml_doc(&text);
    let map = doc.as_hash().expect("saved YAML is a mapping");
    assert_eq!(map.len(), 1, "saved doc: {doc:?}");
    assert_eq!(
        yaml_get(&doc, "name"),
        Some(&Yaml::String("newname".into())),
        "saved doc: {doc:?}"
    );

    // The directive alone supplies the schema on the next run (no -s).
    let r = run_jse("newname\n\n", &[cfg_arg.as_str(), "--accessible"]);
    assert_eq!(
        r.code, 0,
        "directive run exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("Saved to"),
        "directive run not saved:\n{}",
        r.stdout
    );
    let text = read_text(&cfg);
    assert!(
        text.contains(&want_directive)
            && text.contains("# config header")
            && text.contains("name: newname # keep me"),
        "comment/directive lost on second save:\n{text}"
    );
}

#[test]
fn toml_editing_accessible() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let cfg = dir.path().join("config.toml");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}},"required":["name"]}"#,
    );
    write_file(
        &cfg,
        "# config header\nname = \"demo\" # keep me\nhex = 0x1F\nwhen = 1979-05-27T07:32:00Z\n[server]\nport = 8080\n[[mounts]]\nsrc = \"/a\" # first\n[[mounts]]\nsrc = \"/b\"\n",
    );

    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let r = run_jse(
        "newname\n\n",
        &["-s", schema_arg.as_str(), cfg_arg.as_str(), "--accessible"],
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);

    let text = read_text(&cfg);
    let want_directive = format!("#:schema {schema_arg}");
    assert!(
        text.contains(&want_directive),
        "toml schema directive missing:\n{text}"
    );
    assert!(
        !text.contains("#:schema="),
        "unexpected directive form:\n{text}"
    );
    assert!(
        text.contains("# config header"),
        "header comment lost:\n{text}"
    );
    assert!(
        text.contains("name = \"newname\" # keep me"),
        "value or line comment lost:\n{text}"
    );
    assert!(
        !text.contains("name = \"demo\""),
        "old value survived:\n{text}"
    );
    assert!(
        text.contains("hex = 0x1F\n"),
        "numeric literal lost:\n{text}"
    );
    assert!(
        text.contains("when = 1979-05-27T07:32:00Z\n"),
        "datetime literal lost:\n{text}"
    );
    assert!(
        text.contains("[server]\nport = 8080\n"),
        "table content lost:\n{text}"
    );
    assert!(
        text.contains("[[mounts]]\nsrc = \"/a\" # first\n[[mounts]]\nsrc = \"/b\"\n"),
        "table array lost:\n{text}"
    );

    // The directive alone supplies the schema on the next run (no -s).
    let r = run_jse("newname\n\n", &[cfg_arg.as_str(), "--accessible"]);
    assert_eq!(
        r.code, 0,
        "directive run exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("Saved to"),
        "directive run:\n{}",
        r.stdout
    );
    let text = read_text(&cfg);
    assert!(
        text.contains(&want_directive) && text.contains("name = \"newname\" # keep me"),
        "comment/directive lost on second save:\n{text}"
    );
}

#[test]
fn toml_schema_member_records_location_when_declared() {
    let dir = mktemp();
    let schema = dir.path().join("with-member.schema.json");
    let cfg = dir.path().join("config.toml");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"$schema":{"type":"string","title":"Schema"},"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&cfg, "name = \"demo\"\n");
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    // blank keeps the prefilled $schema; then edit Name; blank saves.
    let r = run_jse(
        "\nnewname\n\n",
        &["-s", schema_arg.as_str(), cfg_arg.as_str(), "--accessible"],
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    let text = read_text(&cfg);
    assert!(
        text.contains(&format!("\"$schema\" = \"{schema_arg}\"")),
        "schema location not written to the declared field:\n{text}"
    );
    assert!(
        !text.contains("#:schema"),
        "directive comment written despite the declared field:\n{text}"
    );
    assert!(text.contains("name = \"newname\""), "edit lost:\n{text}");
}

#[test]
fn jsonc_editing_accessible() {
    for name in ["config.jsonc", "config.json"] {
        let dir = mktemp();
        let schema = dir.path().join("mini.schema.json");
        let cfg = dir.path().join(name);
        write_file(
            &schema,
            r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}},"required":["name"]}"#,
        );
        write_file(
            &cfg,
            "// config header\n{\n  \"name\": \"demo\" // keep me\n}\n",
        );

        let schema_arg = arg(&schema);
        let cfg_arg = arg(&cfg);
        let r = run_jse(
            "newname\n\n",
            &["-s", schema_arg.as_str(), cfg_arg.as_str(), "--accessible"],
        );
        assert_eq!(
            r.code, 0,
            "{name}: exit; stdout:\n{}\nstderr:\n{}",
            r.stdout, r.stderr
        );
        assert!(
            r.stdout.contains("Saved to"),
            "{name}: stdout:\n{}",
            r.stdout
        );
        let text = read_text(&cfg);
        assert!(
            text.contains("// config header"),
            "{name}: header comment lost:\n{text}"
        );
        assert!(
            text.contains(r#""name": "newname" // keep me"#),
            "{name}: value or line comment lost:\n{text}"
        );
        assert!(
            !text.contains(r#""name": "demo""#),
            "{name}: old value survived:\n{text}"
        );
    }
}

#[test]
fn remote_schema() {
    let schema =
        std::fs::read(repo_root().join("examples/server.schema.json")).expect("read schema");
    let http = HttpServer::start(schema);
    let out = mktemp();
    let out_file = out.path().join("out.json");
    let url = http.url.clone() + "/server.schema.json";
    let out_arg = arg(&out_file);
    let r = run_jse(
        CREATE_SCRIPT,
        &["-s", url.as_str(), out_arg.as_str(), "--accessible"],
    );
    assert_eq!(r.code, 0, "exit; stdout:\n{}", r.stdout);
    assert!(r.stdout.contains("Saved to"), "stdout:\n{}", r.stdout);
    let got = read_json(&out_file);
    assert_eq!(got["mode"], json!("http"), "wrong doc: {got}");
    assert_eq!(
        got["$schema"],
        json!(url),
        "$schema should record the remote URL: {got}"
    );
}

// --- pty tests --------------------------------------------------------------

#[test]
fn pty_smoke() {
    let out = mktemp();
    let out_file = out.path().join("o.json");
    let out_arg = arg(&out_file);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", "examples/server.schema.json", out_arg.as_str()],
        &[],
        &cache,
    ) else {
        return;
    };
    pty.wait_frame("form frame", 0, &["Save and exit"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert!(!out_file.exists(), "output file written despite abort");
}

#[test]
fn pty_issues_use_document_paths() {
    let dir = mktemp();
    let schema = dir.path().join("strict.schema.json");
    let cfg = dir.path().join("strict.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
		"port":{"type":"integer","title":"Port","minimum":1024},
		"items":{"type":"array","title":"Items","items":{"type":"object","properties":{
			"tag":{"type":"integer","title":"Tag","minimum":1}}}}}}"#,
    );
    write_file(&cfg, r#"{"port":1,"items":[{"tag":0}]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root issues row", 0, &["Save and exit", "validation error"]);
    pty.send(b"\r"); // the issues row is first at the root
    let (snap, off) = pty.wait_frame("issues overlay", 0, &["at '$.port'", "at '$.items[0].tag'"]);
    // The overlay lists errors in the validator library's instance-path order
    // (items before port here), unlike Go's walk order; assert the relative
    // ctrl+j / ctrl+k navigation rather than a fixed starting entry.
    let mut from = off.saturating_sub(2000);
    while !snap.is_char_boundary(from) {
        from -= 1;
    }
    let rest = &snap[from..];
    let last_cursor = rest
        .rfind("> at '")
        .map(|i| rest[i..].lines().next().unwrap_or("").to_string())
        .expect("cursor marker in the overlay");
    let first_is_port = last_cursor.contains("$.port");
    let (down_want, up_want) = if first_is_port {
        ("> at '$.items[0].tag'", "> at '$.port'")
    } else {
        ("> at '$.port'", "> at '$.items[0].tag'")
    };

    pty.send(b"\n"); // ctrl+j: move down
    let (_snap, off2) = pty.wait_frame("ctrl+j moves down", off, &[down_want]);
    pty.send(b"\x0b"); // ctrl+k: move up
    pty.wait_frame("ctrl+k moves up", off2, &[up_want]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_external_raw_editor() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let cfg = dir.path().join("o.json");
    let editor = dir.path().join("editor.sh");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&cfg, r#"{"name":"z"}"#);
    write_file(
        &editor,
        "#!/bin/sh\n\
         grep -q '\"name\": \"z\"' \"$1\" || exit 3\n\
         printf '%s\\n' '{\"name\":\"from-editor\"}' > \"$1\"\n",
    );
    set_executable(&editor);

    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let editor_arg = arg(&editor);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", &schema_arg, &cfg_arg],
        &[("EDITOR", &editor_arg)],
        &cache,
    ) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.send(b"G\r"); // jump to the last row: <Edit raw JSON>
    let (_, off) = pty.wait_frame("editor applied", 0, &["Raw JSON edited"]);
    pty.send(b"\x13"); // ctrl+s save
    let (_, off2) = pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);
    let _ = off2;
    assert_eq!(read_json(&cfg)["name"], json!("from-editor"));
}

#[test]
fn pty_external_raw_yaml_editor() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let cfg = dir.path().join("o.yaml");
    let editor = dir.path().join("editor.sh");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&cfg, "name: demo\n");
    write_file(
        &editor,
        "#!/bin/sh\n\
         case \"$1\" in *.yaml) ;; *) exit 5;; esac\n\
         grep -q '^name: demo$' \"$1\" || exit 3\n\
         grep -q '\"name\"' \"$1\" && exit 4\n\
         printf '# edited comment\\nname: edited\\n' > \"$1\"\n",
    );
    set_executable(&editor);

    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let editor_arg = arg(&editor);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", &schema_arg, &cfg_arg],
        &[("EDITOR", &editor_arg)],
        &cache,
    ) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit", "Edit raw YAML"]);
    pty.send(b"G\r"); // jump to the last row: <Edit raw YAML>
    let (_, off) = pty.wait_frame("YAML editor applied", 0, &["Raw YAML edited"]);
    pty.send(b"\x13"); // ctrl+s save
    pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&cfg);
    assert!(
        text.contains("# edited comment")
            && text.contains("name: edited")
            && !text.contains(r#""name""#),
        "saved config did not keep the raw YAML (and its comment):\n{text}"
    );
}

#[test]
fn pty_issues_search_filters() {
    let dir = mktemp();
    let schema = dir.path().join("strict.schema.json");
    let cfg = dir.path().join("strict.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
		"port":{"type":"integer","title":"Port","minimum":1024},
		"items":{"type":"array","title":"Items","items":{"type":"object","properties":{
			"tag":{"type":"integer","title":"Tag","minimum":1}}}}}}"#,
    );
    write_file(&cfg, r#"{"port":1,"items":[{"tag":0}]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root issues row", 0, &["Save and exit", "validation error"]);
    pty.send(b"\r"); // the issues row is first at the root
    let (_, mut off) = pty.wait_frame("issues overlay", 0, &["at '$.port'", "at '$.items[0].tag'"]);
    pty.send(b"/");
    (_, off) = pty.wait_frame("issues search", off, &["search: "]);
    pty.send(b"port");
    let (_, off) = pty.wait_frame("filtered issues", off, &["search: port", "at '$.port'"]);
    pty.send(b"\r"); // jump to the matching issue
    pty.wait_frame("jumped to port", off, &["> Port"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_enum_array_search_toggles() {
    let dir = mktemp();
    let schema = dir.path().join("levels.schema.json");
    let cfg = dir.path().join("levels.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
		"levels":{"type":"array","title":"Levels",
			"items":{"enum":["alpha","beta","gamma"]}}}}"#,
    );
    write_file(&cfg, r#"{"levels":["beta"]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    let (_, mut off) = pty.wait_frame("root menu", 0, &["Save and exit", "Levels"]);
    pty.send(b"\r"); // Levels is the only field row
    (_, off) = pty.wait_frame("enum array menu", off, &["alpha", "beta", "gamma"]);
    pty.send(b"/");
    (_, off) = pty.wait_frame("enum array search", off, &["search: "]);
    pty.send(b"ga");
    let (_, off) = pty.wait_frame("filtered toggles", off, &["search: ga", "gamma"]);
    pty.send(b"\r"); // toggle the match; the filter stays
    pty.send(b"\x1b"); // esc leaves the search input
    pty.pause(Duration::from_millis(200)); // do not merge ESC with ctrl+s
    pty.send(b"\x13"); // ctrl+s save
    pty.wait_frame("saved", off, &["Write to "]);
    let got = read_json(&cfg);
    assert_eq!(got["levels"], json!(["beta", "gamma"]), "saved doc = {got}");
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_unknown_key_type_picker() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let cfg = dir.path().join("o.json");
    let editor = dir.path().join("editor.sh");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&cfg, r#"{"name":"z","inline":"old","extra":{"old":1}}"#);
    write_file(&editor, "#!/bin/sh\nexit 9\n");
    set_executable(&editor);

    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let editor_arg = arg(&editor);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", &schema_arg, &cfg_arg],
        &[("EDITOR", &editor_arg)],
        &cache,
    ) else {
        return;
    };

    let (_, mut off) = pty.wait_frame("root menu", 0, &["inline", "extra", "(not in schema)"]);
    pty.send(b"j"); // issues row → Name
    (_, off) = pty.wait_frame("cursor on Name", off, &["> Name"]);
    pty.send(b"j"); // Name → $schema
    (_, off) = pty.wait_frame("cursor on $schema", off, &["> $schema"]);
    pty.send(b"j"); // $schema → inline
    (_, off) = pty.wait_frame("cursor on scalar unknown key", off, &["> inline"]);
    // a known string value edits directly, without the type picker
    pty.send(b"\r");
    (_, off) = pty.wait_frame("inline scalar editor", off, &["> inline", "old"]);
    pty.send(b"\x15"); // clear the in-place scalar input
    for ch in ["n", "e", "w"] {
        pty.send(ch.as_bytes()); // type the new scalar value in place
        (_, off) = pty.wait_frame(
            &format!("typed inline character {ch}"),
            off,
            &["> inline", ch],
        );
    }
    pty.send(b"\r"); // apply the scalar edit
    (_, off) = pty.wait_frame("scalar edited in place", off, &["inline", "new"]);
    pty.send(b"j"); // inline → extra
    (_, off) = pty.wait_frame("cursor on object unknown key", off, &["> extra"]);
    // a known object value drills straight into the schema-less menu
    pty.send(b"\r");
    (_, off) = pty.wait_frame("object menu", off, &["Object", "old"]);
    pty.send(b"\r"); // nested known number edits directly
    (_, off) = pty.wait_frame("nested number editor", off, &["old", "1"]);
    pty.send(b"\x15"); // clear, then type 2
    pty.send(b"2");
    pty.send(b"\r");
    (_, off) = pty.wait_frame("nested value edited", off, &["old", "2"]);
    pty.send(b"\x1b"); // back to the root menu
    (_, off) = pty.wait_frame("back on extra", off, &["> extra"]);
    // t explicitly opens the type picker for an untyped value
    pty.send(b"t");
    (_, off) = pty.wait_frame("object type picker", off, &["Object", "JSON"]);
    pty.send(b"/"); // in-picker search: empty query, cursor on the value
    (_, off) = pty.wait_frame("type search opened", off, &["search: ", "> ● Object"]);
    pty.send(b"u"); // "u" is search text here, not undo
    (_, off) = pty.wait_frame("u typed into search", off, &["search: u"]);
    pty.send(b"\x7f"); // clear the query, then narrow it ourselves
    pty.send(b"Obj");
    (_, off) = pty.wait_frame("type search narrowed", off, &["search: Obj", "Object"]);
    pty.send(b"\r"); // select the filtered Object type
    (_, off) = pty.wait_frame("object menu again", off, &["Object", "old"]);
    pty.send(b"\x13"); // ctrl+s save
    pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let got = read_json(&cfg);
    assert_eq!(got["extra"], json!({"old": 2}), "saved doc = {got}");
    assert_eq!(got["inline"], json!("new"), "saved doc = {got}");
    assert_eq!(got["name"], json!("z"), "saved doc = {got}");
}

#[test]
fn pty_held_move_burst_lands_on_last_key() {
    // Terminal key repeat arrives as a burst with no frame between keys.
    // The event loop coalesces a run into one move, but it must land on
    // exactly the same row as the same number of separate presses.
    let dir = mktemp();
    let schema = dir.path().join("wide.schema.json");
    let cfg = dir.path().join("wide.json");
    let mut props = serde_json::Map::new();
    let mut values = serde_json::Map::new();
    for i in 0..300 {
        let key = format!("field_{i:03}");
        props.insert(key.clone(), json!({"type": "string"}));
        values.insert(key, json!("v"));
    }
    write_file(
        &schema,
        &json!({"type": "object", "properties": props}).to_string(),
    );
    write_file(&cfg, &json!(values).to_string());
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    pty.wait_frame("wide menu", 0, &["field_000"]);
    let burst = "j".repeat(120);
    pty.send(burst.as_bytes());
    pty.wait_frame("burst lands", 0, &["> field_120"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_search_jump_into_array_variant() {
    let dir = mktemp();
    let schema = dir.path().join("rules.schema.json");
    let cfg = dir.path().join("config.json");
    write_file(
        &schema,
        r#"{
          "type": "object",
          "properties": {
            "dns": {
              "type": "object",
              "properties": {
                "rules": {
                  "type": "array",
                  "items": {
                    "oneOf": [
                      {
                        "title": "Rule0",
                        "type": "object",
                        "properties": {
                          "type": {"type": "string"},
                          "domain_suffix": {"type": "array", "items": {"type": "string"}},
                          "server": {"type": "string"}
                        }
                      },
                      {
                        "title": "Rule1",
                        "type": "object",
                        "properties": {
                          "type": {"type": "string"},
                          "inbound": {"type": "string"},
                          "ip_version": {"type": "string"},
                          "query_type": {"type": "string"},
                          "domain_suffix": {"type": "array", "items": {"type": "string"}},
                          "server": {"type": "string"}
                        }
                      }
                    ]
                  }
                }
              }
            }
          }
        }"#,
    );
    write_file(
        &cfg,
        r#"{"dns":{"rules":[{"type":"default","server":"other"},{"type":"default","inbound":"any","ip_version":"4","query_type":"A","domain_suffix":["a.com"],"server":"dns-proxy"}]}}"#,
    );
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.send(b"/");
    pty.wait_frame("search", 0, &["search:"]);
    pty.send(b"$.dns.rules[1].server");
    (_, _) = pty.wait_frame(
        "target search row",
        0,
        &["$.dns.rules[1].server  dns-proxy"],
    );
    pty.send(b"\r");
    (_, _) = pty.wait_frame(
        "array item menu with cursor on server",
        0,
        &["$.dns.rules[1]", "> server"],
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_quit_save_prompt() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let out_file = dir.path().join("o.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    let schema_arg = arg(&schema);
    let out_arg = arg(&out_file);
    let cache = mktemp();

    let start = |cache: &tempfile::TempDir| -> Option<PtySession> {
        pty_or_skip(&["-s", &schema_arg, &out_arg], &[], cache)
    };

    // unchanged: esc quits at once, nothing written, no prompt
    let Some(mut pty) = start(&cache) else { return };
    let (_, off) = pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.send(b"\x1b");
    let (all, _) = pty.wait_frame("clean exit", off, &["No changes, nothing written"]);
    assert!(
        !all.contains("Save changes?"),
        "clean quit must not prompt:\n{all}"
    );
    pty.wait_exit(0);
    assert!(!out_file.exists(), "clean quit wrote the file");
    drop(pty);

    // changed: esc asks whether to save; y writes and exits 0
    let Some(mut pty) = start(&cache) else { return };
    let (_, off) = pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.send(b"\rhello\r");
    let (_, off) = pty.wait_frame("edited name", off, &["●"]);
    pty.send(b"\x1b");
    let (_, off) = pty.wait_frame("save prompt", off, &["Save changes?", "n discard & exit"]);
    pty.send(b"y");
    pty.wait_frame("saved exit", off, &["Saved to"]);
    pty.wait_exit(0);
    assert_eq!(read_json(&out_file)["name"], json!("hello"));
    drop(pty);

    // changed: esc then n discards the edit and exits 130
    let Some(mut pty) = start(&cache) else { return };
    let (_, off) = pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.send(b"\r\x15bye\r");
    let (_, off) = pty.wait_frame("edited name", off, &["●"]);
    pty.send(b"\x1b");
    let (_, off) = pty.wait_frame("save prompt", off, &["Save changes?"]);
    pty.send(b"n");
    pty.wait_frame("discard exit", off, &["Aborted, no changes written"]);
    pty.wait_exit(130);
    assert_eq!(
        read_json(&out_file)["name"],
        json!("hello"),
        "discard rewrote the file"
    );
}

#[test]
fn pty_save() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let out_file = dir.path().join("o.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    let schema_arg = arg(&schema);
    let out_arg = arg(&out_file);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &out_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit"]);
    // type the name on the field row, enter to confirm, ctrl+s to save
    pty.pause(Duration::from_millis(300));
    pty.send(b"\rhello\r"); // enter edit on Name, type, confirm
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x13");

    // ctrl+s must write the file and keep the session open: wait for the
    // saved notice instead of an exit
    pty.wait_frame("saved notice", 0, &["Write to "]);
    pty.send(b"\x03"); // session continues after ctrl+s; abort now
    pty.wait_exit(130);
    let content = read_text(&out_file);
    assert!(content.contains(r#""name": "hello""#), "output = {content}");
}

#[test]
fn pty_menu_flow() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let out_file = dir.path().join("o.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
		"name":{"type":"string","title":"Name"},
		"server":{"type":"object","title":"Server","properties":{
			"port":{"type":"integer","title":"Port"}}}}}"#,
    );
    let schema_arg = arg(&schema);
    let out_arg = arg(&out_file);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &out_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.send(b"\x1b[B\r"); // ↓ → Server, enter → drill
    pty.pause(Duration::from_millis(300));
    pty.send(b"\r9050\r"); // enter edit on Port, type, confirm
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b"); // esc back to the root menu
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x13"); // ctrl+s save

    pty.wait_frame("saved notice", 0, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);
    let got = read_json(&out_file);
    assert_eq!(
        got,
        json!({"$schema": schema_arg, "server": {"port": 9050}})
    );
}

#[test]
fn pty_move_item() {
    let dir = mktemp();
    let schema = dir.path().join("tags.schema.json");
    let out_file = dir.path().join("o.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"tags":{"type":"array","title":"Tags","items":{"type":"string"}}}}"#,
    );
    let schema_arg = arg(&schema);
    write_file(
        &out_file,
        &format!(
            r#"{{"$schema": {}, "tags": ["alpha", "beta"]}}"#,
            serde_json::to_string(&schema_arg).unwrap()
        ),
    );
    let out_arg = arg(&out_file);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &out_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.send(b"\r"); // enter → Tags items
    // The breadcrumb of the drilled menu is the evidence: the root frame also
    // mentions "alpha" in the Tags row summary, so wait for both.
    let (_frame, off) = pty.wait_frame("items menu", 0, &["$.tags", "alpha"]);
    pty.send(b"\x1b[BJ"); // ↓ to item 0 (alpha), J moves it down
    // the swap is visible once the rows render beta above alpha
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut moved = false;
    while !moved {
        let snap = pty.snapshot();
        // Inspect the most recently rendered items frame: earlier frames in
        // the accumulated stream still carry the original order.
        if let Some(start) = snap.rfind("$.tags") {
            let frame = &snap[start..];
            if let (Some(i_b), Some(i_a)) = (frame.find("beta"), frame.find("alpha")) {
                if i_b < i_a {
                    moved = true;
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for the moved rows; last frame:\n{}",
                pty.snapshot()
            );
        }
        pty.pause(Duration::from_millis(20));
    }
    pty.send(b"\x13"); // ctrl+s save
    pty.wait_frame("saved notice", off, &["Write to "]);
    pty.send(b"\x03"); // ctrl+c abort
    pty.wait_exit(130);
    assert_eq!(read_json(&out_file)["tags"], json!(["beta", "alpha"]));
}

#[test]
fn pty_search() {
    let dir = mktemp();
    let cfg = dir.path().join("server.config.json");
    let src = std::fs::read(repo_root().join("examples/server.config.json")).unwrap();
    std::fs::write(&cfg, &src).unwrap();
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", "examples/server.schema.json", cfg_arg.as_str()],
        &[],
        &cache,
    ) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.send(b"/"); // open search (single key: chunked runes won't match)
    let (_, off) = pty.wait_frame("search view", 0, &["search:", "alt+x regex"]);
    pty.send(b"8080"); // root search: value hit on the row that owns it
    let prev = off;
    let (frame, off) = pty.wait_frame("value hits", off, &["$.server.port  8080"]);
    if frame[prev..].contains("$.server  port=8080 host=localhost +1") {
        panic!(
            "a container must not match through its child roll-up:\n{}",
            &frame[prev..]
        );
    }
    pty.send(b"\x1b"); // close search
    pty.pause(Duration::from_millis(200));
    pty.send(b"/"); // reopen: the "$." path syntax works in regex mode too
    let (_, off) = pty.wait_frame("search view again", off, &["search:"]);
    pty.send(b"$.server.port");
    let (frame, off) = pty.wait_frame("anchored path hit", off, &["$.server.port  8080"]);
    if frame[off..].contains("invalid regex") {
        panic!(
            "the path anchor must not reach the regexp engine:\n{}",
            &frame[off..]
        );
    }
    pty.send(b"\x1b"); // close search
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[B\x1b[B\x1b[B\r"); // ↓↓↓ enter → Server ($.server); row 0 is the issues row
    let (_, off) = pty.wait_frame("server menu", off, &["$.server"]);
    pty.send(b"/"); // scoped search: nothing below $.server
    let (_, off) = pty.wait_frame("scoped search view", off, &["search:"]);
    pty.send(b"name");
    let (frame, _) = pty.wait_frame("scoped search settles", off, &["search: name"]);
    if frame[off..].contains("$.name") {
        panic!("scoped search leaked $.name:\n{}", &frame[off..]);
    }
    pty.send(b"\x1b");
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03"); // ctrl+c abort

    pty.wait_exit(130);
    let now = std::fs::read(&cfg).unwrap();
    assert!(
        now == src,
        "abort must not rewrite the config ({} vs {} bytes)",
        now.len(),
        src.len()
    );
}

#[test]
fn pty_search_replace() {
    let dir = mktemp();
    let cfg = dir.path().join("server.config.json");
    let src = std::fs::read_to_string(repo_root().join("examples/server.config.json")).unwrap();
    // the shipped example carries mode "quic", illegal for the schema enum:
    // fix it so ctrl+s can actually save after the replace, and add a
    // schema-unknown string key so the replace must reach it too
    let fixed = src.replacen(
        r#""mode": "quic""#,
        r#""mode": "tcp", "comment": "localhost box""#,
        1,
    );
    std::fs::write(&cfg, &fixed).unwrap();
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", "examples/server.schema.json", cfg_arg.as_str()],
        &[],
        &cache,
    ) else {
        return;
    };

    let warnings = format!("{} 2 warnings", WARN);
    pty.wait_frame("root menu", 0, &["Save and exit", &warnings]);
    pty.send(b"r"); // open search-and-replace directly
    let (_, off) = pty.wait_frame("replace view", 0, &["replace:", "case-insensitive"]);
    pty.send(b"localhost");
    let (_, off) = pty.wait_frame("replace preview", off, &["$.server.host  localhost →"]);
    pty.send(b"\t127.0.0.1"); // focus the replace input, type the text
    let (_, off) = pty.wait_frame(
        "replacement typed",
        off,
        &["$.server.host  localhost → 127.0.0.1"],
    );
    pty.send(b"\x12"); // ctrl+r: replace all in scope
    let (_, off) = pty.wait_frame("replace-all notice", off, &["replaced 2 occurrence"]);
    pty.send(b"\x13"); // ctrl+s: persist
    pty.wait_frame("saved", off, &["Write to "]);
    let content = read_text(&cfg);
    assert!(
        content.contains(r#""host": "127.0.0.1""#),
        "replaced host missing after save:\n{content}"
    );
    assert!(
        content.contains("127.0.0.1 box"),
        "unknown-key comment not replaced after save:\n{content}"
    );
    assert!(
        !content.contains("localhost"),
        "old host still present after save:\n{content}"
    );
    pty.send(b"\x03"); // ctrl+c abort
    pty.wait_exit(130);
}

#[test]
fn pty_enum_picker_search() {
    let dir = mktemp();
    let cfg = dir.path().join("server.config.json");
    let src = std::fs::read_to_string(repo_root().join("examples/server.config.json")).unwrap();
    std::fs::write(&cfg, &src).unwrap();
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", "examples/server.schema.json", cfg_arg.as_str()],
        &[],
        &cache,
    ) else {
        return;
    };

    let (_, mut off) = pty.wait_frame("root menu", 0, &["Save and exit", "Name"]);
    pty.pause(Duration::from_millis(300)); // let the TUI finish raw-mode setup
    // the unknown-key status row sits above Name: ↓ Name, ↓ Mode, enter
    pty.send(b"\x1b[B\x1b[B\r");
    (_, off) = pty.wait_frame("enum picker", off, &["Mode", "http", "grpc", "tcp"]);
    pty.send(b"/");
    (_, off) = pty.wait_frame("enum search", off, &["search: "]);
    pty.send(b"GR"); // filtering is case-insensitive; only grpc matches
    (_, off) = pty.wait_frame("enum search narrowed", off, &["search: GR", "grpc"]);
    pty.send(b"\r");
    (_, off) = pty.wait_frame("picker closed", off, &["Mode"]);
    pty.send(b"\x13"); // ctrl+s: persist the picked option
    pty.wait_frame("saved", off, &["Write to "]);
    let content = read_text(&cfg);
    assert!(
        content.contains(r#""mode": "grpc""#),
        "enum search did not select grpc:\n{content}"
    );
    pty.send(b"\x03"); // ctrl+c abort
    pty.wait_exit(130);
}

#[test]
fn pty_undo_redo() {
    let dir = mktemp();
    let cfg = dir.path().join("server.config.json");
    let src = std::fs::read_to_string(repo_root().join("examples/server.config.json")).unwrap();
    // the shipped example carries mode "quic", illegal for the schema enum:
    // fix it so ctrl+s can actually save around the undo/redo steps
    let fixed = src.replacen(r#""mode": "quic""#, r#""mode": "tcp""#, 1);
    std::fs::write(&cfg, &fixed).unwrap();
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", "examples/server.schema.json", cfg_arg.as_str()],
        &[],
        &cache,
    ) else {
        return;
    };

    let (_, mut off) = pty.wait_frame("root menu", 0, &["server.config.json", "Name"]);
    pty.pause(Duration::from_millis(300)); // let the TUI finish raw-mode setup
    // the example carries a schema-unknown key, so a status row sits above
    // Name; step over it. name is pattern-bound (^[a-z][a-z0-9-]*$), so -x
    // is a legal suffix where "!" would be refused.
    pty.send(b"\x1b[B"); // ↓ off the status row
    pty.send(b"\r-x\r"); // enter on Name (input prefills demo), append, commit
    (_, off) = pty.wait_frame("name edit committed", off, &["demo-x"]);
    pty.send(b"u"); // undo
    (_, off) = pty.wait_frame("undone notice", off, &["undone"]);
    pty.send(b"\x13"); // ctrl+s: persist the reverted doc
    (_, off) = pty.wait_frame("save after undo", off, &["Write to "]);
    let content = read_text(&cfg);
    assert!(
        content.contains(r#""name": "demo""#) && !content.contains(r#""name": "demo-x""#),
        "undo did not revert the saved name:\n{content}"
    );
    pty.send(b"\x12"); // ctrl+r: redo
    (_, off) = pty.wait_frame("redone notice", off, &["redone", "demo-x"]);
    pty.send(b"\x13"); // ctrl+s: persist the re-applied doc
    pty.wait_frame("save after redo", off, &["Write to "]);
    let content = read_text(&cfg);
    assert!(
        content.contains(r#""name": "demo-x""#),
        "redo did not re-apply the saved name:\n{content}"
    );
    pty.send(b"\x03"); // ctrl+c abort
    pty.wait_exit(130);
}

/// Schema and YAML document shared by the alias tests: copy references base
/// through *x.
fn alias_pty_files() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = mktemp();
    let schema = dir.path().join("alias.schema.json");
    let cfg = dir.path().join("o.yaml");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
		"base":{"type":"object","title":"Base","properties":{
			"p":{"type":"integer","title":"P"}}},
		"copy":{"type":"object","title":"Copy","properties":{
			"p":{"type":"integer","title":"P"}}}}}"#,
    );
    write_file(&cfg, "base: &x\n  p: 1\ncopy: *x\n");
    (dir, schema, cfg)
}

/// Wait for the ctrl+s "Write to" notice (Go's read loop until saved).
fn wait_saved(pty: &mut PtySession) {
    pty.wait_frame("saved", 0, &["Write to "]);
}

#[test]
fn pty_yaml_alias_edit_keeps_reference() {
    let (_dir, schema, cfg) = alias_pty_files();
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b[B\r"); // ↓ → Copy, enter → drill into the alias
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b[B\r\x7f9\r"); // ↓ past < Expand >, edit P (backspace clears the prefilled 1)
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x13"); // ctrl+s save

    wait_saved(&mut pty);
    let screen = pty.snapshot();
    assert!(
        screen.contains("< Expand >"),
        "alias menu must offer < Expand > on top; screen:\n{screen}"
    );
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&cfg);
    // The yaml-language-server comment heads the file; check the data lines.
    assert!(
        text.contains("base: &x") && text.contains("p: 9"),
        "edit through the alias must write the anchored original:\n{text}"
    );
    assert!(
        text.contains("copy: *x"),
        "an unexpanded alias must be written back as a reference:\n{text}"
    );
}

#[test]
fn pty_yaml_alias_expand() {
    let (_dir, schema, cfg) = alias_pty_files();
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b[B\r"); // ↓ → Copy, enter → drill (Expand is row 0)
    pty.pause(Duration::from_millis(300));
    pty.send(b"\r"); // enter on < Expand >
    pty.pause(Duration::from_millis(300));
    pty.send(b"\r\x7f7\r"); // edit P on the now-independent copy
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x13"); // ctrl+s save

    wait_saved(&mut pty);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&cfg);
    assert!(
        text.contains("base: &x") && text.contains("p: 1"),
        "the anchor and its content must survive the expand:\n{text}"
    );
    assert!(
        !text.contains("*x"),
        "an expanded alias must be written as plain content:\n{text}"
    );
    let doc = yaml_doc(&text);
    let copy = yaml_get(&doc, "copy").expect("copy member");
    assert_eq!(
        yaml_get(copy, "p"),
        Some(&Yaml::Integer(7)),
        "expanded copy must hold its own edit, got {copy:?}:\n{text}"
    );
}

#[test]
fn pty_yaml_anchor_rename() {
    let (_dir, schema, cfg) = alias_pty_files();
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.pause(Duration::from_millis(300));
    pty.send(b"&\x7fy\r"); // & on the base row: rename x → y
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x13"); // ctrl+s save

    wait_saved(&mut pty);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&cfg);
    assert!(
        text.contains("base: &y") && text.contains("copy: *y"),
        "rename must rewrite the anchor and its references:\n{text}"
    );
    assert!(
        !text.contains("*x") && !text.contains("&x"),
        "the old anchor name must be gone:\n{text}"
    );
}

#[test]
fn pty_merge_keys() {
    let dir = mktemp();
    let schema = dir.path().join("wf.schema.json");
    let out_file = dir.path().join("wf.yaml");
    write_file(
        &schema,
        r#"{"type":"object","title":"Workflow","properties":{"jobs":{"type":"object","title":"Jobs","additionalProperties":{"type":"object","title":"Job","properties":{"name":{"type":"string","title":"Name"},"env":{"type":"object","title":"Env","properties":{"env1":{"type":"string","title":"Env1"}}},"outputs":{"type":"object","title":"Outputs","properties":{"version":{"type":"string","title":"Version"}}}}}}}}"#,
    );
    write_file(
        &out_file,
        "jobs:\n  build_base1: &base1\n    env:\n      env1: val1\n  build_base2: &base2\n    outputs:\n      version: v1\n  build2:\n    <<: [*base1, *base2]\n    name: Build Apple App Store clients\n",
    );
    let schema_arg = arg(&schema);
    let out_arg = arg(&out_file);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &out_arg], &[], &cache) else {
        return;
    };

    // drive: jobs → build2's job menu; cursor onto env; expand it; save
    pty.wait_frame("root menu", 0, &["Save and exit"]);
    pty.pause(Duration::from_millis(400));
    pty.send(b"\r"); // enter jobs
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b[B\x1b[B\r"); // ↓ ↓ enter: build2
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b[B"); // ↓ onto the merged env row (schema slot)
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b[B"); // ↓ onto the merged outputs row
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b[A"); // ↑ back onto env
    pty.pause(Duration::from_millis(300));
    pty.send(b"E"); // expand env into build2
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x13"); // ctrl+s

    pty.wait_frame_for("saved", 0, &["Write to "], Duration::from_secs(20));
    let frame = pty.snapshot();
    for want in [
        "<<: *base1 *base2",                  // title names the merge list
        "*base1",                             // env badge
        "*base2",                             // outputs badge
        "$.jobs.build2 → $.jobs.build_base1", // breadcrumb on the merged key
        "$.jobs.build2 → $.jobs.build_base2", // ...and on the other anchor
        "merged key expanded",                // E notice
    ] {
        assert!(frame.contains(want), "screen missing {want:?}:\n{frame}");
    }
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&out_file);
    for want in ["<<:", "*base1", "*base2", "&base1", "&base2"] {
        assert!(text.contains(want), "saved YAML missing {want:?}:\n{text}");
    }
    // Structural checks (Go parsed with yaml.v3, which resolves merge keys;
    // yaml-rust2 does not, so the merged values are checked as text above and
    // the document structure is checked here without merge resolution).
    let doc = yaml_doc(&text);
    let jobs = yaml_get(&doc, "jobs").expect("jobs");
    let build2 = yaml_get(jobs, "build2").expect("build2");
    assert_eq!(
        yaml_get(build2, "name"),
        Some(&Yaml::String("Build Apple App Store clients".into())),
        "build2 wrong after save: {build2:?}\n{text}"
    );
    let base1 = yaml_get(jobs, "build_base1").expect("build_base1");
    let base1_env = yaml_get(base1, "env").expect("base1 env");
    assert_eq!(
        yaml_get(base1_env, "env1"),
        Some(&Yaml::String("val1".into())),
        "anchor value changed by the expand: {base1:?}\n{text}"
    );
    assert!(
        text.contains("env:"),
        "expanded env must be written as build2's own member:\n{text}"
    );
}

#[test]
fn pty_multi_file_session() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let file_a = dir.path().join("a.json");
    let file_b = dir.path().join("b.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&file_a, r#"{"name": "alpha"}"#);
    write_file(&file_b, r#"{"name": "beta"}"#);
    let schema_arg = arg(&schema);
    let a_arg = arg(&file_a);
    let b_arg = arg(&file_b);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &a_arg, &b_arg], &[], &cache) else {
        return;
    };

    let (_, off) = pty.wait_frame("first file root", 0, &["a.json [1/2]", "Save and exit"]);
    pty.send(b"s"); // file switcher
    let (_, off) = pty.wait_frame("file picker", off, &["Files (2)", "b.json"]);
    pty.send(b"j\r"); // switch to b.json
    let (_, off) = pty.wait_frame("second file root", off, &["b.json [2/2]"]);
    pty.send(b"\x0f"); // ctrl+o back across the boundary
    let (_, off) = pty.wait_frame("back on first file", off, &["a.json [1/2]"]);
    pty.send(b"\t"); // ctrl+i forward again
    let (_, off) = pty.wait_frame("forward to second file", off, &["b.json [2/2]"]);
    pty.send(b"\r\x15gamma\r"); // edit b.json's name
    let (_, off) = pty.wait_frame("edited name", off, &["●"]);
    pty.send(b"\x13"); // ctrl+s saves the active file only
    pty.wait_frame("saved b.json", off, &["Write to ", b_arg.as_str()]);
    pty.send(b"\x03");

    pty.wait_exit(130);
    assert_eq!(read_json(&file_b)["name"], json!("gamma"));
    assert_eq!(read_json(&file_a)["name"], json!("alpha"));
}

#[test]
fn pty_multi_file_quit_saves_all() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let file_a = dir.path().join("a.json");
    let file_b = dir.path().join("b.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&file_a, r#"{"name": "alpha"}"#);
    write_file(&file_b, r#"{"name": "beta"}"#);
    let schema_arg = arg(&schema);
    let a_arg = arg(&file_a);
    let b_arg = arg(&file_b);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &a_arg, &b_arg], &[], &cache) else {
        return;
    };

    let (_, off) = pty.wait_frame("first file root", 0, &["a.json [1/2]"]);
    pty.send(b"\r\x15one\r"); // dirty a.json
    let (_, off) = pty.wait_frame("a.json dirty", off, &["●"]);
    pty.send(b"s"); // file switcher (keys split: a pty write of "sj"
    let (_, off) = pty.wait_frame("file picker", off, &["Files (2)"]); // coalesces runes)
    pty.send(b"j\r");
    let (_, off) = pty.wait_frame("second file root", off, &["b.json [2/2]"]);
    pty.send(b"\r\x15two\r"); // dirty b.json
    let (_, off) = pty.wait_frame("b.json dirty", off, &["●"]);
    pty.send(b"\x1b"); // quit at the root: one confirm for both files
    let (_, off) = pty.wait_frame("multi-file save prompt", off, &["Save changes in 2 files?"]);
    pty.send(b"y");
    let (all, _) = pty.wait_frame("saved exit", off, &["Saved to"]);
    pty.wait_exit(0);
    assert_eq!(
        all.matches("Saved to").count(),
        2,
        "output has the wrong number of \"Saved to\" lines:\n{all}"
    );
    assert_eq!(read_json(&file_a)["name"], json!("one"));
    assert_eq!(read_json(&file_b)["name"], json!("two"));
}

#[test]
fn accessible_multi_file() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let file_a = dir.path().join("a.json");
    let file_b = dir.path().join("b.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string"}}}"#,
    );
    write_file(&file_a, r#"{"name": "alpha"}"#);
    write_file(&file_b, r#"{"name": "beta"}"#);
    let schema_arg = arg(&schema);
    let a_arg = arg(&file_a);
    let b_arg = arg(&file_b);
    let cache = mktemp();
    let cache_dir = arg(cache.path());
    let r = run_jse_env(
        repo_root(),
        "\n\n",
        &["--accessible", "-s", &schema_arg, &a_arg, &b_arg],
        &[("JSE_CACHE_DIR", cache_dir.as_str())],
        Duration::from_secs(30),
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stderr.contains("unexpected argument"),
        "second file rejected:\n{}",
        r.stderr
    );
    assert_eq!(
        r.stdout.matches("Saved to").count(),
        2,
        "output has the wrong number of \"Saved to\" lines (one per file):\n{}",
        r.stdout
    );
    assert_eq!(read_json(&file_a)["name"], json!("alpha"));
    assert_eq!(read_json(&file_b)["name"], json!("beta"));
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .expect("editor metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod editor");
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) {
    panic!("the e2e suite drives a POSIX shell editor and a pty");
}

/// The built-in raw textarea used to render the whole value: on a big value
/// every keystroke repainted every line and the cursor could walk below the
/// terminal. It now shows one textarea page that follows the cursor, like
/// the Go/bubbles version.
#[test]
fn pty_internal_raw_textarea_is_windowed() {
    let dir = mktemp();
    let schema = dir.path().join("blob.schema.json");
    let cfg = dir.path().join("blob.json");
    // no "type" on blob: a known value edits directly, t offers types
    write_file(
        &schema,
        r#"{"type":"object","properties":{"blob":{"title":"Blob"}}}"#,
    );
    let mut value = String::from("{\n");
    for i in 0..60 {
        value.push_str(&format!("    \"k{i}\": {i},\n"));
    }
    value.push_str("    \"zlast\": 1\n  }");
    write_file(&cfg, &format!("{{\"blob\": {value}}}\n"));
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    pty.wait_frame("root menu", 0, &["Blob"]);
    pty.send(b"t"); // untyped field: t opens the type picker
    pty.wait_frame("type picker", 0, &["String", "JSON"]);
    pty.send(b"G\r"); // choose the JSON variant: built-in raw textarea
    let (_, off) = pty.wait_frame("raw textarea", 0, &["ctrl+s apply"]);
    pty.pause(std::time::Duration::from_millis(200));
    let snap = pty.snapshot();
    assert!(
        !snap.contains("zlast"),
        "the textarea painted the whole value instead of one page"
    );
    // walking to the end scrolls the one-page window to the tail
    for _ in 0..70 {
        pty.send(b"\x1b[B");
    }
    pty.wait_frame("textarea tail", off, &["zlast"]);
    pty.send(b"\x1b");
    pty.pause(std::time::Duration::from_millis(100));
    pty.send(b"\x03");
    pty.wait_exit(130);
}
