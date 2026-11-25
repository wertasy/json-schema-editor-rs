//! Array item tests — add/move/delete/select, element editing — and
//! the change/diff badge rendering they feed.

use super::*;
use super::{
    app_with, arr_len, arr_strs, child, cursor_cursor, dbuild, empty_doc, find_row, fnode, getv,
    has, items_app, ks, map_one_of_schema, mk_app, mount_src, plain, press, row_at, scalar,
    schema_build, schema_from_json, search_state, set_cursor_path, sget, sp, str_at, test_doc,
    test_opts, test_schema, two_file_app,
};

use super::*;

// --- baseline diff marks -----------------------------------------------------

/// A real `a`-flow insert into an array flags only the new slot against the
/// baseline (not every shifted follower), and the row renders the green `+`
/// badge.
#[test]
fn array_insert_diff_marks_only_the_new_slot() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["a"]);
    if let Some(Overlay::Add(s)) = &mut m.overlay {
        s.ti.set_value("arr");
    }
    press(&mut m, &["enter"]);
    // String, Number, Boolean, Object, Array -> Array
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    // < Add item > opens the per-item type picker; String first. The first
    // element materializes the array; the baseline then holds arr=["first"].
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("first"),
        _ => panic!("string editor missing"),
    }
    press(&mut m, &["enter"]);
    m.fs_mut().baseline = Some(m.fs().doc.clone());

    // a second insert flags only its own slot, not the shifted "first"
    press(&mut m, &["a"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("hello"),
        _ => panic!("string editor missing"),
    }
    press(&mut m, &["enter"]);

    let base = m.fs().baseline.clone().expect("baseline set");
    use crate::doc::jsondoc::PathSeg;
    assert_eq!(
        crate::ui::history::diff_paths(&base, &m.fs().doc),
        vec![vec![ks("arr"), PathSeg::Idx(1)]],
        "only the inserted slot is marked; the shifted follower stays silent"
    );
    let view = plain(&m.view());
    assert!(
        view.contains("hello +"),
        "inserted row carries the + badge:\n{view}"
    );
    assert!(
        !view.contains("first +"),
        "the shifted follower must not be marked:\n{view}"
    );

    // the array's parent row (root menu) aggregates the addition too
    press(&mut m, &["esc"]); // esc back to the root menu
    let view = plain(&m.view());
    let arr_row = view
        .lines()
        .find(|l| l.contains("arr") && l.contains('+'))
        .expect("arr parent row with + badge");
    assert!(arr_row.contains("arr"), "{arr_row:?}");
}

/// Changes inside an array element aggregate onto the element row and the
/// array parent row: adding a field inside a pre-existing element turns the
/// parent's badge green +, modifying a field adds the orange ~.
#[test]
fn array_element_changes_aggregate_to_parent() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{"servers":{"type":"array","items":{"type":"object","properties":{"port":{"type":"integer"}}}}}}"#,
    );
    let mut m = app_with(
        schema,
        dbuild(|db| {
            db.obj(vec![(
                "servers",
                db.arr(vec![db.obj(vec![("port", db.n("1"))])]),
            )])
        }),
        test_opts(),
    );
    // baseline: servers = [{port: 1}] — the element pre-exists
    m.fs_mut().baseline = Some(m.fs().doc.clone());
    press(&mut m, &["enter"]); // servers items
    press(&mut m, &["enter"]); // element 0 menu (port row)
    press(&mut m, &["a"]); // add key
    if let Some(Overlay::Add(s)) = &mut m.overlay {
        s.ti.set_value("host");
    }
    press(&mut m, &["enter"]); // create host
    press(&mut m, &["enter"]); // String variant
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("web"),
        _ => panic!("string editor missing"),
    }
    press(&mut m, &["enter"]); // commit host = "web"
    use crate::doc::jsondoc::PathSeg;
    let base = m.fs().baseline.clone().expect("baseline");
    assert_eq!(
        crate::ui::history::diff_paths(&base, &m.fs().doc),
        vec![vec![ks("servers"), PathSeg::Idx(0), ks("host")]],
        "the added field is the only marked location"
    );
    // walk back to the root (bounded)
    for _ in 0..8 {
        if m.top().map(|t| t.path.is_empty()).unwrap_or(true) {
            break;
        }
        press(&mut m, &["esc"]);
    }
    let view = plain(&m.view());
    assert!(
        view.lines()
            .any(|l| l.contains("servers") && l.contains('+')),
        "the array parent row carries the + after the field add:\n{view}"
    );

    // modify port inside the element: the parent gains the orange ~
    press(&mut m, &["enter"]); // servers items
    press(&mut m, &["enter"]); // element 0 menu
    press(&mut m, &["enter"]); // edit port
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("9"),
        _ => panic!("port editor missing"),
    }
    press(&mut m, &["enter"]); // commit port = 9
    let base = m.fs().baseline.clone().expect("baseline");
    use crate::ui::history::DiffKind;
    let kinds = crate::ui::history::diff_kinds(&base, &m.fs().doc);
    assert!(
        kinds.contains(&(
            vec![ks("servers"), PathSeg::Idx(0), ks("port")],
            DiffKind::Modified
        )),
        "the modified field is marked: {kinds:?}"
    );
    for _ in 0..8 {
        if m.top().map(|t| t.path.is_empty()).unwrap_or(true) {
            break;
        }
        press(&mut m, &["esc"]);
    }
    let view = plain(&m.view());
    let parent_row = view
        .lines()
        .find(|l| l.contains("servers"))
        .expect("servers parent row");
    assert!(
        parent_row.contains('+') && parent_row.contains('~'),
        "the parent row must show both kinds:\n{parent_row:?}\n{view}"
    );
}

