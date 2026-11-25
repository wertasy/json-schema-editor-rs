//! Vi-style `:` command grammar: builtins, shell escapes, completion,
//! persistent history.

use super::*;
use super::{
    app_with, counting_opts, dbuild, mk_app, num_at, press, scalar, schema_build, shell_doc,
    shell_schema, sp, str_at, test_doc, test_opts, two_file_app,
};

use super::*;
use parking_lot::Mutex;

#[test]
fn esc_clears_command_buffer() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["g"]);
    assert_eq!(m.cmd_buf.len(), 1);
    press(&mut m, &["esc"]);
    assert!(m.cmd_buf.is_empty());
    assert!(!m.exiting && !m.aborted && m.overlay.is_none() && m.fs().stack.len() == 1);

    // q aliases esc outside inputs
    press(&mut m, &["g", "q"]);
    assert!(m.cmd_buf.is_empty() && !m.exiting && !m.aborted);

    // without a pending combo esc keeps its meaning
    press(&mut m, &["esc"]);
    assert!(m.exiting);
}

fn command_value(app: &App) -> String {
    match &app.overlay {
        Some(Overlay::Command(s)) => s.ti.value().to_string(),
        _ => panic!("command prompt is not open"),
    }
}

fn command_cursor(app: &App) -> usize {
    match &app.overlay {
        Some(Overlay::Command(s)) => s.ti.cursor_byte(),
        _ => panic!("command prompt is not open"),
    }
}

#[test]
fn shell_inserts_stdout_into_string_and_string_array_values() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    assert!(matches!(app.overlay, Some(Overlay::Command(_))));
    let view = app.view();
    let lines: Vec<&str> = view.lines().collect();
    assert!(
        lines[lines.len() - 2].starts_with(':'),
        "prompt row must sit directly above the help row: {:?}",
        &lines[lines.len() - 2..]
    );
    assert!(lines[lines.len() - 1].contains(": command"));

    press(&mut app, &["r !echo shell"]);
    press(&mut app, &["enter"]);
    assert!(app.overlay.is_none());
    assert_eq!(str_at(&app.doc(), &["name"]), "shell");

    // string array row: append the output as a new item
    press(&mut app, &["j"]);
    press(&mut app, &[":"]);
    press(&mut app, &["r !echo next"]);
    press(&mut app, &["enter"]);
    let tags = app.doc().get(&sp(&["tags"])).unwrap();
    let got: Vec<String> = match app.doc().value(tags) {
        Value::Array(items) => items
            .iter()
            .map(|i| match app.doc().value(*i) {
                Value::Str(s) => s.clone(),
                _ => "<not a string>".into(),
            })
            .collect(),
        _ => panic!("tags is not an array"),
    };
    assert_eq!(got, vec!["a".to_string(), "next".to_string()]);
}

#[test]
fn command_history_recalls_and_bang_bang_reruns_the_last_shell_command() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["r !echo one"]);
    press(&mut app, &["enter"]);
    assert_eq!(str_at(&app.doc(), &["name"]), "one");

    press(&mut app, &[":"]);
    press(&mut app, &["up"]);
    assert_eq!(command_value(&app), "r !echo one");
    press(&mut app, &["down"]);
    assert_eq!(command_value(&app), "");
    press(&mut app, &["esc"]);

    press(&mut app, &[":"]);
    press(&mut app, &["r !echo two"]);
    press(&mut app, &["enter"]);
    assert_eq!(str_at(&app.doc(), &["name"]), "two");
    assert_eq!(app.cmd_history, vec!["r !echo one", "r !echo two"]);

    // `!!` reruns the previous shell escape (including its r/a/w mode).
    let name = app.fs_mut().doc.new_str("changed");
    app.fs_mut()
        .doc
        .set(&sp(&["name"]), name)
        .expect("rewrite name");
    press(&mut app, &[":"]);
    press(&mut app, &["!!"]);
    assert!(matches!(app.overlay, Some(Overlay::Command(_))));
    press(&mut app, &["enter"]);
    assert!(app.overlay.is_none());
    assert_eq!(str_at(&app.doc(), &["name"]), "two");
    assert_eq!(
        app.cmd_history,
        vec!["r !echo one", "r !echo two"],
        "!! must not add another history entry"
    );
}

