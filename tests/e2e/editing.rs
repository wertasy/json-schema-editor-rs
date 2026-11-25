//! Whole-document raw editing, comments, numeric quick keys, narrow
//! terminal editing and completion windows.

use super::*;

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

    pty.wait_frame("root with description", 0, &["Name", "↑/↓ move"]);
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
    pty.wait_frame("menu after comment page", 0, &["Name", "↑/↓ move"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_document_comment_display_and_c_edits_it() {
    let dir = mktemp();
    let schema = dir.path().join("comment.schema.json");
    let cfg = dir.path().join("cfg.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"port":{"type":"number","description":"schema port help"}}}"#,
    );
    write_file(&cfg, "{\n  // port doc note\n  \"port\": 8080\n}\n");
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame(
        "both description sources",
        0,
        &["schema port help", "port doc note"],
    );

    // C opens the comment editor directly; ctrl+s applies, and the text
    // survives to the saved file.
    pty.send(b"C");
    let (_, off) = pty.wait_frame("comment editor", 0, &["Edit comment", "ctrl+s apply"]);
    pty.send(b"\x05 EDITED");
    pty.send(b"\x13");
    let (_, off) = pty.wait_frame("editor closed", off, &["port doc note EDITED"]);
    pty.send(b"w");
    pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&cfg);
    assert!(text.contains("// port doc note EDITED"), "{text}");
    assert!(text.contains("\"port\": 8080"), "{text}");
}

#[test]
fn pty_numeric_quick_keys_step_and_seed() {
    let dir = mktemp();
    let schema = dir.path().join("number.schema.json");
    let cfg = dir.path().join("cfg.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
          "count":{"type":"integer"},
          "missing":{"type":"number"},
          "port":{"type":"integer","default":8080}
        }}"#,
    );
    write_file(&cfg, "{\"count\":5}\n");
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("numeric menu", 0, &["count", "missing"]);
    // 5 → 4 → 5 → 6
    pty.send(b"-");
    pty.send(b"+");
    pty.send(b"=");
    pty.wait_frame("stepped value", 0, &["6"]);
    // the missing schema field is seeded with -1
    pty.send(b"j");
    pty.send(b"-");
    // port has no value but a numeric default: + seeds 8080+1
    pty.send(b"j");
    pty.send(b"+");
    pty.send(b"w");
    pty.wait_frame("numeric changes saved", 0, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&cfg);
    assert!(text.contains("\"count\": 6"), "{text}");
    assert!(text.contains("\"missing\": -1"), "{text}");
    assert!(text.contains("\"port\": 8081"), "{text}");
}

#[test]
fn pty_numeric_quick_keys_array_items() {
    let dir = mktemp();
    let schema = dir.path().join("array-number.schema.json");
    let cfg = dir.path().join("cfg.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"nums":{"type":"array","items":{"type":"integer"}}}}"#,
    );
    write_file(&cfg, "{\"nums\":[3,4]}\n");
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root with array", 0, &["nums"]);
    pty.send(b"\r");
    pty.wait_frame("array menu", 0, &["nums[0]", "nums[1]"]);
    // array menu rows: item 0, item 1
    pty.send(b"-");
    pty.send(b"j");
    pty.send(b"+");
    pty.send(b"w");
    pty.wait_frame("array changes saved", 0, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let doc = read_json(&cfg);
    assert_eq!(doc["nums"], serde_json::json!([2, 5]), "{doc}");
}

#[test]
fn pty_narrow_terminal_edit_scrolls_with_the_caret() {
    // A value wider than the terminal used to clip at the right edge, so
    // the caret and freshly typed characters were invisible. The inline
    // input must scroll horizontally and stay within one line.
    let dir = mktemp();
    let schema = dir.path().join("narrow.schema.json");
    let cfg = dir.path().join("cfg.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string"}}}"#,
    );
    let long = format!("headmarker-{}-tailmarker", "m".repeat(44));
    write_file(&cfg, &format!("{{\"name\":\"{long}\"}}\n"));
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip_sized(&["-s", &schema_arg, &cfg_arg], &[], &cache, 40, 24)
    else {
        return;
    };

    pty.wait_frame("narrow root", 0, &["name"]);
    pty.send(b"\r"); // inline edit, caret at the value end
    pty.wait_frame(
        "edit viewport shows the caret surroundings",
        0,
        &["tailmarker"],
    );
    pty.send(b"ZZZ"); // typed at the caret; the head is far off screen
    let (snap, _) = pty.wait_frame("typed characters visible", 0, &["ZZZ"]);
    let row = snap
        .split("\r\n")
        .find(|l| l.contains("ZZZ"))
        .expect("the edited row");
    assert!(
        row.chars().count() <= 39,
        "the edited row must not wrap at 40 columns: {row:?}"
    );
    pty.send(b"\x1b"); // esc cancels the edit without writing
    pty.send(b"\x03");
    pty.wait_exit(130);

    assert_eq!(read_text(&cfg), format!("{{\"name\":\"{long}\"}}\n"));
}

#[test]
fn pty_right_edge_completion_window_stays_visible() {
    // A long value with the caret near the terminal's right edge: the
    // candidate window anchors at the input text start and must stay fully
    // on screen instead of shrinking or running past the edge.
    let dir = mktemp();
    let schema = dir.path().join("edge.schema.json");
    let cfg = dir.path().join("cfg.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"anyOf":[{"type":"string"},
          {"enum":["alpha-beta-gamma-1x","alpha-beta-gamma-2y","alpha-beta-gamma-3z"]}]}}}"#,
    );
    write_file(&cfg, "{\"name\":\"alpha-beta-gamma-\"}\n");
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip_sized(&["-s", &schema_arg, &cfg_arg], &[], &cache, 36, 24)
    else {
        return;
    };

    pty.wait_frame("narrow root", 0, &["name"]);
    pty.send(b"\r"); // edit opens with the caret at the value end
    let (snap, _) = pty.wait_frame("candidates", 0, &["1x", "2y", "3z"]);
    let row = snap
        .split("\r\n")
        .find(|l| l.contains("2y"))
        .expect("a candidate row");
    assert!(
        row.chars().count() <= 35,
        "the candidate window must not run past 36 columns: {row:?}"
    );
    pty.send(b"\t"); // select the first candidate; the 19-char value shows
    // its tail in the input window on a 36-column terminal
    pty.wait_frame("candidate applied", 0, &["gamma-1x"]);
    pty.send(b"\x1b");
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_raw_editor_json() {
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
         grep -qx 'z' \"$1\" || exit 3\n\
         printf '%s\\n' 'from-editor' > \"$1\"\n",
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

    pty.wait_frame("root menu", 0, &["Name", "↑/↓ move"]);
    pty.send(b"E"); // the Name string opens as its own text, no quotes
    let (_, off) = pty.wait_frame("editor applied", 0, &["Value edited"]);
    pty.send(b"\x13"); // ctrl+s save
    pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert_eq!(read_json(&cfg)["name"], json!("from-editor"));
}

#[test]
fn pty_raw_editor_yaml() {
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
         grep -q '^demo$' \"$1\" || exit 3\n\
         printf 'edited\\n' > \"$1\"\n",
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

    pty.wait_frame("root menu", 0, &["Name", "↑/↓ move"]);
    pty.send(b"E");
    let (_, off) = pty.wait_frame("YAML editor applied", 0, &["Value edited"]);
    pty.send(b"\x13"); // ctrl+s save
    pty.wait_frame("saved", off, &["Write to "]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&cfg);
    assert!(text.ends_with("name: edited\n"), "saved YAML:\n{text}");
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
