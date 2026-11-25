//! Unknown keys, live issues, additionalProperties and severity rows.

use super::*;
use super::{
    app_with, bget, dbuild, find_row, getv, has, ks, live_check, mk_app, n_at_idx, num_at,
    opts_validate, path_marked, press, row_at, scalar, schema_build, schema_from_json, set_cursor,
    set_cursor_path, sget, sp, str_at, test_doc, test_opts,
};

use super::*;

#[test]
fn unknown_key_type_picker_object_and_switch() {
    let mut doc = test_doc();
    let extra = doc.new_object();
    let old = doc.new_str("value");
    let _ = doc.set(&sp(&["extra"]), extra);
    let _ = doc.insert_member(&sp(&["extra"]), "old", 0, old);
    let mut m = mk_app(doc.clone(), test_opts());

    // a known object type drills straight into the schema-less object menu
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "extra");
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert!(m.pending_edit.is_none(), "no $EDITOR is launched");
    let nested = find_row(&mut m, |r, _| {
        r.kind == RowKind::Unknown && r.label == "old"
    })
    .expect("unknown nested key");
    let i = m
        .menu_rows()
        .iter()
        .position(|r| same_menu_row(r, &nested))
        .unwrap();
    m.top_mut().unwrap().cursor = i;

    // a nested known string edits directly
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("new"),
        _ => panic!("string editor missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(sget(&m.fs().doc, &sp(&["extra", "old"])), "new");

    // t explicitly opens the type picker even with a value
    press(&mut m, &["esc"]);
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "extra");
    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    let titles: Vec<String> = m
        .menu_rows()
        .iter()
        .filter_map(|r| r.variant)
        .map(|v| m.fs().schema.nodes[v as usize].title.clone())
        .collect();
    assert_eq!(
        titles,
        [
            "String", "Number", "Boolean", "Object", "Array", "Null", "JSON"
        ]
    );
    press(&mut m, &["home", "enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("text"),
        _ => panic!("string editor missing after type switch"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["extra"]), "text");

    // the confirm nulls the old value as one step; the new string is the
    // next, so two undos restore the object
    press(&mut m, &["u"]);
    assert!(matches!(
        m.fs()
            .doc
            .value(getv(&m.fs().doc, &sp(&["extra"])).unwrap()),
        Value::Null
    ));
    press(&mut m, &["u"]);
    assert_eq!(
        sget(&m.fs().doc, &sp(&["extra", "old"])),
        "new",
        "undo must restore the object"
    );
}

#[test]
fn add_unknown_key_uses_type_picker() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["a"]);
    match &mut m.overlay {
        Some(Overlay::Add(s)) => s.ti.set_value("extra"),
        _ => panic!("add-key input missing"),
    }
    press(&mut m, &["enter"]);
    assert!(has(&m.fs().doc, &sp(&["extra"])));
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::OneOf,
        "a new schema-less key picks its type instead of launching $EDITOR"
    );
    assert!(m.pending_edit.is_none());

    // the null placeholder takes the chosen type
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("value"),
        _ => panic!("string editor missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["extra"]), "value");
}

#[test]
fn unknown_array_add_string_edits_inline() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["a"]);
    match &mut m.overlay {
        Some(Overlay::Add(s)) => s.ti.set_value("arr"),
        _ => panic!("add-key input missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    // String, Number, Boolean, Object, Array -> Array
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);

    // < Add item > inserts a null slot, then the type picker opens
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m.top().unwrap().path, vec![ks("arr"), PathSeg::Idx(0)]);
    press(&mut m, &["enter"]); // String
    let rows = m.menu_rows();
    let rendered = match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.path, vec![ks("arr"), PathSeg::Idx(0)]);
            m.has_inline_edit_row(&rows, e)
        }
        _ => panic!("string editor missing"),
    };
    assert!(rendered, "arr[0] must get a rendered inline editor row");
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("hello"),
        _ => unreachable!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(sget(&m.fs().doc, &[ks("arr"), PathSeg::Idx(0)]), "hello");
}