#[test]
fn command_history_persists_to_the_options_file_and_reloads() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cmd-history");
    let mut opts = test_opts();
    opts.cmd_history_file = Some(file.clone());
    let mut app = app_with(shell_schema(), shell_doc(), opts);
    press(&mut app, &[":"]);
    press(&mut app, &["r !echo one"]);
    press(&mut app, &["enter"]);
    assert_eq!(str_at(&app.doc(), &["name"]), "one");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "r !echo one\n");

    // `!!` reruns without adding an entry or rewriting the file.
    press(&mut app, &[":"]);
    press(&mut app, &["!!"]);
    press(&mut app, &["enter"]);
    assert_eq!(str_at(&app.doc(), &["name"]), "one");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "r !echo one\n");

    // A fresh session seeds its history from the file and recalls it.
    let mut opts = test_opts();
    opts.cmd_history_file = Some(file);
    let mut app = app_with(shell_schema(), shell_doc(), opts);
    assert_eq!(app.cmd_history, vec!["r !echo one"]);
    press(&mut app, &[":"]);
    press(&mut app, &["up"]);
    assert_eq!(command_value(&app), "r !echo one");
}

#[test]
fn command_history_record_caps_memory_and_file_at_100_entries() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cmd-history");
    let mut opts = test_opts();
    opts.cmd_history_file = Some(file.clone());
    let mut app = app_with(shell_schema(), shell_doc(), opts);
    for i in 0..101 {
        press(&mut app, &[":"]);
        press(&mut app, &[&format!("r !echo {i}")]);
        press(&mut app, &["enter"]);
    }
    assert_eq!(app.cmd_history.len(), MAX_CMD_HISTORY);
    assert_eq!(app.cmd_history[0], "r !echo 1", "oldest entry drops first");
    assert_eq!(app.cmd_history[99], "r !echo 100");
    let lines: Vec<String> = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines, app.cmd_history, "file mirrors the capped ring");
}

#[test]
fn command_repeat_without_history_never_runs_a_literal_bang_command() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["!!"]);
    assert!(matches!(app.overlay, Some(Overlay::Command(_))));
    press(&mut app, &["enter"]);
    assert!(app.overlay.is_none());
    assert_eq!(app.notice, ": no previous shell command");
    assert!(!app.notice.contains("exit"), "notice: {}", app.notice);
    assert_eq!(str_at(&app.doc(), &["name"]), "old");
}

#[test]
fn shell_failure_and_non_string_targets_only_report_notices() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["!exit 4"]);
    press(&mut app, &["enter"]);
    assert!(app.overlay.is_none());
    assert!(app.notice.contains("exit 4"), "notice: {}", app.notice);
    assert_eq!(str_at(&app.doc(), &["name"]), "old");

    press(&mut app, &["j", "j"]);
    press(&mut app, &[":"]);
    press(&mut app, &["r !echo nope"]);
    press(&mut app, &["enter"]);
    assert!(
        app.notice.contains("not on a string"),
        "notice: {}",
        app.notice
    );
    assert_eq!(num_at(&app.doc(), &["count"]), "1");
}

#[test]
fn shell_prefers_a_string_variant_over_a_string_array_variant() {
    let schema = schema_build(|b| {
        let string = b.id(scalar("value", "", "string"));
        let array = b.id(FormNode {
            key: "values".into(),
            kind: Kind::Array,
            items: Some(string),
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "field".into(),
            title: "Field".into(),
            kind: Kind::OneOf,
            variants: vec![array, string],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![one],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("field", db.arr(vec![db.s("old")]))]));
    let mut app = app_with(schema, doc, test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["r !echo hi"]);
    press(&mut app, &["enter"]);
    assert_eq!(str_at(&app.doc(), &["field"]), "hi");
}

#[test]
fn shell_bare_command_runs_without_touching_the_document() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["!echo run-only"]);
    press(&mut app, &["enter"]);
    assert!(app.overlay.is_none());
    assert!(
        app.notice.contains("run-only"),
        "bare output must land in the bottom notice: {}",
        app.notice
    );
    assert_eq!(str_at(&app.doc(), &["name"]), "old");
}

