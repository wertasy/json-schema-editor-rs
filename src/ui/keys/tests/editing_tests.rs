//! Inline scalar editing, comments and description pages, numeric quick
//! keys and raw/textarea edits.

use super::*;
use super::{
    app_with, app_with_codec, app_with_meta, bool_at, dbuild, fnode, has, ks, mk_app, nget, num_at,
    press, schema_build, schema_from_json, set_cursor_path, sp, str_at, test_doc, test_opts,
    test_schema,
};

use super::*;

#[test]
fn menu_inline_edit_scalar() {
    let doc = test_doc();
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "down", "enter"]); // into Server
    press(&mut m, &["enter"]); // edit Port

    // invalid: below minimum keeps doc untouched, still editing
    press(&mut m, &["ctrl+u", "1", "enter"]);
    assert_eq!(
        num_at(&m.fs().doc, &["server", "port"]),
        "8080",
        "invalid input must not write"
    );
    assert!(
        matches!(m.overlay, Some(Overlay::Edit(_))),
        "edit must survive a failed confirm"
    );

    // valid: writes through and closes
    press(&mut m, &["ctrl+u", "9050", "enter"]);
    assert_eq!(
        num_at(&m.fs().doc, &["server", "port"]),
        "9050",
        "valid input should write"
    );
    assert!(
        !matches!(m.overlay, Some(Overlay::Edit(_))),
        "edit must close after success"
    );
}

#[test]
fn menu_bool_toggle() {
    let doc = test_doc(); // debug=true
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "down", "enter", "down", "down"]); // Server → Debug
    press(&mut m, &["space"]);
    assert!(
        !bool_at(&m.fs().doc, &["server", "debug"]),
        "space should flip debug to false"
    );
    press(&mut m, &["enter"]); // enter toggles too
    assert!(
        bool_at(&m.fs().doc, &["server", "debug"]),
        "enter should toggle bool rows"
    );
}

#[test]
fn menu_inline_edit_cancel_and_omit() {
    let doc = test_doc();
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "down", "enter"]); // Server
    press(&mut m, &["enter"]); // edit Port
    press(&mut m, &["ctrl+u", "9050", "enter"]);
    assert_eq!(num_at(&m.fs().doc, &["server", "port"]), "9050");

    // esc cancels without writing
    press(&mut m, &["enter", "ctrl+u", "1234", "esc"]);
    assert!(
        !matches!(m.overlay, Some(Overlay::Edit(_))),
        "esc must close the editor"
    );
    assert_eq!(
        num_at(&m.fs().doc, &["server", "port"]),
        "9050",
        "esc must not write"
    );

    // empty optional input removes the key (omit rule): Ratio has no default
    press(
        &mut m,
        &["down", "down", "down", "enter", "ctrl+u", "enter"],
    );
    assert!(
        !has(&m.fs().doc, &sp(&["server", "ratio"])),
        "empty optional input should remove the key"
    );
}

#[test]
fn text_area_view_draws_the_cursor() {
    let mut ta = TextArea::new();
    ta.set_viewport(10, 3);
    ta.set_value("ab\ncd");
    let view = ta_view(&ta);
    assert!(
        view.contains("\x1b[7ma\x1b[0mb"),
        "cursor cell missing at the start: {view:?}"
    );
    ta.handle_key(Key::End);
    let view = ta_view(&ta);
    assert!(
        view.contains("ab\x1b[7m \x1b[0m"),
        "cursor at end of line must use an inverted space: {view:?}"
    );
    // A long line scrolls horizontally to keep the cursor visible.
    ta.set_viewport(4, 3);
    ta.set_value("abcdef");
    ta.handle_key(Key::End);
    let view = ta_view(&ta);
    assert!(view.contains('\x1b'), "{view:?}");
    assert!(view.contains("def"), "{view:?}");
}

#[test]
fn numeric_quick_keys_leave_unrepresentable_literals_alone() {
    let schema = schema_from_json(r#"{"type":"object","properties":{"x":{"type":"number"}}}"#);
    let doc = dbuild(|db| db.obj(vec![("x", db.n("1e400"))]));
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["x"]));
    press(&mut m, &["+"]);
    assert_eq!(num_at(&m.fs().doc, &["x"]), "1e400");
    press(&mut m, &["-"]);
    assert_eq!(num_at(&m.fs().doc, &["x"]), "1e400");
}