#[test]
fn unknown_key_scalar_inline_edit() {
    let mut doc = test_doc();
    for (k, v) in [
        ("text", Value::Str("old".into())),
        ("count", Value::Num("42".into())),
        ("flag", Value::Bool(true)),
        ("nothing", Value::Null),
    ] {
        let n = doc.push(NodeData {
            value: v,
            ..NodeData::default()
        });
        let _ = doc.set(&sp(&[k]), n);
    }
    let objval = doc.new_object();
    let _ = doc.set(&sp(&["objval"]), objval);
    let a = doc.new_num("1");
    let _ = doc.insert_member(&sp(&["objval"]), "a", 0, a);
    let arr = doc.new_array();
    let one = doc.new_num("1");
    let _ = doc.set(&sp(&["arrval"]), arr);
    let _ = doc.insert(&sp(&["arrval"]), 0, one);
    let mut m = mk_app(doc.clone(), test_opts());

    // string: known type edits directly
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "text");
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.synth.as_ref().unwrap().json_type, "string");
            assert_eq!(e.ti.value(), "old");
        }
        _ => panic!("string value must edit in place"),
    }
    assert!(m.pending_edit.is_none());
    press(&mut m, &["ctrl+u", "n", "e", "w", "enter"]);
    assert_eq!(str_at(&m.fs().doc, &["text"]), "new");

    // number: known type edits directly; invalid text rejected
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "count");
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.synth.as_ref().unwrap().json_type, "number");
            assert_eq!(e.ti.value(), "42");
        }
        _ => panic!("number editor missing"),
    }
    press(&mut m, &["ctrl+u", "n", "o", "p", "enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.err, "must be a number"),
        _ => panic!("invalid number must stay in the editor"),
    }
    press(&mut m, &["ctrl+u", "4", "3", "enter"]);
    assert_eq!(num_at(&m.fs().doc, &["count"]), "43");

    // boolean toggles directly
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "flag");
    press(&mut m, &["enter"]);
    assert_eq!(bget(&m.fs().doc, &sp(&["flag"])), Some(false));
    press(&mut m, &["space"]);
    assert_eq!(bget(&m.fs().doc, &sp(&["flag"])), Some(true));

    // null: choose the Null type (last but one row)
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "nothing"
    });
    press(&mut m, &["enter", "G", "up", "enter"]);
    assert!(matches!(
        m.fs()
            .doc
            .value(getv(&m.fs().doc, &sp(&["nothing"])).unwrap()),
        Value::Null
    ));

    // object: known type drills into a schema-less object menu
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "objval"
    });
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "a");
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("2"),
        _ => panic!("nested number editor missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(num_at(&m.fs().doc, &["objval", "a"]), "2");
    press(&mut m, &["esc"]);

    // array: known type drills into an items menu
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "arrval"
    });
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["down", "enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("2"),
        _ => panic!("array item number editor missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(n_at_idx(&m.fs().doc, "arrval", 0), "2");
}

fn render_path(p: &[PathSeg]) -> String {
    p.iter()
        .map(|s| match s {
            PathSeg::Key(k) => k.clone(),
            PathSeg::Idx(i) => i.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[test]
fn parse_issue_missing_properties() {
    let plural = "at '$': missing properties 'name', 'description', 'runs'";
    let p = crate::ui::rows::parse_issue(plural);
    assert_eq!(
        p.as_ref().map(|p| jsondoc::path_string(p)),
        Some("$.name".to_string())
    );
    let got: Vec<String> = crate::ui::rows::issue_flag_paths(plural)
        .iter()
        .map(|f| jsondoc::path_string(f))
        .collect();
    assert_eq!(got.join(","), "$.name,$.description,$.runs");

    let single = "at '$.server': missing property 'port'";
    let p = crate::ui::rows::parse_issue(single);
    assert_eq!(
        p.as_ref().map(|p| jsondoc::path_string(p)),
        Some("$.server.port".to_string())
    );
    let got = crate::ui::rows::issue_flag_paths(single);
    assert_eq!(got.len(), 1);
    assert_eq!(jsondoc::path_string(&got[0]), "$.server.port");

    // parse_issue table (Go TestParseIssue)
    let cases: &[(&str, &str, bool)] = &[
        (
            "at '$.server.port': minimum value is 1024",
            "server/port",
            true,
        ),
        ("at '$': missing property 'port'", "port", true),
        (
            "at '$.inner': additional properties 'extra_key' not allowed",
            "inner/extra_key",
            true,
        ),
        (
            "at '$': additional properties 'a', 'b' not allowed",
            "a",
            true,
        ),
        ("at '$.a[0].b': got number, want string", "a/0/b", true),
        ("at '$.a/b': unexpected", "a/b", true),
        ("at '$': got string, want object", "", true),
        ("jsonschema validation failed", "", false),
    ];
    for (msg, want, ok) in cases {
        let p = crate::ui::rows::parse_issue(msg);
        assert_eq!(p.is_some(), *ok, "{msg}: ok");
        assert_eq!(render_path(p.as_deref().unwrap_or(&[])), *want, "{msg}");
    }
}

#[test]
fn save_error_jump_real_multiple_required() {
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.required = true;
        let mut desc = scalar("description", "Description", "string");
        desc.required = true;
        let mut runs = scalar("runs", "Runs", "integer");
        runs.required = true;
        let (name, desc, runs) = (b.id(name), b.id(desc), b.id(runs));
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![name, desc, runs],
            ..FormNode::default()
        })
    });
    let opts = opts_validate(vec![
        "at '$': missing properties 'name', 'description', 'runs'".into(),
    ]);
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), opts);
    press(&mut m, &["ctrl+s"]);
    assert!(matches!(m.overlay, Some(Overlay::Errs(_))));
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "enter must jump");
    assert_eq!(jsondoc::path_string(&m.top().unwrap().path), "$");
    let r = row_at(&mut m);
    assert_eq!(jsondoc::path_string(&r.path), "$.name");
}

