//! Multi-file sessions, save/quit flows, disk reload prompts, command
//! history persistence.

use super::*;

#[test]
fn pty_command_history_persists_across_sessions() {
    let dir = mktemp();
    let cfg = dir.path().join("cfg.json");
    write_file(&cfg, r#"{"name":"x"}"#);
    let cache = mktemp();
    let cfg_arg = arg(&cfg);
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    let (_snap, mut off) = pty.wait_frame("schema-less root", 0, &["no schema found", "name"]);

    // One shell escape; its line lands in the persistent history file.
    pty.send(b":!echo jse-hist-marker\r");
    (_, off) = pty.wait_frame("shell output shown", off, &["jse-hist-marker"]);
    assert_eq!(
        read_text(&cache.path().join("cmd-history")),
        "!echo jse-hist-marker\n",
        "the executed command must be persisted immediately"
    );
    pty.send(b"\x03");
    pty.wait_exit(130);

    // A second session over the same cache recalls it with up.
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    pty.wait_frame("second session root", 0, &["name"]);
    pty.send(b":");
    pty.pause(Duration::from_millis(150));
    pty.send(b"\x1b[A"); // up
    pty.wait_frame("history recalled", 0, &["!echo jse-hist-marker"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_quit_save_prompt() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let out_file = dir.path().join("o.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    let schema_arg = arg(&schema);
    let out_arg = arg(&out_file);
    let cache = mktemp();

    let start = |cache: &tempfile::TempDir| -> Option<PtySession> {
        pty_or_skip(&["-s", &schema_arg, &out_arg], &[], cache)
    };

    // unchanged: esc quits at once, nothing written, no prompt
    let Some(mut pty) = start(&cache) else { return };
    let (_, off) = pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"\x1b");
    let (all, _) = pty.wait_frame("clean exit", off, &["No changes, nothing written"]);
    assert!(
        !all.contains("Save changes?"),
        "clean quit must not prompt:\n{all}"
    );
    pty.wait_exit(0);
    assert!(!out_file.exists(), "clean quit wrote the file");
    drop(pty);

    // changed: esc asks whether to save; y writes and exits 0
    let Some(mut pty) = start(&cache) else { return };
    let (_, off) = pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"\rhello\r");
    let (_, off) = pty.wait_frame("edited name", off, &["●"]);
    pty.send(b"\x1b");
    let (_, off) = pty.wait_frame("save prompt", off, &["Save changes?", "n discard & exit"]);
    pty.send(b"y");
    pty.wait_frame("saved exit", off, &["Saved to"]);
    pty.wait_exit(0);
    assert_eq!(read_json(&out_file)["name"], json!("hello"));
    drop(pty);

    // changed: esc then n discards the edit and exits 130
    let Some(mut pty) = start(&cache) else { return };
    let (_, off) = pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"\r\x15bye\r");
    let (_, off) = pty.wait_frame("edited name", off, &["●"]);
    pty.send(b"\x1b");
    let (_, off) = pty.wait_frame("save prompt", off, &["Save changes?"]);
    pty.send(b"n");
    pty.wait_frame("discard exit", off, &["Aborted, no changes written"]);
    pty.wait_exit(130);
    assert_eq!(
        read_json(&out_file)["name"],
        json!("hello"),
        "discard rewrote the file"
    );
}

#[test]
fn pty_save() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let out_file = dir.path().join("o.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    let schema_arg = arg(&schema);
    let out_arg = arg(&out_file);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &out_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    // type the name on the field row, enter to confirm, ctrl+s to save
    pty.pause(Duration::from_millis(300));
    pty.send(b"\rhello\r"); // enter edit on Name, type, confirm
    pty.pause(Duration::from_millis(300));
    pty.send(b"\x13");

    // ctrl+s must write the file and keep the session open: wait for the
    // saved notice instead of an exit
    pty.wait_frame("saved notice", 0, &["Write to "]);
    pty.send(b"\x03"); // session continues after ctrl+s; abort now
    pty.wait_exit(130);
    let content = read_text(&out_file);
    assert!(content.contains(r#""name": "hello""#), "output = {content}");
}

#[test]
fn pty_command_wq_saves_and_exits() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let out_file = dir.path().join("o.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    let schema_arg = arg(&schema);
    let out_arg = arg(&out_file);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &out_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"\rhello\r"); // edit Name
    pty.send(b":wq\r");
    pty.wait_exit(0);
    let content = read_text(&out_file);
    assert!(content.contains(r#""name": "hello""#), "output = {content}");
}

#[test]
fn pty_command_percent_expands_to_the_session_file() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let out_file = dir.path().join("o file.json"); // the space proves quoting
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    let schema_arg = arg(&schema);
    let out_arg = arg(&out_file);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &out_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b"\rfrom-percent\r");
    pty.send(b":w\r");
    let (_, saved) = pty.wait_frame("saved", 0, &["Write to "]);
    // `%` is the session file; the shell must receive the quoted path.
    pty.send(b":!cat %\r");
    // The short JSON output fits the blank rows, so it stays inline.
    pty.wait_frame(
        "percent output inline",
        saved,
        &["from-percent", "↑/↓ move"],
    );
    pty.send(b":wq\r");
    pty.wait_exit(0);
    let content = read_text(&out_file);
    assert!(content.contains("from-percent"), "output = {content}");
}

#[test]
fn pty_multi_file_session() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let file_a = dir.path().join("a.json");
    let file_b = dir.path().join("b.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&file_a, r#"{"name": "alpha"}"#);
    write_file(&file_b, r#"{"name": "beta"}"#);
    let schema_arg = arg(&schema);
    let a_arg = arg(&file_a);
    let b_arg = arg(&file_b);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &a_arg, &b_arg], &[], &cache) else {
        return;
    };

    let (_, off) = pty.wait_frame("first file root", 0, &["a.json [1/2]", "↑/↓ move"]);
    pty.send(b"s"); // file switcher
    let (_, off) = pty.wait_frame("file picker", off, &["Files (2)", "b.json"]);
    pty.send(b"j\r"); // switch to b.json
    let (_, off) = pty.wait_frame("second file root", off, &["b.json [2/2]"]);
    pty.send(b"\x0f"); // ctrl+o back across the boundary
    let (_, off) = pty.wait_frame("back on first file", off, &["a.json [1/2]"]);
    pty.send(b"\t"); // ctrl+i forward again
    let (_, off) = pty.wait_frame("forward to second file", off, &["b.json [2/2]"]);
    pty.send(b"\r\x15gamma\r"); // edit b.json's name
    let (_, off) = pty.wait_frame("edited name", off, &["●"]);
    pty.send(b"\x13"); // ctrl+s saves the active file only
    pty.wait_frame("saved b.json", off, &["Write to ", b_arg.as_str()]);
    pty.send(b"\x03");

    pty.wait_exit(130);
    assert_eq!(read_json(&file_b)["name"], json!("gamma"));
    assert_eq!(read_json(&file_a)["name"], json!("alpha"));
}

#[test]
fn pty_multi_file_quit_saves_all() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let file_a = dir.path().join("a.json");
    let file_b = dir.path().join("b.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&file_a, r#"{"name": "alpha"}"#);
    write_file(&file_b, r#"{"name": "beta"}"#);
    let schema_arg = arg(&schema);
    let a_arg = arg(&file_a);
    let b_arg = arg(&file_b);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &a_arg, &b_arg], &[], &cache) else {
        return;
    };

    let (_, off) = pty.wait_frame("first file root", 0, &["a.json [1/2]"]);
    pty.send(b"\r\x15one\r"); // dirty a.json
    let (_, off) = pty.wait_frame("a.json dirty", off, &["●"]);
    pty.send(b"s"); // file switcher (keys split: a pty write of "sj"
    let (_, off) = pty.wait_frame("file picker", off, &["Files (2)"]); // coalesces runes)
    pty.send(b"j\r");
    let (_, off) = pty.wait_frame("second file root", off, &["b.json [2/2]"]);
    pty.send(b"\r\x15two\r"); // dirty b.json
    let (_, off) = pty.wait_frame("b.json dirty", off, &["●"]);
    pty.send(b"\x1b"); // quit at the root: one confirm for both files
    let (_, off) = pty.wait_frame("multi-file save prompt", off, &["Save changes in 2 files?"]);
    pty.send(b"y");
    pty.wait_frame("saved exit", off, &["Saved to"]);
    pty.wait_exit(0);
    // count only after the process is gone: the two save lines stream in
    // separately and a mid-write snapshot can hold just the first one
    let all = pty.snapshot();
    assert_eq!(
        all.matches("Saved to").count(),
        2,
        "output has the wrong number of \"Saved to\" lines:\n{all}"
    );
    assert_eq!(read_json(&file_a)["name"], json!("one"));
    assert_eq!(read_json(&file_b)["name"], json!("two"));
}