#[test]
fn shell_bare_multiline_output_renders_inline_when_it_fits() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["!echo one && echo two"]);
    press(&mut app, &["enter"]);
    assert!(app.overlay.is_none(), "output fits, so no page overlay");
    assert_eq!(app.notice, "one\ntwo");
    assert!(app.list_mode(), "the menu keeps the keyboard");
    assert_eq!(str_at(&app.doc(), &["name"]), "old");

    let view = app.view();
    let rows: Vec<&str> = view.lines().collect();
    assert_eq!(rows.len(), app.height, "view: {view}");
    assert!(rows[app.height - 3].contains("one"), "view: {view}");
    assert!(rows[app.height - 2].contains("two"), "view: {view}");
    assert!(
        rows[app.height - 1].contains("↑/↓ move"),
        "the help row stays pinned: {view}"
    );
}

#[cfg(unix)]
#[test]
fn shell_multiline_output_fills_the_blank_rows_exactly() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    let blank = app.blank_rows_above_notice();
    assert!(blank > 1, "fixture needs free rows below the menu: {blank}");

    // Exactly the blank rows: the inline notice fits without moving rows.
    let fits = format!("!seq 1 {blank}");
    press(&mut app, &[":", fits.as_str(), "enter"]);
    assert!(app.overlay.is_none(), "exactly fitting output stays inline");
    assert_eq!(app.notice.lines().count(), blank);

    // One line more: the standalone page is the only place it fits.
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    let over = format!("!seq 1 {}", blank + 1);
    press(&mut app, &[":", over.as_str(), "enter"]);
    assert!(
        matches!(app.overlay, Some(Overlay::ShellOut(_))),
        "one line past the blank rows must open the page"
    );
}

#[cfg(unix)]
#[test]
fn shell_bare_multiline_output_too_tall_opens_the_scrolling_page() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["!seq 1 40"]);
    press(&mut app, &["enter"]);
    match &app.overlay {
        Some(Overlay::ShellOut(s)) => {
            assert_eq!(s.command, "seq 1 40");
            assert_eq!(s.lines.len(), 40);
            assert_eq!(s.scroll, 0);
        }
        _ => panic!("output taller than the blank rows must open the output page"),
    }
    assert!(app.notice.is_empty());
    press(&mut app, &["q"]);
    assert!(app.overlay.is_none());
}

#[test]
fn other_multiline_notices_render_inline() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    app.notice = "first line\nsecond line".to_string();
    let view = app.view();
    let rows: Vec<&str> = view.lines().collect();
    assert!(rows[app.height - 3].contains("first line"), "view: {view}");
    assert!(rows[app.height - 2].contains("second line"), "view: {view}");
}

#[test]
fn shell_output_page_scrolls_lines_and_pages_and_closes() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    app.width = 40;
    app.height = 5; // two title/hint rows leave a 3-row page
    app.overlay = Some(Overlay::ShellOut(ShellOutState {
        command: "seq 10".to_string(),
        lines: (0..10).map(|i| format!("line{i}")).collect(),
        scroll: 0,
    }));

    press(&mut app, &["j", "down"]);
    assert_eq!(shell_out_scroll(&app), 2);
    press(&mut app, &["k", "up"]);
    assert_eq!(shell_out_scroll(&app), 0);

    press(&mut app, &["pgdown"]);
    assert_eq!(shell_out_scroll(&app), 3);
    press(&mut app, &["ctrl+f"]);
    assert_eq!(shell_out_scroll(&app), 6);
    press(&mut app, &["pgdown"]);
    assert_eq!(
        shell_out_scroll(&app),
        7,
        "scrolling clamps at the last page"
    );
    press(&mut app, &["ctrl+f"]);
    assert_eq!(shell_out_scroll(&app), 7);

    press(&mut app, &["pgup"]);
    assert_eq!(shell_out_scroll(&app), 4);
    press(&mut app, &["ctrl+b"]);
    assert_eq!(shell_out_scroll(&app), 1);
    press(&mut app, &["pgup"]);
    assert_eq!(shell_out_scroll(&app), 0);

    press(&mut app, &["pgdown"]);
    let view = app.view();
    assert!(view.contains("line3"), "view: {view}");
    assert!(!view.contains("line0"), "view: {view}");

    press(&mut app, &["esc"]);
    assert!(app.overlay.is_none());
}

fn shell_out_scroll(app: &App) -> usize {
    match &app.overlay {
        Some(Overlay::ShellOut(s)) => s.scroll,
        _ => panic!("output page is not open"),
    }
}

