//! `:` shell escapes over a pty: output insertion, bash completion,
//! multiline paging.

use super::*;

// --- pty tests --------------------------------------------------------------

#[cfg(unix)]
#[test]
fn pty_shell_tab_arrows_keep_the_candidate_list() {
    use std::os::unix::fs::PermissionsExt;
    let dir = mktemp();
    let schema = dir.path().join("shell.schema.json");
    let cfg = dir.path().join("shell.json");
    write_file(&schema, r#"{"type":"object"}"#);
    write_file(&cfg, r#"{"name":"old"}"#);
    let commands = dir.path().join("bin");
    std::fs::create_dir(&commands).unwrap();
    for name in ["zzalpha", "zzalpine", "zzalps"] {
        let p = commands.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path_env = commands.to_string_lossy().into_owned();
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", schema_arg.as_str(), cfg_arg.as_str()],
        &[("PATH", path_env.as_str())],
        &cache,
    ) else {
        return;
    };
    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b":");
    let (_, off) = pty.wait_frame("command prompt", 0, &[": command"]);
    // Builtin commands appear on open; `!` switches to PATH completion.
    pty.wait_frame("builtin candidates", off, &["wa", "wq", "waq"]);
    pty.send(b"!zzal");
    let (_, off) = pty.wait_frame(
        "typed with automatic candidates",
        off,
        &[":!zzal", "zzalpha", "zzalpine", "zzalps"],
    );
    pty.send(b"\t");
    let (_, off) = pty.wait_frame(
        "candidate overlay",
        off,
        &["zzalpha", "zzalpine", "zzalps", ":!zzalpha"],
    );
    pty.send(b"\x1b[B"); // down
    let (_, off) = pty.wait_frame("down candidate", off, &["zzalpine", ":!zzalpine"]);
    pty.send(b"\x1b[A"); // up
    let (_, off) = pty.wait_frame("up candidate", off, &["zzalpha", ":!zzalpha"]);
    pty.send(b"\t");
    pty.wait_frame("tab after arrows cycles", off, &["zzalpine", ":!zzalpine"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[cfg(unix)]
#[test]
fn pty_shell_tab_uses_real_system_bash_machinery() {
    // No fake scripts: the driver sources the system bash-completion
    // machinery and completes `git st` from git's own completion rule.
    // Skipped when the machinery or git is missing on the host.
    if !std::path::Path::new("/usr/share/bash-completion/bash_completion").exists()
        && !std::path::Path::new("/etc/bash_completion").exists()
    {
        eprintln!("SKIP: no system bash-completion machinery");
        return;
    }
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("SKIP: git missing");
        return;
    }
    let dir = mktemp();
    let schema = dir.path().join("s.json");
    let cfg = dir.path().join("c.json");
    write_file(&schema, r#"{"type":"object"}"#);
    write_file(&cfg, r#"{"name":"x"}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b":");
    let (_, off) = pty.wait_frame("command prompt", 0, &[": command"]);
    pty.send(b"!git st\t");
    pty.wait_frame(
        "git candidates from the system machinery",
        off,
        &["git stash"],
    );
    pty.send(b"\x03");
    pty.wait_exit(130);
}

fn pty_shell_argument_tab_uses_bash_completion() {
    // a unix machine without bash cannot run the completion driver
    if std::process::Command::new("bash")
        .arg("-c")
        .arg("exit 0")
        .output()
        .is_err()
    {
        return;
    }
    let dir = mktemp();
    let schema = dir.path().join("shell.schema.json");
    let cfg = dir.path().join("shell.json");
    write_file(&schema, r#"{"type":"object"}"#);
    write_file(&cfg, r#"{"name":"old"}"#);

    // a fake bash-completion core plus a completion rule for `mysvc`;
    // JSE_BASH_COMPLETION points the binary's completion driver at them
    let comps = dir.path().join("comps");
    std::fs::create_dir(&comps).unwrap();
    write_file(
        &comps.join("mysvc"),
        concat!(
            "\n_mysvc() {\n",
            "  local cur=${COMP_WORDS[COMP_CWORD]}\n",
            "  COMPREPLY=($(compgen -W \"start stop status\" -- \"$cur\"))\n",
            "}\n",
            "complete -F _mysvc mysvc\n"
        ),
    );
    let core = dir.path().join("core.sh");
    write_file(
        &core,
        "_completion_loader() { [ -f \"$JSE_FAKE_DIR/$1\" ] && . \"$JSE_FAKE_DIR/$1\"; }\n",
    );
    let core_arg = core.to_string_lossy().into_owned();
    let comps_arg = comps.to_string_lossy().into_owned();

    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &["-s", schema_arg.as_str(), cfg_arg.as_str()],
        &[
            ("JSE_BASH_COMPLETION", core_arg.as_str()),
            ("JSE_FAKE_DIR", comps_arg.as_str()),
        ],
        &cache,
    ) else {
        return;
    };
    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b":");
    let (_, off) = pty.wait_frame("command prompt", 0, &[": command"]);
    pty.send(b"!mysvc ");
    // the space opens a fresh argument word: bash runs once and the
    // candidate window lists the rule's words without a tab press
    let (_, off) = pty.wait_frame("auto bash candidates", off, &["status", ":!mysvc "]);
    pty.send(b"st");
    // typing inside the word filters the fetched words locally
    let (_, off) = pty.wait_frame("typed shell argument", off, &[":!mysvc st", "start"]);
    pty.send(b"\t");
    // the tab press re-asks bash for the word's context and applies the
    // matching candidate
    pty.wait_frame("tab applies start", off, &[":!mysvc start"]);
    pty.send(b"\x03");
    pty.wait_exit(130);
}

#[test]
fn pty_shell_command_inserts_output() {
    let dir = mktemp();
    let schema = dir.path().join("shell.schema.json");
    let cfg = dir.path().join("shell.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{
		"name":{"type":"string","title":"Name"},
		"tags":{"type":"array","title":"Tags","items":{"type":"string"}}}}"#,
    );
    write_file(&cfg, r#"{"name":"old","tags":["a"]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b":");
    pty.wait_frame("command prompt", 0, &[": command"]);
    pty.send(b"r !echo from-shell");
    let (_, typed_off) = pty.wait_frame("command typed", 0, &[":r !echo from-shell"]);
    pty.send(b"\r");
    // The redraw after enter shows the inserted value; the typed prompt text
    // was already consumed by typed_off.
    pty.wait_frame("inserted output", typed_off, &["from-shell"]);
    pty.send(b"\x13"); // ctrl+s
    pty.wait_frame("saved", typed_off, &["Write to"]);
    pty.send(b"q");
    pty.wait_exit(0);

    let saved = read_json(&cfg);
    assert_eq!(
        saved["name"],
        json!("from-shell"),
        "shell output must replace the focused string"
    );
    assert_eq!(saved["tags"], json!(["a"]), "array untouched");
}

#[test]
fn pty_shell_bare_multiline_output_pages_and_closes() {
    let dir = mktemp();
    let schema = dir.path().join("shell.schema.json");
    let cfg = dir.path().join("shell.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#,
    );
    write_file(&cfg, r#"{"name":"old"}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };

    pty.wait_frame("root menu", 0, &["↑/↓ move"]);
    pty.send(b":");
    pty.wait_frame("command prompt", 0, &[": command"]);
    let command = "!seq 1 40 | sed 's/^/row-/'";
    pty.send(command.as_bytes());
    let (_, typed_off) = pty.wait_frame("command typed", 0, &[":!seq 1 40"]);
    pty.send(b"\r");
    let (_, first_page) = pty.wait_frame(
        "multi-line output page",
        typed_off,
        &["row-28", "q/esc close"],
    );
    // 40 output lines over a 28-line page: the first page ends at row-28.
    pty.send(b"\x06"); // ctrl+f
    let (_, paged) = pty.wait_frame("paged to the end", first_page, &["row-40"]);
    pty.send(b"q");
    let (_, closed) = pty.wait_frame("output page closed", paged, &["↑/↓ move"]);

    // A short multi-line output fits in the blank rows above the help bar:
    // it renders inline instead of opening the output page.
    pty.send(b":");
    let (_, prompt) = pty.wait_frame("second prompt", closed, &[": command"]);
    pty.send(b"!printf 'alpha\\nbeta\\n'");
    let (_, typed_short) = pty.wait_frame("short command typed", prompt, &["printf 'alpha"]);
    pty.send(b"\r");
    let (snap, _) = pty.wait_frame(
        "short output inline",
        typed_short,
        &["alpha", "beta", "↑/↓ move"],
    );
    let rows: Vec<&str> = snap[typed_short..].split("\r\n").collect();
    assert_eq!(rows.len(), 30, "one full frame: {rows:?}");
    assert_eq!(rows[27].trim(), "alpha", "inline output: {rows:?}");
    assert_eq!(rows[28].trim(), "beta", "inline output: {rows:?}");
    assert!(rows[29].contains("↑/↓ move"), "help row: {rows:?}");

    pty.send(b"q");
    pty.wait_exit(0);
    assert_eq!(
        read_json(&cfg)["name"],
        json!("old"),
        "bare command edits nothing"
    );
}
