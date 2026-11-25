//! General TUI flows: navigation, search, undo/redo, arrays, variants,
//! clipboard, smoke runs.

use super::*;

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
        &["no schema found", "name", "nested", "↑/↓ move"],
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
    write_file(&cfg, "{}");
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

    pty.wait_frame("schema-less object root", 0, &["< Add Key >", "↑/↓ move"]);
    pty.send(b"E"); // no value row: external editor with the whole document
    pty.wait_frame("raw edit applied", 0, &["Raw JSON edited"]);
    // The document is now an array: the root menu follows the new shape.
    let (_, off) = pty.wait_frame("array root", 0, &["$[0]", "$[1]"]);
    pty.send(b"\x13"); // ctrl+s
    pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    assert_eq!(read_json(&cfg), json!([1, "two"]));
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

    pty.wait_frame("root", 0, &["profile", "↑/↓ move"]);
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
    pty.wait_frame("form frame", 0, &["↑/↓ move"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert!(!out_file.exists(), "output file written despite abort");
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

    let (_, mut off) = pty.wait_frame("root menu", 0, &["↑/↓ move", "Levels"]);
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

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
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
fn pty_undo_of_child_edit_stays_on_the_key() {
    // schema a = oneOf[null, {b}]: pick {b}, edit b, undo — the undo must
    // keep the pinned {b} menu open and land the cursor back on b, not exit
    // to the root menu.
    let dir = mktemp();
    let schema = dir.path().join("uqx.schema.json");
    let cfg = dir.path().join("uqxnullcfg.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
          "a":{"oneOf":[{"type":"null"},{"type":"object","properties":{"b":{"type":"string"}}}]}}}"#,
    );
    write_file(&cfg, r#"{"a": null}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", &schema_arg, &cfg_arg],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"\r"); // variant picker for a
    (_, _) = pty.wait_frame("variant picker", 0, &["{b}"]);
    pty.send(b"j\r"); // pick {b}
    let (_, off) = pty.wait_frame("inside a", 0, &["variant: {b}"]);
    pty.send(b"\r1\r"); // edit b (absent -> "1")
    (_, _) = pty.wait_frame("child value set", off, &["●"]);
    pty.send(b"u");
    let (snap, off2) = pty.wait_frame("undone", off, &["undone"]);
    // the accumulated snapshot also contains pre-undo frames ($.a picker
    // title, "> b" seeded cursor), so assert on the tail written after the
    // edit: the pinned editor footer rules out ejection to the root menu
    let tail = &snap[off..off2];
    assert!(
        tail.contains("variant: {b}"),
        "still inside a's pinned editor:\n{tail}"
    );
    assert!(tail.contains("$.a"), "breadcrumb is a's menu:\n{tail}");
    assert!(tail.contains("> b"), "cursor back on key b:\n{tail}");
    pty.send(b"\x03");
    let _ = pty.wait_exit(130);
}

#[test]
fn pty_variant_switch_string_wraps_into_array() {
    // oneOf string | array[string]: switching the variant must keep the
    // typed string as the array's first element
    let dir = mktemp();
    let schema = dir.path().join("host.schema.json");
    let cfg = dir.path().join("host.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
          "host":{"oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]}}}"#,
    );
    write_file(&cfg, r#"{"host":"old.example"}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"t");
    (_, _) = pty.wait_frame("variant picker", 0, &["list of string"]);
    pty.send(b"j");
    pty.send(b"\r");
    (_, _) = pty.wait_frame("replace confirm", 0, &["Replace current value?"]);
    pty.send(b"y");
    (_, _) = pty.wait_frame(
        "array editor holds the old string",
        0,
        &["$.host", "host[0]", "old.example"],
    );
    pty.send(b"\x13"); // ctrl+s
    pty.wait_frame("saved", 0, &["Write to "]);
    let got = read_json(&cfg);
    assert_eq!(got["host"], json!(["old.example"]), "saved doc = {got}");
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_undo_lands_on_change_and_ctrl_j_cycles_the_ring() {
    // undo of a J move auto-lands on the first moved element; ctrl+j/k walk
    // the ring of changed rows
    let dir = mktemp();
    let schema = dir.path().join("items.schema.json");
    let cfg = dir.path().join("items.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
          "items":{"type":"array","items":{"type":"string"}}}}"#,
    );
    write_file(&cfg, r#"{"items":["a","b","c"]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"\r"); // drill into the array
    let (_, off) = pty.wait_frame("items menu", 0, &["$.items", "> items[0]         a"]);
    pty.send(b"J"); // move the first item down: [b a c], cursor follows it
    let (_, off) = pty.wait_frame("moved", off, &["> items[1]         a"]);
    pty.send(b"u"); // undo: [a b c], cursor auto-lands on items[0]
    let (_, off) = pty.wait_frame(
        "undo lands on the first change",
        off,
        &["> items[0]         a"],
    );
    pty.send(b"\n"); // ctrl+j: next changed row
    let (_, off) = pty.wait_frame("ctrl+j rings to items[1]", off, &["> items[1]         b"]);
    pty.send(b"\x0b"); // ctrl+k: back
    pty.wait_frame("ctrl+k rings back", off, &["> items[0]         a"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_a_inserts_after_cursor_in_array() {
    // a inserts a fresh item after the cursor item (A inserts before)
    let dir = mktemp();
    let schema = dir.path().join("items.schema.json");
    let cfg = dir.path().join("items.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
          "items":{"type":"array","items":{"type":"string"}}}}"#,
    );
    write_file(&cfg, r#"{"items":["a","b","c"]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"\r"); // drill into the array, cursor on items[0]
    let (_, off) = pty.wait_frame("items menu", 0, &["$.items", "> items[0]"]);
    pty.send(b"az\r"); // a: insert after items[0], type the value, commit
    pty.send(b"\x13"); // ctrl+s (the saved notice syncs the commit)
    pty.wait_frame("saved", off, &["Write to "]);
    let got = read_json(&cfg);
    assert_eq!(
        got["items"],
        json!(["a", "z", "b", "c"]),
        "a must insert after the cursor item; saved doc = {got}"
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_search_enum_array_element_jump() {
    // enum-array elements are document values: a document search lists them
    // and lands in the toggle menu on the matching row
    let dir = mktemp();
    let schema = dir.path().join("levels.schema.json");
    let cfg = dir.path().join("levels.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
		"levels":{"type":"array","title":"Levels",
			"items":{"enum":["debug","info","warn"]}}}}"#,
    );
    write_file(&cfg, r#"{"levels":["debug","info"]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"/");
    let (_, off) = pty.wait_frame("search", 0, &["search: "]);
    pty.send(b"info");
    let (_, off) = pty.wait_frame("enum element hit", off, &["$.levels[1]  info"]);
    pty.send(b"\r");
    pty.wait_frame(
        "toggle menu focused on info",
        off,
        &["$.levels", "> [x] info"],
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_search_schemaless_array_elements_jump() {
    // arrays without `items` and nested arrays have no per-element menu; the
    // elements (and nested values) are still searchable and the jump lands
    // on the field that owns the raw subtree
    let dir = mktemp();
    let schema = dir.path().join("free.schema.json");
    let cfg = dir.path().join("free.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
		"free":{"type":"array","title":"Free"},
		"matrix":{"type":"array","title":"Matrix",
			"items":{"type":"array","items":{"type":"integer"}}}}}"#,
    );
    write_file(&cfg, r#"{"free":["alpha","beta"],"matrix":[[1,2],[3,4]]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"/");
    let (_, mut off) = pty.wait_frame("search", 0, &["search: "]);
    pty.send(b"alpha");
    (_, off) = pty.wait_frame("free element hit", off, &["$.free[0]  alpha"]);
    pty.send(b"\r");
    let (_, mut off) = pty.wait_frame("cursor on the free field", off, &["> Free"]);

    pty.send(b"/");
    (_, off) = pty.wait_frame("second search", off, &["search: "]);
    pty.send(b"3");
    (_, off) = pty.wait_frame("nested element hit", off, &["$.matrix[1][0]  3"]);
    pty.send(b"\r");
    pty.wait_frame("cursor on the matrix field", off, &["> Matrix"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_search_array_variant_elements_jump() {
    // sing-box route.rules[].rule_set: an anyOf string|array field inside a
    // oneOf item; the array elements must be searchable and jump into the
    // items menu
    let dir = mktemp();
    let schema = dir.path().join("rules.schema.json");
    let cfg = dir.path().join("config.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
		"route":{"type":"object","properties":{
			"rules":{"type":"array","items":{"oneOf":[
				{"title":"DefaultRule","type":"object","properties":{
					"type":{"type":"string","enum":["default"]},
					"rule_set":{"anyOf":[
						{"type":"string"},
						{"type":"array","items":{"type":"string"}}]},
					"outbound":{"type":"string"}}},
				{"title":"LogicalRule","type":"object","properties":{
					"type":{"const":"logical"},
					"mode":{"type":"string","enum":["and","or"]}}}]}},
			"rule_set":{"type":"array","items":{"type":"object","properties":{
				"tag":{"type":"string"}}}}}}}}"#,
    );
    write_file(
        &cfg,
        r#"{"route":{"rules":[{"type":"default","rule_set":["ruleset-proxy-ai","ruleset-proxy-github"],"outbound":"proxy"}],"rule_set":[{"tag":"ruleset-proxy-ai"}]}}"#,
    );
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"/");
    let (_, off) = pty.wait_frame("search", 0, &["search: "]);
    pty.send(b"ruleset-proxy-ai");
    let (_, off) = pty.wait_frame(
        "element and tag hits",
        off,
        &[
            "$.route.rules[0].rule_set[0]  ruleset-proxy-ai",
            "$.route.rule_set[0].tag  ruleset-proxy-ai",
        ],
    );
    pty.send(b"\r");
    pty.wait_frame(
        "rule_set items menu focused on element 0",
        off,
        &["$.route.rules[0].rule_set", "> rule_set[0]"],
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_search_jump_schema_less() {
    // No schema: Enter on a deep hit must rebuild the runtime menus (object
    // value, array items) and land on the actual row.
    let dir = mktemp();
    let cfg = dir.path().join("cfg.json");
    write_file(
        &cfg,
        r#"{"server":{"port":8080,"host":"localhost"},"tags":["alpha","beta"],"arr":[{"name":"deep"}]}"#,
    );
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &[&cfg_arg],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move", "no schema found"]);
    pty.send(b"/");
    let (_, mut off) = pty.wait_frame("search", 0, &["search: "]);
    pty.send(b"port");
    (_, off) = pty.wait_frame("port hit", off, &["$.server.port  8080"]);
    pty.send(b"\r");
    (_, off) = pty.wait_frame("server menu on port", off, &["$.server", "> port"]);

    pty.send(b"\x1b"); // reopen the search list
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b"); // close it, back to the root
    pty.pause(Duration::from_millis(200));
    pty.send(b"/");
    (_, off) = pty.wait_frame("second search", off, &["search: "]);
    pty.send(b"beta");
    (_, off) = pty.wait_frame("array item hit", off, &["$.tags[1]  beta"]);
    pty.send(b"\r");
    (_, off) = pty.wait_frame("items menu on tags[1]", off, &["$.tags", "> tags[1]"]);

    pty.send(b"\x1b");
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b");
    pty.pause(Duration::from_millis(200));
    pty.send(b"/");
    (_, off) = pty.wait_frame("third search", off, &["search: "]);
    pty.send(b"deep");
    (_, off) = pty.wait_frame("nested hit", off, &["$.arr[0].name  deep"]);
    pty.send(b"\r");
    pty.wait_frame("item field menu on name", off, &["$.arr[0]", "> name"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_builtin_clipboard_round_trip_without_clipboard_tools() {
    // Empty PATH leaves no wl-copy/xclip/xsel/pbcopy: y must fall back to
    // the session's built-in clipboard and P must read it back.
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let cfg = dir.path().join("clip.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"$schema":{"type":"string"},"a":{"type":"string"},"b":{"type":"string"}}}"#,
    );
    write_file(&cfg, r#"{"a":"keep","b":"copy-me"}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", schema_arg.as_str(), cfg_arg.as_str()],
        &[("PATH", "")],
        &cache,
    ) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"jjY"); // copy b's value with no system clipboard tool
    pty.wait_frame(
        "built-in copy notice",
        0,
        &["Copied value (built-in clipboard)"],
    );
    pty.send(b"kP"); // paste onto a's string value
    pty.wait_frame("paste notice", 0, &["pasted value into $.a"]);
    pty.send(b":wq\r");
    pty.wait_exit(0);
    assert_eq!(
        read_json(&cfg)["a"],
        json!("copy-me"),
        "paste must come from the built-in clipboard"
    );
    assert_eq!(read_json(&cfg)["b"], json!("copy-me"));
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

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
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

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"\r"); // enter → Tags items
    // The breadcrumb of the drilled menu is the evidence: the root frame also
    // mentions "alpha" in the Tags row summary, so wait for both.
    let (_frame, off) = pty.wait_frame("items menu", 0, &["$.tags", "alpha"]);
    pty.send(b"J"); // cursor starts on item 0 (alpha), J moves it down
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

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
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
    pty.wait_frame("root menu", 0, &["↑/↓ move", &warnings]);
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
fn pty_enum_dropdown_search() {
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

    let (_, mut off) = pty.wait_frame("root menu", 0, &["↑/↓ move", "Name"]);
    pty.pause(Duration::from_millis(300)); // let the TUI finish raw-mode setup
    // the unknown-key status row sits above Name: ↓ Name, ↓ Mode, enter
    pty.send(b"\x1b[B\x1b[B\r");
    (_, off) = pty.wait_frame("enum dropdown", off, &["Mode", "http"]);
    pty.send(b"GR"); // typing over the highlighted option searches; only grpc matches
    (_, off) = pty.wait_frame("enum search narrowed", off, &["GR", "grpc"]);
    pty.send(b"\r");
    (_, off) = pty.wait_frame("dropdown closed", off, &["Mode"]);
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

/// The wheel scrolls the help page: two ticks down reveal entries from the
/// page's tail, two ticks up return to the top.
#[test]
fn pty_wheel_scrolls_help_page() {
    let dir = mktemp();
    let cfg = dir.path().join("wheel.json");
    write_file(&cfg, r#"{"alpha": 1}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    // A 12-row terminal shows 9 of the 47 help entries; the "p / P" entry
    // (desc: smart paste…) sits below the fold.
    let Some(mut pty) = pty_or_skip_sized(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
        80,
        12,
    ) else {
        return;
    };
    let (_, off) = pty.wait_frame("schema-less root", 0, &["alpha", "↑/↓ move"]);
    pty.send(b"?");
    let (_, off) = pty.wait_frame("help page open", off, &["Keyboard help"]);
    pty.pause(Duration::from_millis(200));
    let snap = pty.snapshot();
    let page = &snap[snap.rfind("Keyboard help").expect("help frame")..];
    assert!(
        page.contains("move (wraps at both ends") && !page.contains("smart paste"),
        "the top of the help page must show the first entries:\n{page}"
    );

    pty.send(b"\x1b[<65;10;5M"); // wheel down
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[<65;10;5M"); // wheel down again
    pty.wait_frame("help scrolled down", off, &["smart paste"]);
    pty.pause(Duration::from_millis(200));

    pty.send(b"\x1b[<64;10;5M"); // wheel up
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[<64;10;5M"); // wheel up again
    pty.pause(Duration::from_millis(300));
    let snap = pty.snapshot();
    let page = &snap[snap.rfind("Keyboard help").expect("help frame")..];
    assert!(
        !page.contains("smart paste"),
        "wheel up must scroll back to the top:\n{page}"
    );
    assert!(
        page.contains("move (wraps at both ends"),
        "the first entry must be visible again:\n{page}"
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}