#[test]
fn save_error_jump_missing_properties() {
    let cases = [
        (
            "root plural",
            "at '$': missing properties 'name', 'description', 'runs'",
            "$",
            "$.name",
        ),
        (
            "nested plural",
            "at '$.server': missing properties 'port', 'host'",
            "$.server",
            "$.server.port",
        ),
    ];
    for (name, msg, top, row) in cases {
        let mut m = mk_app(test_doc(), opts_validate(vec![msg.to_string()]));
        press(&mut m, &["ctrl+s"]);
        assert!(
            matches!(m.overlay, Some(Overlay::Errs(_))),
            "{name}: overlay"
        );
        press(&mut m, &["enter"]);
        assert!(m.overlay.is_none(), "{name}: enter must jump");
        assert_eq!(jsondoc::path_string(&m.top().unwrap().path), top, "{name}");
        let r = row_at(&mut m);
        assert_eq!(jsondoc::path_string(&r.path), row, "{name}");
    }
}

#[test]
fn issues_live_refresh() {
    let doc = test_doc();
    let mut opts = test_opts();
    opts.load_errors = live_check(&doc);
    opts.validate = Some(Arc::new(|d: &Doc| live_check(d)));
    let mut m = mk_app(doc, opts);

    assert!(m.fs().issues_now.len() == 2);
    let rows = m.menu_rows();
    assert!(
        rows[0].label.contains("2 validation errors"),
        "label = {}",
        rows[0].label
    );
    assert!(path_marked(&m.fs().err_paths, &sp(&["server", "port"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["name"])));

    // fix the port
    set_cursor_path(&mut m, &sp(&["server"]));
    press(&mut m, &["enter"]);
    set_cursor_path(&mut m, &sp(&["server", "port"]));
    press(&mut m, &["enter", "ctrl+u", "9", "0", "5", "0", "enter"]);
    assert_eq!(m.fs().issues_now.len(), 1);
    assert!(
        !path_marked(&m.fs().err_paths, &sp(&["server", "port"])),
        "the fixed row must lose its mark"
    );
    press(&mut m, &["esc"]);
    let rows = m.menu_rows();
    assert!(
        rows[0].label.contains("1 validation error"),
        "label = {}",
        rows[0].label
    );

    // fix the name too
    set_cursor_path(&mut m, &sp(&["name"]));
    press(
        &mut m,
        &["enter", "ctrl+u", "f", "i", "x", "e", "d", "enter"],
    );
    assert!(m.fs().issues_now.is_empty());
    assert!(m.fs().err_paths.is_empty() && m.fs().warn_paths.is_empty());

    // returning to a parked list refreshes it
    let doc2 = test_doc();
    let mut o2 = test_opts();
    o2.load_errors = live_check(&doc2);
    o2.validate = Some(Arc::new(|d: &Doc| live_check(d)));
    let mut m2 = mk_app(doc2, o2);
    press(&mut m2, &["enter"]);
    press(&mut m2, &["enter"]);
    set_cursor_path(&mut m2, &sp(&["server", "port"]));
    press(&mut m2, &["enter", "ctrl+u", "9", "0", "5", "0", "enter"]);
    press(&mut m2, &["esc"]);
    match &m2.overlay {
        Some(Overlay::Issues(s)) => {
            assert_eq!(s.items.len(), 1);
            assert!(
                !s.items[0].msg.contains("$.server.port"),
                "fixed entry still listed"
            );
        }
        _ => panic!("returned list should hold the remaining issue"),
    }
}

// ===========================================================================
// menu_issues_test.go
// ===========================================================================

#[test]
fn worst_issue_icon_selection() {
    let cases: Vec<(Vec<Issue>, &str)> = vec![
        (vec![], WARN),
        (
            vec![Issue {
                msg: "unknown key".into(),
                warn: true,
                ..Issue::default()
            }],
            WARN,
        ),
        (
            vec![
                Issue {
                    msg: "unknown key".into(),
                    warn: true,
                    ..Issue::default()
                },
                Issue {
                    msg: "bad value".into(),
                    ..Issue::default()
                },
            ],
            ERR,
        ),
        (
            vec![Issue {
                msg: "bad value".into(),
                ..Issue::default()
            }],
            ERR,
        ),
    ];
    for (items, want) in cases {
        assert_eq!(worst_issue_icon(&items), want);
    }
}

#[test]
fn unknown_key_rows() {
    let mut doc = test_doc();
    let n = doc.new_str("x");
    let _ = doc.set(&sp(&["extra_key"]), n);
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["server", "legacy"]), n);
    let mut m = mk_app(doc, test_opts());

    let extra = find_row(&mut m, |r, _| {
        r.kind == RowKind::Unknown && r.label == "extra_key"
    })
    .expect("unknown row");
    // enter edits a scalar unknown key in place
    let i = m
        .menu_rows()
        .iter()
        .position(|r| same_menu_row(r, &extra))
        .unwrap();
    m.top_mut().unwrap().cursor = i;
    // a known string value edits in place without the type picker
    press(&mut m, &["enter"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Edit(_))),
        "string unknown key should edit in place"
    );
    press(&mut m, &["ctrl+u", "y", "enter"]);
    assert_eq!(str_at(&m.fs().doc, &["extra_key"]), "y");

    // nested unknown key shows in the sub menu
    set_cursor_path(&mut m, &sp(&["server"]));
    press(&mut m, &["enter"]);
    assert!(
        find_row(&mut m, |r, _| r.kind == RowKind::Unknown
            && r.label == "legacy")
        .is_some()
    );
    press(&mut m, &["esc"]);

    // delete removes the root unknown key
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "extra_key"
    });
    press(&mut m, &["delete"]);
    assert!(!has(&m.fs().doc, &sp(&["extra_key"])));
}

