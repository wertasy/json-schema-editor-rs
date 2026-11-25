//! Document search, jump and replace flows.

use super::*;
use super::{
    app_with, cursor_cursor, dbuild, items_app, ks, mk_app, num_at, opts_validate, press, row_at,
    schema_from_json, search_state, session, set_cursor_path, sget, sp, str_at, test_doc,
    test_opts, test_schema,
};

use super::*;

#[test]
fn search_opens_empty_on_the_current_value() {
    // oneOf: the cursor snaps back to the variant matching the document
    // value, even after it was moved away
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "rule": {"oneOf": [
              {"title": "Alpha", "type": "object", "required": ["kind"],
               "properties": {"kind": {"const": "a"}}},
              {"title": "Beta", "type": "object", "required": ["kind"],
               "properties": {"kind": {"const": "b"}}}
            ]}
          }
        }"#,
    );
    let doc = dbuild(|db| {
        let rule = db.obj(vec![("kind", db.s("b"))]);
        db.obj(vec![("rule", rule)])
    });
    let mut m = app_with(schema, doc, test_opts());
    m.width = 80;
    m.height = 16;
    set_cursor_path(&mut m, &sp(&["rule"]));
    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().cursor, 1, "seeded on Beta");
    press(&mut m, &["/"]);
    assert!(m.top().unwrap().filter.active);
    assert_eq!(m.top().unwrap().filter.ti.value(), "", "empty query");
    assert_eq!(m.top().unwrap().cursor, 1, "cursor keeps the value row");
    press(&mut m, &["a", "l"]);
    press(&mut m, &["esc"]);
    assert_eq!(
        m.top().unwrap().cursor,
        1,
        "cancelling restores the value cursor"
    );

    // enum: opening highlights the current value in the dropdown; typing
    // over it searches the options by keyword
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "mode":{"type":"string","enum":["http","grpc","tcp"]}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("mode", db.s("tcp"))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.sug_idx, 2, "current value tcp is highlighted");
            assert_eq!(e.ti.value(), "tcp");
            assert_eq!(e.sugs, vec!["http", "grpc", "tcp"]);
        }
        _ => panic!("enum dropdown should open"),
    }
    press(&mut m, &["g", "r"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.ti.value(), "gr");
            assert_eq!(e.sugs, vec!["grpc"], "keyword narrowed the options");
        }
        _ => panic!("enum dropdown should stay open"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
}

#[test]
fn issues_list_search_and_cancel() {
    let mk = |msg: &str, path: &[&str]| Issue {
        msg: msg.into(),
        path: Some(sp(path)),
        ..Issue::default()
    };
    let mut m = mk_app(test_doc(), test_opts());
    m.width = 80;
    m.height = 12;
    m.overlay = Some(Overlay::Issues(IssuesState {
        items: vec![
            mk("at '$.name': alpha problem", &["name"]),
            mk("at '$.server': beta problem", &["server"]),
        ],
        cursor: 1,
        ..IssuesState::default()
    }));

    press(&mut m, &["/", "b", "e", "t", "a"]);
    assert!(m.overlay.is_some());
    let view = m.view();
    assert!(view.contains("beta problem"), "{view}");
    assert!(!view.contains("alpha problem"), "{view}");

    press(&mut m, &["esc"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => {
            assert!(!s.filter.active);
            assert_eq!(s.filter.ti.value(), "");
            assert_eq!(s.cursor, 1, "cancel restores the row");
        }
        _ => panic!("issues overlay should stay open"),
    }

    press(&mut m, &["/", "b", "e", "t", "a"]);
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "enter jumps and closes the list");
    assert_eq!(m.top().unwrap().path, sp(&["server"]));
}

#[test]
fn save_error_list_search() {
    let mut m = mk_app(test_doc(), test_opts());
    m.width = 80;
    m.height = 12;
    m.overlay = Some(Overlay::Errs(ErrsState {
        lines: vec!["$.name: bad".into(), "$.server: worse".into()],
        paths: vec![Some(sp(&["name"])), Some(sp(&["server"]))],
        cursor: 0,
        exit: false,
        target: String::new(),
        filter: Default::default(),
    }));

    press(&mut m, &["/", "w", "o", "r"]);
    let view = m.view();
    assert!(view.contains("worse"), "{view}");
    assert!(!view.contains("bad"), "{view}");
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().path, sp(&["server"]));
}

