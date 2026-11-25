//! Schema resolution (-s, relative, URL, cache) and schema-location
//! recording for YAML/TOML directives.

use super::*;

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
