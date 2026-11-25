//! Cursor navigation: change ring (ctrl+j/ctrl+k), fast moves, undo/redo
//! landing and macro record/replay.

use super::*;
use super::{
    app_with, arr_len, cursor_cursor, dbuild, has, items_app, ks, live_check, mk_app, press,
    row_at, scalar, schema_build, search_state, set_cursor_path, sget, sp, str_at, test_doc,
    test_opts,
};

use super::*;

/// After undo/redo the ctrl+j/k ring must still cycle between the changed
/// entries (the batch delete restores three locations; the ring walks them).
#[test]
fn undo_redo_ring_cycles_changed_entries() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/a"))]),
                db.obj(vec![("src", db.s("/b"))]),
                db.obj(vec![("src", db.s("/c"))]),
                db.obj(vec![("src", db.s("/d"))]),
            ]),
        )])
    });
    let mut m = items_app(doc);
    m.fs_mut().baseline = Some(m.fs().doc.clone());
    press(&mut m, &["down", "down", "down", "down", "enter"]); // mounts items
    // visual-select rows 1..3 and batch-delete them in one key
    press(&mut m, &["v", "down", "down", "d"]);
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 1);
    press(&mut m, &["u"]); // undo the batch delete: three restored locations
    let ring = m.fs().change_jump.clone().expect("ring armed");
    assert_eq!(ring.len(), 3, "three restored locations ringed: {ring:?}");
    press(&mut m, &["ctrl+j"]);
    let c1 = cursor_cursor(&mut m);
    press(&mut m, &["ctrl+j"]);
    let c2 = cursor_cursor(&mut m);
    assert!(
        c1 != c2,
        "ctrl+j must move between changed entries; ring={ring:?} c1={c1} c2={c2}"
    );
    press(&mut m, &["ctrl+j"]);
    let c3 = cursor_cursor(&mut m);
    assert!(c3 != c2 || c3 != c1, "ctrl+j must keep cycling");
}

#[test]
fn undo_relands_changed_key_and_ctrl_j_wraps_ring() {
    let doc = dbuild(|db| db.obj(vec![("a", db.s("1")), ("b", db.s("2")), ("c", db.s("3"))]));
    let mut m = app_with(crate::smodel::untyped(&doc), doc, test_opts());

    // two edits; undo reverts the latest one and must land on its key
    for (key, val) in [(&["b"], "B2"), (&["a"], "A2")] {
        set_cursor_path(&mut m, &sp(key));
        press(&mut m, &["enter"]);
        if let Some(Overlay::Edit(e)) = &mut m.overlay {
            e.ti.set_value(val);
        }
        press(&mut m, &["enter"]);
    }
    press(&mut m, &["u"]);
    assert_eq!(sget(&m.fs().doc, &sp(&["a"])), "1");
    assert_eq!(row_at(&mut m).path, sp(&["a"]), "undo lands on the change");
    assert_eq!(m.fs().change_jump, Some(vec![sp(&["a"])]));

    // ctrl+j jumps to the entry still changed against the baseline (b was
    // edited to B2 and never undone); ctrl+k returns to the undone key
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, sp(&["b"]));
    // the only remaining changed entry: ctrl+k wraps onto itself
    press(&mut m, &["ctrl+k"]);
    assert_eq!(row_at(&mut m).path, sp(&["b"]));
}

#[test]
fn replace_all_lands_on_first_replaced_and_ctrl_j_cycles() {
    let doc = dbuild(|db| {
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
    let mut m = items_app(doc);
    press(
        &mut m,
        &[
            "r", "a", "l", "p", "h", "a", "tab", "b", "e", "t", "a", "ctrl+r",
        ],
    );
    assert_eq!(m.notice, "replaced 2 occurrence(s) in 2 value(s)");
    assert_eq!(
        row_at(&mut m).path,
        sp(&["name"]),
        "replace-all lands on the first replaced value"
    );
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, sp(&["server", "host"]));
    press(&mut m, &["ctrl+k"]);
    assert_eq!(row_at(&mut m).path, sp(&["name"]));
}

#[test]
fn issues_drive_ctrl_j_without_a_ring() {
    let doc = test_doc();
    let mut opts = test_opts();
    opts.load_errors = live_check(&doc);
    opts.validate = Some(Arc::new(|d: &Doc| live_check(d)));
    let mut m = mk_app(doc, opts);

    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["server", "port"]),
        "first issue key in validator order"
    );
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, sp(&["name"]));
    press(&mut m, &["ctrl+k"]);
    assert_eq!(row_at(&mut m).path, sp(&["server", "port"]));
}