#[test]
fn numeric_quick_keys_step_seed_and_preserve_precision() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "count":{"type":"integer"},
          "ratio":{"type":"number"},
          "big":{"type":"integer"},
          "name":{"type":"string"}
        }}"#,
    );
    let doc = dbuild(|db| {
        db.obj(vec![
            ("count", db.n("5")),
            ("ratio", db.n("1.5")),
            ("big", db.n("9007199254740993")),
            ("name", db.s("x")),
            ("extra", db.n("10")),
        ])
    });
    let mut m = app_with(schema, doc, test_opts());
    m.width = 80;
    m.height = 24;

    set_cursor_path(&mut m, &sp(&["count"]));
    press(&mut m, &["-"]);
    assert_eq!(num_at(&m.fs().doc, &["count"]), "4");
    press(&mut m, &["="]);
    assert_eq!(num_at(&m.fs().doc, &["count"]), "5");
    press(&mut m, &["+"]);
    assert_eq!(num_at(&m.fs().doc, &["count"]), "6");
    // one key = one undo step
    press(&mut m, &["u"]);
    assert_eq!(num_at(&m.fs().doc, &["count"]), "5");

    // integer precision is exact, not an f64 round trip
    set_cursor_path(&mut m, &sp(&["big"]));
    press(&mut m, &["+"]);
    assert_eq!(num_at(&m.fs().doc, &["big"]), "9007199254740994");
    press(&mut m, &["-"]);
    assert_eq!(num_at(&m.fs().doc, &["big"]), "9007199254740993");

    // fractional values keep their fraction
    set_cursor_path(&mut m, &sp(&["ratio"]));
    press(&mut m, &["+"]);
    assert_eq!(num_at(&m.fs().doc, &["ratio"]), "2.5");

    // no value: - seeds -1, = seeds 1
    set_cursor_path(&mut m, &sp(&["ratio"]));
    press(&mut m, &["d"]);
    assert_eq!(num_at(&m.fs().doc, &["ratio"]), "<missing>");
    set_cursor_path(&mut m, &sp(&["ratio"]));
    press(&mut m, &["-"]);
    assert_eq!(num_at(&m.fs().doc, &["ratio"]), "-1");
    press(&mut m, &["+"]);
    assert_eq!(num_at(&m.fs().doc, &["ratio"]), "0");
    press(&mut m, &["d"]);
    set_cursor_path(&mut m, &sp(&["ratio"]));
    press(&mut m, &["="]);
    assert_eq!(num_at(&m.fs().doc, &["ratio"]), "1");

    // non-numeric rows ignore the keys
    set_cursor_path(&mut m, &sp(&["name"]));
    press(&mut m, &["-", "=", "+"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "x");

    // a number the schema does not define works by its runtime value
    set_cursor_path(&mut m, &sp(&["extra"]));
    press(&mut m, &["-"]);
    assert_eq!(num_at(&m.fs().doc, &["extra"]), "9");
    press(&mut m, &["+"]);
    assert_eq!(num_at(&m.fs().doc, &["extra"]), "10");
}