#[test]
fn file_picker_search() {
    let opts = |path: &str| {
        let mut o = test_opts();
        o.path = path.into();
        o
    };
    let schema1 = test_schema();
    let schema2 = test_schema();
    let mut m = App::new(vec![
        session(schema1, test_doc(), opts("alpha.json")),
        session(schema2, test_doc(), opts("beta.json")),
    ]);
    m.width = 80;
    m.height = 12;
    m.overlay = Some(Overlay::FilePick(FilePickState {
        cursor: 0,
        filter: Default::default(),
    }));

    press(&mut m, &["/", "b", "e", "t", "a"]);
    let view = m.view();
    assert!(view.contains("beta.json"), "{view}");
    assert!(!view.contains("alpha.json"), "{view}");
    press(&mut m, &["enter"]);
    assert_eq!(m.cur, 1);
    assert!(!matches!(m.overlay, Some(Overlay::FilePick(_))));
}

#[test]
fn search_from_a_null_object_lists_its_fields() {
    let mut doc = test_doc();
    let null = doc.new_null();
    doc.set(&sp(&["server"]), null).unwrap();
    let mut m = app_with(test_schema(), doc, test_opts());
    set_cursor_path(&mut m, &sp(&["server"]));
    press(&mut m, &["enter"]);
    assert_eq!(m.fs().stack.len(), 2, "null object still opens its menu");

    press(&mut m, &["/", "port"]);
    let results = &search_state(&m).results;
    assert!(
        results.iter().any(|r| r.path == sp(&["server", "port"])),
        "a null object's fields must be searchable: {:?}",
        results.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn jump_keys() {
    let mut m = mk_app(test_doc(), test_opts());
    let last = m.menu_rows().len() - 1;
    press(&mut m, &["G"]);
    assert_eq!(cursor_cursor(&mut m), last);
    press(&mut m, &["end"]);
    assert_eq!(cursor_cursor(&mut m), last);
    press(&mut m, &["g", "g"]);
    assert_eq!(cursor_cursor(&mut m), 0);
    press(&mut m, &["home"]);
    assert_eq!(cursor_cursor(&mut m), 0);

    // a lone g arms but does not move; another key disarms it
    press(&mut m, &["j"]);
    press(&mut m, &["g"]);
    assert_eq!(cursor_cursor(&mut m), 1);
    assert_eq!(m.cmd_buf, vec!["g".to_string()]);
    press(&mut m, &["j"]);
    assert_eq!(cursor_cursor(&mut m), 2);
    assert!(m.cmd_buf.is_empty());

    // an armed g still fires after a jump key
    press(&mut m, &["G", "g", "g"]);
    assert_eq!(cursor_cursor(&mut m), 0);

    // the buffer never expires
    press(&mut m, &["G", "g"]);
    assert_eq!(m.cmd_buf, vec!["g".to_string()]);
    press(&mut m, &["g"]);
    assert_eq!(cursor_cursor(&mut m), 0);

    // recording and a pending command coexist
    press(&mut m, &["g", "@"]);
    assert!(m.recording);
    press(&mut m, &["@"]);
    assert!(!m.recording);
    assert_eq!(
        m.cmd_buf,
        vec!["g".to_string()],
        "macro control must leave the pending command alone"
    );
    press(&mut m, &["g"]);
    assert_eq!(cursor_cursor(&mut m), 0);
    assert!(m.cmd_buf.is_empty());
}

#[test]
fn jump_keys_overlays() {
    // issues overlay
    let msgs = vec![
        "at '$.server.port': minimum value is 1024".to_string(),
        "at '$.name': got number, want string".to_string(),
    ];
    let mut opts = test_opts();
    opts.load_errors = msgs;
    let mut m = mk_app(test_doc(), opts);
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!("issues overlay should open"),
    }
    press(&mut m, &["G"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 1),
        _ => panic!(),
    }
    press(&mut m, &["end"]);
    press(&mut m, &["g", "g"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
    press(&mut m, &["home"]);
    press(&mut m, &["esc"]);

    // enum select dropdown: the highlight wraps over the options
    press(&mut m, &["down", "down"]); // issues row → Name → Mode
    press(&mut m, &["enter"]);
    let n = match &m.overlay {
        Some(Overlay::Edit(e)) => e.sugs.len(),
        _ => panic!("enum dropdown should open"),
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

    // save-error overlay
    let mut eopts = opts_validate(vec![
        "at '$.name': got number, want string".into(),
        "at '$.server.port': minimum value is 1024".into(),
        "at '$.mode': bad mode".into(),
    ]);
    eopts.force = true;
    eopts.save = Some(Arc::new(|_, _| Ok(())));
    let mut m2 = mk_app(test_doc(), eopts);
    press(&mut m2, &["ctrl+s"]);
    press(&mut m2, &["G"]);
    match &m2.overlay {
        Some(Overlay::Errs(s)) => assert_eq!(s.cursor, 2),
        _ => panic!("errs overlay should open"),
    }
    press(&mut m2, &["g", "g"]);
    match &m2.overlay {
        Some(Overlay::Errs(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
    press(&mut m2, &["home"]);
    press(&mut m2, &["end"]);
    match &m2.overlay {
        Some(Overlay::Errs(s)) => assert_eq!(s.cursor, 2),
        _ => panic!(),
    }
}

#[test]
fn search_jump_to_array_item_variant_field() {
    // sing-box-like dns.rules: an array whose items are classic oneOf
    // objects, with a scalar fallback so the picker is not a dynamic menu.
    let mut variants = String::new();
    for i in 0..6 {
        if i > 0 {
            variants.push(',');
        }
        variants.push_str(&format!(
            r#"{{
              "title": "Rule{i}",
              "type": "object",
              "properties": {{
                "type": {{"type": "string"}},
                "inbound": {{"type": "string"}},
                "ip_version": {{"type": "string"}},
                "domain_suffix": {{"type": "array", "items": {{"type": "string"}}}},
                "server": {{"type": "string"}}
              }},
              "required": ["type", "server"]
            }}"#
        ));
    }
    let src = format!(
        r#"{{
          "type": "object",
          "properties": {{
            "dns": {{
              "type": "object",
              "properties": {{
                "rules": {{
                  "type": "array",
                  "items": {{ "oneOf": [{variants}] }}
                }}
              }}
            }}
          }}
        }}"#
    );
    let schema = schema_from_json(&src);
    let doc = dbuild(|db| {
        let rule = db.obj(vec![
            ("type", db.s("default")),
            ("server", db.s("dns-proxy")),
            ("domain_suffix", db.arr(vec![db.s("a.com")])),
        ]);
        let first = db.obj(vec![("type", db.s("default")), ("server", db.s("other"))]);
        db.obj(vec![(
            "dns",
            db.obj(vec![("rules", db.arr(vec![first, rule]))]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "d", "n", "s"]);
    let want = vec![ks("dns"), ks("rules"), PathSeg::Idx(1), ks("server")];
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.path == want)
        .expect("rules[1].server search row");
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::Object,
        "jump must open the array item menu, not the variant picker"
    );
    assert_eq!(
        m.top().unwrap().path,
        vec![ks("dns"), ks("rules"), PathSeg::Idx(1)]
    );
    let row = row_at(&mut m);
    assert_eq!(row.path, want);
    assert_eq!(row.kind, RowKind::Field);
}

#[test]
fn search_jump_to_array_variant_element() {
    // anyOf string|array: a document array matches the array variant, so its
    // elements are indexed and the jump opens the items menu on the element
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "rule_set": {"anyOf": [
              {"type": "string"},
              {"type": "array", "items": {"type": "string"}}
            ]}
          }
        }"#,
    );
    let doc = dbuild(|db| {
        db.obj(vec![(
            "rule_set",
            db.arr(vec![db.s("ruleset-proxy-ai"), db.s("ruleset-proxy-github")]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "g", "i", "t", "h", "u", "b"]);
    let want = vec![ks("rule_set"), PathSeg::Idx(1)];
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.path == want)
        .expect("rule_set[1] search row");
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    assert_eq!(m.top().unwrap().path, vec![ks("rule_set")]);
    let row = row_at(&mut m);
    assert_eq!(row.path, want);
    assert_eq!(row.kind, RowKind::Field);
}

#[test]
fn search_jump_schema_less_nested() {
    // No schema: a hit below an unknown key must rebuild the runtime menus
    // Enter would open, landing on the actual row instead of the top key.
    let doc = dbuild(|db| {
        db.obj(vec![
            ("server", db.obj(vec![("port", db.n("8080"))])),
            ("tags", db.arr(vec![db.s("alpha"), db.s("beta")])),
            ("arr", db.arr(vec![db.obj(vec![("name", db.s("deep"))])])),
        ])
    });
    let schema = crate::smodel::untyped(&doc);
    let mut m = app_with(schema, doc, test_opts());

    let jump = |m: &mut App, query: &[&str], want: &[PathSeg]| {
        press(m, &["/"]);
        for q in query {
            press(m, &[*q]);
        }
        let idx = search_state(m)
            .results
            .iter()
            .position(|r| r.path == want)
            .unwrap_or_else(|| {
                panic!(
                    "search row {want:?} missing: {:?}",
                    search_state(m)
                        .results
                        .iter()
                        .map(|r| (r.kind, r.path.clone()))
                        .collect::<Vec<_>>()
                )
            });
        if let Some(Overlay::Search(s)) = &mut m.overlay {
            s.cursor = idx;
        }
        press(m, &["enter"]);
        assert!(m.overlay.is_none());
    };
    // leave the landing, reopen the search list, then close it to the root
    let rewind = |m: &mut App| {
        press(m, &["esc"]);
        press(m, &["esc"]);
    };

    jump(&mut m, &["p", "o", "r", "t"], &[ks("server"), ks("port")]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert_eq!(m.top().unwrap().path, vec![ks("server")]);
    let row = row_at(&mut m);
    assert_eq!(row.path, vec![ks("server"), ks("port")]);
    assert_eq!(row.kind, RowKind::Unknown);

    rewind(&mut m);
    jump(
        &mut m,
        &["b", "e", "t", "a"],
        &[ks("tags"), PathSeg::Idx(1)],
    );
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    assert_eq!(m.top().unwrap().path, vec![ks("tags")]);
    assert_eq!(row_at(&mut m).path, vec![ks("tags"), PathSeg::Idx(1)]);
    assert_eq!(row_at(&mut m).kind, RowKind::Drill);

    rewind(&mut m);
    jump(
        &mut m,
        &["d", "e", "e", "p"],
        &[ks("arr"), PathSeg::Idx(0), ks("name")],
    );
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert_eq!(m.top().unwrap().path, vec![ks("arr"), PathSeg::Idx(0)]);
    let row = row_at(&mut m);
    assert_eq!(row.path, vec![ks("arr"), PathSeg::Idx(0), ks("name")]);
    assert_eq!(row.kind, RowKind::Unknown);
}

#[test]
fn search_skips_missing_parent_paths() {
    let src = r#"{
      "type": "object",
      "properties": {
        "endpoints": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "servers": {
                "type": "array",
                "items": {
                  "type": "object",
                  "properties": {
                    "server": {
                      "oneOf": [
                        {"type": "string"},
                        {
                          "title": "ServerObject",
                          "type": "object",
                          "properties": {"addr": {"type": "string"}}
                        }
                      ]
                    }
                  }
                }
              }
            }
          }
        }
      }
    }"#;

    // servers is absent under endpoints[0]
    let schema = schema_from_json(src);
    let doc = dbuild(|db| db.obj(vec![("endpoints", db.arr(vec![db.obj(vec![])]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "s", "e", "r", "v", "e", "r"]);
    let deep = [
        ks("endpoints"),
        PathSeg::Idx(0),
        ks("servers"),
        PathSeg::Idx(0),
        ks("server"),
    ];
    assert!(
        !search_state(&m).results.iter().any(|r| r.path == deep),
        "do not index below a missing parent object"
    );

    // the array itself is empty: no endpoints[0] rows at all
    let schema = schema_from_json(src);
    let doc = dbuild(|db| db.obj(vec![("endpoints", db.arr(vec![]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "s", "e", "r", "v", "e", "r"]);
    let prefix = [ks("endpoints"), PathSeg::Idx(0)];
    assert!(
        !search_state(&m)
            .results
            .iter()
            .any(|r| r.path.starts_with(&prefix)),
        "empty arrays must not synthesize item paths"
    );
}

#[test]
fn menu_search_jump() {
    let mut m = items_app(test_doc());
    press(&mut m, &["/"]);
    assert_eq!(
        m.overlay.as_ref().map(|o| matches!(o, Overlay::Search(_))),
        Some(true)
    );
    press(&mut m, &["p", "o", "r", "t"]);
    assert_eq!(search_state(&m).results.len(), 1);
    press(&mut m, &["enter"]);
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(m.top().unwrap().path, sp(&["server"]));
    assert_eq!(row_at(&mut m).path, sp(&["server", "port"]));

    // esc clears search without moving
    let mut m2 = items_app(test_doc());
    press(&mut m2, &["/", "t", "o", "k", "esc"]);
    assert!(!matches!(m2.overlay, Some(Overlay::Search(_))));
    assert_eq!(m2.fs().stack.len(), 1);

    // search is scoped to the breadcrumb path
    let mut m3 = items_app(test_doc());
    press(&mut m3, &["down", "down", "enter"]);
    press(&mut m3, &["/", "n", "a", "m", "e"]);
    assert_eq!(search_state(&m3).results.len(), 0);
    press(&mut m3, &["esc"]);
    press(&mut m3, &["/", "0", ".", "0"]);
    assert_eq!(search_state(&m3).results.len(), 2);
    assert!(
        search_state(&m3)
            .results
            .iter()
            .any(|r| r.path == sp(&["server", "host"]))
    );
    press(&mut m3, &["esc"]);
    press(&mut m3, &["/", "0", "\\", ".", "0"]);
    assert_eq!(search_state(&m3).results.len(), 1);
    press(&mut m3, &["esc"]);
}

#[test]
fn menu_search_jump_return() {
    let mut m = items_app(test_doc());
    press(&mut m, &["/", "8", "0", "8", "0"]);
    press(&mut m, &["enter"]);
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(row_at(&mut m).path, sp(&["server", "port"]));

    press(&mut m, &["esc"]);
    let s = search_state(&m);
    assert_eq!(s.results.len(), 1);
    assert_eq!(s.cursor, 0);
    assert_eq!(s.ti.value(), "8080");

    press(&mut m, &["esc"]);
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 1);
    assert_eq!(row_at(&mut m).path, sp(&["name"]));

    // edits at the landing refresh the list on return
    let mut m2 = items_app(test_doc());
    press(&mut m2, &["/", "8", "0", "8", "0", "enter"]);
    press(&mut m2, &["enter", "ctrl+u", "9", "9", "9", "9", "enter"]);
    assert_eq!(num_at(&m2.fs().doc, &["server", "port"]), "9999");
    press(&mut m2, &["esc"]);
    assert_eq!(search_state(&m2).results.len(), 0);

    // landing on a container: drill deeper first, the overlay comes last
    let mut m3 = items_app(test_doc());
    press(&mut m3, &["/", "m", "o", "u", "n", "t", "s"]);
    press(&mut m3, &["enter"]);
    assert_eq!(m3.fs().stack.len(), 2);
    press(&mut m3, &["enter"]);
    assert_eq!(m3.fs().stack.len(), 3);
    press(&mut m3, &["esc"]);
    assert!(!matches!(m3.overlay, Some(Overlay::Search(_))));
    assert_eq!(m3.fs().stack.len(), 2);
    press(&mut m3, &["esc"]);
    assert!(matches!(m3.overlay, Some(Overlay::Search(_))));

    // replace mode parks and returns the same way
    let mut m4 = items_app(test_doc());
    press(&mut m4, &["r", "w", "e", "b", "a", "p", "p", "enter"]);
    press(&mut m4, &["esc"]);
    let s = search_state(&m4);
    assert!(s.replace);
}

#[test]
fn menu_search_second_jump_return() {
    let mut m = items_app(test_doc());
    press(&mut m, &["/", "8", "0", "8", "0", "enter"]);
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["esc"]);
    assert!(matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 1);

    press(
        &mut m,
        &["ctrl+u", "0", ".", "0", ".", "0", ".", "0", "enter"],
    );
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["esc"]);
    assert!(matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 1);
    press(&mut m, &["esc"]);
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 1);
    assert_eq!(row_at(&mut m).path, sp(&["name"]));
}

#[test]
fn menu_search_replace() {
    // r opens replace mode with the search input focused
    let mut m = items_app(test_doc());
    press(&mut m, &["r"]);
    assert!(search_state(&m).replace);
    assert!(!search_state(&m).replace_focused);

    press(&mut m, &["/", "d", "a", "t", "a"]);
    assert_eq!(search_state(&m).results.len(), 1);
    press(&mut m, &["tab"]);
    assert!(search_state(&m).replace_focused);
    press(&mut m, &["/", "o", "p", "t"]);
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("src")]),
        "/opt"
    );
    assert!(
        matches!(m.overlay, Some(Overlay::Search(_))),
        "enter keeps the overlay open"
    );
    assert_eq!(search_state(&m).results.len(), 0);
    // esc leaves replace mode first; the overlay itself stays open
    press(&mut m, &["esc"]);
    let s = search_state(&m);
    assert!(!s.replace && !s.replace_focused);
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("dst")]),
        "/mnt"
    );

    // ctrl+r replaces every hit in scope; numbers stay untouched
    let doc2 = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("alpha")),
            (
                "server",
                db.obj(vec![
                    ("host", db.s("alpha.example.com")),
                    ("port", db.n("8080")),
                ]),
            ),
        ])
    });
    let mut m2 = items_app(doc2);
    press(
        &mut m2,
        &[
            "r", "a", "l", "p", "h", "a", "tab", "b", "e", "t", "a", "ctrl+r",
        ],
    );
    assert_eq!(str_at(&m2.fs().doc, &["name"]), "beta");
    assert_eq!(
        str_at(&m2.fs().doc, &["server", "host"]),
        "beta.example.com"
    );
    assert_eq!(num_at(&m2.fs().doc, &["server", "port"]), "8080");
    assert!(m2.overlay.is_none());
    assert_eq!(m2.notice, "replaced 2 occurrence(s) in 2 value(s)");

    // alt+i toggles case sensitivity
    let doc3 = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("alpha")),
            ("server", db.obj(vec![("host", db.s("alpha.example.com"))])),
        ])
    });
    let mut m3 = items_app(doc3);
    press(&mut m3, &["r", "A", "L", "P", "H", "A"]);
    assert_eq!(search_state(&m3).results.len(), 2);
    press(&mut m3, &["alt+i"]);
    assert_eq!(search_state(&m3).results.len(), 0);
    press(&mut m3, &["enter", "ctrl+r"]);
    assert_eq!(str_at(&m3.fs().doc, &["name"]), "alpha");
    press(&mut m3, &["alt+i"]);
    assert_eq!(search_state(&m3).results.len(), 2);

    // the replace scope follows the breadcrumb
    let doc6 = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("same")),
            ("server", db.obj(vec![("host", db.s("same"))])),
        ])
    });
    let mut m6 = items_app(doc6);
    press(&mut m6, &["down", "down", "enter"]);
    press(
        &mut m6,
        &[
            "r", "s", "a", "m", "e", "tab", "o", "t", "h", "e", "r", "ctrl+r",
        ],
    );
    assert_eq!(str_at(&m6.fs().doc, &["server", "host"]), "other");
    assert_eq!(str_at(&m6.fs().doc, &["name"]), "same");

    // / arms replace with tab
    let mut m4 = items_app(test_doc());
    press(&mut m4, &["/", "t", "a", "b"]);
    assert!(!search_state(&m4).replace);
    press(&mut m4, &["tab"]);
    assert!(search_state(&m4).replace && search_state(&m4).replace_focused);

    // esc aborts without writing
    let doc5 = test_doc();
    let mut m5 = items_app(doc5);
    press(
        &mut m5,
        &["r", "/", "d", "a", "t", "a", "tab", "z", "z", "z", "esc"],
    );
    assert!(!search_state(&m5).replace, "esc drops back to search mode");
    assert_eq!(
        sget(&m5.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("src")]),
        "/data"
    );

    // esc leaves replace mode, the second esc closes the overlay
    press(&mut m4, &["esc"]);
    assert!(!search_state(&m4).replace);
    press(&mut m4, &["esc"]);
    assert!(m4.overlay.is_none());

    // with replace armed, enter on the search input still jumps
    press(&mut m4, &["/", "w", "e", "b", "a", "p", "p"]);
    assert_eq!(search_state(&m4).results.len(), 1);
    press(&mut m4, &["tab", "tab"]);
    assert!(search_state(&m4).replace && !search_state(&m4).replace_focused);
    press(&mut m4, &["enter"]);
    assert!(!matches!(m4.overlay, Some(Overlay::Search(_))));
    assert_eq!(cursor_cursor(&mut m4), 0);
    assert_eq!(row_at(&mut m4).path, sp(&["name"]));
}