#[test]
fn search_hits_drive_ctrl_j_after_a_jump() {
    let doc = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("alpha")),
            ("server", db.obj(vec![("host", db.s("alpha.example.com"))])),
        ])
    });
    let mut m = items_app(doc);
    press(&mut m, &["/", "a", "l", "p", "h", "a"]);
    assert_eq!(search_state(&m).results.len(), 2);
    press(&mut m, &["enter"]);
    assert_eq!(row_at(&mut m).path, sp(&["name"]));
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["server", "host"]),
        "ctrl+j continues through the parked search hits"
    );
    press(&mut m, &["ctrl+k"]);
    assert_eq!(row_at(&mut m).path, sp(&["name"]));
}

#[test]
fn ctrl_j_priority_ring_then_issues_then_search() {
    let doc = test_doc();
    let mut opts = test_opts();
    opts.load_errors = live_check(&doc);
    opts.validate = Some(Arc::new(|d: &Doc| live_check(d)));
    let mut m = mk_app(doc, opts);

    // park a search (two hits: server.port and server.host)
    press(&mut m, &["/", "0"]);
    assert_eq!(search_state(&m).results.len(), 2);
    press(&mut m, &["enter"]);
    assert_eq!(row_at(&mut m).path, sp(&["server", "port"]));

    // scenario a beats b and c
    m.fs_mut().change_jump = Some(vec![sp(&["mounts"])]);
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, sp(&["mounts"]));

    // scenario b beats c
    m.fs_mut().change_jump = None;
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, sp(&["server", "port"]));

    // scenario c: no ring, no issues
    m.fs_mut().opts.validate = None;
    m.fs_mut().issues_now.clear();
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, sp(&["server", "host"]));
}

#[test]
fn edit_clears_the_change_ring_and_ctrl_j_walks_changed_keys() {
    let doc = dbuild(|db| db.obj(vec![("a", db.s("1")), ("b", db.s("2"))]));
    let mut m = app_with(crate::smodel::untyped(&doc), doc, test_opts());
    set_cursor_path(&mut m, &sp(&["b"]));
    press(&mut m, &["enter"]);
    if let Some(Overlay::Edit(e)) = &mut m.overlay {
        e.ti.set_value("B2");
    }
    press(&mut m, &["enter"]);
    press(&mut m, &["u"]);
    assert_eq!(row_at(&mut m).path, sp(&["b"]));

    // an unrelated edit invalidates the ring
    set_cursor_path(&mut m, &sp(&["a"]));
    press(&mut m, &["enter"]);
    if let Some(Overlay::Edit(e)) = &mut m.overlay {
        e.ti.set_value("9");
    }
    press(&mut m, &["enter"]);
    assert!(m.fs().change_jump.is_none(), "edit must clear the ring");

    // the default walk is the baseline diff: both edited keys differ from
    // the baseline, and ctrl+j/k cycle them in document order
    set_cursor_path(&mut m, &sp(&["b"]));
    press(&mut m, &["enter"]);
    if let Some(Overlay::Edit(e)) = &mut m.overlay {
        e.ti.set_value("B2");
    }
    press(&mut m, &["enter"]);
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["a"]),
        "first changed key in document order"
    );
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["b"]),
        "wraps to the next changed key"
    );
    press(&mut m, &["ctrl+k"]);
    assert_eq!(row_at(&mut m).path, sp(&["a"]), "ctrl+k walks back");
}

