//! jse — schema-driven terminal editor for JSON/JSONC/YAML/TOML config files.
//! Rust port of the Go implementation (behavior spec: internal/**, cmd/jse).
#![cfg_attr(not(test), forbid(unsafe_code))]

mod catalog;
mod cli;
mod doc;
mod fetch;
mod icons;
mod paths;
mod session;
mod smodel;
mod ui;
mod validate;
mod version;

#[cfg(test)]
mod catalog_tests;
#[cfg(test)]
mod fetch_tests;
#[cfg(test)]
mod validate_tests;
// Build-time-only helper (included by build.rs); tests live in the file.
#[cfg(test)]
#[path = "vcsver.rs"]
mod vcsver;

use std::io::IsTerminal;

use cli::{Invocation, parse_args, print_usage};
use session::load_session;
use ui::{FileSession, Outcome};

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
            println!("{}", version::version_line());
            return 0;
        }
        Ok(Invocation::Edit(flags)) => flags,
        Err(msg) => {
            eprintln!("{msg}");
            print_usage();
            return 2;
        }
    };

    if flags.files.is_empty() {
        eprintln!(
            "no config file: pass <config.(json|jsonc|yaml|yml|toml)> as a positional argument"
        );
        print_usage();
        return 2;
    }

    // The full-screen TUI needs a real stdin; fail before loading anything.
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

    let outcome: Outcome = ui::run(sessions);
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

// --- session assembly ------------------------------------------------------------

// --- schema source resolution ----------------------------------------------------

// --- filesystem helpers ----------------------------------------------------------

#[cfg(test)]
mod main_test {
    use super::*;
    use std::path::Path;

    use crate::cli::Flags;
    use crate::doc::Parsed;
    use crate::paths::{abs_lexical, search_stem};
    use crate::session::resolve_schema_loc;

    fn session_with_schema(cfg: &str, loc: &str) -> Parsed {
        let codec = doc::codec_for(cfg);
        let mut p = codec.empty();
        codec.set_schema_location(&mut p.doc, &mut p.meta, loc);
        p
    }