#[test]
fn search_replace_caret_follows_focused_input() {
    let mut m = items_app(test_doc());
    m.width = 80;
    m.height = 20;
    fn row<'a>(view: &'a str, prefix: &str) -> &'a str {
        view.lines()
            .find(|l| l.contains(prefix))
            .unwrap_or_else(|| panic!("missing {prefix:?} row:\n{view}"))
    }
    press(&mut m, &["r", "d", "a", "t", "a", "tab", "x"]);
    let view = m.view();
    // tab focused the replace input: only it shows the reverse-video caret
    assert!(
        !row(&view, "search:").contains("\x1b[7m"),
        "search row has a caret:\n{view}"
    );
    assert!(
        row(&view, "replace:").contains("\x1b[7m"),
        "replace row lost its caret:\n{view}"
    );

    press(&mut m, &["tab"]);
    let view = m.view();
    assert!(
        row(&view, "search:").contains("\x1b[7m"),
        "search row lost its caret:\n{view}"
    );
    assert!(
        !row(&view, "replace:").contains("\x1b[7m"),
        "replace row has a caret:\n{view}"
    );
}

#[test]
fn search_input_alt_word_motion() {
    let mut m = items_app(test_doc());
    press(&mut m, &["/"]);
    press(&mut m, &["o", "n", "e", "space", "t", "w", "o"]);
    press(&mut m, &["alt+b", "alt+b"]);
    press(&mut m, &["alt+f"]);
    press(&mut m, &["X"]);
    assert_eq!(search_state(&m).ti.value(), "oneX two");
    press(&mut m, &["alt+f"]);
    press(&mut m, &["Z"]);
    assert_eq!(search_state(&m).ti.value(), "oneX twoZ");
    press(&mut m, &["alt+b"]);
    press(&mut m, &["Y"]);
    assert_eq!(search_state(&m).ti.value(), "oneX YtwoZ");
}