#[test]
fn numeric_quick_keys_seed_from_the_schema_default() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "port":{"type":"integer","default":8080},
          "ratio":{"type":"number","default":0.5},
          "neg":{"type":"number","default":-3},
          "host":{"type":"string","default":"h"}
        }}"#,
    );
    let doc = dbuild(|db| db.obj(vec![]));
    let mut m = app_with(schema, doc, test_opts());
    m.width = 80;
    m.height = 24;

    set_cursor_path(&mut m, &sp(&["port"]));
    press(&mut m, &["-"]);
    assert_eq!(num_at(&m.fs().doc, &["port"]), "8079");
    press(&mut m, &["d"]);
    set_cursor_path(&mut m, &sp(&["port"]));
    press(&mut m, &["="]);
    assert_eq!(num_at(&m.fs().doc, &["port"]), "8081");

    // fractional and negative defaults keep their spelling
    set_cursor_path(&mut m, &sp(&["ratio"]));
    press(&mut m, &["="]);
    assert_eq!(num_at(&m.fs().doc, &["ratio"]), "1.5");
    set_cursor_path(&mut m, &sp(&["neg"]));
    press(&mut m, &["-"]);
    assert_eq!(num_at(&m.fs().doc, &["neg"]), "-4");
    press(&mut m, &["+"]);
    assert_eq!(num_at(&m.fs().doc, &["neg"]), "-3");

    // a non-numeric default does not make its key numeric
    set_cursor_path(&mut m, &sp(&["host"]));
    press(&mut m, &["-", "=", "+"]);
    assert!(
        m.fs().doc.get(&sp(&["host"])).is_none(),
        "host stays absent"
    );
}

#[test]
fn numeric_quick_keys_work_on_array_items() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "nums":{"type":"array","items":{"type":"number"}}
        }}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("nums", db.arr(vec![db.n("1"), db.n("2.5")]))]));
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["nums"]));
    press(&mut m, &["enter"]);
    // array menu: item 0, item 1
    press(&mut m, &["-"]);
    assert_eq!(nget(&m.fs().doc, &[ks("nums"), PathSeg::Idx(0)]), "0");
    press(&mut m, &["j", "+"]);
    assert_eq!(nget(&m.fs().doc, &[ks("nums"), PathSeg::Idx(1)]), "3.5");
    // schema-less root array: item rows are raw-typed but hold runtime numbers
    let doc = dbuild(|db| db.arr(vec![db.n("7")]));
    let schema = crate::smodel::untyped(&doc);
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["-"]);
    assert_eq!(nget(&m.fs().doc, &[PathSeg::Idx(0)]), "6");

    // schema-less nested array: the unknown key drills into a generic array
    let doc = dbuild(|db| db.obj(vec![("nums", db.arr(vec![db.n("11")]))]));
    let schema = crate::smodel::untyped(&doc);
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["nums"]));
    press(&mut m, &["enter"]);
    press(&mut m, &["-"]);
    assert_eq!(nget(&m.fs().doc, &[ks("nums"), PathSeg::Idx(0)]), "10");
}

#[test]
fn numeric_quick_keys_work_without_a_schema() {
    let doc = dbuild(|db| db.obj(vec![("n", db.n("7")), ("s", db.s("x"))]));
    let schema = crate::smodel::untyped(&doc);
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["n"]));
    press(&mut m, &["-"]);
    assert_eq!(num_at(&m.fs().doc, &["n"]), "6");
    press(&mut m, &["="]);
    assert_eq!(num_at(&m.fs().doc, &["n"]), "7");
    set_cursor_path(&mut m, &sp(&["s"]));
    press(&mut m, &["-"]);
    assert_eq!(str_at(&m.fs().doc, &["s"]), "x");
}