/// Removing an array element marks the parent row with the red -.
#[test]
fn array_element_removal_marks_parent() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{"servers":{"type":"array","items":{"type":"object","properties":{"port":{"type":"integer"}},"required":["port"]}}}}"#,
    );
    let mut m = app_with(
        schema,
        dbuild(|db| {
            db.obj(vec![(
                "servers",
                db.arr(vec![
                    db.obj(vec![("port", db.n("1"))]),
                    db.obj(vec![("port", db.n("2"))]),
                ]),
            )])
        }),
        test_opts(),
    );
    m.fs_mut().baseline = Some(m.fs().doc.clone());
    press(&mut m, &["enter"]); // servers items
    press(&mut m, &["d"]); // delete element 0 (no confirm)
    let base = m.fs().baseline.clone().expect("baseline");
    use crate::doc::jsondoc::PathSeg;
    assert_eq!(
        crate::ui::history::diff_paths(&base, &m.fs().doc),
        vec![vec![ks("servers"), PathSeg::Idx(0)]],
        "the removed slot is the only marked location"
    );
    press(&mut m, &["esc"]); // back to root
    let view = plain(&m.view());
    assert!(
        view.lines()
            .any(|l| l.contains("servers") && l.contains('-')),
        "the array parent row carries the -:\n{view}"
    );
}

/// Deleting two adjacent array elements badges both surviving positions
/// (the rows now occupying the deleted slots) and the array parent row.
#[test]
fn array_multi_delete_marks_every_affected_slot() {
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
    press(&mut m, &["enter"]); // nums items: 1..5
    press(&mut m, &["down"]); // cursor on 2
    press(&mut m, &["d"]); // delete 2 -> [1,3,4,5]
    press(&mut m, &["d"]); // cursor on 3: delete -> [1,4,5]
    let base = m.fs().baseline.clone().expect("baseline");
    use crate::doc::jsondoc::PathSeg;
    let marks = crate::ui::history::diff_paths(&base, &m.fs().doc);
    assert_eq!(
        marks,
        vec![
            vec![ks("nums"), PathSeg::Idx(1)],
            vec![ks("nums"), PathSeg::Idx(2)],
        ],
        "both removed slots are marked"
    );
    // consecutive deletions merge into one mark at the deletion point: the
    // row now holding "4" (first deleted slot), nothing below it
    let view = plain(&m.view());
    let merged = view
        .lines()
        .find(|l| l.contains("4 -"))
        .expect("merged deletion mark");
    assert!(merged.contains("nums[1]"), "{merged:?}");
    assert!(
        !view.lines().any(|l| l.contains("5 -") || l.contains("3 -")),
        "the marks must not spread past the merged deletion point:\n{view}"
    );
    press(&mut m, &["esc"]); // back to the root menu
    let view = plain(&m.view());
    let parent = view
        .lines()
        .find(|l| l.contains("nums") && l.contains('-'))
        .expect("parent row");
    assert!(parent.contains('-'), "parent row carries -:\n{parent:?}");
}