#[test]
fn ctrl_j_without_changes_reports_and_stays() {
    let doc = dbuild(|db| db.obj(vec![("a", db.s("1")), ("b", db.s("2"))]));
    let mut m = app_with(crate::smodel::untyped(&doc), doc, test_opts());
    set_cursor_path(&mut m, &sp(&["b"]));
    press(&mut m, &["ctrl+j"]);
    assert_eq!(m.notice, "no changes since load/save");
    assert_eq!(row_at(&mut m).path, sp(&["b"]), "cursor untouched");
    press(&mut m, &["ctrl+k"]);
    assert_eq!(m.notice, "no changes since load/save");
    assert_eq!(row_at(&mut m).path, sp(&["b"]));
}

#[test]
fn ctrl_j_parked_between_changes_follows_row_order() {
    // changed: beta, delta; the cursor parks on the unchanged gamma between
    // them. next must be delta (below), previous beta (above) — not the
    // ring's top/bottom entry points
    let doc = dbuild(|db| {
        db.obj(vec![
            ("alpha", db.s("0")),
            ("beta", db.s("1")),
            ("gamma", db.s("2")),
            ("delta", db.s("3")),
        ])
    });
    let mut m = app_with(crate::smodel::untyped(&doc), doc, test_opts());
    for (key, val) in [("beta", "B2"), ("delta", "D2")] {
        set_cursor_path(&mut m, &sp(&[key]));
        press(&mut m, &["enter"]);
        if let Some(Overlay::Edit(e)) = &mut m.overlay {
            e.ti.set_value(val);
        }
        press(&mut m, &["enter"]);
    }
    set_cursor_path(&mut m, &sp(&["gamma"]));
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["delta"]),
        "next change sits below the cursor"
    );
    press(&mut m, &["ctrl+k"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["beta"]),
        "previous from delta walks back past gamma"
    );

    // from the top row nothing changed sits above: wrap to the bottommost
    set_cursor_path(&mut m, &sp(&["alpha"]));
    press(&mut m, &["ctrl+k"]);
    assert_eq!(row_at(&mut m).path, sp(&["delta"]));
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["beta"]),
        "no change below delta wraps to the topmost"
    );
}

#[test]
fn ctrl_j_orders_nested_changes_against_ancestor_rows() {
    // changed: server.host, db.url. At the root the changed rows are not
    // listed, but each ranks right after its drill row, so ctrl+j/k still
    // order them around the cursor
    let doc = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("x")),
            ("server", db.obj(vec![("host", db.s("h"))])),
            ("db", db.obj(vec![("url", db.s("u"))])),
        ])
    });
    let mut m = app_with(crate::smodel::untyped(&doc), doc, test_opts());
    for (key, leaf, val) in [("server", "host", "H2"), ("db", "url", "U2")] {
        set_cursor_path(&mut m, &sp(&[key]));
        press(&mut m, &["enter"]);
        set_cursor_path(&mut m, &sp(&[key, leaf]));
        press(&mut m, &["enter"]);
        if let Some(Overlay::Edit(e)) = &mut m.overlay {
            e.ti.set_value(val);
        }
        press(&mut m, &["enter"]);
        press(&mut m, &["esc"]);
    }
    set_cursor_path(&mut m, &sp(&["db"]));
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["db", "url"]),
        "the drill row's changed child is next"
    );
    press(&mut m, &["ctrl+k"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["server", "host"]),
        "previous from db.url is the earlier nested change"
    );
    press(&mut m, &["esc"]); // back to the root menu
    set_cursor_path(&mut m, &sp(&["db"]));
    press(&mut m, &["ctrl+k"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["server", "host"]),
        "last change above the db drill row"
    );
}

#[test]
fn ctrl_k_orders_ring_entries_around_a_drill_row() {
    let doc = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("alpha")),
            ("server", db.obj(vec![("host", db.s("alpha.example.com"))])),
        ])
    });
    let mut m = items_app(doc);
    press(
        &mut m,
        &[
            "r", "a", "l", "p", "h", "a", "tab", "b", "e", "t", "a", "ctrl+r",
        ],
    );
    // ring = [name, server.host], cursor on name; both changed rows are not
    // listed at the root, but server.host ranks right after its drill row
    set_cursor_path(&mut m, &sp(&["server"])); // a row outside the ring
    press(&mut m, &["ctrl+k"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["name"]),
        "ctrl+k from the drill row takes the change above it"
    );
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["server", "host"]),
        "ctrl+j walks back to the change below the drill row"
    );
}

