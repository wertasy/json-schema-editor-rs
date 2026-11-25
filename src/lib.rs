//! jse — schema-driven editor for JSON/JSONC/YAML/TOML config files.
//! Rust port of the Go implementation (behavior spec: internal/**, cmd/jse).
//!
//! The crate builds in two shapes: the `jse` binary (this library plus the
//! thin `main.rs` entry) and the library itself, which GUI front ends and
//! the Native Shell runners link (`--features gui`).
#![cfg_attr(not(test), forbid(unsafe_code))]

mod catalog;
pub mod cli;
pub mod doc;
mod fetch;
mod icons;
mod paths;
pub mod session;
pub mod smodel;
pub mod ui;
mod validate;

pub mod version {
    //! `--version` output.
    //!
    //! Build metadata comes from build.rs.

    /// Constants injected by build.rs into $OUT_DIR/version_info.rs: the version
    /// stamp plus the VCS revision and build time (sentinels "none"/"unknown"
    /// when built outside a git checkout). The version is already display-ready:
    /// the highest reachable tag (or `v0.0.0`), or the crate version outside git,
    /// with `+dirty` when the worktree is uncommitted.
    mod meta {
        include!(concat!(env!("OUT_DIR"), "/version_info.rs"));
    }

    /// versionLine renders the --version output. Unknown extras are left out, so
    /// an installed build reads "jse v0.2.1" rather than naming missing metadata.
    pub fn version_line_from(v: &str, c: &str, d: &str) -> String {
        let mut line = format!("jse {v}");
        let mut extra: Vec<String> = Vec::new();
        if c != "none" {
            extra.push(format!("commit {c}"));
        }
        if d != "unknown" {
            extra.push(format!("built {d}"));
        }
        if !extra.is_empty() {
            line += &format!(" ({})", extra.join(", "));
        }
        line
    }

    pub fn version_line() -> String {
        version_line_from(meta::VERSION, meta::COMMIT, meta::BUILD_DATE)
    }
}

mod progress {
    //! In-flight HTTP fetch progress for the UI's progress bar. The download
    //! loop in [`crate::fetch`] updates the state; the UI only reads it, so the
    //! fetch layer stays free of UI dependencies and the UI never touches the
    //! fetch machinery directly.

    use parking_lot::Mutex;

    /// FetchProgress snapshots the in-flight HTTP fetch for the UI's progress
    /// bar. The download loop updates it; the UI only reads it.
    #[derive(Clone, Debug)]
    pub struct FetchProgress {
        /// human-readable phase, e.g. "updating SchemaStore cache"
        pub label: String,
        /// bytes downloaded so far
        pub got: u64,
        /// Content-Length when the server sent one
        pub total: Option<u64>,
    }

    static PROGRESS: Mutex<Option<ProgressState>> = Mutex::new(None);

    #[derive(Clone, Debug)]
    struct ProgressState {
        label: String,
        got: u64,
        total: Option<u64>,
    }

    /// progress snapshots the running fetch, if any.
    pub fn progress() -> Option<FetchProgress> {
        PROGRESS.lock().as_ref().map(|s| FetchProgress {
            label: s.label.clone(),
            got: s.got,
            total: s.total,
        })
    }

    /// progress_label names the fetch phase for a location: SchemaStore URLs
    /// are the cache update, everything else is a schema download.
    fn progress_label(loc: &str) -> String {
        if loc.contains("schemastore.org") {
            "updating SchemaStore cache".to_string()
        } else {
            "fetching schema".to_string()
        }
    }

    /// progressBegin opens a fetch phase for `loc`; the state clears when the
    /// matching [`ProgressGuard`] drops.
    pub(crate) fn progress_begin(loc: &str) {
        *PROGRESS.lock() = Some(ProgressState {
            label: progress_label(loc),
            got: 0,
            total: None,
        });
    }

    pub(crate) fn progress_total(total: u64) {
        if let Some(s) = PROGRESS.lock().as_mut() {
            s.total = Some(total);
        }
    }

    pub(crate) fn progress_tick(got: u64) {
        if let Some(s) = PROGRESS.lock().as_mut() {
            s.got = got;
        }
    }

    pub(crate) fn progress_end() {
        *PROGRESS.lock() = None;
    }

    /// ProgressGuard clears the progress state on every exit path of a download.
    pub(crate) struct ProgressGuard;

    impl Drop for ProgressGuard {
        fn drop(&mut self) {
            progress_end();
        }
    }
}

#[cfg(feature = "gui")]
pub mod gui;

#[cfg(test)]
mod validate_tests;
// Build-time-only helper (included by build.rs); tests live in the file.
#[cfg(test)]
#[path = "vcsver.rs"]
mod vcsver;

#[cfg(test)]
mod main_test {
    use crate::cli::{Invocation, parse_args};
    use crate::doc::Parsed;
    use crate::paths::{abs_lexical, search_stem};
    use crate::session::resolve_schema_loc;
    use std::path::Path;

    use crate::cli::Flags;
    use crate::doc::codec_for;

    fn session_with_schema(cfg: &str, loc: &str) -> Parsed {
        let codec = codec_for(cfg);
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
        let got = resolve_schema_loc("", cfg, codec_for(cfg), &sess.doc, &sess.meta).unwrap();
        assert_eq!(got, config_dir.join("schema.json").to_str().unwrap());

        // falls back to cwd
        write_schema(&work_dir.join("fallback.json"));
        let sess = session_with_schema(cfg, "fallback.json");
        let got = resolve_schema_loc("", cfg, codec_for(cfg), &sess.doc, &sess.meta).unwrap();
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
            parse_args(&args)
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
            crate::version::version_line_from("v0.2.1", "b2b73a4", "2026-10-18T13:18:54Z"),
            "jse v0.2.1 (commit b2b73a4, built 2026-10-18T13:18:54Z)"
        );
        assert_eq!(
            crate::version::version_line_from("v0.2.1", "none", "unknown"),
            "jse v0.2.1"
        );
        assert_eq!(
            crate::version::version_line_from("dev", "63d1088", "unknown"),
            "jse dev (commit 63d1088)"
        );
        // build.rs already produces the short development form.
        assert_eq!(
            crate::version::version_line_from("v0.1.1-0", "01554d6", "2026-10-22T12:07:33Z"),
            "jse v0.1.1-0 (commit 01554d6, built 2026-10-22T12:07:33Z)"
        );
        assert_eq!(
            crate::version::version_line_from("v0.1.1-0+dirty", "01554d6", "2026-10-22T12:07:33Z"),
            "jse v0.1.1-0+dirty (commit 01554d6, built 2026-10-22T12:07:33Z)"
        );
        assert_eq!(
            crate::version::version_line_from("v1.2.3-rc.1", "none", "unknown"),
            "jse v1.2.3-rc.1"
        );
    }
}