#[test]
fn array_variant_key_search_rows_and_jump() {
    let src = r#"{
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
                    "title": "RuleA",
                    "type": "object",
                    "properties": {
                      "server": {
                        "oneOf": [
                          {"title": "ServerName", "type": "string"},
                          {
                            "title": "ServerObject",
                            "type": "object",
                            "properties": {"addr": {"type": "string"}}
                          }
                        ]
                      }
                    },
                    "required": ["server"]
                  },
                  {
                    "title": "RuleB",
                    "type": "object",
                    "properties": {"other": {"type": "string"}},
                    "required": ["other"]
                  },
                  {"title": "RuleText", "type": "string"}
                ]
              }
            }
          }
        }
      }
    }"#;
    let server_path = vec![ks("dns"), ks("rules"), PathSeg::Idx(0), ks("server")];

    // Unset variant key inside an array item: one search row, ending in "—".
    let schema = schema_from_json(src);
    let doc = dbuild(|db| {
        db.obj(vec![(
            "dns",
            db.obj(vec![("rules", db.arr(vec![db.obj(vec![])]))]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "s", "e", "r", "v", "e", "r"]);
    let hits: Vec<Row> = search_state(&m)
        .results
        .iter()
        .filter(|r| r.path == server_path)
        .cloned()
        .collect();
    assert_eq!(hits.len(), 1, "one row per unset variant key");
    assert!(hits[0].label.ends_with("—"), "label = {}", hits[0].label);

    // Set variant key: jumping lands on the key row in the variant editor.
    let schema = schema_from_json(src);
    let doc = dbuild(|db| {
        let rule = db.obj(vec![("server", db.s("addr"))]);
        db.obj(vec![("dns", db.obj(vec![("rules", db.arr(vec![rule]))]))])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "s", "e", "r", "v", "e", "r"]);
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.path == server_path && r.kind == RowKind::Field)
        .expect("server key row in search results");
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert!(m.top().unwrap().one_of.is_some());
    let row = row_at(&mut m);
    assert_eq!(row.path, server_path);
    assert_eq!(row.kind, RowKind::Drill);
}

#[test]
fn element_labels_and_edit_title() {
    let schema = map_one_of_schema();
    let doc = dbuild(|db| {
        db.obj(vec![(
            "headers",
            db.obj(vec![("Host", db.arr(vec![db.s("a"), db.s("b")]))]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    press(&mut m, &["enter"]);
    let labels: Vec<String> = m.menu_rows().iter().map(|r| r.label.clone()).collect();
    assert!(labels.iter().any(|l| l == "Host[0]"), "labels = {labels:?}");
    assert!(labels.iter().any(|l| l == "Host[1]"), "labels = {labels:?}");
}

#[test]
fn variant_switch_string_wraps_as_first_array_element() {
    let schema = map_one_of_schema();
    let headers = child(&schema, schema.root, "headers");
    let one_of = schema.nodes[headers as usize].items.unwrap();
    let list = schema.nodes[one_of as usize].variants[1];
    let doc = dbuild(|db| db.obj(vec![("headers", db.obj(vec![("Host", db.s("old"))]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);

    let path = sp(&["headers", "Host"]);
    m.push(one_of, path.clone());
    m.enter_variant(&Row {
        kind: RowKind::Variant,
        variant: Some(list),
        path: path.clone(),
        idx: -1,
        ..Row::default()
    });
    press(&mut m, &["y"]);
    assert_eq!(m.top().unwrap().node, list);
    assert_eq!(
        arr_len(&m.fs().doc, &path),
        1,
        "the string must survive as element 0"
    );
    assert_eq!(
        sget(&m.fs().doc, &[ks("headers"), ks("Host"), PathSeg::Idx(0)]),
        "old"
    );
}

#[test]
fn variant_switch_integer_wraps_as_first_array_element() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "port":{"oneOf":[{"type":"integer"},{"type":"array","items":{"type":"integer"}}]}}}"#,
    );
    let one_of = child(&schema, schema.root, "port");
    let list = schema.nodes[one_of as usize].variants[1];
    let doc = dbuild(|db| db.obj(vec![("port", db.n("8080"))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);

    let path = sp(&["port"]);
    m.push(one_of, path.clone());
    m.enter_variant(&Row {
        kind: RowKind::Variant,
        variant: Some(list),
        path: path.clone(),
        idx: -1,
        ..Row::default()
    });
    press(&mut m, &["y"]);
    assert_eq!(m.top().unwrap().node, list);
    assert_eq!(arr_len(&m.fs().doc, &path), 1);
    let el = getv(&m.fs().doc, &[ks("port"), PathSeg::Idx(0)]).expect("element 0");
    assert!(matches!(m.fs().doc.value(el), Value::Num(t) if t == "8080"));
}

#[test]
fn variant_switch_type_mismatch_does_not_wrap() {
    // array[string] items cannot take a number: the old value is still
    // discarded instead of becoming an invalid element
    let schema = map_one_of_schema();
    let doc = dbuild(|db| db.obj(vec![("headers", db.obj(vec![("Host", db.n("42"))]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    let headers = child(&m.fs().schema, m.fs().schema.root, "headers");
    let one_of = m.fs().schema.nodes[headers as usize].items.unwrap();
    let list = m.fs().schema.nodes[one_of as usize].variants[1];
    let path = sp(&["headers", "Host"]);
    m.push(one_of, path.clone());
    m.enter_variant(&Row {
        kind: RowKind::Variant,
        variant: Some(list),
        path: path.clone(),
        idx: -1,
        ..Row::default()
    });
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    assert!(!has(&m.fs().doc, &path));
}

#[test]
fn variant_switch_wraps_root_scalar() {
    let schema = schema_from_json(
        r#"{"oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]}"#,
    );
    let one_of = schema.root;
    let list = schema.nodes[one_of as usize].variants[1];
    let doc = dbuild(|db| db.s("abc"));
    let mut m = app_with(schema, doc, test_opts());

    m.push(one_of, Vec::new());
    m.enter_variant(&Row {
        kind: RowKind::Variant,
        variant: Some(list),
        path: Vec::new(),
        idx: -1,
        ..Row::default()
    });
    press(&mut m, &["y"]);
    assert_eq!(arr_len(&m.fs().doc, &[]), 1, "root string wraps in place");
    assert_eq!(sget(&m.fs().doc, &[PathSeg::Idx(0)]), "abc");
}

#[test]
fn array_item_selection_colors_labels_and_deletes_batch() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/a")), ("dst", db.s("/1"))]),
                db.obj(vec![("src", db.s("/b")), ("dst", db.s("/2"))]),
                db.obj(vec![("src", db.s("/c")), ("dst", db.s("/3"))]),
            ]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["m", "down", "down", "m"]);
    let st = m.top().unwrap();
    assert_eq!(st.sel.len(), 2, "first and last items marked");
    assert!(!st.visual);

    // the "mounts[N]" label is styled like an object key
    let rows = m.menu_rows();
    let tw = m.menu_title_col_width(&rows);
    let off = m.row_line(&rows[0], false, tw);
    assert!(
        off.contains(&crate::ui::menu::style::fg9("mounts[0]")),
        "unfocused marked item must be red: {off:?}"
    );
    assert!(
        off.starts_with(&crate::ui::menu::style::fg9("> ")),
        "marked item needs the red marker: {off:?}"
    );
    let focused = m.row_line(&rows[2], true, tw);
    assert!(
        focused.contains(&crate::ui::menu::style::bold(&crate::ui::menu::style::fg9(
            "mounts[2]"
        ))),
        "focused marked item must be bold red: {focused:?}"
    );

    // batch delete splices from the end: the surviving item is the middle one
    press(&mut m, &["d"]);
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 1);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("src")]),
        "/b"
    );
    assert!(m.top().unwrap().sel.is_empty());
}

#[test]
fn array_item_visual_select_marks_items() {
    let mut m = items_app(test_doc());
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down", "v", "down"]);
    let st = m.top().unwrap();
    assert!(st.visual);
    assert_eq!(st.sel.len(), 1, "one item in the visual range");
    press(&mut m, &["v"]);
    assert!(m.top().unwrap().visual == false);
}

#[test]
fn array_item_selection_copies_values() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("tags", db.arr(vec![db.s("prod"), db.s("dev")]))]),
        )])
    });
    let mut opts = test_opts();
    let copies = Arc::new(parking_lot::Mutex::new(String::new()));
    let sink = copies.clone();
    opts.copy = Some(Arc::new(move |t: &str| {
        *sink.lock() = t.to_string();
        Ok(())
    }));
    let mut m = app_with(test_schema(), doc, opts);
    press(&mut m, &["down", "down", "down", "enter"]); // Logging
    press(&mut m, &["down", "enter"]); // Tags
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["down", "m", "down", "m"]);
    press(&mut m, &["y"]);
    assert_eq!(*copies.lock(), "prod,dev");
    assert_eq!(m.notice, "Copied 2 values");
    assert!(m.top().unwrap().sel.is_empty());
}

