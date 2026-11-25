//! Menu navigation basics: drilling, cursor motion, vim keys, esc/quit,
//! mouse hover and wheel.

use super::*;
use super::{
    TB, app_with, arr_len, arr_strs, body_val, child, counting_opts, cursor_cursor, dbuild,
    find_row, getv, has, infer_schema, items_app, ks, mk_app, mstate, path, plain, press,
    required_merge_model, row_at, scalar, schema_build, schema_from_json, search_state,
    set_cursor_path, sget, sp, str_at, test_doc, test_opts, test_schema,
};

use super::*;

// --- menu_test.go -----------------------------------------------------------

#[test]
fn menu_drill_and_restore_cursor() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["down", "down", "enter"]);
    assert_eq!(
        m.fs().stack.len(),
        2,
        "drill into Server should push a menu"
    );
    press(&mut m, &["esc"]);
    assert_eq!(m.fs().stack.len(), 1, "esc should pop back to root");
    // cursor was restored to row 2 (Server); one down lands on Logging
    press(&mut m, &["down", "enter"]);
    assert_eq!(
        m.fs().stack.len(),
        2,
        "cursor restore should drill into Logging"
    );
}

/// Deleting more elements than the array keeps rows for: the removed slots
/// beyond the end cannot render rows, so only the array parent row shows
/// the removal instead.
#[test]
fn multi_tail_deletions_count_on_parent() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{"nums":{"type":"array","items":{"type":"integer"}}}}"#,
    );
    let mut m = app_with(
        schema,
        dbuild(|db| {
            db.obj(vec![(
                "nums",
                db.arr(vec![db.n("1"), db.n("2"), db.n("3"), db.n("4"), db.n("5")]),
            )])
        }),
        test_opts(),
    );
    m.fs_mut().baseline = Some(m.fs().doc.clone());
    press(&mut m, &["enter"]); // nums items
    press(&mut m, &["down", "down"]); // cursor on 3
    press(&mut m, &["d", "d", "d"]); // delete 3, 4, 5 -> [1, 2]
    let base = m.fs().baseline.clone().expect("baseline");
    use crate::doc::jsondoc::PathSeg;
    assert_eq!(
        crate::ui::history::diff_paths(&base, &m.fs().doc),
        vec![
            vec![ks("nums"), PathSeg::Idx(2)],
            vec![ks("nums"), PathSeg::Idx(3)],
            vec![ks("nums"), PathSeg::Idx(4)],
        ],
        "every removed slot is marked"
    );
    // the surviving rows (1, 2) stay unmarked inside the items menu
    let view = plain(&m.view());
    for want in ["1", "2"] {
        let row = view.lines().find(|l| l.contains(want)).expect(want);
        assert!(
            !row.contains('-'),
            "survivor {want} must not carry a removal mark:\n{row:?}"
        );
    }
    press(&mut m, &["esc"]); // back to root
    let view = plain(&m.view());
    let parent = view
        .lines()
        .find(|l| l.contains("nums"))
        .expect("parent row");
    assert!(
        parent.contains('-'),
        "the parent row must show the removal:\n{parent:?}\n{view}"
    );
}

#[test]
fn menu_enum_dropdown_applies_non_string_members() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "level":{"enum":[true,1,null,"auto"]}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("level", db.s("auto"))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]); // current value auto is highlighted
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.sugs, vec!["true", "1", "null", "auto"]);
            assert_eq!(e.sug_idx, 3);
        }
        _ => panic!("enum dropdown should open"),
    }
    // typing "t" over the highlighted auto starts a search: true and auto
    // contain it; the highlighted top match is true
    press(&mut m, &["t"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sugs, vec!["true", "auto"]),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert!(
        matches!(
            m.fs().doc.get(&path(&["level"])),
            Some(n) if matches!(m.fs().doc.value(n), Value::Bool(true))
        ),
        "the boolean member is applied as a bool"
    );

    // a null member is reachable by keyword too
    set_cursor_path(&mut m, &sp(&["level"]));
    press(&mut m, &["enter", "n"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sugs, vec!["null"]),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert!(
        matches!(
            m.fs().doc.get(&path(&["level"])),
            Some(n) if matches!(m.fs().doc.value(n), Value::Null)
        ),
        "the null member is applied as null"
    );
}

#[test]
fn menu_enum_dropdown_disambiguates_duplicate_displays() {
    // `enum: ["1", 1]` renders two identical rows; enter must commit the
    // highlighted member, not the first textual match
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "level":{"enum":["1",1]}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("level", db.n("1"))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]); // the number member is highlighted
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.sugs, vec!["1", "1"]);
            assert_eq!(e.sug_idx, 1, "the current number member is highlighted");
        }
        _ => panic!("enum dropdown should open"),
    }
    press(&mut m, &["enter"]); // no-op commit must not flip number → string
    assert!(
        matches!(
            m.fs().doc.get(&path(&["level"])),
            Some(n) if matches!(m.fs().doc.value(n), Value::Num(s) if s == "1")
        ),
        "the number member survives a no-op enter"
    );

    // cycling onto the second row commits the number by keyword too
    set_cursor_path(&mut m, &sp(&["level"]));
    press(&mut m, &["enter", "1", "down", "enter"]);
    assert!(
        matches!(
            m.fs().doc.get(&path(&["level"])),
            Some(n) if matches!(m.fs().doc.value(n), Value::Num(s) if s == "1")
        ),
        "the highlighted duplicate row commits the number member"
    );
}