#[test]
fn load_issues_row_and_jump() {
    let mut doc = test_doc();
    let n = doc.new_str("x");
    let _ = doc.set(&sp(&["extra_key"]), n);
    let mut opts = test_opts();
    opts.load_errors = vec![
        "at '$.server.port': minimum value is 1024".into(),
        "at '$': additional properties 'extra_key' not allowed".into(),
    ];
    let mut m = mk_app(doc, opts);
    let rows = m.menu_rows();
    assert_eq!(rows[0].kind, RowKind::Issues);
    assert!(
        rows[0].label.contains("2 validation errors"),
        "label = {}",
        rows[0].label
    );

    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.items.len(), 2),
        _ => panic!("issues overlay should list both messages"),
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "jump should close the overlay");
    assert_eq!(
        m.fs().schema.nodes[m.top().unwrap().node as usize].key,
        "server"
    );
    assert_eq!(row_at(&mut m).path, sp(&["server", "port"]));

    press(&mut m, &["esc"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Issues(_))),
        "esc at the jump target returns to the list"
    );
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none() && m.fs().stack.len() == 1);

    // the second entry jumps to the unknown-key row
    m.top_mut().unwrap().cursor = 0;
    press(&mut m, &["enter", "down", "enter"]);
    assert!(m.overlay.is_none());
    let r = row_at(&mut m);
    assert_eq!(r.kind, RowKind::Unknown);
    assert_eq!(r.label, "extra_key");

    // the overlay recomputes against the current doc when a validator exists
    m.fs_mut().opts.validate = Some(Arc::new(|_: &Doc| {
        vec!["at '$.name': got number, want string".into()]
    }));
    m.top_mut().unwrap().cursor = 0;
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => {
            assert_eq!(s.items.len(), 2);
            assert!(
                s.items[0].msg.contains("$.name"),
                "items = {:?}",
                s.items[0].msg
            );
            assert!(
                s.items[1].msg.contains(r#"unknown key "extra_key""#),
                "items = {:?}",
                s.items[1].msg
            );
        }
        _ => panic!("overlay should recompute from the validator"),
    }
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());
}