#[test]
fn menu_mounts_add_fill_back() {
    let mut m = items_app(empty_doc());
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert_eq!(m.top().unwrap().path, vec![ks("mounts"), PathSeg::Idx(0)]);
    let rows = m.menu_rows();
    assert!(
        rows.iter()
            .all(|r| matches!(r.kind, RowKind::Field | RowKind::Drill))
    );

    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["/", "d", "a", "t", "a", "enter"]);
    assert!(m.overlay.is_none(), "edit should close");
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("src")]),
        "/data"
    );

    press(&mut m, &["esc"]);
    press(&mut m, &["down", "enter"]);
    let src = find_row(&mut m, |r, schema| {
        r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "src"
    })
    .expect("Source row");
    assert_eq!(sget(&m.fs().doc, &src.path), "/data");
}

#[test]
fn menu_delete_item() {
    let mut m = items_app(test_doc());
    press(&mut m, &["down", "down", "down", "down", "enter", "down"]);
    press(&mut m, &["d"]);
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 0);

    // scalar array items delete the same way
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("tags", db.arr(vec![db.s("prod"), db.s("dev")]))]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(&mut m2, &["down", "down", "down", "enter", "down", "enter"]);
    press(&mut m2, &["d"]);
    assert_eq!(
        arr_strs(&m2.fs().doc, &sp(&["logging", "tags"])),
        vec!["dev".to_string()]
    );
}

