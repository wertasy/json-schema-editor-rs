//! YAML-specific TUI behavior: aliases, anchors, merge keys, comments.

use super::*;

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
        &["no schema found", "name", "↑/↓ move"],
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

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
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

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
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

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
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
    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
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
    pty.send(b"\x05"); // ctrl+e: expand env into build2
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
