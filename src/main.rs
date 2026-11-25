//! jse binary entry: CLI parsing, session loading, exit-code mapping.
//! The editor itself lives in the library (`jse`): the terminal UI under
//! `jse::ui`, the GUI under `jse::gui` (`--features gui`).
#![cfg_attr(not(test), forbid(unsafe_code))]
// The GUI artifact is a window app: no console window behind it on Windows.
// The terminal build keeps its console.
#![cfg_attr(
    all(target_os = "windows", feature = "gui"),
    windows_subsystem = "windows"
)]

#[cfg(not(feature = "gui"))]
use std::io::IsTerminal;

use jse::cli::{Invocation, parse_args, print_usage};
use jse::session::load_session;
use jse::ui::{FileSession, Outcome};

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags = match parse_args(&args) {
        Ok(Invocation::Help) => {
            print_usage();
            return 0;
        }
        Ok(Invocation::Version) => {
            println!("{}", jse::version::version_line());
            return 0;
        }
        Ok(Invocation::Edit(flags)) => flags,
        Err(msg) => {
            eprintln!("{msg}");
            print_usage();
            return 2;
        }
    };

    // --accessible drives the terminal line protocol; the GUI build has no
    // line mode, so say so instead of silently ignoring the flag.
    #[cfg(feature = "gui")]
    if flags.accessible {
        eprintln!("note: --accessible is a terminal mode; the GUI build ignores it");
    }

    // The GUI may start at the open screen; the TUI requires a config file.
    #[cfg(not(feature = "gui"))]
    if flags.files.is_empty() {
        eprintln!(
            "no config file: pass <config.(json|jsonc|yaml|yml|toml)> as a positional argument"
        );
        print_usage();
        return 2;
    }

    // The full-screen TUI needs a real stdin; fail before loading anything.
    // The GUI runs without a terminal, so the check is TUI-only.
    #[cfg(not(feature = "gui"))]
    if !flags.accessible && !std::io::stdin().is_terminal() {
        eprintln!("stdin is not a terminal; rerun with --accessible for line-based prompts");
        return 1;
    }

    // Every file opens in one session (the first is shown); any file that
    // cannot load or resolve a schema aborts the whole run before the UI
    // starts.
    let mut sessions: Vec<FileSession> = Vec::with_capacity(flags.files.len());
    for config in &flags.files {
        match load_session(
            config,
            &flags.schema,
            flags.force,
            flags.pick,
            flags.accessible,
        ) {
            Ok(s) => sessions.push(s),
            Err(e) => {
                eprintln!("{e}");
                return e.exit_code();
            }
        }
    }

    #[cfg(feature = "gui")]
    let outcome: Outcome = {
        let schema = flags.schema.clone();
        let force = flags.force;
        let pick = flags.pick;
        let open: jse::gui::OpenFn = std::sync::Arc::new(move |p| {
            load_session(p, &schema, force, pick, false).map_err(|e| e.to_string())
        });
        jse::gui::run(sessions, Some(open))
    };
    #[cfg(not(feature = "gui"))]
    let outcome: Outcome = jse::ui::run(sessions);
    if let Some(err) = &outcome.err {
        eprintln!("ui: {err}");
        return 1;
    }
    if outcome.aborted {
        if outcome.saved {
            if outcome.saved_files.len() == 1 {
                // A ctrl+s during the session already wrote the file; edits
                // made after that save are discarded.
                println!(
                    "Saved to {}; aborted (changes after the last save are lost)",
                    outcome.saved_files[0]
                );
            } else {
                for p in &outcome.saved_files {
                    println!("Saved to {p}");
                }
                println!("Aborted (changes after the last save are lost)");
            }
        } else {
            println!("Aborted, no changes written");
        }
        return 130;
    }
    if !outcome.saved {
        if outcome.unchanged {
            // Clean quit on a document identical to the loaded/last-saved
            // state: nothing to write.
            println!("No changes, nothing written");
            return 0;
        }
        println!("Aborted, no changes written");
        return 130;
    }
    if outcome.with_errors {
        println!(
            "Saved with validation errors (--force): {}",
            outcome.saved_files.join(", ")
        );
        return 0;
    }
    for p in &outcome.saved_files {
        println!("Saved to {p}");
    }
    0
}