#[test]
fn edit_raw_current_node() {
    // E edits the value under the cursor, not the root document.
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["E"]);
    let edit = m.pending_edit.as_ref().expect("value editor armed");
    assert!(!edit.whole_doc, "a row value is not the whole document");
    assert_eq!(edit.target, sp(&["name"]));
    // a string value is text: the temp file holds it without quotes and the
    // edited text applies verbatim
    assert!(edit.verbatim_str, "string values arm a verbatim edit");
    assert_eq!(
        std::fs::read_to_string(&edit.path).unwrap(),
        "webapp",
        "the string opens unquoted"
    );
    m.finish_external_value("edited");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "edited");
    assert!(m.dirty());
    press(&mut m, &["u"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "webapp");

    // verbatim applies the text as-is: even JSON-invalid text is string
    // content (type changes go through the type picker instead)
    press(&mut m, &["E"]);
    m.finish_external_value("{oops");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "{oops");
    press(&mut m, &["u"]);

    // an editor failure keeps the document too
    press(&mut m, &["E"]);
    m.external_edit_failed("boom");
    assert!(m.notice.contains("boom"), "notice = {}", m.notice);

    // a container row edits that object/value in the file's format: the
    // parse still rejects invalid JSON and keeps the document
    press(&mut m, &["down", "down"]); // Server
    press(&mut m, &["E"]);
    let edit = m.pending_edit.as_ref().expect("value editor armed");
    assert!(!edit.whole_doc);
    assert!(!edit.verbatim_str, "containers keep format parsing");
    m.finish_external_value("{oops");
    assert_eq!(num_at(&m.fs().doc, &["server", "port"]), "8080");
    assert!(m.notice.contains("invalid JSON"), "notice = {}", m.notice);
    press(&mut m, &["E"]);
    let edit = m.pending_edit.as_ref().expect("value editor armed");
    assert_eq!(edit.target, sp(&["server"]));
    m.finish_external_value(r#"{"port":9090}"#);
    assert_eq!(num_at(&m.fs().doc, &["server", "port"]), "9090");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "webapp");

    // no value row under the cursor: fall back to the whole document
    let schema = schema_build(|b| {
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            ..FormNode::default()
        })
    });
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut m, &["E"]);
    assert!(
        m.pending_edit.as_ref().map_or(false, |e| e.whole_doc),
        "the empty-object placeholder falls back to the whole document"
    );
}

#[test]
fn external_edit_string_is_verbatim_text() {
    // A string value opens in $EDITOR as its own text — no quotes, no
    // format escaping, no YAML block scalar — and the edited text applies
    // verbatim: the value stays a string in every format.
    let doc = dbuild(|db| db.obj(vec![("name", db.s("line1\nline2"))]));
    let mut m = mk_app(doc, test_opts());
    press(&mut m, &["e"]);
    let edit = m.pending_edit.as_ref().expect("value editor armed");
    assert!(edit.verbatim_str);
    assert_eq!(
        std::fs::read_to_string(&edit.path).unwrap(),
        "line1\nline2",
        "JSON strings open without quotes or \\n escapes"
    );
    // the editor's trailing newline is not string content; spaces are
    m.finish_external_value("one two  \n");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "one two  ");
    press(&mut m, &["u"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "line1\nline2");

    // multiline text round-trips without \n escapes
    press(&mut m, &["e"]);
    m.finish_external_value("a\nb");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "a\nb");

    // YAML/TOML scalars that would parse as other types stay strings
    let doc = dbuild(|db| db.obj(vec![("name", db.s("007"))]));
    for ext in ["a.yaml", "a.toml"] {
        let mut m = app_with_codec(
            test_schema(),
            doc.clone(),
            test_opts(),
            crate::doc::codec_for(ext),
        );
        press(&mut m, &["e"]);
        let edit = m.pending_edit.as_ref().expect("value editor armed");
        assert!(edit.verbatim_str);
        assert_eq!(
            std::fs::read_to_string(&edit.path).unwrap(),
            "007",
            "{ext} opens the quoted scalar unquoted"
        );
        m.finish_external_value("008");
        assert_eq!(
            str_at(&m.fs().doc, &["name"]),
            "008",
            "{ext} applies verbatim"
        );
    }
}

// --- regression: the text input renders on the edited row (bug: enter on a
// scalar set fn_ = None, so inline_edit_row never matched and no input box
// appeared) ---------------------------------------------------------------

#[test]
fn inline_edit_renders_on_the_field_row() {
    let mut m = mk_app(test_doc(), test_opts());
    m.height = 30;
    press(&mut m, &["enter"]);
    let fn_id = match &m.overlay {
        Some(Overlay::Edit(e)) => e.fn_,
        _ => panic!("enter on a scalar must open the inline editor"),
    };
    assert!(fn_id.is_some(), "schema field edits keep the real node id");
    let rows = m.menu_rows();
    let view = m.view();
    let matched = match &m.overlay {
        Some(Overlay::Edit(e)) => m.inline_edit_row(&rows[0], e),
        _ => false,
    };
    assert!(matched, "the edited row must render the input");
    // the input row shows the current value with a reverse-video cursor cell
    assert!(
        view.contains("\u{1b}[7m") && view.contains("webapp"),
        "inline editor must render the value with a cursor cell:\n{view}"
    );
}

