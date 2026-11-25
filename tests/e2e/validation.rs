//! Save-time validation, issue rows and validation bypass.

use super::*;

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

    pty.wait_frame("root issues row", 0, &["↑/↓ move", "validation error"]);
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

    pty.wait_frame("root issues row", 0, &["↑/↓ move", "validation error"]);
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

/// `:wq` refuses to exit while save-time validation fails; `:wq!` skips the
/// validation, writes the file and exits 0.
#[test]
fn command_wq_bang_skips_validation() {
    let dir = mktemp();
    let schema = dir.path().join("req.schema.json");
    let out_file = dir.path().join("o.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"port":{"type":"integer","title":"Port"}},"required":["port"]}"#,
    );
    write_file(&out_file, r#"{"name": "x"}"#);
    let schema_arg = arg(&schema);
    let out_arg = arg(&out_file);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &out_arg], &[], &cache) else {
        return;
    };

    let (_, off) = pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b":wq\r");
    let (_, off) = pty.wait_frame("save refused", off, &["Cannot save", "port"]);
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b"); // close the error list; the session must continue
    let (_, _off) = pty.wait_frame("back at the menu", off, &["↑/↓ move"]);
    pty.pause(Duration::from_millis(200));
    pty.send(b":wq!\r"); // forced save bypasses the validation
    pty.wait_exit(0);

    assert_eq!(read_json(&out_file)["name"], json!("x"));
}