#[cfg(unix)]
#[test]
fn shell_w_pipes_the_document_to_the_command_stdin() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["w !cat"]);
    press(&mut app, &["enter"]);
    assert!(app.overlay.is_none());
    assert!(
        app.notice.contains("\"name\"") && app.notice.contains("old"),
        "cat must echo the document from stdin: {}",
        app.notice
    );
    assert_eq!(str_at(&app.doc(), &["name"]), "old");
    assert_eq!(app.cmd_history, vec!["w !cat"]);
    assert!(!app.dirty(), "w !cat must not modify the document");
}

#[test]
fn shell_a_mode_appends_each_output_line_to_a_string_array() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &["j"]); // tags row
    press(&mut app, &[":"]);
    press(&mut app, &["a !echo x && echo y"]);
    press(&mut app, &["enter"]);
    let tags = app.doc().get(&sp(&["tags"])).unwrap();
    let got: Vec<String> = match app.doc().value(tags) {
        Value::Array(items) => items
            .iter()
            .map(|i| match app.doc().value(*i) {
                Value::Str(s) => s.clone(),
                _ => "<not a string>".into(),
            })
            .collect(),
        _ => panic!("tags is not an array"),
    };
    assert_eq!(got, vec!["a", "x", "y"]);
}

#[test]
fn shell_a_mode_rejects_a_string_target() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["a !echo x"]);
    press(&mut app, &["enter"]);
    assert!(
        app.notice.contains("not on a string array"),
        "notice: {}",
        app.notice
    );
    assert_eq!(str_at(&app.doc(), &["name"]), "old");
}

#[test]
fn command_tab_completes_the_word_before_the_cursor_and_keeps_the_suffix() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
    std::fs::write(dir.path().join("alpine"), "").unwrap();
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    let line = format!("!cat {}/alZZ", dir.path().display());
    let base = dir.path().display().to_string();
    press(&mut app, &[line.as_str()]);

    // Put the cursor before `ZZ`: completion uses only the prefix and
    // leaves the suffix in place.
    press(&mut app, &["left", "left"]);
    press(&mut app, &["tab"]);
    // The candidates come from bash in the completion script's order:
    // file completion answers in readdir order (and may spell the words
    // as full paths), so which file leads is environment-specific. The
    // contract under test is the mechanics: the leading candidate lands
    // before the untouched suffix, with the cursor between them.
    let selection = |app: &App| -> (Vec<String>, usize) {
        match &app.overlay {
            Some(Overlay::Command(s)) => (s.sug.clone(), s.sug_idx.unwrap_or(0) as usize),
            _ => panic!("command prompt is not open"),
        }
    };
    let (cands, i0) = selection(&app);
    let tails: Vec<String> = cands
        .iter()
        .map(|c| {
            let tail = c.rsplit('/').next().unwrap_or(c);
            match tail {
                "alpha.txt" | "alpine" => tail.to_string(),
                other => panic!("unexpected candidate {other:?}: {cands:?}"),
            }
        })
        .collect();
    let mut sorted = tails.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["alpha.txt", "alpine"], "candidates: {cands:?}");
    let value = command_value(&app);
    assert_eq!(
        value,
        format!("!cat {base}/{}ZZ", tails[i0]),
        "tab must insert the leading candidate before the suffix"
    );
    assert_eq!(
        command_cursor(&app),
        value.len() - "ZZ".len(),
        "the cursor lands after the inserted candidate, before the suffix"
    );

    // The candidate rows are stripped of the prefix they share with the
    // token and anchor at the caret the completion applied (the end of
    // the current candidate).
    let stripped = |tail: &str| -> String {
        let mut s = tail.to_string();
        for _ in 0..2 {
            let _ = s.remove(0); // drop the shared "al"
        }
        s
    };
    let selected_line = |app: &mut App, tail: &str| -> (String, usize) {
        let view = app.view();
        let line = view
            .lines()
            .find(|l| l.contains(&format!("\x1b[7m{}", stripped(tail))))
            .expect("selected command candidate line")
            .to_string();
        let start = line.find("\x1b[7m").expect("candidate background");
        (line, start)
    };
    let caret_col = |app: &App| {
        let value = command_value(app);
        display_width(":") + display_width(&value[..command_cursor(app)])
    };
    {
        let view = app.view();
        assert!(
            view.contains("pha.txt") && view.contains("pine"),
            "both candidates must be rendered: {view}"
        );
    }
    let (line, start) = selected_line(&mut app, &tails[i0]);
    assert_eq!(
        display_width(&line[..start]),
        caret_col(&app),
        "stripped command candidates anchor at the caret column"
    );
    // While completion is active, the arrow keys move the candidate cursor
    // instead of recalling command history.
    let i1 = (i0 + 1) % 2;
    press(&mut app, &["down"]);
    assert_eq!(
        command_value(&app),
        format!("!cat {base}/{}ZZ", tails[i1]),
        "the arrow key must select the other candidate"
    );
    let (line, start) = selected_line(&mut app, &tails[i1]);
    assert_eq!(
        display_width(&line[..start]),
        caret_col(&app),
        "cycling moves the window with the caret: {}",
        app.view()
    );
    press(&mut app, &["up"]);
    assert_eq!(command_value(&app), format!("!cat {base}/{}ZZ", tails[i0]));
    // Tab after arrow movement keeps using the same candidate list.
    press(&mut app, &["tab"]);
    assert_eq!(command_value(&app), format!("!cat {base}/{}ZZ", tails[i1]));
}