#[test]
fn item_count_jump() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![(
                "tags",
                db.arr(vec![db.s("a"), db.s("b"), db.s("c")]),
            )]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "enter", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);

    press(&mut m, &["2"]);
    assert_eq!(m.cmd_buf.join(""), "2");
    press(&mut m, &["g"]);
    assert_eq!(m.cmd_buf.join(""), "2g");
    press(&mut m, &["g"]);
    assert_eq!(row_at(&mut m).idx, 2);

    press(&mut m, &["0", "g", "g"]);
    assert_eq!(row_at(&mut m).idx, 0);
    press(&mut m, &["9", "g", "g"]);
    assert_eq!(row_at(&mut m).idx, 2);
    press(&mut m, &["1", "2", "g", "g"]);
    assert_eq!(row_at(&mut m).idx, 2);
    press(&mut m, &["g", "g"]);
    assert_eq!(row_at(&mut m).idx, 0);

    // digits outside array menus do not arm a count
    press(&mut m, &["esc", "esc"]);
    set_cursor_path(&mut m, &sp(&["auth"]));
    press(&mut m, &["3"]);
    assert!(
        m.cmd_buf.is_empty(),
        "digits outside array menus must not arm a count"
    );
    press(&mut m, &["g", "g"]);
    assert_eq!(cursor_cursor(&mut m), 0);
}

#[test]
fn menu_move_item() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![(
                "tags",
                db.arr(vec![db.s("prod"), db.s("dev"), db.s("stage")]),
            )]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "enter", "down", "enter"]);
    let tags = sp(&["logging", "tags"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["prod", "dev", "stage"]);

    press(&mut m, &["J"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["dev", "prod", "stage"]);
    assert_eq!(cursor_cursor(&mut m), 1);
    press(&mut m, &["K"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["prod", "dev", "stage"]);
    assert_eq!(cursor_cursor(&mut m), 0);
    press(&mut m, &["K"]);
    assert_eq!(m.notice, "already first item");
    press(&mut m, &["down", "down"]);
    press(&mut m, &["J"]);
    assert_eq!(m.notice, "already last item");

    // one key = one undo step
    press(&mut m, &["up", "up"]);
    press(&mut m, &["J"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["dev", "prod", "stage"]);
    press(&mut m, &["u"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["prod", "dev", "stage"]);

    // single-item array: both directions refuse
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("tags", db.arr(vec![db.s("solo")]))]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(&mut m2, &["down", "down", "down", "enter", "down", "enter"]);
    press(&mut m2, &["J"]);
    assert_eq!(m2.notice, "already last item");
    press(&mut m2, &["K"]);
    assert_eq!(m2.notice, "already first item");

    // object item rows move the same way; undo restores
    let doc3 = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/a"))]),
                db.obj(vec![("src", db.s("/b"))]),
            ]),
        )])
    });
    let mut m3 = items_app(doc3);
    press(&mut m3, &["down", "down", "down", "down", "enter"]);
    press(&mut m3, &["J"]);
    assert_eq!(mount_src(&m3.fs().doc, 0), "/b");
    assert_eq!(cursor_cursor(&mut m3), 1);
    press(&mut m3, &["u"]);
    assert_eq!(mount_src(&m3.fs().doc, 0), "/a");
}

#[test]
fn menu_scalar_array_add() {
    // `< Add item >` (enter) and `a` share one contract: splice the null
    // placeholder, open the new-item editor; esc keeps the spliced slot.
    let mut m = items_app(empty_doc());
    press(
        &mut m,
        &["down", "down", "down", "enter", "down", "enter", "enter"],
    );
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert!(e.is_new),
        _ => panic!("scalar add should open the new-item editor"),
    }
    press(&mut m, &["esc"]);
    assert_eq!(arr_len(&m.fs().doc, &sp(&["logging", "tags"])), 1);
    let slot = m
        .fs()
        .doc
        .get(&[ks("logging"), ks("tags"), PathSeg::Idx(0)]);
    assert!(
        matches!(slot, Some(v) if matches!(m.fs().doc.value(v), Value::Null)),
        "esc keeps the spliced placeholder"
    );

    press(&mut m, &["enter"]);
    press(&mut m, &["p", "r", "o", "d", "enter"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &sp(&["logging", "tags"])),
        vec!["prod".to_string()]
    );
}

