//! Inline value completion and floating candidate overlays.

use super::*;
use super::{
    app_with, dbuild, items_app, ks, plain, press, scalar, schema_build, set_cursor, sget,
    shell_doc, shell_schema, sp, test_opts, test_schema,
};

use super::*;

#[test]
fn completion_scrolling() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("v1"))]),
                db.obj(vec![("src", db.s("v2"))]),
                db.obj(vec![("src", db.s("v3"))]),
                db.obj(vec![("src", db.s("v4"))]),
                db.obj(vec![("src", db.s("v5"))]),
            ]),
        )])
    });
    let mut m = app_with(test_schema(), doc, test_opts());
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["G", "A"]);
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.cands.len(), 5),
        _ => panic!("candidates missing"),
    }
    press(&mut m, &["tab", "tab", "tab", "tab"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "v4"),
        _ => panic!(),
    }
    press(&mut m, &["tab"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "v5"),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(5), ks("src")]),
        "v5"
    );
}

#[test]
fn completion_dedup() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/data"))]),
                db.obj(vec![("src", db.s("/data"))]),
                db.obj(vec![("src", db.s("/data2"))]),
                db.obj(vec![("src", db.s(""))]),
            ]),
        )])
    });
    let mut m = app_with(test_schema(), doc, test_opts());
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["G", "A"]);
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.cands, vec!["/data".to_string(), "/data2".to_string()])
        }
        _ => panic!(),
    }
    press(&mut m, &["esc", "esc", "esc"]);

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
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one],
            ..FormNode::default()
        })
    });
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "http_clients",
            db.arr(vec![
                db.obj(vec![("tag", db.s("c1"))]),
                db.obj(vec![("tag", db.s("c1"))]),
                db.obj(vec![("tag", db.s("c2"))]),
            ]),
        )])
    });
    let mut m2 = app_with(schema, doc2, test_opts());
    press(&mut m2, &["down", "enter", "enter"]);
    match &m2.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.cands, vec!["c1".to_string(), "c2".to_string()]),
        _ => panic!("tag candidates missing"),
    }
}

#[test]
fn edit_completion() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/data")), ("dst", db.s("/mnt"))]),
                db.obj(vec![("src", db.s("/data2"))]),
            ]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["G", "a"]);
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(
                e.cands,
                vec![
                    "/data".to_string(),
                    "/data2".to_string(),
                    "/mnt".to_string()
                ],
                "candidate order"
            );
        }
        _ => panic!(),
    }
    press(&mut m, &["/", "d", "a", "t", "a", "2"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sugs, vec!["/data2".to_string()]),
        _ => panic!(),
    }
    press(&mut m, &["ctrl+u"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sugs.len(), 3),
        _ => panic!(),
    }
    press(&mut m, &["tab"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "/data"),
        _ => panic!(),
    }
    press(&mut m, &["tab"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "/data2"),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(2), ks("src")]),
        "/data2"
    );

    // no prefix match: no suggestions, plain input still commits
    press(&mut m, &["down", "enter"]);
    press(&mut m, &["z", "z"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert!(e.sugs.is_empty()),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(2), ks("dst")]),
        "zz"
    );
}