/// viewport_cell_shift mirrors ti_window's scroll rule but returns the
/// display cells skipped before the viewport start — the unit the window
/// anchor must subtract. A char count undercounts wide prefixes (each CJK
/// char is two cells), so this pins the unit independently of ti_window's
/// tuple layout.
fn viewport_cell_shift(value: &str, cursor_cell: usize, avail: usize) -> usize {
    let mut skipped = 0usize;
    for c in value.chars() {
        if cursor_cell - skipped < avail.max(1) {
            break;
        }
        skipped += char_width(c);
    }
    skipped
}

#[test]
fn command_window_anchors_at_the_caret_when_rows_are_stripped() {
    // Long shared prefixes strip the candidate rows to `…tail` form; the
    // window then anchors at the caret recorded when the list was built.
    // A long command scrolls the input viewport, so the anchor maps through
    // the skipped cells and clamps at the right edge instead of running
    // past the terminal.
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("日本語").join("b".repeat(40));
    std::fs::create_dir_all(&inner).unwrap();
    let long = format!("{}lpha.txt", "a".repeat(24));
    std::fs::write(inner.join(&long), "").unwrap();
    std::fs::write(inner.join(format!("{}lpine", "a".repeat(24))), "").unwrap();
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    app.width = 40;
    app.height = 24;
    press(&mut app, &[":"]);
    let base = dir.path().display().to_string();
    let trigger = format!("!cat {base}/日本語/{}/a", "b".repeat(40));
    press(&mut app, &[trigger.as_str()]);
    press(&mut app, &["tab"]);
    assert_eq!(
        command_value(&app),
        format!("!cat {base}/日本語/{}/{long}", "b".repeat(40))
    );
    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains(&format!("\x1b[7m{}lpha.txt", "a".repeat(23))))
        .expect("selected command candidate line");
    let start = line.find("\x1b[7m").expect("candidate background");
    let body = &line[start + "\x1b[7m".len()..];
    let rect = display_width(&body[..body.find("\x1b[0m").expect("candidate reset")]);
    let value = command_value(&app);
    let sug_end = match &app.overlay {
        Some(Overlay::Command(s)) => s.sug_end,
        _ => panic!("command prompt is not open"),
    };
    let avail = input_avail(app.width - 1, display_width(":"));
    let shift = viewport_cell_shift(&value, display_width(&value), avail);
    // Stripped rows anchor at the caret recorded when the list was built
    // (the applied candidate's end, not the word start in sug_col); the
    // anchor maps through the skipped viewport cells and the rectangle
    // slides left when it would overflow the terminal.
    let base = display_width(&value[..sug_end.min(value.len())]);
    let expected_left = (display_width(":") + base.saturating_sub(shift)).min(app.width - 1 - rect);
    assert_eq!(
        display_width(&line[..start]),
        expected_left,
        "the stripped-row anchor maps the caret column through the skipped display cells: {view}"
    );
}

#[test]
fn command_tab_expands_percent_to_the_session_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config with space.json");
    std::fs::write(&file, "").unwrap();
    let path = file.display().to_string();
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    app.fs_mut().opts.path = path.clone();
    press(&mut app, &[":"]);
    press(&mut app, &["!cat %"]);
    press(&mut app, &["tab"]);
    let want = crate::ui::command::shell_quote(&crate::ui::command::absolute_path(&path));
    assert_eq!(command_value(&app), format!("!cat {want}"));
}