// --- regression: the raw editor renders only its viewport -----------------
// It used to emit the whole buffer every keystroke, and a cursor below the
// terminal's height was invisible (Go/bubbles showed one textarea page).

#[test]
fn raw_textarea_view_is_windowed() {
    use crate::ui::widgets::TextArea;
    let mut ta = TextArea::new();
    ta.set_viewport(20, 5);
    ta.set_value(
        &(0..50)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let plain = |s: &str| {
        regex::Regex::new("\x1b\\[[0-9;]*m")
            .unwrap()
            .replace_all(s, "")
            .to_string()
    };
    let v = crate::ui::keys::ta_view(&ta);
    let lines: Vec<&str> = v.split('\n').collect();
    assert_eq!(lines.len(), 5, "only the viewport is rendered: {lines:?}");
    assert_eq!(plain(lines[0]), "line-0");
    // the page follows the cursor row: after 20 newlines it rests on line 0's
    // row (line 20 of the buffer) and the old page top is gone
    for _ in 0..20 {
        ta.insert_newline();
    }
    let v = crate::ui::keys::ta_view(&ta);
    let page: Vec<&str> = v.split('\n').collect();
    assert_eq!(page.len(), 5);
    assert_eq!(
        plain(page[4]),
        "line-0",
        "cursor row is the last visible line: {page:?}"
    );
    assert!(
        !v.contains("line-30"),
        "off-page lines are not rendered: {v:?}"
    );
    // long lines are clipped to the textarea width, never wrapped
    let mut ta = TextArea::new();
    ta.set_viewport(10, 3);
    ta.set_value(&"x".repeat(100));
    let v = crate::ui::keys::ta_view(&ta);
    assert_eq!(plain(&v).chars().count(), 10);
}

// --- YAML string values edit as plain text --------------------------------
// A string is text, not a document: prefilling the raw textarea with its YAML
// encoding gave multiline values a "|" header plus the alignment spaces, and
// quoted values their quotes. Strings now open as their own content and apply
// verbatim; mappings/sequences still open as YAML.

fn raw_field_schema() -> Schema {
    schema_build(|b| {
        let msg = b.id(fnode("msg", "Msg", Kind::Raw));
        b.id(FormNode {
            key: String::new(),
            kind: Kind::Object,
            children: vec![msg],
            ..FormNode::default()
        })
    })
}

fn open_msg_textarea(schema: Schema, doc: Doc, codec: &'static dyn crate::doc::Codec) -> App {
    let mut m = app_with_codec(schema, doc, test_opts(), codec);
    // t opens the type picker; the JSON variant is the explicit
    // raw-textarea escape hatch
    press(&mut m, &["t"]);
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::OneOf,
        "t selects a different value type"
    );
    press(&mut m, &["G", "enter"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Text(_))),
        "the JSON variant edits in the built-in textarea"
    );
    m
}

fn textarea_text(m: &App) -> String {
    match &m.overlay {
        Some(Overlay::Text(s)) => s.ta.value(),
        _ => panic!("no textarea open"),
    }
}

#[test]
fn untyped_field_with_value_edits_directly() {
    let doc = dbuild(|db| db.obj(vec![("msg", db.s("old"))]));
    let mut m = app_with(raw_field_schema(), doc, test_opts());
    // a known runtime type edits directly; no picker, no external editor
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    assert!(m.pending_edit.is_none());
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("new"),
        _ => unreachable!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["msg"]), "new");

    // null has no inferable type: enter opens the picker
    let doc = dbuild(|db| db.obj(vec![("msg", db.push(Value::Null))]));
    let mut m = app_with(raw_field_schema(), doc, test_opts());
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
}

#[test]
fn yaml_multiline_string_opens_as_plain_text() {
    let (doc, _) = crate::doc::yamldoc::parse("msg: |\n  hello\n  world\n").unwrap();
    let m = open_msg_textarea(raw_field_schema(), doc, crate::doc::codec_for("a.yaml"));
    assert_eq!(
        textarea_text(&m),
        "hello\nworld\n",
        "the YAML literal header and its alignment spaces must not reach the editor"
    );
}

