//! Port of `e2e/e2e_test.go`: the built `jse` binary driven end to end —
//! deterministic accessible-mode line protocols plus the full-screen TUI over
//! a real pty.
//!
//! Assertion wording follows the Rust binary where it is library-dependent
//! (validation error text); structure, exit codes, saved documents, key
//! order, comments/anchors/aliases and navigation behavior are asserted like
//! the Go suite.

#![allow(clippy::needless_range_loop)]

use common::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;
use yaml_rust2::{Yaml, YamlLoader};

mod common;

#[path = "e2e/accessible.rs"]
mod accessible;
#[path = "e2e/badges.rs"]
mod badges;
#[path = "e2e/cli.rs"]
mod cli;
#[path = "e2e/editing.rs"]
mod editing;
#[path = "e2e/files.rs"]
mod files;
#[path = "e2e/mouse.rs"]
mod mouse;
#[path = "e2e/schema.rs"]
mod schema;
#[path = "e2e/shell.rs"]
mod shell;
#[path = "e2e/tui.rs"]
mod tui;
#[path = "e2e/validation.rs"]
mod validation;
#[path = "e2e/yaml.rs"]
mod yaml;

const CREATE_SCRIPT: &str = concat!(
    "demo\n",                         // Name
    "2\n",                            // Mode → http
    "1\n",                            // Navigate → Server
    "8080\nlocalhost\ny\n\n\n",       // Port, Host, Debug=y, Ratio=omit, Done
    "demo\n\n2\n",                    // root rerun: Name, Mode=http, Navigate → Logging
    "2\n0\n",                         // Levels: toggle info, confirm
    "2\n",                            // Logging navigate → tags items
    "1\nprod\n6\n",                   // mgr: add item, value, done
    "0\n1\n",                         // Logging rerun: confirm levels, Done
    "demo\n\n3\n",                    // root: → Mounts
    "1\ndemos/ROOT\n/var/www\n\n6\n", // mgr: add mount {src,dst}, done
    "demo\n\n4\n",                    // root: → Auth
    "2\nsecret123\n\n",               // variant Token, token, done
    "demo\n\n5\n",                    // root: save
);

fn arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn mktemp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// The binary needs an isolated schema cache; every pty session gets one.
fn cache_env(cache: &tempfile::TempDir) -> (String, String) {
    (
        "JSE_CACHE_DIR".to_string(),
        cache.path().to_string_lossy().into_owned(),
    )
}

/// Start a pty session or skip (return false) when no pty is available,
/// mirroring Go's `t.Skipf("no pty available")`.
fn pty_or_skip(
    args: &[&str],
    extra_envs: &[(&str, &str)],
    cache: &tempfile::TempDir,
) -> Option<PtySession> {
    let (k, v) = cache_env(cache);
    let mut envs: Vec<(&str, &str)> = vec![(k.as_str(), v.as_str())];
    envs.extend_from_slice(extra_envs);
    match PtySession::spawn(repo_root(), args, &envs) {
        PtySpawn::Ok(s) => Some(s),
        PtySpawn::Unavailable(e) => {
            eprintln!("SKIP: no pty available: {e}");
            None
        }
    }
}

/// pty_or_skip_sized is pty_or_skip on a terminal of the given size.
fn pty_or_skip_sized(
    args: &[&str],
    extra_envs: &[(&str, &str)],
    cache: &tempfile::TempDir,
    cols: u16,
    rows: u16,
) -> Option<PtySession> {
    let (k, v) = cache_env(cache);
    let mut envs: Vec<(&str, &str)> = vec![(k.as_str(), v.as_str())];
    envs.extend_from_slice(extra_envs);
    match PtySession::spawn_sized(repo_root(), args, &envs, cols, rows) {
        PtySpawn::Ok(s) => Some(s),
        PtySpawn::Unavailable(e) => {
            eprintln!("SKIP: no pty available: {e}");
            None
        }
    }
}

fn yaml_doc(text: &str) -> Yaml {
    let docs = YamlLoader::load_from_str(text)
        .unwrap_or_else(|e| panic!("saved YAML parses: {e}\n{text}"));
    docs.into_iter().next().expect("one YAML document")
}

fn yaml_get<'a>(node: &'a Yaml, key: &str) -> Option<&'a Yaml> {
    node.as_hash()?.get(&Yaml::String(key.to_string()))
}

const MINI_SCHEMA: &str =
    r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}}}"#;

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .expect("editor metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod editor");
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) {
    panic!("the e2e suite drives a POSIX shell editor and a pty");
}