#[test]
fn issues_jump_return() {
    let mut opts = test_opts();
    opts.load_errors = vec![
        "at '$.server.port': minimum value is 1024".into(),
        "at '$.name': got number, want string".into(),
    ];
    let mut m = mk_app(test_doc(), opts);

    press(&mut m, &["enter"]);
    press(&mut m, &["down"]);
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "jump closes the overlay");
    assert_eq!(row_at(&mut m).path, sp(&["name"]));
    press(&mut m, &["esc"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 1),
        _ => panic!("esc at the jump target returns to the issues list"),
    }
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());

    // deeper drill from the jump target pops first, then returns
    m.top_mut().unwrap().cursor = 0;
    press(&mut m, &["enter", "enter"]);
    assert_eq!(
        m.fs().schema.nodes[m.top().unwrap().node as usize].key,
        "server"
    );
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["esc"]);
    assert!(
        m.overlay.is_none(),
        "closing the input must not return to the list"
    );
    press(&mut m, &["esc"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!("esc at the jump target returns to the list"),
    }

    // a search jump clears the parked issues list
    press(&mut m, &["esc"]);
    m.top_mut().unwrap().cursor = 0;
    press(&mut m, &["enter", "enter"]);
    press(&mut m, &["/", "p", "o", "r", "t", "enter"]);
    press(&mut m, &["esc"]);
    assert!(
        !matches!(m.overlay, Some(Overlay::Issues(_))),
        "a search jump must clear the parked issues list"
    );
}

#[test]
fn fast_move_takes_u_as_target() {
    let mut doc = test_doc();
    let n = doc.new_str("x");
    let _ = doc.set(&sp(&["u_key"]), n);
    let mut m = mk_app(doc, test_opts());

    // create one undoable edit so a stray undo is observable
    set_cursor_path(&mut m, &sp(&["name"]));
    press(&mut m, &["enter"]);
    press(&mut m, &["ctrl+u"]);
    for c in "changed".chars() {
        press(&mut m, &[&c.to_string()]);
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "changed");

    // f waits for a target character: u must move, not undo
    press(&mut m, &["home", "f", "u"]);
    assert!(m.fast_move.is_none(), "fast move consumed u");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "changed", "u must not undo");
    assert_eq!(row_at(&mut m).label, "u_key");
}

#[test]
fn undo_ring_cycles_past_an_editor_stay_landing() {
    // one undo step flips a (rule 1 keeps the editor open with the cursor
    // inside) and changes q: ctrl+j must seed from the focused row inside
    // a's editor and advance to q instead of re-landing on a forever.
    let schema = schema_build(|b| {
        let v1 = b.id(scalar("a", "null", "null"));
        let bs = b.id(scalar("b", "B", "string"));
        let v2 = b.id(FormNode {
            key: "a".into(),
            title: "obj".into(),
            kind: Kind::Object,
            children: vec![bs],
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "a".into(),
            title: "A".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        let q = b.id(scalar("q", "Q", "string"));
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one, q],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("a", db.push(Value::Null)), ("q", db.s("old"))]));
    let mut m = app_with(schema, doc, test_opts());

    set_cursor_path(&mut m, &sp(&["a"]));
    press(&mut m, &["enter"]); // variant picker for a
    let obj = m
        .menu_rows()
        .iter()
        .position(|r| {
            r.variant
                .map(|v| m.fs().schema.nodes[v as usize].kind == Kind::Object)
                .unwrap_or(false)
        })
        .expect("object variant row");
    m.top_mut().unwrap().cursor = obj;
    press(&mut m, &["enter"]); // editor opens, doc unchanged

    // one undo step whose diff covers both the a flip and the q change:
    // snapshot the pre-state, then swap the whole document out-of-band
    let pre = m.fs().doc.clone();
    m.fs_mut().history.push_undo(pre);
    m.fs_mut().doc = dbuild(|db| {
        db.obj(vec![
            ("a", db.obj(vec![("b", db.s("1"))])),
            ("q", db.s("new")),
        ])
    });

    press(&mut m, &["u"]);
    assert_eq!(
        m.fs().change_jump,
        Some(vec![sp(&["q"]), sp(&["a"])]),
        "the step reports both changes (landed entry rotated last)"
    );
    assert_eq!(m.fs().stack.len(), 2, "editor stays open");
    assert_eq!(
        row_at(&mut m).path,
        sp(&["a", "b"]),
        "rule 1 keeps the cursor inside a's editor"
    );

    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        row_at(&mut m).path,
        sp(&["q"]),
        "the ring advances past the stay landing to q"
    );
    assert_eq!(m.fs().stack.len(), 1, "q lands in the root menu");
}