#[test]
fn yaml_string_edits_apply_verbatim() {
    let (doc, _) = crate::doc::yamldoc::parse("msg: |\n  hello\n  world\n").unwrap();
    let mut m = open_msg_textarea(raw_field_schema(), doc, crate::doc::codec_for("a.yaml"));
    // unchanged text keeps the value (no YAML re-parse surprises)
    press(&mut m, &["ctrl+s"]);
    assert_eq!(str_at(&m.fs().doc, &["msg"]), "hello\nworld\n");
    // a multiline edit is stored exactly, indentation and all
    let mut m2 = open_msg_textarea(
        raw_field_schema(),
        crate::doc::yamldoc::parse("msg: hello\n").unwrap().0,
        crate::doc::codec_for("a.yaml"),
    );
    if let Some(Overlay::Text(s)) = &mut m2.overlay {
        s.ta.set_value("  spaced\nline2");
    }
    press(&mut m2, &["ctrl+s"]);
    assert_eq!(str_at(&m2.fs().doc, &["msg"]), "  spaced\nline2");
}

#[test]
fn yaml_mapping_values_still_open_as_yaml() {
    let (doc, _) = crate::doc::yamldoc::parse("msg:\n  a: 1\n  b: two\n").unwrap();
    let m = open_msg_textarea(raw_field_schema(), doc, crate::doc::codec_for("a.yaml"));
    assert_eq!(textarea_text(&m), "a: 1\nb: two\n");
}

#[test]
fn json_string_values_open_as_plain_text_too() {
    // the same rule holds for JSON: the editor shows the string, not "\"v\""
    let doc = dbuild(|db| {
        let v = db.push(Value::Str("v".into()));
        db.obj(vec![("msg", v)])
    });
    let m = open_msg_textarea(raw_field_schema(), doc, crate::doc::codec_for("a.json"));
    assert_eq!(textarea_text(&m), "v");
}

#[test]
fn e_hints_no_value_to_edit_on_valueless_rows() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["down", "down", "down", "enter"]); // logging
    press(&mut m, &["enter"]); // levels: enum array
    assert_eq!(m.top().unwrap().kind, MenuKind::EnumArray);
    press(&mut m, &["e"]); // cursor on a toggle option row
    assert_eq!(m.notice, "no value to edit");
    assert!(m.pending_edit.is_none(), "no editor armed");

    // contrast: a row holding a value arms $EDITOR (stashed in tests)
    press(&mut m, &["esc", "esc", "home"]);
    press(&mut m, &["e"]);
    let edit = m.pending_edit.as_ref().expect("value editor armed");
    assert_eq!(edit.target, sp(&["name"]));
}

#[test]
fn comment_preview_is_capped_and_c_expands_it() {
    let desc = "L1\nL2\nL3\nL4\nL5\nTAILMARK";
    let src = serde_json::json!({
        "type": "object",
        "properties": {"name": {"type": "string", "title": "Name", "description": desc}}
    });
    let schema = schema_from_json(&src.to_string());
    let mut m = app_with(schema, test_doc(), test_opts());
    m.width = 40;
    m.height = 24;
    set_cursor_path(&mut m, &sp(&["name"]));

    let view = m.view();
    assert!(view.contains("L1") && view.contains("L2"), "{view}");
    assert!(
        view.contains("…"),
        "collapsed preview must mark the cut:\n{view}"
    );
    assert!(
        !view.contains("L3") && !view.contains("TAILMARK"),
        "collapsed block must stop at two lines:\n{view}"
    );

    press(&mut m, &["c"]);
    assert!(
        m.desc_expanded,
        "c must expand a description that fits one page"
    );
    let view = m.view();
    assert!(
        view.contains("TAILMARK"),
        "expanded description lost text:\n{view}"
    );

    // Any other key folds the description and still performs its action.
    let before = m.top().unwrap().cursor;
    press(&mut m, &["j"]);
    assert!(!m.desc_expanded, "j must collapse the expanded description");
    assert_eq!(
        m.top().unwrap().cursor,
        before + 1,
        "j must still move the cursor"
    );
    let view = m.view();
    assert!(
        !view.contains("TAILMARK"),
        "collapsed view still expanded:\n{view}"
    );
}