#[test]
fn additional_properties_dedupes_all_names() {
    let mut doc = test_doc();
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["server", "legacy"]), n);
    let n = doc.new_num("2");
    let _ = doc.set(&sp(&["server", "other"]), n);
    let mut m = mk_app(doc, test_opts());

    for msg in [
        "at '$.server': additional properties 'legacy', 'other' not allowed",
        "at '$.server': additional properties 'other', 'legacy' not allowed",
        "at '$.server': additional properties 'other', 'legacy', 'x' not allowed",
    ] {
        for it in m.live_issues(vec![msg.to_string()]) {
            assert!(!it.warn, "{msg}: warning {:?} was not deduped", it.msg);
        }
    }

    let warned = m
        .live_issues(vec![
            "at '$.server': additional properties 'legacy' not allowed".into(),
        ])
        .into_iter()
        .any(|it| it.warn && it.msg.contains(r#"unknown key "other""#));
    assert!(warned, "unlisted unknown key should keep its warning");
}

#[test]
fn additional_properties_marks_real_unknowns_only() {
    let mut doc = test_doc();
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["server", "legacy"]), n);
    let n = doc.new_num("2");
    let _ = doc.set(&sp(&["server", "other"]), n);
    let mut m = mk_app(doc, test_opts());

    let items = m.live_issues(vec![
        "at '$.server': additional properties 'port', 'host', 'legacy' not allowed".into(),
    ]);
    m.set_issues(items);
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "port"])));
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "host"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["server", "legacy"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["server"])));

    let items = m.live_issues(vec![
        "at '$.server': additional properties 'legacy', 'host', 'port' not allowed".into(),
    ]);
    m.set_issues(items);
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "port"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["server", "legacy"])));

    let items = m.live_issues(vec![
        "at '$.server': additional properties 'port', 'host' not allowed".into(),
    ]);
    m.set_issues(items);
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "port"])));
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "host"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["server"])));

    let items = m.live_issues(vec![
        "at '$': additional properties 'name' not allowed".into(),
    ]);
    m.set_issues(items);
    assert!(
        m.fs().err_paths.is_empty(),
        "root-level mark leaked to rows: {:?}",
        m.fs().err_paths
    );
}