#[test]
fn edit_input_arrows_move_completion_selection() {
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.suggestions = vec!["alpha".into(), "alpine".into(), "alps".into()];
        let name = b.id(name);
        let other1 = b.id(scalar("other1", "Other 1", "string"));
        let other2 = b.id(scalar("other2", "Other 2", "string"));
        let other3 = b.id(scalar("other3", "Other 3", "string"));
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name, other1, other2, other3],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("")),
            ("other1", db.s("x")),
            ("other2", db.s("y")),
            ("other3", db.s("z")),
        ])
    });
    let mut app = app_with(schema, doc, test_opts());
    press(&mut app, &["enter"]);

    let edit = |app: &App| match &app.overlay {
        Some(Overlay::Edit(e)) => (e.ti.value().to_string(), e.sug_idx, e.sugs.len()),
        _ => panic!("edit overlay is not open"),
    };
    assert_eq!(edit(&app), (String::new(), -1, 3));

    // Entering the input starts completion without a tab press.
    let view = app.view();
    assert!(
        view.contains("\x1b[2;48;5;236malpha"),
        "candidates must appear as soon as the input opens: {view}"
    );
    assert!(
        !view.contains("\x1b[7malpha"),
        "the popup must not preselect a candidate: {view}"
    );

    press(&mut app, &["tab"]);
    assert_eq!(edit(&app), ("alpha".to_string(), 0, 3));
    let view = app.view();
    assert!(
        view.contains("\x1b[7malpha"),
        "selected candidate must use reverse video: {view}"
    );
    assert!(
        view.contains("\x1b[2;48;5;236malpine"),
        "other candidates must be rendered faintly: {view}"
    );
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[7malpha"))
        .expect("selected candidate line");
    let start = line.find("\x1b[7m").expect("candidate background");
    assert!(
        line[..start].contains("Other 1"),
        "the overlay must leave the row content left of its rectangle visible: {view}"
    );
    let body = &line[start + "\x1b[7m".len()..];
    let end = body.find("\x1b[0m").expect("candidate reset");
    assert_eq!(
        display_width(&body[..end]),
        SUG_MIN_WIDTH,
        "short candidate rows use the minimum overlay width"
    );
    let rows = app.menu_rows();
    let tw = app.menu_title_col_width(&rows);
    let input_left = 2 + display_width(&app.title_col("Name", tw));
    assert_eq!(
        display_width(&line[..start]),
        input_left,
        "candidate overlay must start where the edit input text starts"
    );
    press(&mut app, &["down"]);
    assert_eq!(edit(&app), ("alpine".to_string(), 1, 3));
    press(&mut app, &["up"]);
    assert_eq!(edit(&app), ("alpha".to_string(), 0, 3));
    // Tab keeps cycling the same list after arrow movement.
    press(&mut app, &["tab"]);
    assert_eq!(edit(&app), ("alpine".to_string(), 1, 3));
}

#[test]
fn edit_input_ctrl_j_k_move_completion_selection() {
    // ctrl+j/ctrl+k are aliases of ctrl+n/ctrl+p while the candidate
    // window is up.
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.suggestions = vec!["alpha".into(), "alpine".into(), "alps".into()];
        let name = b.id(name);
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("name", db.s(""))]));
    let mut app = app_with(schema, doc, test_opts());
    press(&mut app, &["enter"]);
    let edit = |app: &App| match &app.overlay {
        Some(Overlay::Edit(e)) => (e.ti.value().to_string(), e.sug_idx),
        _ => panic!("edit overlay is not open"),
    };
    assert_eq!(edit(&app), (String::new(), -1));
    press(&mut app, &["ctrl+j"]);
    assert_eq!(edit(&app), ("alpha".to_string(), 0));
    press(&mut app, &["ctrl+j"]);
    assert_eq!(edit(&app), ("alpine".to_string(), 1));
    press(&mut app, &["ctrl+k"]);
    assert_eq!(edit(&app), ("alpha".to_string(), 0));
}

#[test]
fn edit_completion_window_anchors_at_the_caret_for_stripped_rows() {
    // Candidates sharing a long prefix with the typed value render as
    // `…tail` rows, which anchor at the caret so the visible tails stay
    // next to the input position.
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.suggestions = vec![
            "alpha-beta-gamma-1".into(),
            "alpha-beta-gamma-2".into(),
            "alpha-beta-gamma-3".into(),
        ];
        let name = b.id(name);
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("name", db.s("alpha-beta-gamma-"))]));
    let mut app = app_with(schema, doc, test_opts());
    press(&mut app, &["enter"]); // caret at the end of the long value
    let rows = app.menu_rows();
    let tw = app.menu_title_col_width(&rows);
    let input_left = 2 + display_width(&app.title_col("Name", tw));
    let caret_left = input_left + display_width("alpha-beta-gamma-");
    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[2;48;5;236m1"))
        .expect("candidate line");
    let start = line.find("\x1b[2;48;5;236m").expect("candidate background");
    assert_eq!(
        display_width(&line[..start]),
        caret_left,
        "stripped rows anchor at the caret column: {view}"
    );

    // Cycling to the applied candidate keeps the stripped-row anchor: the
    // list (and its query) is unchanged until the next keystroke.
    press(&mut app, &["tab"]);
    match &app.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "alpha-beta-gamma-1"),
        _ => panic!("edit overlay is not open"),
    }
    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[7m1"))
        .expect("selected candidate line");
    let start = line.find("\x1b[7m").expect("selected candidate background");
    assert_eq!(
        display_width(&line[..start]),
        caret_left,
        "cycling keeps the stripped-row anchor: {view}"
    );
}