#[test]
fn enum_array_search_filters_toggles() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "levels":{"type":"array","items":{"enum":["alpha","beta","gamma"]}}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("levels", db.arr(vec![db.s("beta")]))]));
    let mut m = app_with(schema, doc, test_opts());
    m.width = 80;
    m.height = 14;
    set_cursor_path(&mut m, &sp(&["levels"]));
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::EnumArray);
    assert_eq!(m.menu_rows().len(), 3);

    press(&mut m, &["/", "g", "a"]);
    assert!(m.top().unwrap().filter.active);
    assert_eq!(m.top().unwrap().filter.ti.value(), "ga");
    assert_eq!(m.menu_rows().len(), 1, "only gamma matches");
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));

    press(&mut m, &["enter"]); // toggles the only match, filter stays
    assert_eq!(arr_strs(&m.fs().doc, &sp(&["levels"])), ["beta", "gamma"]);
    assert!(m.top().unwrap().filter.active);

    press(&mut m, &["esc"]);
    assert!(!m.top().unwrap().filter.active);
    assert_eq!(m.menu_rows().len(), 3);
}

#[test]
fn menu_ctrl_c() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["down", "enter", "ctrl+c"]);
    assert!(m.aborted, "ctrl+c must abort anywhere");
}

#[test]
fn menu_root_esc_quit() {
    // clean root esc quits directly
    let mut clean = mk_app(test_doc(), test_opts());
    clean.update(Msg::Esc);
    assert!(clean.exiting && !clean.aborted, "clean root esc must quit");
    assert!(clean.overlay.is_none() && !clean.fs().saved);

    // changed: esc asks whether to save
    let mut discard = mk_app(test_doc(), test_opts());
    press(&mut discard, &["enter", "ctrl+u", "x", "enter"]);
    assert!(discard.dirty(), "editing Name must dirty the document");
    discard.update(Msg::Esc);
    match &discard.overlay {
        Some(Overlay::Confirm(c)) => assert!(c.title.contains("Save changes?")),
        _ => panic!("dirty root esc must ask to save"),
    }
    press(&mut discard, &["n"]);
    assert!(discard.overlay.is_none() && discard.aborted);
    assert!(
        !discard.exiting,
        "n aborts; the process must not report a clean quit"
    );

    // esc dismisses the prompt and stays in the session
    let mut stay = mk_app(test_doc(), test_opts());
    press(&mut stay, &["enter", "ctrl+u", "x", "enter", "esc"]);
    assert!(matches!(stay.overlay, Some(Overlay::Confirm(_))));
    press(&mut stay, &["esc"]);
    assert!(stay.overlay.is_none() && !stay.aborted && !stay.exiting);

    // y saves and exits
    let (opts, saves) = counting_opts();
    let mut save = mk_app(test_doc(), opts);
    press(&mut save, &["enter", "ctrl+u", "x", "enter", "esc"]);
    press(&mut save, &["y"]);
    assert_eq!(saves.load(Ordering::SeqCst), 1);
    assert!(save.fs().saved && save.exiting && !save.aborted);
    assert!(!save.dirty(), "saving must refresh the baseline");
}

#[test]
fn root_esc_clears_selection_before_quit() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["m", "down", "m"]);
    assert_eq!(m.top().unwrap().sel.len(), 2);
    m.update(Msg::Esc);
    assert!(
        !m.exiting && !m.aborted,
        "esc must not quit with a selection"
    );
    assert!(m.top().unwrap().sel.is_empty(), "esc must clear the marks");
    assert_eq!(m.notice, "selection cleared");
    assert!(m.fs().stack.len() == 1);

    // q aliases esc outside inputs
    press(&mut m, &["m", "q"]);
    assert!(!m.exiting && !m.aborted && m.top().unwrap().sel.is_empty());

    // without a selection esc keeps its meaning
    m.update(Msg::Esc);
    assert!(m.exiting);
}