#[test]
fn document_comment_and_schema_help_render_together_and_c_edits_comment() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{"port":{"type":"number","description":"schema port help"}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("port", db.n("8080"))]));
    let mut meta = crate::doc::Meta::default();
    meta.set_comment_text(&sp(&["port"]), "port doc note", "//");
    let mut m = app_with_meta(schema, doc, test_opts(), meta);
    m.width = 90;
    m.height = 24;
    set_cursor_path(&mut m, &sp(&["port"]));

    // both sources, no labels, distinct colors
    let view = m.view();
    assert!(
        view.contains("schema port help") && view.contains("port doc note"),
        "{view}"
    );
    assert!(
        !view.contains("schema:") && !view.contains("comment:"),
        "source labels must be gone:\n{view}"
    );
    assert!(
        view.contains("\x1b[38;5;141m") && view.contains("\x1b[38;5;117m"),
        "schema and comment must use different colors:\n{view}"
    );

    // C edits the document comment directly, pre-filled with its text
    press(&mut m, &["C"]);
    match &mut m.overlay {
        Some(Overlay::CommentEdit(s)) => {
            assert_eq!(s.ta.value(), "port doc note");
            s.ta.set_value("edited note");
        }
        _ => panic!("C must open the comment editor"),
    }
    assert!(
        m.view().contains("\x1b[7me\x1b[0m"),
        "the comment editor must draw the cursor cell"
    );
    press(&mut m, &["ctrl+s"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        m.fs().meta.comment_text(&sp(&["port"])).as_deref(),
        Some("edited note")
    );
    assert!(m.view().contains("edited note"));
}

#[test]
fn c_adds_and_removes_a_document_comment() {
    let schema = schema_from_json(r#"{"type":"object","properties":{"port":{"type":"number"}}}"#);
    let doc = dbuild(|db| db.obj(vec![("port", db.n("8080"))]));
    let meta = crate::doc::Meta::default();
    let mut m = app_with_meta(schema, doc, test_opts(), meta);
    set_cursor_path(&mut m, &sp(&["port"]));

    press(&mut m, &["C"]);
    match &mut m.overlay {
        Some(Overlay::CommentEdit(s)) => {
            assert_eq!(s.ta.value(), "");
            s.ta.set_value("new note");
        }
        _ => panic!("C must open the comment editor"),
    }
    press(&mut m, &["ctrl+s"]);
    assert_eq!(
        m.fs().meta.comment_text(&sp(&["port"])).as_deref(),
        Some("new note")
    );

    press(&mut m, &["C"]);
    match &mut m.overlay {
        Some(Overlay::CommentEdit(s)) => s.ta.set_value(""),
        _ => panic!("C must reopen the comment editor"),
    }
    press(&mut m, &["ctrl+s"]);
    assert!(m.fs().meta.comment_text(&sp(&["port"])).is_none());
}

#[test]
fn comment_edit_requires_a_document_value() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "port":{"type":"number"},
          "missing":{"type":"number"}
        }}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("port", db.n("8080"))]));
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["missing"]));
    press(&mut m, &["C"]);
    assert!(m.overlay.is_none(), "absent keys must not open the editor");
    assert!(m.notice.contains("no value"), "notice = {}", m.notice);
    assert!(!m.dirty());
}