#[test]
fn edit_completion_window_stays_at_the_input_start() {
    // The edited value is the prefix being matched, so the candidate window
    // aligns with the input text start and must not follow the caret while
    // the user types.
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.suggestions = vec!["alpha".into(), "alpine".into(), "alps".into()];
        let name = b.id(name);
        let other = b.id(scalar("other", "Other", "string"));
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name, other],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("name", db.s("a")), ("other", db.s("x"))]));
    let mut app = app_with(schema, doc, test_opts());
    press(&mut app, &["enter"]); // edit Name with the caret after "a"
    match &app.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.ti.value(), "a");
            assert_eq!(e.sugs.len(), 3);
        }
        _ => panic!("edit overlay is not open"),
    }

    let rows = app.menu_rows();
    let tw = app.menu_title_col_width(&rows);
    let expected = 2 + display_width(&app.title_col("Name", tw));

    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[2;48;5;236") && plain(l).contains("alpha"))
        .expect("candidate line");
    let start = line.find("\x1b[2;48;5;236").expect("candidate background");
    assert_eq!(
        display_width(&line[..start]),
        expected,
        "the candidate window starts at the input text: {view}"
    );

    // Typing another character filters the list but must not move the
    // window to the new caret column.
    press(&mut app, &["l"]);
    match &app.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "al"),
        _ => panic!("edit overlay is not open"),
    }
    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[2;48;5;236") && plain(l).contains("alpha"))
        .expect("candidate line");
    let start = line.find("\x1b[2;48;5;236").expect("candidate background");
    assert_eq!(
        display_width(&line[..start]),
        expected,
        "typing must not move the window: {view}"
    );

    // Applying a candidate keeps the window in place too.
    press(&mut app, &["tab"]);
    match &app.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "alpha"),
        _ => panic!("edit overlay is not open"),
    }
    let view = app.view();
    // the window's first row is the selected candidate (sug_idx 0); the
    // faint rows below mark where the overlay rectangle sits
    let win = view
        .lines()
        .position(|l| l.contains("\x1b[2;48;5;236"))
        .expect("candidate window");
    let line = view.lines().nth(win - 1).expect("selected candidate line");
    let start = line.find("\x1b[7").expect("selected candidate background");
    assert_eq!(
        display_width(&line[..start]),
        expected,
        "applying a candidate must not move the window: {view}"
    );
}

#[test]
fn edit_input_window_keeps_the_caret_visible_on_a_narrow_terminal() {
    // A value wider than the terminal used to clip at the right edge: the
    // caret and everything typed after it were invisible. The input now
    // scrolls horizontally like the command line, so the caret and its
    // surroundings stay on screen.
    let schema = schema_build(|b| {
        let name = b.id(scalar("name", "Name", "string"));
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        })
    });
    let long = "abcdefghij".repeat(6); // 60 chars
    let doc = dbuild(|db| db.obj(vec![("name", db.s(&long))]));
    let mut app = app_with(schema, doc, test_opts());
    app.width = 30;
    app.height = 24;
    press(&mut app, &["enter"]); // inline edit, caret at the value end
    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[7m"))
        .expect("the edited row must show a caret");
    assert!(
        display_width(&line) <= app.width - 1,
        "the edited row must not wrap: {line:?}"
    );
    assert!(
        line.contains("hij"),
        "the text around the caret must stay visible: {line:?}"
    );
    // Typing at the end scrolls the window with the caret.
    press(&mut app, &["X", "Y", "Z"]);
    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[7m"))
        .expect("the edited row must show a caret");
    assert!(
        line.contains("XYZ"),
        "freshly typed characters must be visible: {line:?}"
    );
    // The value itself is untouched by the viewport.
    match &app.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), format!("{long}XYZ")),
        _ => panic!("edit overlay is not open"),
    }
}