#[test]
fn fast_move_forward_no_match_and_wrap() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["f", "s"]);
    assert!(m.fast_move.is_none(), "the target key consumed the wait");
    assert_eq!(row_at(&mut m).path, sp(&["server"]));

    // no key starts with z: the cursor stays put
    press(&mut m, &["f", "z"]);
    assert!(m.fast_move.is_none());
    assert_eq!(cursor_cursor(&mut m), 2, "no match keeps the cursor");
    assert_eq!(m.notice, "no key starts with \"z\"");

    // the search wraps past the end
    press(&mut m, &["G"]);
    assert_eq!(cursor_cursor(&mut m), 5);
    press(&mut m, &["f", "n"]);
    assert_eq!(row_at(&mut m).path, sp(&["name"]), "wrap lands on name");
}

#[test]
fn fast_move_reverse_and_esc_cancel() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["F", "m"]);
    assert_eq!(row_at(&mut m).path, sp(&["mounts"]), "F searches upward");

    press(&mut m, &["f", "esc"]);
    assert!(m.fast_move.is_none());
    assert_eq!(m.notice, "fast move cancelled");
    assert_eq!(cursor_cursor(&mut m), 4, "cancel leaves the cursor alone");
    press(&mut m, &["j"]);
    assert_eq!(cursor_cursor(&mut m), 5, "movement resumes after cancel");
}

#[test]
fn fast_move_waits_consume_literal_keys() {
    let mut m = mk_app(test_doc(), test_opts());
    // q during the wait is a target character, not the esc alias
    press(&mut m, &["f", "q"]);
    assert!(m.fast_move.is_none(), "the wait consumed q");
    assert!(!m.exiting, "q must not exit the session here");
    assert_eq!(m.notice, "no key starts with \"q\"");
    assert_eq!(cursor_cursor(&mut m), 0);

    // the same for `.`: no replay while a fast move waits
    press(&mut m, &["f", "."]);
    assert!(m.fast_move.is_none());
    assert_eq!(m.notice, "no key starts with \".\"");
    assert_eq!(cursor_cursor(&mut m), 0, "nothing was played");
}