#[test]
fn unknown_key_issues_listed() {
    let mut doc = test_doc();
    let n = doc.new_str("x");
    let _ = doc.set(&sp(&["extra_key"]), n);
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["server", "legacy"]), n);
    let mut m = mk_app(doc, test_opts());

    assert_eq!(m.fs().issues_now.len(), 2);
    assert!(m.fs().issues_now.iter().all(|it| it.warn));
    assert_eq!(m.fs().issues_now[0].path, Some(sp(&["server", "legacy"])));
    assert_eq!(m.fs().issues_now[1].path, Some(sp(&["extra_key"])));
    let rows = m.menu_rows();
    assert!(
        rows[0].label.contains("2 warnings"),
        "label = {}",
        rows[0].label
    );
    assert!(path_marked(&m.fs().warn_paths, &sp(&["server", "legacy"])));
    assert!(path_marked(&m.fs().warn_paths, &sp(&["extra_key"])));

    // validator output combines with the warnings without double-flagging
    m.fs_mut().opts.validate = Some(Arc::new(|d: &Doc| {
        if has(d, &sp(&["extra_key"])) {
            vec!["at '$': additional properties 'extra_key' not allowed".into()]
        } else {
            Vec::new()
        }
    }));
    m.refresh_issues();
    assert_eq!(m.fs().issues_now.len(), 2);
    assert!(m.fs().issues_now[0].msg.contains("additional properties"));
    assert!(m.fs().issues_now[1].msg.contains(r#"unknown key "legacy""#));
    assert!(!m.fs().issues_now[0].warn && m.fs().issues_now[1].warn);
    let rows = m.menu_rows();
    assert!(
        rows[0].label.contains("1 validation error"),
        "label = {}",
        rows[0].label
    );
    assert!(
        rows[0].label.contains("1 warning"),
        "label = {}",
        rows[0].label
    );
    assert!(path_marked(&m.fs().err_paths, &sp(&["extra_key"])));
    assert!(path_marked(&m.fs().warn_paths, &sp(&["server", "legacy"])));

    // removing the keys clears their warnings
    m.fs_mut().doc.remove(&sp(&["extra_key"]));
    m.fs_mut().doc.remove(&sp(&["server", "legacy"]));
    m.refresh_issues();
    assert!(m.fs().issues_now.is_empty());
}

#[test]
fn open_additional_properties_not_unknown() {
    let open_schema = schema_from_json(
        r#"{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":{"type":"string"}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("a", db.s("x")), ("extra", db.s("y"))]));
    assert!(
        crate::ui::rows::unknown_key_issues(&open_schema, &doc).is_empty(),
        "an open additionalProperties object defines every key"
    );
    let mut m = app_with(open_schema, doc, test_opts());
    let row = m
        .menu_rows()
        .into_iter()
        .find(|r| r.path == sp(&["extra"]))
        .expect("extra row");
    assert_eq!(row.kind, RowKind::Unknown, "extra still edits by shape");
    assert!(row.in_schema, "extra lives in an open mapping");
    assert!(
        !m.row_line(&row, false, 40).contains("(not in schema)"),
        "open-mapping keys are not marked unknown: {}",
        m.row_line(&row, false, 40)
    );

    // an unresolved oneOf counts any open branch as opening the mapping
    let union_schema = schema_from_json(
        r#"{"type":"object","properties":{"v":{"oneOf":[
            {"type":"object","properties":{"kind":{"type":"string"}},"additionalProperties":false},
            {"type":"object","properties":{"kind":{"type":"string"}},"additionalProperties":{"type":"string"}}]}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("v", db.obj(vec![("extra", db.s("y"))]))]));
    assert!(
        crate::ui::rows::unknown_key_issues(&union_schema, &doc).is_empty(),
        "any open branch opens an unresolved oneOf"
    );

    // a closed object keeps flagging keys outside `properties`
    let closed_schema = schema_from_json(
        r#"{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":false}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("a", db.s("x")), ("extra", db.s("y"))]));
    let issues = crate::ui::rows::unknown_key_issues(&closed_schema, &doc);
    assert_eq!(issues.len(), 1, "closed object flags extra: {issues:?}");
    let mut m = app_with(closed_schema, doc, test_opts());
    let row = m
        .menu_rows()
        .into_iter()
        .find(|r| r.path == sp(&["extra"]))
        .expect("extra row");
    assert_eq!(row.kind, RowKind::Unknown);
    assert!(!row.in_schema);
    assert!(
        m.row_line(&row, false, 40).contains("(not in schema)"),
        "closed-object extra key stays unknown: {}",
        m.row_line(&row, false, 40)
    );
}