#[test]
fn menu_enum_array_toggle() {
    let mut m = items_app(empty_doc());
    press(&mut m, &["down", "down", "down", "enter", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::EnumArray);
    let levels = sp(&["logging", "levels"]);
    press(&mut m, &["space"]);
    assert_eq!(arr_strs(&m.fs().doc, &levels), vec!["debug".to_string()]);
    press(&mut m, &["down", "space"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &levels),
        vec!["debug".to_string(), "info".to_string()]
    );
    press(&mut m, &["space"]);
    assert_eq!(arr_strs(&m.fs().doc, &levels), vec!["debug".to_string()]);
    press(&mut m, &["up", "space"]);
    assert!(
        !has(&m.fs().doc, &levels),
        "empty optional enum array should be removed"
    );
}

#[test]
fn menu_one_of_variants() {
    let doc = dbuild(|db| db.obj(vec![("auth", db.obj(vec![("token", db.s("secret123"))]))]));
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert_eq!(
        m.fs().schema.nodes[m.top().unwrap().node as usize].title,
        "Token"
    );
    assert!(m.top().unwrap().one_of.is_some());

    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m.top().unwrap().cursor, 1);
    press(&mut m, &["enter"]);
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(
        m.fs().schema.nodes[m.top().unwrap().node as usize].title,
        "Token"
    );
    press(&mut m, &["esc"]);

    // picker, cursor onto None, confirm, then the enum picker
    press(&mut m, &["enter", "t", "up"]);
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    assert!(!has(&m.fs().doc, &sp(&["auth"])));
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["auth"]), "none");

    // n keeps the value
    let doc2 = dbuild(|db| db.obj(vec![("auth", db.s("none"))]));
    let mut m2 = items_app(doc2);
    press(
        &mut m2,
        &[
            "down", "down", "down", "down", "down", "enter", "esc", "t", "down", "enter",
        ],
    );
    assert!(matches!(m2.overlay, Some(Overlay::Confirm(_))));
    press(&mut m2, &["n"]);
    assert!(m2.overlay.is_none());
    assert_eq!(str_at(&m2.fs().doc, &["auth"]), "none");
}

#[test]
fn menu_remove_item_confirm() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![db.obj(vec![("src", db.s("/data"))])]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["enter"]);
    press(&mut m, &["D"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["n"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert!(has(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0)]));

    press(&mut m, &["D", "y"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    assert!(!has(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0)]));

    // esc also cancels
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![db.obj(vec![("src", db.s("/a"))])]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(
        &mut m2,
        &["down", "down", "down", "down", "enter", "enter", "D", "esc"],
    );
    assert!(m2.overlay.is_none());
    assert!(has(&m2.fs().doc, &[ks("mounts"), PathSeg::Idx(0)]));
}

#[test]
fn menu_d_removes_item_from_items_list() {
    let mut m = items_app(test_doc());
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["D"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    assert!(m.overlay.is_none());
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 0);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
}

#[test]
fn menu_delete_key() {
    let mut m = items_app(test_doc());
    press(&mut m, &["down", "down", "enter"]);
    press(&mut m, &["down", "delete"]);
    assert!(!has(&m.fs().doc, &sp(&["server", "host"])));
    assert!(m.notice.is_empty());
    press(&mut m, &["up", "delete"]);
    assert!(!has(&m.fs().doc, &sp(&["server", "port"])));
    assert!(m.notice.is_empty());

    // optional object field
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("levels", db.arr(vec![db.s("info")]))]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(&mut m2, &["down", "down", "down", "delete"]);
    assert!(!has(&m2.fs().doc, &sp(&["logging"])));

    // array item rows delete like d
    let mut m3 = items_app(test_doc());
    press(&mut m3, &["down", "down", "down", "down", "enter"]);
    press(&mut m3, &["down", "delete"]);
    assert!(!has(&m3.fs().doc, &[ks("mounts"), PathSeg::Idx(0)]));
}

fn tags_items_model() -> (App, Doc) {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![(
                "tags",
                db.arr(vec![db.s("a"), db.s("b"), db.s("c")]),
            )]),
        )])
    });
    let mut m = items_app(doc.clone());
    press(&mut m, &["down", "down", "down", "enter", "down", "enter"]);
    (m, doc)
}

#[test]
fn insert_item_around_cursor() {
    // a after item 0
    let (mut m, _) = tags_items_model();
    press(&mut m, &["a"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["x", "enter"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &sp(&["logging", "tags"])),
        vec![
            "a".to_string(),
            "x".to_string(),
            "b".to_string(),
            "c".to_string()
        ]
    );

    // A before item 1
    let (mut m, _) = tags_items_model();
    press(&mut m, &["down", "A"]);
    press(&mut m, &["x", "enter"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &sp(&["logging", "tags"])),
        vec![
            "a".to_string(),
            "x".to_string(),
            "b".to_string(),
            "c".to_string()
        ]
    );

    // a after the last item appends
    let (mut m, _) = tags_items_model();
    press(&mut m, &["down", "down", "a"]);
    press(&mut m, &["x", "enter"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &sp(&["logging", "tags"])),
        vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "x".to_string()
        ]
    );

    // object items: a splices an empty container at idx+1 and drills in
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/a"))]),
                db.obj(vec![("src", db.s("/b"))]),
            ]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(&mut m2, &["down", "down", "down", "down", "enter"]);
    press(&mut m2, &["a"]);
    assert_eq!(m2.top().unwrap().kind, MenuKind::Object);
    assert_eq!(m2.top().unwrap().path, vec![ks("mounts"), PathSeg::Idx(1)]);
    assert_eq!(arr_len(&m2.fs().doc, &sp(&["mounts"])), 3);
    let spliced = getv(&m2.fs().doc, &[ks("mounts"), PathSeg::Idx(1)]).unwrap();
    match m2.fs().doc.value(spliced) {
        Value::Object(m) => assert!(m.is_empty()),
        _ => panic!("spliced item must be an empty object"),
    }
    assert_eq!(mount_src(&m2.fs().doc, 2), "/b");
}