#[test]
fn search_help_lists_only_the_active_mode() {
    let mut m = items_app(test_doc());
    m.width = 120;
    m.height = 20;
    press(&mut m, &["/"]);
    let view = m.view();
    assert!(view.contains("tab replace"), "search help:\n{view}");
    assert!(view.contains("esc close"), "search help:\n{view}");
    assert!(!view.contains("ctrl+r"), "search help:\n{view}");

    press(&mut m, &["tab"]);
    let view = m.view();
    assert!(view.contains("ctrl+r all"), "replace help:\n{view}");
    assert!(view.contains("tab focus"), "replace help:\n{view}");
    assert!(!view.contains("tab replace"), "replace help:\n{view}");
    assert!(!view.contains("esc close"), "replace help:\n{view}");

    // esc drops back to search mode, and the help follows
    press(&mut m, &["esc"]);
    let view = m.view();
    assert!(view.contains("tab replace"), "search help:\n{view}");
    assert!(!view.contains("ctrl+r"), "search help:\n{view}");
}

#[test]
fn menu_search_regex() {
    // regex is the default query mode; alt+x toggles literal matching
    let mut m = items_app(test_doc());
    press(&mut m, &["/"]);
    assert!(search_state(&m).regex);
    press(&mut m, &["0", "\\", ".", "0"]);
    assert_eq!(search_state(&m).results.len(), 1);
    press(&mut m, &["alt+x"]);
    assert_eq!(search_state(&m).results.len(), 0);
    press(&mut m, &["alt+x"]);
    assert_eq!(search_state(&m).results.len(), 1);
    press(&mut m, &["esc"]);

    // "$." anchors the match at the document root
    let mut m5 = items_app(test_doc());
    press(&mut m5, &["/"]);
    press(&mut m5, &["$", ".", "s", "e", "r", "v", "e", "r"]);
    let results = &search_state(&m5).results;
    assert!(!results.is_empty());
    for r in results {
        assert!(
            jsondoc::path_string(&r.path).starts_with("$.server"),
            "hit {:?}",
            r.path
        );
    }
    assert_eq!(search_state(&m5).results[0].path, sp(&["server"]));
    press(&mut m5, &["esc"]);

    // deeper than the root: an anchored query is not a substring search
    let mut m6 = items_app(test_doc());
    press(&mut m6, &["/"]);
    press(&mut m6, &["$", ".", "p", "o", "r", "t"]);
    assert_eq!(search_state(&m6).results.len(), 0);
    press(&mut m6, &["esc"]);

    // a path index still being typed keeps the regex valid
    let mut m7 = items_app(test_doc());
    press(&mut m7, &["/"]);
    press(&mut m7, &["$", ".", "m", "o", "u", "n", "t", "s", "["]);
    assert!(
        search_state(&m7).re_err.is_empty(),
        "re_err = {}",
        search_state(&m7).re_err
    );
    press(&mut m7, &["0", "]"]);
    assert_eq!(search_state(&m7).results.len(), 3);
    assert_eq!(
        search_state(&m7).results[0].path,
        vec![ks("mounts"), PathSeg::Idx(0)]
    );
    press(&mut m7, &["esc"]);

    // an uncompilable pattern yields no results plus an error
    let mut m2 = items_app(test_doc());
    press(&mut m2, &["/", "("]);
    assert_eq!(search_state(&m2).results.len(), 0);
    assert!(!search_state(&m2).re_err.is_empty());
    press(&mut m2, &["esc"]);

    // replace mode: regex by default, $1 groups expand
    let mut m3 = items_app(test_doc());
    press(&mut m3, &["r", "(", "\\", "d", "+", ")"]);
    assert_eq!(search_state(&m3).results.len(), 1);
    press(&mut m3, &["tab", "[", "$", "1", "]"]);
    press(&mut m3, &["enter"]);
    assert_eq!(str_at(&m3.fs().doc, &["server", "host"]), "[0].[0].[0].[0]");
}

#[test]
fn search_replace_unknown_key() {
    let mut doc = test_doc();
    let n = doc.new_str("hello world");
    let _ = doc.set(&sp(&["comment"]), n);
    let mut m = items_app(doc);
    press(&mut m, &["r"]);
    assert!(search_state(&m).replace);
    press(&mut m, &["h", "e", "l", "l", "o"]);
    assert!(
        search_state(&m)
            .results
            .iter()
            .any(|r| r.kind == RowKind::Unknown && r.path == sp(&["comment"]))
    );
    press(&mut m, &["tab"]);
    press(&mut m, &["g", "o", "o", "d", "b", "y", "e"]);
    press(&mut m, &["ctrl+r"]);
    assert_eq!(str_at(&m.fs().doc, &["comment"]), "goodbye world");
    assert!(
        m.notice.contains("replaced 1 occurrence"),
        "notice = {}",
        m.notice
    );
}