#[test]
fn root_esc_clears_selection_before_save_prompt() {
    // a dirty root with marks clears them first; the next esc asks to save
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["enter", "ctrl+u", "x", "enter", "m"]);
    assert!(m.dirty() && !m.top().unwrap().sel.is_empty());
    m.update(Msg::Esc);
    assert!(m.overlay.is_none() && !m.exiting && !m.aborted);
    assert!(m.top().unwrap().sel.is_empty());
    m.update(Msg::Esc);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
}

#[test]
fn esc_clears_buffer_in_submenu_and_overlay() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["down", "down", "enter"]); // Server
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["g", "esc"]);
    assert!(m.cmd_buf.is_empty() && m.fs().stack.len() == 2);
    press(&mut m, &["esc"]);
    assert_eq!(m.fs().stack.len(), 1);

    // the issues overlay shares the g buffer
    let mut opts = test_opts();
    opts.load_errors = vec!["at '$.name': boom".into()];
    let mut mi = mk_app(test_doc(), opts);
    press(&mut mi, &["enter"]);
    assert!(matches!(mi.overlay, Some(Overlay::Issues(_))));
    press(&mut mi, &["g"]);
    assert_eq!(mi.cmd_buf.len(), 1);
    press(&mut mi, &["esc"]);
    assert!(
        matches!(mi.overlay, Some(Overlay::Issues(_))),
        "esc with a pending combo must not close the issues list"
    );
    assert!(mi.cmd_buf.is_empty());
    press(&mut mi, &["esc"]);
    assert!(mi.overlay.is_none());
}