#[test]
fn delete_from_element_form_returns_to_item_list() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/data")), ("dst", db.s("/mnt"))]),
                db.obj(vec![("src", db.s("/b")), ("dst", db.s("/b2"))]),
            ]),
        )])
    });
    let mut m = mk_app(doc, test_opts());
    press(&mut m, &["down", "down", "down", "down", "enter"]); // mounts items
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["enter"]); // element form of item 0
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);

    press(&mut m, &["D"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["n"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::Object, "n keeps the form");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 2);

    press(&mut m, &["D", "y"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::Items,
        "confirm pops back to the item list"
    );
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 1);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("src")]),
        "/b",
        "the surviving item is the old second one"
    );
}

/// Rows modified against the baseline carry ` ~`, rows the schema still
/// lists after removal carry ` -`, each in its own color.
#[test]
fn modified_and_removed_rows_get_distinct_badges() {
    let schema = schema_build(|b| {
        let a = b.id(scalar("a", "A", "integer"));
        let bb = b.id(scalar("b", "B", "string"));
        let c = b.id(scalar("c", "C", "string"));
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![a, bb, c],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("a", db.n("2"))]));
    let mut m = app_with(schema, doc, test_opts());
    // baseline: a=1, b present, c present; current doc dropped b and c
    m.fs_mut().baseline = Some(dbuild(|db| {
        db.obj(vec![("a", db.n("1")), ("b", db.s("x")), ("c", db.s("y"))])
    }));
    let view = m.view();
    assert!(
        view.contains("\x1b[38;5;214m ~"),
        "modified row carries the orange ~ badge: {view:?}"
    );
    assert!(
        view.contains("\x1b[38;5;203m -"),
        "removed-but-listed rows carry the red - badge: {view:?}"
    );
    assert!(!view.contains("\x1b[38;5;114m +"), "nothing is added here");
}

/// A parent row aggregates every change kind inside its subtree (`+ ~` for
/// an added sibling and a modified child); on a narrow terminal the badges
/// collapse to a single plain `●`.
#[test]
fn parent_rows_aggregate_child_change_kinds() {
    let build_schema = || {
        schema_build(|b| {
            let mut srv = fnode("srv", "Srv", Kind::Object);
            let port = b.id(scalar("port", "Port", "integer"));
            let host = b.id(scalar("host", "Host", "string"));
            srv.children = vec![port, host];
            let srv = b.id(srv);
            b.id(FormNode {
                key: "$".into(),
                kind: Kind::Object,
                children: vec![srv],
                ..FormNode::default()
            })
        })
    };
    let mut m = app_with(
        build_schema(),
        dbuild(|db| {
            db.obj(vec![(
                "srv",
                db.obj(vec![("port", db.n("9")), ("extra", db.s("x"))]),
            )])
        }),
        test_opts(),
    );
    m.fs_mut().baseline = Some(dbuild(|db| {
        db.obj(vec![(
            "srv",
            db.obj(vec![("port", db.n("1")), ("host", db.s("a"))]),
        )])
    }));
    let view = plain(&m.view());
    let srv_line = view
        .lines()
        .find(|l| l.contains("Srv") && l.contains('+'))
        .unwrap_or_else(|| panic!("srv row with badges:\n{view}"));
    assert!(
        srv_line.contains('+') && srv_line.contains('~') && srv_line.contains('-'),
        "the parent row must aggregate every child change kind:\n{srv_line:?}"
    );
    // each glyph keeps its own color in the raw frame
    let raw = m.view();
    assert!(
        raw.contains("\x1b[38;5;114m +"),
        "added glyph green: {raw:?}"
    );
    assert!(
        raw.contains("\x1b[38;5;203m -"),
        "removed glyph red: {raw:?}"
    );
    assert!(
        raw.contains("\x1b[38;5;214m ~"),
        "modified glyph orange: {raw:?}"
    );

    // narrow terminal: the badge string collapses to one plain dot
    let mut narrow = app_with(
        build_schema(),
        dbuild(|db| {
            db.obj(vec![(
                "srv",
                db.obj(vec![("port", db.n("9")), ("extra", db.s("x"))]),
            )])
        }),
        test_opts(),
    );
    narrow.width = 24;
    narrow.fs_mut().baseline = Some(dbuild(|db| {
        db.obj(vec![(
            "srv",
            db.obj(vec![("port", db.n("1")), ("host", db.s("a"))]),
        )])
    }));
    let view = plain(&narrow.view());
    let srv_line = view.lines().find(|l| l.contains("srv")).expect("srv row");
    assert!(
        srv_line.contains('●') && !srv_line.contains('+'),
        "insufficient space collapses the badges:\n{srv_line:?}"
    );
}