#[test]
fn unknown_key_issues_schema_pointer_exempt() {
    let schema = schema_build(|b| {
        let name = b.id(scalar("name", "Name", "string"));
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| {
        db.obj(vec![
            ("$schema", db.s("mini.schema.json")),
            ("name", db.s("z")),
        ])
    });
    assert!(
        crate::ui::rows::unknown_key_issues(&schema, &doc).is_empty(),
        "$schema must not warn"
    );

    let schema2 = schema_build(|b| {
        let server = b.id(FormNode {
            key: "server".into(),
            title: "Server".into(),
            kind: Kind::Object,
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![server],
            ..FormNode::default()
        })
    });
    let doc2 = dbuild(|db| db.obj(vec![("server", db.obj(vec![("$schema", db.s("x"))]))]));
    let got = crate::ui::rows::unknown_key_issues(&schema2, &doc2);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].path, Some(sp(&["server", "$schema"])));
}

#[test]
fn multi_pattern_keys_are_not_unknown() {
    let schema = schema_from_json(
        r##"{
          "type": "object",
          "properties": {
            "jobs": {
              "type": "object",
              "patternProperties": {
                "^a": {"type": "string"},
                "^b": {"type": "integer"}
              },
              "additionalProperties": false
            }
          }
        }"##,
    );
    let doc = dbuild(|db| {
        db.obj(vec![(
            "jobs",
            db.obj(vec![
                ("a1", db.s("x")),
                ("b1", db.n("2")),
                ("c1", db.b(true)),
            ]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["jobs"]));
    press(&mut m, &["enter"]);
    let rows = m.menu_rows();
    let row = |key: &str| {
        rows.iter()
            .find(|r| r.path == vec![PathSeg::key("jobs"), PathSeg::key(key)])
            .unwrap_or_else(|| panic!("{key} row missing"))
    };
    assert!(row("a1").in_schema, "pattern ^a defines a1");
    assert!(row("b1").in_schema, "pattern ^b defines b1");
    assert!(!row("c1").in_schema, "c1 matches no pattern");
    assert!(
        !m.row_line(row("a1"), false, 40).contains("(not in schema)"),
        "pattern-defined key must not be marked unknown"
    );
    assert!(
        m.row_line(row("c1"), false, 40).contains("(not in schema)"),
        "a key outside every pattern stays unknown"
    );

    let issues = crate::ui::rows::unknown_key_issues(&m.fs().schema, &m.fs().doc);
    assert!(
        issues
            .iter()
            .all(|i| !i.msg.contains("a1") && !i.msg.contains("b1")),
        "pattern matches must not produce unknown-key warnings: {issues:?}"
    );
    assert!(
        issues.iter().any(|i| i.msg.contains("c1")),
        "nonmatching key must still warn: {issues:?}"
    );
}