#[test]
fn menu_next_prev_non_null() {
    let mut m = mk_app(test_doc(), test_opts());
    let cursor = |m: &mut App| cursor_cursor(m);
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 2, "n from Name lands on Server");
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 4, "n from Server lands on Mounts");
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 0, "n wraps to Name");
    press(&mut m, &["N"]);
    assert_eq!(cursor(&mut m), 4, "N wraps backwards to Mounts");

    // explicit nulls and schema-less nulls are skipped
    let mut doc = test_doc();
    let n = doc.new_null();
    let _ = doc.set(&sp(&["mode"]), n);
    let n = doc.new_null();
    let _ = doc.set(&sp(&["null_extra"]), n);
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["extra"]), n);
    let mut m = mk_app(doc, test_opts());
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 1, "n from the issues row lands on Name");
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 3, "n skips the explicit null mode");
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 5, "n skips unset Logging/Auth");
    press(&mut m, &["n"]);
    let r = row_at(&mut m);
    assert_eq!(r.kind, RowKind::Unknown);
    assert_eq!(r.path, sp(&["extra"]), "n lands on the set unknown key");
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 1, "n wraps to Name");

    // warnings are live issues, so ctrl+j cycles the flagged keys instead of
    // walking non-null rows
    assert_eq!(m.fs().issues_now.len(), 2);
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["null_extra"]),
        "ctrl+j jumps to the first warning key"
    );
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, sp(&["extra"]));
    press(&mut m, &["ctrl+k"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["null_extra"]),
        "ctrl+k wraps back"
    );

    // no non-null keys: n/N leave the cursor alone
    let schema = schema_build(|b| {
        let a = b.id(scalar("a", "A", "string"));
        let bb = b.id(scalar("b", "B", "string"));
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![a, bb],
            ..FormNode::default()
        })
    });
    let mut empty = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut empty, &["n"]);
    assert_eq!(cursor(&mut empty), 0);
    press(&mut empty, &["N"]);
    assert_eq!(cursor(&mut empty), 0);

    // array items are entries too; null placeholders are skipped
    let schema = schema_build(|b| {
        let x = b.id(scalar("x", "X", "string"));
        let item = b.id(FormNode {
            key: "items".into(),
            kind: Kind::Object,
            children: vec![x],
            ..FormNode::default()
        });
        let items = b.id(FormNode {
            key: "items".into(),
            title: "Items".into(),
            kind: Kind::Array,
            items: Some(item),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![items],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| {
        db.obj(vec![(
            "items",
            db.arr(vec![
                db.push(Value::Null),
                db.obj(vec![("x", db.s("v"))]),
                db.obj(vec![("x", db.s("w"))]),
            ]),
        )])
    });
    let mut im = app_with(schema, doc, test_opts());
    press(&mut im, &["enter"]);
    press(&mut im, &["n"]);
    assert_eq!(cursor_cursor(&mut im), 1, "n must skip a null array item");
    press(&mut im, &["n"]);
    assert_eq!(cursor_cursor(&mut im), 2);
    press(&mut im, &["n"]);
    assert_eq!(cursor_cursor(&mut im), 1);
}

#[test]
fn a_inserts_after_and_A_inserts_before_array_items() {
    let schema = schema_build(|b| {
        let it = b.id(scalar("items", "", "string"));
        let items = b.id(FormNode {
            key: "items".into(),
            title: "Items".into(),
            kind: Kind::Array,
            items: Some(it),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![items],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("items", db.arr(vec![db.s("a"), db.s("b")]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);

    // a: after the cursor item
    set_cursor_path(&mut m, &[ks("items"), PathSeg::Idx(0)]);
    press(&mut m, &["a"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["esc"]);
    assert_eq!(arr_len(&m.fs().doc, &sp(&["items"])), 3);
    assert!(matches!(
        m.fs()
            .doc
            .value(getv(&m.fs().doc, &[ks("items"), PathSeg::Idx(1)]).expect("idx1")),
        Value::Null,
    ));
    assert_eq!(sget(&m.fs().doc, &[ks("items"), PathSeg::Idx(2)]), "b");

    // A: before the cursor item
    set_cursor_path(&mut m, &[ks("items"), PathSeg::Idx(2)]);
    press(&mut m, &["A"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["esc"]);
    assert_eq!(arr_len(&m.fs().doc, &sp(&["items"])), 4);
    assert!(matches!(
        m.fs()
            .doc
            .value(getv(&m.fs().doc, &[ks("items"), PathSeg::Idx(2)]).expect("idx2")),
        Value::Null,
    ));
    assert_eq!(sget(&m.fs().doc, &[ks("items"), PathSeg::Idx(3)]), "b");
}

#[test]
fn delete_expanded_merged_key_allowed_when_required() {
    let mut m = required_merge_model();
    press(&mut m, &["down", "enter"]);
    press(&mut m, &["ctrl+e"]);
    assert!(
        has(&m.fs().doc, &sp(&["child", "name"])),
        "ctrl+e must materialize the merged name"
    );
    press(&mut m, &["d"]);
    assert!(
        !has(&m.fs().doc, &sp(&["child", "name"])),
        "the expanded copy must be deletable"
    );
    assert!(!m.notice.contains("required"), "notice = {}", m.notice);
    assert!(find_row(&mut m, |r, _| r.label == "name" && r.merged == "b").is_some());
}

#[test]
fn current_variant_value_aware() {
    let (schema, one_of, _) = infer_schema();
    let (d1, v1) = body_val(|db| db.obj(vec![("type", db.s("direct")), ("listen", db.s("::"))]));
    assert_eq!(
        crate::ui::rows::current_variant(&schema, one_of, &d1, Some(v1)),
        0
    );
    let (d2, v2) = body_val(|db| db.obj(vec![("type", db.s("trojan"))]));
    assert_eq!(
        crate::ui::rows::current_variant(&schema, one_of, &d2, Some(v2)),
        1
    );

    // examples-style mixed oneOf keeps its semantics
    let examples = test_schema();
    let auth = child(&examples, examples.root, "auth");
    let (ds, vs) = body_val(|db| db.s("none"));
    assert_eq!(
        crate::ui::rows::current_variant(&examples, auth, &ds, Some(vs)),
        0
    );
    let (dt, vt) = body_val(|db| db.obj(vec![("token", db.s("t"))]));
    assert_eq!(
        crate::ui::rows::current_variant(&examples, auth, &dt, Some(vt)),
        1
    );
    assert_eq!(
        crate::ui::rows::current_variant(&examples, auth, &Doc::new(), None),
        -1
    );
}

#[test]
fn null_only_variant_does_not_shadow_object() {
    let root_schema = schema_from_json(
        r#"{
	  "type": "object",
	  "properties": {
	    "push": {"oneOf": [
	      {"type": "null"},
	      {"type": "object", "properties": {
	        "tags": {"type": "array", "items": {"type": "string"}}}}]}}}"#,
    );
    let push = child(&root_schema, root_schema.root, "push");
    assert_eq!(root_schema.nodes[push as usize].variants.len(), 2);
    let nullv = root_schema.nodes[push as usize].variants[0];
    assert_eq!(root_schema.nodes[nullv as usize].kind, Kind::Raw);
    assert!(root_schema.nodes[nullv as usize].null_only);

    let (dn, vn) = body_val(|db| db.push(Value::Null));
    assert!(crate::ui::rows::variant_matches(
        &root_schema,
        nullv,
        &dn,
        Some(vn)
    ));
    let (dv, vv) = body_val(|db| db.obj(vec![("tags", db.arr(vec![db.s("v*")]))]));
    assert!(!crate::ui::rows::variant_matches(
        &root_schema,
        nullv,
        &dv,
        Some(vv)
    ));
    assert_eq!(
        crate::ui::rows::current_variant(&root_schema, push, &dv, Some(vv)),
        1
    );

    let doc = dbuild(|db| {
        db.obj(vec![(
            "push",
            db.obj(vec![("tags", db.arr(vec![db.s("v*")]))]),
        )])
    });
    let mut m = app_with(root_schema, doc, test_opts());
    press(&mut m, &["enter"]);
    assert!(
        !matches!(m.overlay, Some(Overlay::Text(_))),
        "push must not open the raw textarea"
    );
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert!(find_row(&mut m, |r, _| r.path == sp(&["push", "tags"])).is_some());
}

#[test]
fn shared_ref_row_labels() {
    let schema = schema_from_json(
        r##"{
  "type": "object",
  "properties": {
    "push": {"type": "object", "properties": {
      "branches": {"$ref": "#/definitions/branch"},
      "branches-ignore": {"$ref": "#/definitions/branch"},
      "tags": {"$ref": "#/definitions/branch"},
      "tags-ignore": {"$ref": "#/definitions/branch"},
      "paths": {"$ref": "#/definitions/path"},
      "paths-ignore": {"$ref": "#/definitions/path"}
    }}
  },
  "definitions": {
    "branch": {"$ref": "#/definitions/globs"},
    "path": {"$ref": "#/definitions/globs"},
    "globs": {"type": "array", "items": {"type": "string"}}
  }
}"##,
    );
    let doc = dbuild(|db| {
        db.obj(vec![(
            "push",
            db.obj(vec![("branches", db.arr(vec![db.s("main")]))]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    let labels: Vec<String> = m
        .menu_rows()
        .iter()
        .map(|r| match r.fn_ {
            Some(f) => m.fs().schema.nodes[f as usize].key.clone(),
            None => r.label.clone(),
        })
        .collect();
    for label in [
        "branches",
        "branches-ignore",
        "tags",
        "tags-ignore",
        "paths",
        "paths-ignore",
    ] {
        assert!(
            labels.iter().any(|l| l == label),
            "row label {label:?} missing: {labels:?}"
        );
    }
}

#[test]
fn tag_reference_completion() {
    let schema = schema_build(|b| {
        let mut v1 = scalar("http_client", "variant 1", "string");
        v1.tag_ref = "http_client".into();
        let v1 = b.id(v1);
        let engine = b.id(scalar("engine", "Engine", "string"));
        let v2 = b.id(FormNode {
            key: "http_client".into(),
            title: "variant 2".into(),
            kind: Kind::Object,
            children: vec![engine],
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "http_client".into(),
            title: "HTTP Client".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        let tag = b.id(scalar("tag", "Tag", "string"));
        let client = b.id(FormNode {
            key: "http_clients".into(),
            title: "Client".into(),
            kind: Kind::Object,
            children: vec![tag],
            ..FormNode::default()
        });
        let clients = b.id(FormNode {
            key: "http_clients".into(),
            title: "HTTP Clients".into(),
            kind: Kind::Array,
            items: Some(client),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one, clients],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| {
        db.obj(vec![(
            "http_clients",
            db.arr(vec![
                db.obj(vec![("tag", db.s("c1"))]),
                db.obj(vec![("tag", db.s("c2"))]),
            ]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter", "enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.cands, vec!["c1".to_string(), "c2".to_string()]),
        _ => panic!("scalar variant editor did not open"),
    }
    press(&mut m, &["c", "2"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sugs, vec!["c2".to_string()]),
        _ => panic!(),
    }
    press(&mut m, &["tab", "enter"]);
    assert_eq!(str_at(&m.fs().doc, &["http_client"]), "c2");
}

#[test]
fn breadcrumb_path() {
    let (schema, one_of, _) = infer_schema();
    let root = schema.root;
    let inbounds = child(&schema, root, "inbounds");
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    m.fs_mut().stack = vec![
        mstate(MenuKind::Object, root, Vec::new()),
        mstate(MenuKind::Items, inbounds, sp(&["inbounds"])),
        mstate(
            MenuKind::Object,
            one_of,
            vec![ks("inbounds"), PathSeg::Idx(0)],
        ),
    ];
    assert_eq!(m.breadcrumb(), "$.inbounds[0]");
    m.fs_mut().stack.push(mstate(
        MenuKind::OneOf,
        one_of,
        vec![ks("inbounds"), PathSeg::Idx(0)],
    ));
    assert_eq!(m.breadcrumb(), "$.inbounds[0]");
    m.fs_mut().stack.push(mstate(
        MenuKind::Object,
        one_of,
        vec![ks("inbounds"), PathSeg::Idx(3), ks("listen")],
    ));
    assert_eq!(m.breadcrumb(), "$.inbounds[3].listen");
    m.width = 12;
    assert_eq!(m.breadcrumb(), "…s[3].listen");
    m.fs_mut().stack.truncate(1);
    assert_eq!(m.breadcrumb(), "$");
}

#[test]
fn field_required() {
    let (schema, _, _) = infer_schema();
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut m, &["enter", "enter"]);

    let row_of = |m: &mut App, k: &str| {
        find_row(m, |r, schema| {
            r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == k
        })
        .unwrap_or_else(|| panic!("row {k} not found"))
    };
    let r = row_of(&mut m, "type");
    assert!(m.field_required(&r));
    let r = row_of(&mut m, "listen");
    assert!(!m.field_required(&r));
    let r = row_of(&mut m, "password");
    assert!(m.field_required(&r));

    let n = m.fs_mut().doc.new_str("trojan");
    let _ = m
        .fs_mut()
        .doc
        .set(&[ks("inbounds"), PathSeg::Idx(0), ks("type")], n);
    m.refresh_variant_menu();
    let r = row_of(&mut m, "password");
    assert!(m.field_required(&r));

    let n = m.fs_mut().doc.new_str("direct");
    let _ = m
        .fs_mut()
        .doc
        .set(&[ks("inbounds"), PathSeg::Idx(0), ks("type")], n);
    m.refresh_variant_menu();
    let r = row_of(&mut m, "listen");
    assert!(!m.field_required(&r));
}

#[test]
fn nested_esc_clears_array_selection_before_back() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/a")), ("dst", db.s("/1"))]),
                db.obj(vec![("src", db.s("/b")), ("dst", db.s("/2"))]),
            ]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["down", "m", "down", "m"]); // row 0 is < Add item >
    assert_eq!(m.top().unwrap().sel.len(), 2);

    m.update(Msg::Esc);
    assert_eq!(
        m.fs().stack.len(),
        2,
        "esc must clear the marks, not leave the array menu"
    );
    assert!(m.top().unwrap().sel.is_empty());
    assert_eq!(m.notice, "selection cleared");

    // the next esc navigates back
    m.update(Msg::Esc);
    assert_eq!(m.fs().stack.len(), 1);

    // q aliases esc outside inputs
    press(&mut m, &["enter", "down", "m"]);
    assert_eq!(m.top().unwrap().sel.len(), 1);
    press(&mut m, &["q"]);
    assert_eq!(m.fs().stack.len(), 2);
    assert!(m.top().unwrap().sel.is_empty());

    // a visual selection is cancelled in one esc as well
    press(&mut m, &["down", "v", "down"]);
    assert!(m.top().unwrap().visual);
    assert!(!m.top().unwrap().sel.is_empty());
    m.update(Msg::Esc);
    assert_eq!(m.fs().stack.len(), 2);
    assert!(!m.top().unwrap().visual);
    assert!(m.top().unwrap().sel.is_empty());
}

#[test]
fn cursor_wrap() {
    let mut m = items_app(test_doc());
    let last = m.menu_rows().len() - 1;
    press(&mut m, &["up"]);
    assert_eq!(cursor_cursor(&mut m), last);
    assert_eq!(row_at(&mut m).path, sp(&["auth"]));
    press(&mut m, &["down"]);
    assert_eq!(cursor_cursor(&mut m), 0);

    // enum dropdown wraps
    press(&mut m, &["down", "enter"]);
    let n = match &m.overlay {
        Some(Overlay::Edit(e)) => e.sugs.len(),
        _ => panic!("dropdown did not open"),
    };
    press(&mut m, &["up"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sug_idx, n as i64 - 1),
        _ => panic!(),
    }
    press(&mut m, &["down"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sug_idx, 0),
        _ => panic!(),
    }
    press(&mut m, &["esc"]);

    // search results wrap
    let mut m2 = items_app(test_doc());
    press(&mut m2, &["/", "e"]);
    let n = search_state(&m2).results.len();
    assert!(n >= 2);
    press(&mut m2, &["up"]);
    assert_eq!(search_state(&m2).cursor, n - 1);
    press(&mut m2, &["down"]);
    assert_eq!(search_state(&m2).cursor, 0);
    press(&mut m2, &["ctrl+k"]);
    assert_eq!(search_state(&m2).cursor, n - 1);
    press(&mut m2, &["ctrl+j"]);
    assert_eq!(search_state(&m2).cursor, 0);
    assert_eq!(search_state(&m2).ti.value(), "e");

    // issues overlay wraps
    let mut opts = test_opts();
    opts.load_errors = vec![
        "at '$.server.port': minimum value is 1024".into(),
        "at '$.name': got number, want string".into(),
    ];
    let mut m3 = mk_app(test_doc(), opts);
    press(&mut m3, &["enter"]);
    press(&mut m3, &["up"]);
    match &m3.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 1),
        _ => panic!(),
    }
    press(&mut m3, &["down"]);
    match &m3.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
    press(&mut m3, &["ctrl+j"]);
    match &m3.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 1),
        _ => panic!(),
    }
    press(&mut m3, &["ctrl+k"]);
    match &m3.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
}

#[test]
fn vim_keys() {
    let mut m = items_app(test_doc());
    press(&mut m, &["j", "j"]);
    assert_eq!(cursor_cursor(&mut m), 2);
    press(&mut m, &["k"]);
    assert_eq!(cursor_cursor(&mut m), 1);

    assert_eq!(str_at(&m.fs().doc, &["mode"]), "<missing>");
    press(&mut m, &["l"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
    press(&mut m, &["h"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "http");

    set_cursor_path(&mut m, &sp(&["server"]));
    press(&mut m, &["l"]);
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["h"]);
    assert_eq!(m.fs().stack.len(), 1);

    press(&mut m, &["up", "up", "enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["h", "l"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert!(e.ti.value().contains("hl"), "value = {}", e.ti.value()),
        _ => panic!(),
    }
    press(&mut m, &["esc"]);

    // enum dropdown: arrows move the highlight
    let mut m2 = items_app(test_doc());
    set_cursor_path(&mut m2, &sp(&["mode"]));
    press(&mut m2, &["enter"]);
    let start = match &m2.overlay {
        Some(Overlay::Edit(e)) => e.sug_idx,
        _ => panic!("dropdown should open"),
    };
    press(&mut m2, &["down"]);
    match &m2.overlay {
        Some(Overlay::Edit(e)) => assert_ne!(e.sug_idx, start),
        _ => panic!(),
    }
    press(&mut m2, &["up"]);
    match &m2.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sug_idx, start),
        _ => panic!(),
    }
}

#[test]
fn schema_suggestions_reach_the_editor() {
    // x-tag-reference/completion candidates come from the field's own node
    let mut tb = TB::new();
    let mut f = scalar("tag", "Tag", "string");
    f.suggestions = vec!["alpha".into(), "beta".into()];
    let tag = tb.id(f);
    let root = tb.id(crate::smodel::FormNode {
        key: String::new(),
        kind: Kind::Object,
        children: vec![tag],
        ..Default::default()
    });
    tb.root = root;
    let schema = tb.finish();
    let doc = dbuild(|db| db.obj(vec![]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    let sugs = match &m.overlay {
        Some(Overlay::Edit(e)) => e.sugs.clone(),
        _ => panic!("editor must open"),
    };
    assert_eq!(
        sugs,
        vec!["alpha".to_string(), "beta".to_string()],
        "schema suggestions missing"
    );
}

#[test]
fn empty_object_menu_add_key_is_accented() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {"empty": {"type": "object", "properties": {}}}
        }"#,
    );
    let doc = dbuild(|db| db.obj(vec![("empty", db.obj(vec![]))]));
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["empty"]));
    press(&mut m, &["enter"]);
    let rows = m.menu_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, RowKind::NoItems);
    assert_eq!(rows[0].label, "< Add Key >");
    let line = m.row_line(&rows[0], false, 40);
    assert!(
        line.contains(&style::accent("< Add Key >")),
        "< Add Key > is an action row and must use the accent color: {line:?}"
    );
}

#[test]
fn empty_oneof_search_placeholder_stays_faint() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {"value": {"oneOf": [{"title": "Server", "type": "object"}]}}
        }"#,
    );
    let doc = dbuild(|db| db.obj(vec![("value", db.s("x"))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["t"]);
    press(&mut m, &["/", "z"]);
    let rows = m.menu_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, RowKind::NoItems);
    let line = m.row_line(&rows[0], false, 40);
    assert!(
        line.contains(&style::faint(&rows[0].label)),
        "a no-match placeholder is informational and must stay faint: {line:?}"
    );
}

/// row_paths lists the current menu's row paths in order.
fn row_paths(m: &mut App) -> Vec<Vec<PathSeg>> {
    m.menu_rows().into_iter().map(|r| r.path).collect()
}

#[test]
fn z_hides_and_restores_empty_key_rows() {
    let mut m = mk_app(test_doc(), test_opts());
    let before = row_paths(&mut m);
    for key in ["mode", "logging", "auth"] {
        assert!(before.contains(&sp(&[key])), "{key} row is listed");
    }
    assert!(before.contains(&sp(&["name"])));
    assert_eq!(cursor_cursor(&mut m), 0);

    press(&mut m, &["z"]);
    assert!(m.hide_empty);
    assert_eq!(m.notice, "empty fields hidden · z shows them");
    assert_eq!(
        row_paths(&mut m),
        vec![sp(&["name"]), sp(&["server"]), sp(&["mounts"])]
    );
    assert_eq!(
        row_at(&mut m).path,
        sp(&["name"]),
        "cursor stays on its row"
    );

    press(&mut m, &["z"]);
    assert!(!m.hide_empty);
    assert_eq!(m.notice, "empty fields shown");
    let restored = row_paths(&mut m);
    for key in ["mode", "logging", "auth"] {
        assert!(restored.contains(&sp(&[key])), "{key} row is back");
    }
}

/// Mouse hover must stay inert while an overlay owns the screen or a
/// visual selection is live: it neither moves the cursor nor repaints.
#[test]
fn hover_guards_overlay_and_visual_select() {
    let mut m = mk_app(test_doc(), test_opts());
    m.width = 80;
    let _ = m.view(); // populate the frame's click-target map
    // visual select live: hover is ignored
    press(&mut m, &["v"]); // visual on at row 0
    let _ = m.view();
    let before = cursor_cursor(&mut m);
    assert!(
        !m.mouse_hover(4),
        "hover must be inert during visual select"
    );
    assert_eq!(cursor_cursor(&mut m), before);
    press(&mut m, &["v"]); // visual off
    let _ = m.view();
    assert!(m.mouse_hover(4), "plain-menu hover moves the cursor");
    press(&mut m, &["?"]); // help overlay owns the screen
    let _ = m.view();
    let cur = cursor_cursor(&mut m);
    assert!(!m.mouse_hover(4), "hover must be inert under an overlay");
    assert_eq!(cursor_cursor(&mut m), cur);
    press(&mut m, &["esc"]); // close help
    let _ = m.view();
    // hover a DIFFERENT row: the row the cursor already sits on is a
    // deliberate no-op (no repaint)
    assert!(
        m.mouse_hover(5),
        "hover works again after the overlay closes"
    );
    assert_eq!(cursor_cursor(&mut m), 3);
}

/// The wheel on a numeric edit seeds an empty input from ±1 (schema default
/// first), keeps stepping, and ignores non-numeric content entirely.
#[test]
fn edit_wheel_seeds_empty_numeric_and_ignores_text() {
    let schema = schema_build(|b| {
        let port = b.id(scalar("port", "Port", "integer"));
        let name = b.id(scalar("name", "Name", "string"));
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![port, name],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("port", db.n("7")), ("name", db.s("abc"))]));
    let mut m = app_with(schema, doc, test_opts());
    let port = sp(&["port"]);
    let name = sp(&["name"]);

    // numeric edit with an existing value: wheel up/down step the literal
    set_cursor_path(&mut m, &port);
    press(&mut m, &["enter"]);
    assert!(m.edit_wheel(1), "wheel up on a number edits it");
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "8"),
        _ => panic!("edit overlay missing"),
    }
    assert!(m.edit_wheel(-2), "wheel down steps by the delta");
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "6"),
        _ => panic!("edit overlay missing"),
    }
    press(&mut m, &["esc"]);

    // an empty numeric input seeds before stepping (schema has no default:
    // ±1)
    set_cursor_path(&mut m, &port);
    press(&mut m, &["enter"]);
    if let Some(Overlay::Edit(e)) = &mut m.overlay {
        e.ti.set_value("");
    }
    assert!(m.edit_wheel(1), "empty numeric seeds from the wheel");
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "1"),
        _ => panic!("edit overlay missing"),
    }
    press(&mut m, &["esc"]);

    // a text edit ignores the wheel entirely
    set_cursor_path(&mut m, &name);
    press(&mut m, &["enter"]);
    assert!(!m.edit_wheel(1), "text edits ignore the wheel");
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "abc"),
        _ => panic!("edit overlay missing"),
    }
    press(&mut m, &["esc"]);
}