#[test]
fn untitled_schema_variants_render_signature_in_title_column() {
    // The any-type picker shows each type name in the row's title column;
    // schema oneOf variants without a title must use their signature there
    // too instead of leaving an empty, padded title cell.
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "auth": {"oneOf": [
              {"type": "object", "properties": {"token": {"type": "string"}}},
              {"type": "object", "properties": {"user": {"type": "string"}},
               "required": ["user"]}
            ]}
          }
        }"#,
    );
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    m.width = 100;
    m.height = 24;
    set_cursor_path(&mut m, &sp(&["auth"]));
    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf, "variant picker");

    let view = m.view();
    let variant_lines: Vec<&str> = view
        .lines()
        .filter(|l| l.contains('○') || l.contains('●'))
        .collect();
    assert_eq!(variant_lines.len(), 2, "{view}");
    let ansi = regex::Regex::new("\x1b\\[[0-9;]*m").unwrap();
    for line in variant_lines {
        let plain = ansi.replace_all(line, "");
        assert!(
            plain.contains("○ {token}") || plain.contains("○ {user*}"),
            "untitled variant must carry its signature in the title column: {plain:?}"
        );
    }
}

#[test]
fn narrow_rows_keep_the_diff_badge_and_mark_the_cut() {
    let schema = schema_build(|b| {
        let name = b.id(scalar("name", "Name", "string"));
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("name", db.s("value-0123456789-abcdefghij"))]));
    let mut app = app_with(schema, doc, test_opts());
    app.width = 30;
    app.height = 24;
    // Edit and apply: the row becomes dirty, so it carries the ~ tail.
    press(&mut app, &["enter", "Z", "enter"]);
    let view = app.view();
    let ansi = regex::Regex::new("\x1b\\[[0-9;]*m").unwrap();
    let row = view
        .lines()
        .map(|l| ansi.replace_all(l, ""))
        .find(|l| l.contains("Name"))
        .expect("the edited row");
    assert!(
        row.chars().count() <= app.width - 1,
        "the row must fit the terminal: {row:?}"
    );
    assert!(row.contains('…'), "the cut must be marked: {row:?}");
    assert!(
        row.ends_with('●'),
        "the collapsed change dot must survive: {row:?}"
    );
}

#[test]
fn narrow_title_keeps_the_dirty_dot_and_marks_the_cut() {
    let mut app = mk_app(test_doc(), test_opts());
    app.width = 24;
    app.height = 24;
    app.fs_mut().opts.path = format!("/tmp/{}.json", "longname".repeat(4));
    set_cursor_path(&mut app, &sp(&["name"]));
    press(&mut app, &["enter", "Z", "enter"]);
    let view = app.view();
    let ansi = regex::Regex::new("\x1b\\[[0-9;]*m").unwrap();
    let title = ansi.replace_all(view.lines().next().expect("title line"), "");
    assert!(
        title.ends_with('●'),
        "the dirty dot must survive: {title:?}"
    );
    assert!(title.contains('…'), "the cut must be marked: {title:?}");
    assert!(
        title.chars().count() <= app.width - 1,
        "the title must fit the terminal: {title:?}"
    );
}

#[test]
fn file_picker_keeps_dirty_dots_on_narrow_paths() {
    let (mut app, _, _) = two_file_app();
    app.width = 30;
    app.height = 24;
    for i in 0..app.files.len() {
        app.files[i].opts.path = format!("/tmp/dir-{}.json", "segment".repeat(3 + i));
    }
    press(&mut app, &["s"]); // file picker, both files dirty
    let view = app.view();
    let ansi = regex::Regex::new("\x1b\\[[0-9;]*m").unwrap();
    let rows: Vec<String> = view
        .lines()
        .map(|l| ansi.replace_all(l, "").into_owned())
        .filter(|l| l.contains("dir-"))
        .collect();
    assert_eq!(rows.len(), 2, "both files are listed: {view}");
    for row in rows {
        assert!(row.contains('…'), "the cut path must be marked: {row:?}");
        assert!(row.ends_with('●'), "the dirty dot must survive: {row:?}");
    }
}