#[test]
fn edit_completion_window_stays_inside_the_terminal_at_the_right_edge() {
    // A long value puts the caret near the terminal's right edge; the
    // candidate window anchors at the input text start and must still stay
    // fully inside the terminal, with every candidate visible.
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.suggestions = vec![
            "alpha-beta-gamma-1".into(),
            "alpha-beta-gamma-2".into(),
            "alpha-beta-gamma-3".into(),
        ];
        let name = b.id(name);
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("name", db.s("alpha-beta-gamma-"))]));
    let mut app = app_with(schema, doc, test_opts());
    app.width = 36;
    app.height = 24;
    press(&mut app, &["enter"]); // caret at the end of the long value
    let rows = app.menu_rows();
    let tw = app.menu_title_col_width(&rows);
    let input_left = 2 + display_width(&app.title_col("Name", tw));
    let caret = input_left + display_width("alpha-beta-gamma-");
    assert!(
        caret > app.width - 1,
        "fixture: the caret reaches past the terminal edge"
    );
    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[2;48;5;236m2"))
        .expect("candidate line");
    let start = line.find("\x1b[2;48;5;236m").expect("candidate background");
    let body = &line[start + "\x1b[2;48;5;236m".len()..];
    let rect = display_width(&body[..body.find("\x1b[0m").expect("candidate reset")]);
    let left = display_width(&line[..start]);
    assert_eq!(rect, SUG_MIN_WIDTH, "the rectangle keeps its full width");
    // input_left + rect would run past the edge, so the window slides left
    // to the largest column that still fits it.
    let expected_left = input_left.min(app.width - 1 - rect);
    assert_eq!(
        left, expected_left,
        "the window anchors at the input text start and slides left only to fit"
    );
    assert!(
        left + rect <= app.width - 1,
        "the candidate window must end inside the terminal: left {left} rect {rect}"
    );
    assert!(
        body.contains("2"),
        "candidates must not be cut at the right edge: {body:?}"
    );
    // Selecting a candidate keeps the window in place.
    press(&mut app, &["tab"]);
    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[7m1"))
        .expect("selected candidate line");
    let start = line.find("\x1b[7m").expect("selected candidate background");
    assert!(
        display_width(&line[..start]) + rect <= app.width - 1,
        "the selected window stays inside the terminal: {view}"
    );
}

#[test]
fn write_path_arrows_move_completion_selection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
    std::fs::write(dir.path().join("alpine"), "").unwrap();
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    app.fs_mut().opts.path = format!("{}/al", dir.path().display());
    press(&mut app, &["W"]);

    let write = |app: &App| match &app.overlay {
        Some(Overlay::Write(w)) => (w.ti.value().to_string(), w.sug_idx, w.sugs.len()),
        _ => panic!("write overlay is not open"),
    };
    let typed = write(&app).0;
    let view = app.view();
    assert!(
        view.contains("\x1b[2;48;5;236mpha.txt"),
        "write suggestions must appear on open: {view}"
    );
    press(&mut app, &["tab"]);
    assert_eq!(write(&app).2, 2);
    let view = app.view();
    assert!(
        view.contains("\x1b[7mpha.txt"),
        "write-path selection must use reverse video: {view}"
    );
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[7mpha.txt"))
        .expect("selected write candidate line");
    let start = line.find("\x1b[7m").expect("candidate background");
    let rows = app.menu_rows();
    let tw = app.menu_title_col_width(&rows);
    let input_left = display_width(&app.title_col("Write to", tw));
    // The typed path shares a long prefix with the candidates: stripped
    // `…tail` rows anchor at the caret column recorded when the list was
    // rebuilt (typing `{dir}/al`), and cycling keeps that anchor.
    let anchor = input_left + display_width(&typed);
    assert_eq!(
        display_width(&line[..start]),
        anchor,
        "stripped write candidates anchor at the caret column"
    );
    press(&mut app, &["down"]);
    assert_eq!(
        write(&app).2,
        2,
        "arrows must not rebuild the candidate list"
    );
    press(&mut app, &["up"]);
    assert_eq!(write(&app).2, 2);
}

#[test]
fn write_path_ctrl_j_k_move_completion_selection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
    std::fs::write(dir.path().join("alpine"), "").unwrap();
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    app.fs_mut().opts.path = format!("{}/al", dir.path().display());
    press(&mut app, &["W", "tab"]);
    let write = |app: &App| match &app.overlay {
        Some(Overlay::Write(w)) => (w.ti.value().to_string(), w.sug_idx, w.sugs.len()),
        _ => panic!("write overlay is not open"),
    };
    let (v0, i0, n) = write(&app);
    assert_eq!(n, 2);
    press(&mut app, &["ctrl+j"]);
    let (v1, i1, _) = write(&app);
    assert_eq!(i1, i0 + 1, "ctrl+j moves to the next candidate");
    assert_ne!(v1, v0);
    press(&mut app, &["ctrl+k"]);
    assert_eq!(write(&app), (v0, i0, n), "ctrl+k moves back");
}