#[test]
fn undo_of_array_move_rings_both_moved_indices() {
    let arr_idx = |i: usize| vec![ks("items"), PathSeg::Idx(i)];
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
    let doc = dbuild(|db| {
        db.obj(vec![(
            "items",
            db.arr(vec![db.s("a"), db.s("b"), db.s("c")]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    press(&mut m, &["J"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("items"), PathSeg::Idx(0)]),
        "b",
        "J moved the first item down"
    );

    // undo restores the order; both moved indices form the ctrl+j/k ring
    // (rotated so the landed index sits last)
    press(&mut m, &["u"]);
    assert_eq!(row_at(&mut m).path, arr_idx(0), "lands on the first change");
    assert_eq!(
        m.fs().change_jump,
        Some(vec![arr_idx(1), arr_idx(0)]),
        "the unchanged tail must not enter the ring"
    );
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, arr_idx(1));
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, arr_idx(0), "wraps at the ring end");
}

#[test]
fn undo_of_an_addition_lands_on_the_surviving_container() {
    let doc = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("alpha")),
            ("server", db.obj(vec![("host", db.s("h"))])),
        ])
    });
    let mut m = items_app(doc);
    // add a key inside server through the real add-key flow
    set_cursor_path(&mut m, &sp(&["server"]));
    press(&mut m, &["enter"]);
    press(&mut m, &["a"]);
    match &mut m.overlay {
        Some(Overlay::Add(s)) => s.ti.set_value("extra"),
        _ => panic!("add-key input missing"),
    }
    press(&mut m, &["enter"]);
    assert!(has(&m.fs().doc, &sp(&["server", "extra"])));

    // undo removes the added key: the ring points at its own (dangling)
    // path; the schema-less menu no longer lists a row, so the cursor
    // lands on the surviving container
    press(&mut m, &["u"]);
    assert!(!has(&m.fs().doc, &sp(&["server", "extra"])));
    assert_eq!(
        m.fs().change_jump,
        Some(vec![sp(&["server", "extra"])]),
        "the removal reports the removed key's own path"
    );
    assert_eq!(
        row_at(&mut m).path,
        sp(&["server"]),
        "a removed key without a row lands on the container row"
    );
    // a single-entry ring wraps on itself
    press(&mut m, &["ctrl+j"]);
    assert_eq!(row_at(&mut m).path, sp(&["server"]));
}

#[test]
fn undo_without_diffs_does_not_arm_the_ring() {
    let doc = dbuild(|db| db.obj(vec![("a", db.s("1"))]));
    let mut m = app_with(crate::smodel::untyped(&doc), doc, test_opts());
    // a snapshot identical to the current document: undoing it reports no
    // paths, so no ring arms and the cursor stays put
    let pre = m.fs().doc.clone();
    m.fs_mut().history.push_undo(pre);
    set_cursor_path(&mut m, &sp(&["a"]));
    press(&mut m, &["u"]);
    assert_eq!(m.notice, "undone");
    assert!(
        m.fs().change_jump.is_none(),
        "an empty diff must not arm a ring"
    );
    assert_eq!(row_at(&mut m).path, sp(&["a"]), "cursor untouched");
}

// --- undo landing -------------------------------------------------------------

