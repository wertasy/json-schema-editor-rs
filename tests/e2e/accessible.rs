//! Accessible-mode line-protocol sessions.

use super::*;

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