#[test]
fn edit_candidate_overlay_uses_free_rows_below_the_last_row() {
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.suggestions = vec!["alpha".into(), "alpine".into(), "alps".into()];
        let name = b.id(name);
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("name", db.s(""))]));
    let mut app = app_with(schema, doc, test_opts());
    press(&mut app, &["enter"]);
    press(&mut app, &["tab"]);
    let view = app.view();
    assert!(
        view.contains("\x1b[7malpha"),
        "candidate overlay must use free rows below the last menu row: {view}"
    );
    assert!(
        view.contains("\x1b[2;48;5;236malpine") && view.contains("\x1b[2;48;5;236malps"),
        "all candidates must be visible: {view}"
    );
    assert!(
        !view.contains("tab/↑↓:"),
        "candidates are an overlay, not an inserted hint line: {view}"
    );
}

#[test]
fn edit_completion_moves_above_a_row_at_the_bottom() {
    let schema = schema_build(|b| {
        let first = b.id(scalar("first", "First", "string"));
        let second = b.id(scalar("second", "Second", "string"));
        let third = b.id(scalar("third", "Third", "string"));
        let mut last = scalar("last", "Last", "string");
        last.suggestions = vec!["alpha".into(), "alpine".into(), "alps".into()];
        let last = b.id(last);
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![first, second, third, last],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| {
        db.obj(vec![
            ("first", db.s("x")),
            ("second", db.s("y")),
            ("third", db.s("z")),
            ("last", db.s("")),
        ])
    });
    let mut app = app_with(schema, doc, test_opts());
    // title + breadcrumb + four rows + help: the edited row sits at the
    // bottom of the terminal with no space below it.
    app.height = 7;
    set_cursor(&mut app, |r| r.path == sp(&["last"]));
    press(&mut app, &["enter"]);
    assert!(matches!(app.overlay, Some(Overlay::Edit(_))));

    let view = app.view();
    let rows: Vec<&str> = view.lines().collect();
    assert_eq!(rows.len(), app.height, "view: {view}");
    let input = rows
        .iter()
        .position(|l| l.contains("Last"))
        .expect("edit input row");
    let candidate = rows
        .iter()
        .position(|l| l.contains("alpha"))
        .expect("candidate row");
    assert!(
        candidate < input,
        "candidates must move above the input at the bottom: {view}"
    );
    assert!(
        view.contains("alpine") && view.contains("alps"),
        "all candidates stay visible: {view}"
    );
}

#[test]
fn candidate_overlay_width_tracks_content_with_a_cap_and_ellipsis() {
    let short = vec!["alpha".to_string(), "beta".to_string()];
    let lines = App::suggestion_overlay_lines(&short, 0, "", 2, 0, 80, &[]);
    assert_eq!(display_width(&lines[0]), SUG_MIN_WIDTH);
    assert_eq!(
        display_width(&lines[1]),
        SUG_MIN_WIDTH,
        "short rows share the minimum width"
    );

    let long = vec![format!("{}TAIL", "x".repeat(100))];
    let lines = App::suggestion_overlay_lines(&long, 0, "", 1, 0, 80, &[]);
    assert_eq!(display_width(&lines[0]), 80, "fills the available width");
    assert!(
        lines[0].contains('…'),
        "the cut start is marked with an ellipsis"
    );
    assert!(
        lines[0].contains("TAIL"),
        "the differing tail must never be dropped"
    );
    assert!(
        !lines[0].contains(&"x".repeat(80)),
        "the full long candidate must be truncated"
    );

    let lines = App::suggestion_overlay_lines(&long, 0, "", 1, 0, 20, &[]);
    assert_eq!(display_width(&lines[0]), 20, "terminal width still caps it");
    assert!(lines[0].contains("TAIL"), "tail kept at narrow widths too");
}

// --- fuzzy completion matching ----------------------------------------------

#[test]
fn fuzzy_match_scores_and_indices() {
    // subsequence: "outp" matches inside "out-proxy" (the '-' is skipped),
    // indices mark the hit
    let (score, hits) = fuzzy_match("outp", "out-proxy").expect("outp matches");
    assert_eq!(score, 3);
    assert_eq!(hits, vec![0, 1, 2, 4]);
    // "pro" and "op" are subsequences too
    assert!(fuzzy_match("pro", "out-proxy").is_some());
    let (_, op_hits) = fuzzy_match("op", "out-proxy").expect("op matches");
    assert_eq!(op_hits, vec![0, 4]); // o(ut-) p(roxy), greedy earliest
    // tighter matches rank better
    assert!(fuzzy_match("prod", "prod").unwrap().0 < fuzzy_match("prod", "prod-uat").unwrap().0);
    assert!(fuzzy_match("rod", "prod").unwrap().0 < fuzzy_match("rd", "prod").unwrap().0);
    assert!(fuzzy_match("xyz", "out-proxy").is_none());
    assert!(fuzzy_match("", "out-proxy").unwrap().1.is_empty());
}

#[test]
fn fuzzy_filter_ranks_prefix_before_subsequence() {
    let cands = vec![
        "out-proxy".to_string(),
        "prod".to_string(),
        "proxy-url".to_string(),
    ];
    let matched = filter_completions(&cands, "pro");
    assert_eq!(
        matched.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(),
        vec!["prod", "proxy-url", "out-proxy"],
        "prefix matches first, then longer subsequences"
    );
    let (_, hits) = &matched[2];
    assert_eq!(hits.clone(), vec![4, 5, 6], "pro at the tail of out-proxy");
}

#[test]
fn edit_suggestions_fuzzy_match_and_highlight() {
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.suggestions = vec!["out-proxy".into(), "upstream".into()];
        let name = b.id(name);
        let other = b.id(scalar("other", "Other", "string"));
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name, other],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("name", db.s("")), ("other", db.s("x"))]));
    let mut app = app_with(schema, doc, test_opts());
    press(&mut app, &["enter"]);
    press(&mut app, &["o", "u", "t", "p"]);
    match &app.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.sugs, vec!["out-proxy"], "fuzzy subsequence matches");
            assert_eq!(
                e.sug_hl,
                vec![vec![0, 1, 2, 4]],
                "matched chars highlighted"
            );
        }
        _ => panic!("edit overlay is not open"),
    }
    // the dropdown renders the matched runs in bold red on the row background
    let view = app.view();
    assert!(
        view.contains("\x1b[2;48;5;236;1;31mout\x1b[0m"),
        "matched text must be red: {view:?}"
    );
    assert!(plain(&view).contains("out-proxy"), "candidate still listed");
}

#[test]
fn candidate_overlay_hides_a_long_shared_prefix() {
    let sugs = vec![
        "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/ad".to_string(),
        "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/ai".to_string(),
    ];
    let lines = App::suggestion_overlay_lines(
        &sugs,
        -1,
        "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/",
        2,
        0,
        120,
        &[],
    );
    assert!(
        !lines[0].contains("https://raw.githubusercontent.com"),
        "the shared URL prefix must be hidden: {:?}",
        lines[0]
    );
    assert!(
        lines[0].contains("ad") && lines[1].contains("ai"),
        "the differing tails must stay visible: {:?}",
        lines
    );
    // Short tails fit the width: no ellipsis marker is added.
    assert!(!lines[0].contains('…'), "{:?}", lines[0]);

    // Below the length threshold the candidates keep their full text: a
    // short typed prefix stays visible in every row.
    let lines = App::suggestion_overlay_lines(
        &["wa".to_string(), "wq".to_string()],
        -1,
        "w",
        2,
        0,
        80,
        &[],
    );
    assert!(
        lines[0].contains("wa") && lines[1].contains("wq"),
        "short typed prefixes must not be stripped: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains('…')),
        "fitting candidates must not carry an ellipsis: {lines:?}"
    );
}