    /// Restores the process working directory on every exit path (Go t.Chdir).
    struct CwdGuard(std::path::PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    #[test]
    fn test_resolve_schema_loc_relative() {
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join("config");
        let work_dir = root.path().join("work");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        let cfg = config_dir.join("cfg.json");
        let cfg = cfg.to_str().unwrap();
        let write_schema = |path: &Path| {
            std::fs::write(path, br#"{"type":"object"}"#).unwrap();
        };

        let _cwd = CwdGuard(std::env::current_dir().unwrap());
        std::env::set_current_dir(&work_dir).unwrap();

        // config directory wins
        write_schema(&config_dir.join("schema.json"));
        write_schema(&work_dir.join("schema.json"));
        let sess = session_with_schema(cfg, "schema.json");
        let got = resolve_schema_loc("", cfg, doc::codec_for(cfg), &sess.doc, &sess.meta).unwrap();
        assert_eq!(got, config_dir.join("schema.json").to_str().unwrap());

        // falls back to cwd
        write_schema(&work_dir.join("fallback.json"));
        let sess = session_with_schema(cfg, "fallback.json");
        let got = resolve_schema_loc("", cfg, doc::codec_for(cfg), &sess.doc, &sess.meta).unwrap();
        assert_eq!(got, "fallback.json");
        let abs = abs_lexical(&got);
        assert_eq!(abs, work_dir.join("fallback.json").to_str().unwrap());
    }

    #[test]
    fn test_search_stem() {
        let cases = [
            ("server.json", "server"),
            ("a.b.json", "a.b"),
            (".cshrc", ".cshrc"), // Ext(".cshrc") is the whole name
            ("noext", "noext"),
        ];
        for (input, want) in cases {
            assert_eq!(search_stem(input), want, "search_stem({input:?})");
        }
    }

    #[test]
    fn test_parse_args() {
        fn parse(input: &[&str]) -> Result<Invocation, String> {
            let args: Vec<String> = input.iter().map(|s| s.to_string()).collect();
            super::parse_args(&args)
        }
        fn edit(
            schema: &str,
            force: bool,
            accessible: bool,
            pick: bool,
            files: &[&str],
        ) -> Invocation {
            Invocation::Edit(Flags {
                schema: schema.to_string(),
                force,
                accessible,
                pick,
                files: files.iter().map(|s| s.to_string()).collect(),
            })
        }

        let cases: Vec<(&str, Vec<&str>, Result<Invocation, String>)> = vec![
            ("short version", vec!["-v"], Ok(Invocation::Version)),
            ("long version", vec!["--version"], Ok(Invocation::Version)),
            ("short help", vec!["-h"], Ok(Invocation::Help)),
            ("long help", vec!["--help"], Ok(Invocation::Help)),
            // The first terminal flag encountered wins.
            ("version then help", vec!["-vh"], Ok(Invocation::Version)),
            ("help then version", vec!["-hv"], Ok(Invocation::Help)),
            (
                "version ignores files",
                vec!["a.json", "-v"],
                Ok(Invocation::Version),
            ),
            (
                "schema separate",
                vec!["-s", "a", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "schema attached",
                vec!["-sa", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "short equals compatibility",
                vec!["-s=a", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "long equals",
                vec!["--schema=a", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "long separate",
                vec!["--schema", "a", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "flags after path",
                vec!["b.json", "-f", "-a"],
                Ok(edit("", true, true, false, &["b.json"])),
            ),
            (
                "short cluster",
                vec!["-fa", "b.json"],
                Ok(edit("", true, true, false, &["b.json"])),
            ),
            (
                "full short cluster",
                vec!["-pfa", "b.json"],
                Ok(edit("", true, true, true, &["b.json"])),
            ),
            (
                "repeated booleans",
                vec!["-ff", "--force", "b.json"],
                Ok(edit("", true, false, false, &["b.json"])),
            ),
            (
                "double dash terminator",
                vec!["--", "-x.json"],
                Ok(edit("", false, false, false, &["-x.json"])),
            ),
            (
                "double dash stops options",
                vec!["--", "-v", "-h"],
                Ok(edit("", false, false, false, &["-v", "-h"])),
            ),
            (
                "lone dash is an operand",
                vec!["-", "b.json", "--force"],
                Ok(edit("", true, false, false, &["-", "b.json"])),
            ),
            (
                "schema value may start with dash",
                vec!["--schema", "-x"],
                Ok(edit("-x", false, false, false, &[])),
            ),
            (
                "schema consumes double dash",
                vec!["-s", "--"],
                Ok(edit("--", false, false, false, &[])),
            ),
            (
                "missing short schema value",
                vec!["-s"],
                Err("option requires an argument -- 's'".to_string()),
            ),
            (
                "missing long schema value",
                vec!["--schema"],
                Err("option '--schema' requires an argument".to_string()),
            ),
            (
                "unknown short",
                vec!["-x"],
                Err("invalid option -- 'x'".to_string()),
            ),
            (
                "unknown long",
                vec!["--unknown"],
                Err("unrecognized option '--unknown'".to_string()),
            ),
            (
                "single-dash long is rejected",
                vec!["-force"],
                Err("invalid option -- 'o'".to_string()),
            ),
            (
                "old --s spelling is rejected",
                vec!["--s"],
                Err("unrecognized option '--s'".to_string()),
            ),
            (
                "boolean long takes no value",
                vec!["--force=false"],
                Err("option '--force' doesn't allow an argument".to_string()),
            ),
            (
                "help takes no value",
                vec!["--help=x"],
                Err("option '--help' doesn't allow an argument".to_string()),
            ),
        ];
        for (name, input, want) in cases {
            assert_eq!(parse(&input), want, "{name}");
        }
    }

    #[test]
    fn test_version_line_formats() {
        assert_eq!(
            version::version_line_from("v0.2.1", "b2b73a4", "2026-10-18T13:18:54Z"),
            "jse v0.2.1 (commit b2b73a4, built 2026-10-18T13:18:54Z)"
        );
        assert_eq!(
            version::version_line_from("v0.2.1", "none", "unknown"),
            "jse v0.2.1"
        );
        assert_eq!(
            version::version_line_from("dev", "63d1088", "unknown"),
            "jse dev (commit 63d1088)"
        );
        // build.rs already produces the short development form.
        assert_eq!(
            version::version_line_from("v0.1.1-0", "01554d6", "2026-10-22T12:07:33Z"),
            "jse v0.1.1-0 (commit 01554d6, built 2026-10-22T12:07:33Z)"
        );
        assert_eq!(
            version::version_line_from("v0.1.1-0+dirty", "01554d6", "2026-10-22T12:07:33Z"),
            "jse v0.1.1-0+dirty (commit 01554d6, built 2026-10-22T12:07:33Z)"
        );
        assert_eq!(
            version::version_line_from("v1.2.3-rc.1", "none", "unknown"),
            "jse v1.2.3-rc.1"
        );
    }
}