#[test]
fn applying_an_unchanged_comment_keeps_metadata_and_dirty_state() {
    let schema = schema_from_json(r#"{"type":"object","properties":{"port":{"type":"number"}}}"#);
    let doc = dbuild(|db| db.obj(vec![("port", db.n("8080"))]));
    let mut meta = crate::doc::Meta::default();
    let mut cs = crate::doc::CommentSet::default();
    cs.head = "/* keep me */".into();
    meta.comments.insert(sp(&["port"]), cs);
    let before = meta.comments.clone();
    let mut m = app_with_meta(schema, doc, test_opts(), meta);
    set_cursor_path(&mut m, &sp(&["port"]));
    press(&mut m, &["C"]);
    assert!(matches!(m.overlay, Some(Overlay::CommentEdit(_))));
    press(&mut m, &["ctrl+s"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.fs().meta.comments, before, "no-op apply rewrote metadata");
    assert!(!m.dirty());
}

#[test]
fn both_description_sources_fold_independently_at_two_lines() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{"port":{"type":"number","description":"S1\nS2\nS3"}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("port", db.n("8080"))]));
    let mut meta = crate::doc::Meta::default();
    meta.set_comment_text(&sp(&["port"]), "C1\nC2\nC3", "//");
    let mut m = app_with_meta(schema, doc, test_opts(), meta);
    m.width = 60;
    m.height = 24;
    set_cursor_path(&mut m, &sp(&["port"]));

    let view = m.view();
    assert!(
        view.contains("S1") && view.contains("S2") && view.contains("C1") && view.contains("C2"),
        "{view}"
    );
    assert!(
        !view.contains("S3") && !view.contains("C3"),
        "each source must fold at two lines:\n{view}"
    );
    assert_eq!(view.matches('…').count(), 2, "{view}");
    assert!(
        view.contains(" \x1b[38;5;39m…"),
        "the fold marker must use the accent color:\n{view}"
    );

    press(&mut m, &["c"]);
    let view = m.view();
    assert!(
        view.contains("S3") && view.contains("C3"),
        "expansion must show both full blocks:\n{view}"
    );
}

#[test]
fn long_comment_opens_a_scrollable_page() {
    let desc = (1..=40)
        .map(|i| format!("L{i:02}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\nTAILMARK";
    let src = serde_json::json!({
        "type": "object",
        "properties": {"name": {"type": "string", "title": "Name", "description": desc}}
    });
    let schema = schema_from_json(&src.to_string());
    let mut m = app_with(schema, test_doc(), test_opts());
    m.width = 40;
    m.height = 24;
    set_cursor_path(&mut m, &sp(&["name"]));

    press(&mut m, &["c"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => {
            assert_eq!(c.lines.len(), 41);
            assert_eq!(c.scroll, 0);
        }
        _ => panic!("long comment did not open the comment page"),
    }
    let view = m.view();
    assert!(view.contains("Comment: Name"), "{view}");
    assert!(view.contains("L01"), "{view}");
    assert!(
        !view.contains("TAILMARK"),
        "first page leaks the tail:\n{view}"
    );
    assert!(view.contains("c/q/esc close"), "page hint missing:\n{view}");

    // A page key jumps a full screen and clamps at the ends.
    press(&mut m, &["pgdown"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => {
            assert_eq!(c.scroll, 19);
            assert!(m.view().contains("TAILMARK"));
        }
        _ => panic!("pgdown closed the comment page"),
    }
    press(&mut m, &["ctrl+b"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => assert_eq!(c.scroll, 0),
        _ => panic!("ctrl+b closed the comment page"),
    }
    press(&mut m, &["ctrl+f"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => assert_eq!(c.scroll, 19),
        _ => panic!("ctrl+f closed the comment page"),
    }
    press(&mut m, &["pgup"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => assert_eq!(c.scroll, 0),
        _ => panic!("pgup closed the comment page"),
    }

    for _ in 0..25 {
        press(&mut m, &["down"]);
    }
    match &m.overlay {
        Some(Overlay::Comment(c)) => {
            assert_eq!(c.scroll, 19, "scroll must clamp to the last page");
            let view = m.view();
            assert!(!view.contains("L01"), "old lines still visible:\n{view}");
            assert!(
                view.contains("TAILMARK"),
                "last page lost the tail:\n{view}"
            );
        }
        _ => panic!("comment page closed unexpectedly"),
    }

    press(&mut m, &["c"]);
    assert!(m.overlay.is_none(), "c must close the comment page");

    // q aliases esc outside inputs and also closes the page.
    press(&mut m, &["c"]);
    assert!(matches!(m.overlay, Some(Overlay::Comment(_))));
    press(&mut m, &["q"]);
    assert!(m.overlay.is_none(), "q must close the comment page");
}