#[cfg(unix)]
#[test]
fn shell_command_expands_percent_when_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config with space.json");
    std::fs::write(&file, "needle\n").unwrap();
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    app.fs_mut().opts.path = file.display().to_string();
    press(&mut app, &[":"]);
    press(&mut app, &["!cat %"]);
    press(&mut app, &["enter"]);
    assert!(
        app.notice.contains("needle"),
        "the command must see the expanded file path: {}",
        app.notice
    );
}

#[test]
fn command_completion_offers_builtins_but_not_shell_commands() {
    let sug = |app: &App| match &app.overlay {
        Some(Overlay::Command(s)) => (s.sug.clone(), s.sug_prefix.clone()),
        _ => panic!("command prompt is not open"),
    };
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":", "w"]);
    let (sugs, prefix) = sug(&app);
    assert_eq!(prefix, "w");
    assert!(sugs.contains(&"wa".to_string()), "{sugs:?}");
    assert!(sugs.contains(&"wq!".to_string()), "{sugs:?}");
    assert!(sugs.contains(&"waq".to_string()), "{sugs:?}");

    // `!` switches to shell completion: PATH commands, never builtin names.
    press(&mut app, &["ctrl+u", "!"]);
    let (sugs, prefix) = sug(&app);
    assert_eq!(prefix, "");
    assert!(!sugs.contains(&"waq".to_string()), "{sugs:?}");
    assert!(!sugs.contains(&"wq!".to_string()), "{sugs:?}");
}

#[test]
fn command_ctrl_j_k_cycle_candidates_while_active() {
    let st = |app: &App| match &app.overlay {
        Some(Overlay::Command(s)) => (s.sug.clone(), s.sug_idx, s.ti.value().to_string()),
        _ => panic!("command prompt is not open"),
    };
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":", "w", "tab"]);
    let (cands, i0, v0) = st(&app);
    assert!(i0.is_some(), "the first tab must start a completion cycle");
    press(&mut app, &["ctrl+j"]);
    let (cands1, i1, v1) = st(&app);
    assert_eq!(cands1, cands, "ctrl+j must cycle the stored list");
    assert_ne!(i1, i0);
    assert_eq!(v1, cands[i1.unwrap()]);
    press(&mut app, &["ctrl+k"]);
    let (_, i2, v2) = st(&app);
    assert_eq!(i2, i0);
    assert_eq!(v2, v0);
}

#[test]
fn command_ctrl_k_still_kills_when_no_completion_is_active() {
    // Without a live completion cycle ctrl+j/ctrl+k keep their input
    // bindings: ctrl+k kills to the end of the line.
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":", "w", "a", "left", "ctrl+k"]);
    match &app.overlay {
        Some(Overlay::Command(s)) => assert_eq!(s.ti.value(), "w"),
        _ => panic!("command prompt is not open"),
    }
}