#[test]
fn undo_of_child_edit_lands_back_on_the_key_inside_its_menu() {
    // schema root {a: object {b: string}}; b is absent, an edit creates it,
    // and the undo removes it again. a's menu still lists b (absent members
    // render), so the cursor must return to b inside a's menu instead of
    // exiting to the root.
    let schema = schema_build(|b| {
        let bs = b.id(scalar("b", "B", "string"));
        let a = b.id(FormNode {
            key: "a".into(),
            title: "A".into(),
            kind: Kind::Object,
            children: vec![bs],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![a],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("a", db.obj(vec![]))]));
    let mut m = app_with(schema, doc, test_opts());

    set_cursor_path(&mut m, &sp(&["a"]));
    press(&mut m, &["enter"]); // inside a's menu, b absent
    set_cursor_path(&mut m, &sp(&["a", "b"]));
    press(&mut m, &["enter"]); // edit creates b
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("1"),
        _ => panic!("edit overlay missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(sget(&m.fs().doc, &sp(&["a", "b"])), "1");

    press(&mut m, &["u"]);
    assert!(!has(&m.fs().doc, &sp(&["a", "b"])), "undo removed b");
    assert_eq!(m.fs().stack.len(), 2, "still inside a's menu");
    let r = row_at(&mut m);
    assert_eq!(
        r.path,
        sp(&["a", "b"]),
        "undo of the child edit lands back on b inside a's menu"
    );
}

#[test]
fn undo_of_first_child_edit_stays_in_the_variant_editor() {
    // oneOf [null, {b}]: picking the {b} variant pushes the editor without
    // touching the document, so the first child edit is one snapshot
    // (a: null -> {"b":"1"}). Undoing it restores a=null; the editor menu
    // the user sits in must stay open with the cursor on b (now absent).
    let schema = schema_build(|b| {
        let v1 = b.id(scalar("a", "null", "null"));
        let bs = b.id(scalar("b", "B", "string"));
        let v2 = b.id(FormNode {
            key: "a".into(),
            title: "obj".into(),
            kind: Kind::Object,
            children: vec![bs],
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "a".into(),
            title: "A".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("a", db.push(Value::Null))]));
    let mut m = app_with(schema, doc, test_opts());

    set_cursor_path(&mut m, &sp(&["a"]));
    press(&mut m, &["enter"]); // variant picker for a
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    let obj = m
        .menu_rows()
        .iter()
        .position(|r| {
            r.variant
                .map(|v| m.fs().schema.nodes[v as usize].kind == Kind::Object)
                .unwrap_or(false)
        })
        .expect("object variant row");
    m.top_mut().unwrap().cursor = obj;
    press(&mut m, &["enter"]); // pick {b}: editor opens, doc unchanged
    assert_eq!(m.fs().stack.len(), 2, "variant editor pushed");
    assert!(matches!(
        m.fs().doc.value(m.fs().doc.get(&sp(&["a"])).unwrap()),
        Value::Null
    ));
    set_cursor_path(&mut m, &sp(&["a", "b"]));
    press(&mut m, &["enter"]); // edit b (absent -> created)
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("1"),
        _ => panic!("edit overlay missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(sget(&m.fs().doc, &sp(&["a", "b"])), "1");

    press(&mut m, &["u"]);
    assert!(matches!(
        m.fs().doc.value(m.fs().doc.get(&sp(&["a"])).unwrap()),
        Value::Null
    ));
    assert_eq!(m.fs().stack.len(), 2, "variant editor survives the undo");
    let r = row_at(&mut m);
    assert_eq!(
        r.path,
        sp(&["a", "b"]),
        "undo lands the cursor back on b inside the editor"
    );
}

#[test]
fn repeated_move_matches_individual_moves_and_wraps() {
    let mut one = mk_app(test_doc(), test_opts());
    let mut many = mk_app(test_doc(), test_opts());
    for _ in 0..9 {
        one.update(Msg::Char('j', false));
    }
    many.update_repeat(Msg::Char('j', false), 9);
    assert_eq!(one.top().unwrap().cursor, many.top().unwrap().cursor);
    // a run longer than the row count wraps exactly like separate presses
    for _ in 0..20 {
        one.update(Msg::Down);
    }
    many.update_repeat(Msg::Down, 20);
    assert_eq!(one.top().unwrap().cursor, many.top().unwrap().cursor);
}

#[test]
fn repeated_move_extends_visual_selection_once() {
    let mut one = mk_app(test_doc(), test_opts());
    let mut many = mk_app(test_doc(), test_opts());
    for app in [&mut one, &mut many] {
        app.update(Msg::Char('v', false));
    }
    for _ in 0..3 {
        one.update(Msg::Char('j', false));
    }
    many.update_repeat(Msg::Char('j', false), 3);
    assert_eq!(
        one.top().unwrap().cursor,
        many.top().unwrap().cursor,
        "visual range ends on the same row"
    );
    assert_eq!(one.top().unwrap().sel, many.top().unwrap().sel);
}

#[test]
fn repeated_move_still_records_every_macro_key() {
    let mut app = mk_app(test_doc(), test_opts());
    app.update(Msg::Char('@', false));
    assert!(app.recording);
    app.update_repeat(Msg::Char('j', false), 3);
    assert_eq!(app.rec_buf.len(), 3);
    assert_eq!(app.top().unwrap().cursor, 3);
}

#[test]
fn repeated_typing_replays_every_character() {
    let mut app = mk_app(test_doc(), test_opts());
    press(&mut app, &["enter"]); // edit the first field
    app.update(Msg::Ctrl('u'));
    app.update_repeat(Msg::Char('a', false), 3);
    press(&mut app, &["enter"]);
    assert_eq!(str_at(&app.fs().doc, &["name"]), "aaa");
}

/// macro_keys renders a recorded macro as its key names.
fn macro_keys(m: &App) -> Vec<String> {
    m.macro_buf.iter().map(|k| k.key_str()).collect()
}

#[test]
fn macro_record_stop_and_replay_moves_cursor() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["@"]);
    assert!(m.recording, "@ arms the recorder");
    assert_eq!(
        m.notice, "recording — @ stops, . is ignored while recording",
        "notice = {}",
        m.notice
    );
    press(&mut m, &["j", "j"]);
    assert_eq!(m.rec_buf.len(), 2, "each key is captured");
    assert_eq!(cursor_cursor(&mut m), 2, "recorded keys still execute");

    // `.` while recording is swallowed: neither played nor captured
    press(&mut m, &["."]);
    assert!(m.recording, "recording continues");
    assert_eq!(m.rec_buf.len(), 2, "the dot is not recorded");
    assert_eq!(cursor_cursor(&mut m), 2, "the dot did not replay");

    press(&mut m, &["@"]);
    assert!(!m.recording, "second @ stops recording");
    assert_eq!(macro_keys(&m), vec!["j", "j"]);
    assert_eq!(m.notice, "macro recorded: 2 keys — . plays it");

    // replay walks the same keys from the current position
    press(&mut m, &["."]);
    assert_eq!(cursor_cursor(&mut m), 4, "replayed j j from cursor 2");
    assert_eq!(m.notice, "played 2 keys");
    assert!(!m.recording, "replay never re-arms the recorder");
}

#[test]
fn macro_replays_pending_gg_as_one_jump() {
    // g has pending semantics; a macro stores the raw keys and replay runs
    // them one by one, so the buffered gg lands on row 0.
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["down", "down", "@", "g", "g", "@"]);
    assert!(!m.recording);
    assert_eq!(macro_keys(&m), vec!["g", "g"]);
    assert_eq!(cursor_cursor(&mut m), 0, "gg executed while recording");
    press(&mut m, &["down", "down"]);
    assert_eq!(cursor_cursor(&mut m), 2);
    press(&mut m, &["."]);
    assert_eq!(cursor_cursor(&mut m), 0, "replayed gg jumps home");
    assert_eq!(m.notice, "played 2 keys");
}

#[test]
fn macro_empty_stop_cancels_and_keeps_previous_macro() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["@", "j", "@"]);
    assert_eq!(macro_keys(&m), vec!["j"]);

    // stopping with an empty buffer cancels: the old macro survives
    press(&mut m, &["@", "@"]);
    assert!(!m.recording);
    assert_eq!(m.notice, "recording cancelled (no keys captured)");
    assert_eq!(macro_keys(&m), vec!["j"], "previous macro kept");
    press(&mut m, &["."]);
    assert_eq!(cursor_cursor(&mut m), 2, "replay still uses the old macro");
}