#[test]
fn accessible_multi_file() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let file_a = dir.path().join("a.json");
    let file_b = dir.path().join("b.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string"}}}"#,
    );
    write_file(&file_a, r#"{"name": "alpha"}"#);
    write_file(&file_b, r#"{"name": "beta"}"#);
    let schema_arg = arg(&schema);
    let a_arg = arg(&file_a);
    let b_arg = arg(&file_b);
    let cache = mktemp();
    let cache_dir = arg(cache.path());
    let r = run_jse_env(
        repo_root(),
        "\n\n",
        &["--accessible", "-s", &schema_arg, &a_arg, &b_arg],
        &[("JSE_CACHE_DIR", cache_dir.as_str())],
        Duration::from_secs(30),
    );
    assert_eq!(
        r.code, 0,
        "exit; stdout:\n{}\nstderr:\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stderr.contains("unexpected argument"),
        "second file rejected:\n{}",
        r.stderr
    );
    assert_eq!(
        r.stdout.matches("Saved to").count(),
        2,
        "output has the wrong number of \"Saved to\" lines (one per file):\n{}",
        r.stdout
    );
    assert_eq!(read_json(&file_a)["name"], json!("alpha"));
    assert_eq!(read_json(&file_b)["name"], json!("beta"));
}

/// After reloading a file from disk, undo/redo keep working: `u` first
/// undoes the post-reload edit, then reverts the reload itself back to the
/// pre-reload content.
#[test]
fn pty_reload_keeps_undo_redo_usable() {
    let dir = mktemp();
    let cfg = dir.path().join("reload.json");
    write_file(&cfg, r#"{"name":"old"}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["name", "old", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    // external change + reload
    write_file(&cfg, r#"{"name":"fresh"}"#);
    pty.wait_frame("reload question", off, &["changed on disk"]);
    pty.send(b"y");
    pty.wait_frame("reloaded", off, &["fresh"]);
    pty.pause(Duration::from_millis(200));

    // edit the reloaded value: enter, type, commit
    pty.send(b"\r");
    pty.pause(Duration::from_millis(200));
    pty.send(b"zzz\r");
    pty.wait_frame("edited", off, &["zzz"]);

    // undo 1: back to the reloaded content
    pty.send(b"u");
    pty.wait_frame("undo edit", off, &["fresh"]);
    // undo 2: back to the pre-reload content
    pty.send(b"u");
    pty.wait_frame("undo reload", off, &["old"]);
    // redo re-applies the reload
    pty.send(b"\x12");
    pty.wait_frame("redo reload", off, &["fresh"]);
    pty.send(b"\x13"); // ctrl+s
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert_eq!(read_json(&cfg), json!({"name": "fresh"}));
}

#[test]
fn pty_external_disk_change_prompts_and_reloads() {
    let dir = mktemp();
    let cfg = dir.path().join("watched.json");
    write_file(&cfg, r#"{"name":"old"}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    let (_, off) = pty.wait_frame("schema-less root", 0, &["name", "old", "↑/↓ move"]);

    // Another process rewrites the file while the editor idles: the next
    // poll tick (250ms) must surface the reload question.
    write_file(&cfg, r#"{"name":"fresh-on-disk","extra":true}"#);
    pty.wait_frame(
        "reload question",
        off,
        &["changed on disk", "Reload? (y/n)"],
    );

    // n keeps the in-memory document and closes the ask.
    pty.send(b"n");
    let (snap, off) = pty.wait_frame("declined", off, &["old"]);
    assert!(
        !snap.get(off..).unwrap_or("").contains("changed on disk"),
        "the ask must be gone after n"
    );

    // A second change asks again; this time reload and observe the new value.
    write_file(&cfg, r#"{"name":"second-on-disk"}"#);
    pty.wait_frame("reload question again", off, &["changed on disk"]);
    pty.send(b"y");
    let (snap, off) = pty.wait_frame("reloaded from disk", off, &["second-on-disk"]);
    assert!(
        !snap.get(off..).unwrap_or("").contains("changed on disk"),
        "the ask must be gone after reload"
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}

/// Quitting a multi-file session with one dirty file asks once; answering n
/// discards everything and aborts with exit 130, writing no file.
#[test]
fn pty_multi_file_quit_no_exits_130() {
    let dir = mktemp();
    let schema = dir.path().join("mini.schema.json");
    let file_a = dir.path().join("a.json");
    let file_b = dir.path().join("b.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&file_a, r#"{"name": "alpha"}"#);
    write_file(&file_b, r#"{"name": "beta"}"#);
    let schema_arg = arg(&schema);
    let a_arg = arg(&file_a);
    let b_arg = arg(&file_b);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &a_arg, &b_arg], &[], &cache) else {
        return;
    };

    let (_, off) = pty.wait_frame("first file root", 0, &["a.json [1/2]"]);
    pty.send(b"\r\x15one\r"); // dirty a.json only
    let (_, off) = pty.wait_frame("a.json dirty", off, &["●"]);
    pty.send(b"\x1b"); // quit at the root: the save prompt appears
    let (_, _off) = pty.wait_frame("save prompt", off, &["Save changes? (y/n)"]);
    pty.send(b"n"); // discard and exit
    pty.wait_exit(130);

    assert_eq!(read_json(&file_a)["name"], json!("alpha"));
    assert_eq!(read_json(&file_b)["name"], json!("beta"));
}