#[test]
fn command_sug_window_reanchors_only_at_word_starts() {
    // sug_col is the completed word's start column: typing inside one word
    // never moves the candidate window, a separator that starts a new word
    // re-anchors it.
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    let col = |app: &App| match &app.overlay {
        Some(Overlay::Command(s)) => s.sug_col,
        _ => panic!("command prompt is not open"),
    };
    press(&mut app, &[":"]);
    press(&mut app, &["w"]);
    assert_eq!(col(&app), 0, "the first word starts at the prompt");
    // Full candidate rows anchor at the word start in the rendered view.
    let view = app.view();
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[2;48;5;236mwa"))
        .expect("candidate row");
    let start = line.find("\x1b[2;48;5;236m").expect("candidate background");
    assert_eq!(
        display_width(&line[..start]),
        1,
        "the window sits right after the prompt: {view}"
    );
    press(&mut app, &["a"]);
    assert_eq!(
        col(&app),
        0,
        "typing inside the word must not move the window"
    );
    press(&mut app, &[" "]);
    assert_eq!(col(&app), 3, "the separator starts a new word after `wa `");
    press(&mut app, &["/", "t"]);
    assert_eq!(col(&app), 3, "path characters stay inside the same word");
}

#[test]
fn command_sug_window_anchors_at_the_caret_for_stripped_rows() {
    // Candidates sharing a long prefix with the typed token render as
    // `…tail` rows, which anchor at the caret so the visible tails stay
    // next to the input position.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sing-geosite-steam.srs"), "").unwrap();
    std::fs::write(dir.path().join("sing-geosite-star.srs"), "").unwrap();
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    let line = format!("!cat {}/sing-geosite-s", dir.path().display());
    press(&mut app, &[line.as_str()]);
    let value = command_value(&app);
    let view = app.view();
    let row = view
        .lines()
        .find(|l| l.contains("\x1b[2;48;5;236mteam"))
        .expect("candidate row");
    let start = row.find("\x1b[2;48;5;236m").expect("candidate background");
    assert_eq!(
        display_width(&row[..start]),
        display_width(":") + display_width(&value),
        "stripped rows anchor at the caret column: {view}"
    );
}

#[test]
fn command_w_saves_and_wq_saves_then_exits() {
    let (opts, n) = counting_opts();
    let mut app = app_with(shell_schema(), shell_doc(), opts);
    press(&mut app, &[":", "w", "enter"]);
    assert_eq!(n.load(Ordering::SeqCst), 1);
    assert!(app.overlay.is_none());
    assert!(!app.exiting);
    assert_eq!(app.notice, "Saved");

    press(&mut app, &[":", "wq", "enter"]);
    assert_eq!(n.load(Ordering::SeqCst), 2);
    assert!(app.exiting);
}

#[test]
fn command_q_refuses_dirty_files_and_q_bang_discards() {
    let (opts, n) = counting_opts();
    let mut app = app_with(shell_schema(), shell_doc(), opts);
    let name = app.fs_mut().doc.new_str("changed");
    app.fs_mut().doc.set(&sp(&["name"]), name).unwrap();

    press(&mut app, &[":", "q", "enter"]);
    assert!(!app.exiting);
    assert_eq!(n.load(Ordering::SeqCst), 0);
    assert!(
        app.notice.contains("unsaved changes"),
        "notice: {}",
        app.notice
    );

    press(&mut app, &[":", "q!", "enter"]);
    assert!(app.exiting);
    assert_eq!(n.load(Ordering::SeqCst), 0, "q! must not save");
}

#[test]
fn command_wa_saves_all_dirty_files_and_stays() {
    let (mut app, n1, n2) = two_file_app();
    press(&mut app, &[":", "wa", "enter"]);
    assert_eq!(n1.load(Ordering::SeqCst), 1);
    assert_eq!(n2.load(Ordering::SeqCst), 1);
    assert_eq!(app.cur, 0, "wa returns to the active file");
    assert!(!app.exiting);
    assert_eq!(app.notice, "Saved 2 files");
}

#[test]
fn command_waq_saves_all_dirty_files_and_exits() {
    let (mut app, n1, n2) = two_file_app();
    press(&mut app, &[":", "waq", "enter"]);
    assert_eq!(n1.load(Ordering::SeqCst), 1);
    assert_eq!(n2.load(Ordering::SeqCst), 1);
    assert!(app.exiting);
}

#[test]
fn command_w_path_writes_to_the_given_target() {
    let mut opts = test_opts();
    opts.path = "/tmp/orig.json".to_string();
    let seen = Arc::new(std::sync::Mutex::new(String::new()));
    let got = Arc::clone(&seen);
    opts.write_to = Some(Arc::new(move |path: &str, _: &Doc, _: &Meta| {
        *got.lock().unwrap() = path.to_string();
        Ok(())
    }));
    let mut app = app_with(shell_schema(), shell_doc(), opts);
    press(&mut app, &[":", "w /tmp/out.json", "enter"]);
    assert_eq!(*seen.lock().unwrap(), "/tmp/out.json");
    assert!(app.overlay.is_none());
}

#[test]
fn shell_ctrl_u_then_ctrl_y_restores_the_command() {
    let mut app = app_with(shell_schema(), shell_doc(), test_opts());
    press(&mut app, &[":"]);
    press(&mut app, &["!echo hi"]);
    press(&mut app, &["ctrl+u"]);
    assert_eq!(command_value(&app), "");
    press(&mut app, &["ctrl+y"]);
    assert_eq!(command_value(&app), "!echo hi");
    press(&mut app, &["enter"]);
    assert!(app.notice.contains("hi"), "notice: {}", app.notice);
}