#[test]
fn macro_dot_without_macro_hints() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["."]);
    assert!(!m.recording);
    assert_eq!(m.notice, "no macro recorded (@ records one)");
    assert_eq!(cursor_cursor(&mut m), 0, "nothing was played");
}

#[test]
fn macro_keys_stay_literal_inside_text_input() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["enter"]); // name row: in-place editor
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["q", "@", "."]);
    assert!(!m.recording, "input keys never arm the recorder");
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "webappq@.", "all three are literal"),
        _ => panic!("editor should stay open"),
    }
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());
    assert_eq!(str_at(&m.fs().doc, &["name"]), "webapp", "esc cancels");
}

#[test]
fn macro_limit_stops_recording_with_notice() {
    let mut m = mk_app(test_doc(), test_opts());
    let mut keys = vec!["@"];
    for _ in 0..crate::ui::menu::MAX_MACRO_KEYS {
        keys.push("j");
    }
    press(&mut m, &keys);
    assert!(!m.recording, "the cap stops recording by itself");
    assert_eq!(m.macro_buf.len(), crate::ui::menu::MAX_MACRO_KEYS);
    assert!(
        m.macro_buf.iter().all(|k| k.key_str() == "j"),
        "everything up to the cap is kept"
    );
    assert!(
        m.notice.contains("macro limit reached"),
        "notice = {}",
        m.notice
    );
}
