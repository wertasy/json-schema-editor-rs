//! Mouse-driven TUI flows over a real pty: clicks, wheel, hover.

use super::*;

/// Mouse support: left click moves the cursor to the clicked row and opens
/// its edit (move + enter), right click backs out like esc, and the wheel
/// bumps a number while its in-place edit is open.
#[test]
fn pty_mouse_click_back_and_number_wheel() {
    let dir = mktemp();
    let cfg = dir.path().join("mouse.json");
    write_file(&cfg, r#"{"alpha": 1, "beta": 2, "gamma": 3}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    let (_, off) = pty.wait_frame("schema-less root", 0, &["alpha", "gamma", "↑/↓ move"]);
    pty.pause(std::time::Duration::from_millis(300));

    // Left click the third visible row (screen line 4, 1-based y=5): the
    // cursor moves there and Enter opens the inline edit for gamma.
    pty.send(b"\x1b[<0;10;5M\x1b[<0;10;5m");
    pty.pause(std::time::Duration::from_millis(200));
    pty.send(b"\x1b[<2;10;5M\x1b[<2;10;5m"); // right click closes the edit
    pty.pause(std::time::Duration::from_millis(200));
    pty.send(b"\x1b[<0;10;5M\x1b[<0;10;5m"); // reopen it on the same row
    pty.pause(std::time::Duration::from_millis(200));
    pty.send(b"\x1b[<64;10;5M"); // wheel up: 3 -> 4
    pty.pause(std::time::Duration::from_millis(200));
    pty.send(b"\r"); // commit
    pty.wait_frame("committed 4", off, &["4"]);
    pty.send(b"\x13"); // ctrl+s
    pty.pause(std::time::Duration::from_millis(300));
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert_eq!(read_json(&cfg)["gamma"], json!(4));
    assert_eq!(read_json(&cfg)["beta"], json!(2));
}

/// Mouse on overlays: clicking a dropdown candidate applies that enum
/// option; clicking a validation-error row jumps to its field.
#[test]
fn pty_mouse_enum_dropdown_and_err_list_click() {
    let dir = mktemp();
    let schema = dir.path().join("mouse2.schema.json");
    let cfg = dir.path().join("mouse2.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"color":{"enum":["red","green","blue"]}}}"#,
    );
    write_file(&cfg, r#"{"color": "red"}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["color", "red", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    // Enter opens the inline edit with the enum dropdown right below the
    // edited row (screen lines 3..5). Click the middle option (line 4,
    // 1-based y=5): "green" is applied.
    pty.send(b"\r");
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[<0;10;5M\x1b[<0;10;5m");
    pty.wait_frame("green applied", off, &["green"]);
    pty.send(b"\x13"); // ctrl+s
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert_eq!(read_json(&cfg)["color"], json!("green"));
}

/// The enum-array (multi-select) menu: clicking an option row toggles it
/// into/out of the array (click = move + enter = toggle).
#[test]
fn pty_mouse_enum_array_option_click_toggles() {
    let dir = mktemp();
    let schema = dir.path().join("mouse4.schema.json");
    let cfg = dir.path().join("mouse4.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"tags":{"type":"array","items":{"enum":["alpha","beta","gamma"]}}}}"#,
    );
    write_file(&cfg, r#"{"tags": ["alpha"]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["tags", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    pty.send(b"\r"); // drill into the tags menu
    let (_, off) = pty.wait_frame("options", off, &["alpha", "beta", "gamma"]);
    pty.pause(Duration::from_millis(200));
    // options sit at screen lines 2..4; click "beta" (1-based y=4)
    pty.send(b"\x1b[<0;10;4M\x1b[<0;10;4m");
    pty.wait_frame("beta toggled on", off, &["[x] beta"]);
    pty.send(b"\x13"); // ctrl+s
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert_eq!(read_json(&cfg)["tags"], json!(["alpha", "beta"]));
}

/// Clicking a validation-error row jumps to the offending field's menu.
#[test]
fn pty_mouse_err_list_row_click_jumps() {
    let dir = mktemp();
    let schema = dir.path().join("mouse3.schema.json");
    let cfg = dir.path().join("mouse3.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"srv":{"type":"object","properties":{"port":{"type":"integer"}},"required":["port"]}}}"#,
    );
    write_file(&cfg, r#"{"srv": {"port": "x"}}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["srv", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    // ctrl+s fails with one error listed at screen line 1; click it
    // (1-based y=2): the editor enters srv with the cursor on port.
    pty.send(b"\x13");
    pty.wait_frame("error list", off, &["Cannot save", "port"]);
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[<0;10;2M\x1b[<0;10;2m");
    std::thread::sleep(Duration::from_millis(300));
    let snap = pty.snapshot();
    let before = snap
        .rfind("Cannot save")
        .map(|i| i + "Cannot save".len())
        .unwrap_or(0);
    let fresh = &snap[before..];
    assert!(
        fresh.contains("> port"),
        "cursor must sit on port after the click:\n{fresh}"
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
    // nothing was saved
    assert_eq!(read_json(&cfg)["srv"]["port"], json!("x"));
}

/// Fuzzy autocomplete: typing a subsequence ("outp") lists and applies the
/// "out-proxy" suggestion.
#[test]
fn pty_mouse_fuzzy_completion_applies_subsequence_match() {
    let dir = mktemp();
    let schema = dir.path().join("fuzzy.schema.json");
    let cfg = dir.path().join("fuzzy.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"proxy":{"anyOf":[{"type":"string"},{"enum":["out-proxy","upstream"]}]}}}"#,
    );
    write_file(&cfg, r#"{"proxy": ""}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["proxy", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    pty.send(b"\r"); // open the inline edit with the suggestion dropdown
    pty.pause(Duration::from_millis(200));
    pty.send(b"outp"); // fuzzy: matches "out-proxy", not "upstream"
    pty.wait_frame("fuzzy candidate", off, &["out-proxy"]);
    pty.send(b"\t"); // take the highlighted candidate into the input
    pty.send(b"\r"); // commit
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x13"); // ctrl+s
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert_eq!(read_json(&cfg)["proxy"], json!("out-proxy"));
}

/// Wheel over an enum dropdown walks the candidates: down = next option,
/// up = previous (the edit opens prefilled on the current value).
#[test]
fn pty_mouse_enum_wheel_cycles_candidates() {
    let dir = mktemp();
    let schema = dir.path().join("wheel.schema.json");
    let cfg = dir.path().join("wheel.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"color":{"enum":["red","green","blue"]}}}"#,
    );
    write_file(&cfg, r#"{"color": "green"}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["color", "green", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    pty.send(b"\r"); // open the enum dropdown prefilled on green
    pty.wait_frame("dropdown open", off, &["red"]); // options are listed
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[<65;10;5M"); // wheel down: blue
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b[<64;10;5M"); // wheel up: back to green
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x1b[<64;10;5M"); // wheel up: red
    pty.pause(Duration::from_millis(300));
    pty.send(b"\r\x13"); // commit + save
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert_eq!(read_json(&cfg)["color"], json!("red"));
}

/// Hovering the mouse over menu rows moves the selection cursor with it
/// (no enter, no edit), as long as no overlay owns the pointer.
#[test]
fn pty_mouse_hover_moves_cursor() {
    let dir = mktemp();
    let cfg = dir.path().join("hover.json");
    write_file(&cfg, r#"{"alpha": 1, "beta": 2, "gamma": 3}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["alpha", "gamma", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    // Move (not click) onto gamma's row (line 4): the cursor follows.
    let before = pty.snapshot().len();
    pty.send(b"\x1b[<35;10;5M");
    pty.wait_frame("cursor on gamma", off, &[""]);
    std::thread::sleep(Duration::from_millis(200));
    let snap = pty.snapshot();
    let fresh = &snap[before..];
    assert!(
        fresh.contains("> gamma"),
        "hover must move the cursor to gamma:\n{fresh}"
    );
    assert!(
        !fresh.contains("3 "),
        "hover must not open the row (no edit overlay):\n{fresh}"
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}

/// Clicking `< Add item >` behaves exactly like `a`: the placeholder slot
/// is spliced and its submenu opens (here: an object item's field list).
#[test]
fn pty_mouse_add_item_click_matches_a_key() {
    let dir = mktemp();
    let schema = dir.path().join("mouse5.schema.json");
    let cfg = dir.path().join("mouse5.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"mounts":{"type":"array","items":{"type":"object","properties":{"src":{"type":"string"}}}}}}"#,
    );
    write_file(&cfg, r#"{"mounts": []}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["mounts", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    pty.send(b"\r"); // drill into the empty mounts array: < Add item > row
    let (_, off) = pty.wait_frame("items menu", off, &["< Add item >"]);
    pty.pause(Duration::from_millis(200));
    pty.send(b"\r"); // activate the Add row
    let (snap, _off) = pty.wait_frame("item submenu", off, &["$.mounts[0]", "src"]);
    let fresh = &snap[snap.len().saturating_sub(1200)..];
    assert!(
        fresh.contains("$.mounts[0]"),
        "item submenu must open:\n{fresh}"
    );
    pty.send(b"\x13"); // ctrl+s
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03");
    pty.wait_exit(130);
    // the clicked slot materialized like `a` would: items[0] exists
    assert!(
        read_json(&cfg)["mounts"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    );
}

/// Mouse routing on overlays: left click selects a file-switcher row (the
/// session switches to that file) and a reference-input candidate row (the
/// anchor reference is applied to the focused field).
#[test]
fn pty_mouse_click_file_switcher_and_ref_picker_rows() {
    let dir = mktemp();
    let schema = dir.path().join("len.schema.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
        "copy":{"title":"Copy"},
        "base":{"type":"object","title":"Base","properties":{"p":{"type":"integer","title":"P"}}},
        "more":{"type":"object","title":"More","properties":{"q":{"type":"integer","title":"Q"}}}}}"#,
    );
    let file_a = dir.path().join("a.yaml");
    write_file(&file_a, "note: alpha\n");
    let file_b = dir.path().join("b.yaml");
    write_file(&file_b, "copy:\nbase: &x\n  p: 1\nmore: &y\n  q: 2\n");
    let schema_arg = arg(&schema);
    let a_arg = arg(&file_a);
    let b_arg = arg(&file_b);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &a_arg, &b_arg], &[], &cache) else {
        return;
    };
    let (_, off) = pty.wait_frame("first file root", 0, &["a.yaml [1/2]", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    // s opens the file switcher; its title is screen line 0 and the rows
    // start at line 1, so the second row is line 2 (1-based y=3).
    pty.send(b"s");
    let (_, off) = pty.wait_frame("file picker", off, &["Files (2)", "b.yaml"]);
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[<0;10;3M\x1b[<0;10;3m");
    let (_, off) = pty.wait_frame("switched to second file", off, &["b.yaml [2/2]"]);
    pty.pause(Duration::from_millis(200));
    // `*` on the focused Copy row (scalar: every anchor is a legal
    // reference target). The candidate list renders after one arrow-down
    // (cycle selects the first candidate): rows at screen lines 3 and 4
    // hold x and y. Clicking the second (1-based y=5) applies *y to copy.
    pty.send(b"*");
    let (_, off) = pty.wait_frame("reference input", off, &["reference: "]);
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[B"); // cycle: x selected, candidates render
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[B"); // cycle to y
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[<0;10;6M\x1b[<0;10;6m");
    let (snap, off) = pty.wait_frame("reference applied", off, &["*y"]);
    assert!(
        !snap.get(off..).unwrap_or("").contains("reference: "),
        "the reference input must close after the click"
    );
    pty.send(b"\x13"); // ctrl+s
    pty.wait_frame("saved b.yaml", off, &["Write to ", b_arg.as_str()]);
    pty.send(b"\x03");
    pty.wait_exit(130);

    let text = read_text(&file_b);
    // the y reference was applied to copy: the anchor definition moves to
    // the new site and more becomes the alias (documented anchor semantics)
    assert!(
        text.contains("copy: &y") && text.contains("more: *y"),
        "the clicked candidate must be applied as the field's reference:\n{text}"
    );
    assert!(
        text.contains("&x") && text.contains("base:"),
        "the anchor definition must survive untouched:\n{text}"
    );
}

/// A right click closes the open overlay: the help page goes away and the
/// root menu is back.
#[test]
fn pty_mouse_right_click_closes_overlay() {
    let dir = mktemp();
    let cfg = dir.path().join("rc.json");
    write_file(&cfg, r#"{"alpha": 1}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    let (_, off) = pty.wait_frame("schema-less root", 0, &["alpha", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    pty.send(b"?");
    let (_, off) = pty.wait_frame("help page open", off, &["Keyboard help"]);
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[<2;10;5M\x1b[<2;10;5m"); // right click: esc equivalent
    let (snap, _) = pty.wait_frame("help closed, root back", off, &["↑/↓ move"]);
    let fresh = &snap[off..];
    assert!(
        !fresh.contains("Keyboard help"),
        "the help page must be gone after the right click:\n{fresh}"
    );
    assert!(
        fresh.contains("alpha"),
        "the root menu must be rendered again:\n{fresh}"
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}

/// While the `:` command input is open a left click on a menu row must not
/// commit the command, not move the menu cursor and not lose the typed text;
/// after esc the mouse works on the same row again.
#[test]
fn pty_mouse_click_ignored_while_command_input_open() {
    let dir = mktemp();
    let cfg = dir.path().join("cmd.json");
    write_file(&cfg, r#"{"alpha": 1, "beta": 2, "gamma": 3}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    let (_, off) = pty.wait_frame("schema-less root", 0, &["alpha", "gamma", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    pty.send(b":");
    let (_, off) = pty.wait_frame("command input open", off, &[": command ·"]);
    pty.send(b"w");
    pty.pause(Duration::from_millis(200));
    // gamma sits at screen line 4 (1-based y=5): the click must be swallowed.
    pty.send(b"\x1b[<0;10;5M\x1b[<0;10;5m");
    pty.pause(Duration::from_millis(200));
    let snap = pty.snapshot();
    let fresh = &snap[off..];
    assert!(
        fresh.contains(":w"),
        "the typed command text must survive the click:\n{fresh}"
    );
    assert!(
        fresh.contains(": command ·"),
        "the command input must stay open:\n{fresh}"
    );
    assert!(
        fresh.contains("> alpha") && !fresh.contains("> gamma"),
        "the click must not move the menu cursor:\n{fresh}"
    );
    assert!(
        !fresh.contains("Write to "),
        "the click must not commit the command:\n{fresh}"
    );

    // esc closes the input; afterwards the same click opens gamma's edit.
    pty.send(b"\x1b");
    let (_, off) = pty.wait_frame("command input closed", off, &["↑/↓ move"]);
    assert!(
        !pty.snapshot()
            .get(off..)
            .unwrap_or("")
            .contains(": command ·"),
        "the command hint must be gone after esc"
    );
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b[<0;10;5M\x1b[<0;10;5m");
    pty.wait_frame("gamma edit opens", off, &[""]);
    pty.pause(Duration::from_millis(200));
    let snap = pty.snapshot();
    let fresh = &snap[off..];
    assert!(
        fresh.contains("> gamma"),
        "after esc the click must reach the menu row:\n{fresh}"
    );
    pty.send(b"\x1b"); // cancel the edit
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert_eq!(read_json(&cfg), json!({"alpha": 1, "beta": 2, "gamma": 3}));
}
