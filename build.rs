//! Build metadata injection — the Rust equivalent of Go's `-ldflags -X` plus
//! `debug.ReadBuildInfo`:
//!
//! - `VERSION`: `JSE_VERSION_OVERRIDE` from the environment wins (release
//!   workflows set it, mirroring the ldflags stamp). Otherwise it follows
//!   `cmd/go`'s build-info derivation: a tag on HEAD is used verbatim, a
//!   commit past a tag becomes a Go pseudo-version
//!   (`v0.3.1-0.<UTC commit time>-<12-char revision>`), and a repository
//!   without tags yields `v0.0.0-...`. Uncommitted changes append Go's semver
//!   build metadata `+dirty` (or `.dirty` after `+incompatible`). Outside a
//!   git checkout the crate version is used, like an installed module version.
//! - `COMMIT` / `BUILD_DATE`: `JSE_COMMIT` / `JSE_BUILD_DATE` win (release
//!   workflows set them); otherwise stamped from the git checkout a VCS build
//!   is made in (7-char revision, RFC3339 UTC build time); a build without git
//!   metadata behaves like an installed Go build and reports neither, so
//!   `--version` reads just `jse v0.1.0` — see version.rs.
//!
//! The values are written to `$OUT_DIR/version_info.rs` and included by
//! `src/version.rs`.
//!
//! Cargo reruns a build script only for its declared `rerun-if-changed`
//! paths. Dirty state, HEAD, tags, and the index are therefore tracked
//! explicitly: every tracked file, its top-level directory, the resolved refs,
//! and the git index are declared, so editing a source file or tagging a
//! commit updates the next `--version` instead of leaving a stale stamp.

mod vcsver {
    include!("src/vcsver.rs");
}

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-env-changed=JSE_VERSION_OVERRIDE");
    println!("cargo:rerun-if-env-changed=JSE_COMMIT");
    println!("cargo:rerun-if-env-changed=JSE_BUILD_DATE");
    emit_rerun_triggers();

    let head = head_revision();

    let version = env_nonempty("JSE_VERSION_OVERRIDE").unwrap_or_else(|| {
        let mut version = head.as_deref().and_then(git_version).unwrap_or_else(|| {
            format!(
                "v{}",
                std::env::var("CARGO_PKG_VERSION").unwrap_or_default()
            )
        });
        // Go's build-info stamping appends dirty metadata to the VCS version.
        if head.is_some() && worktree_dirty() {
            if version.ends_with("+incompatible") {
                version.push_str(".dirty");
            } else {
                version.push_str("+dirty");
            }
        }
        version
    });

    // A VCS checkout stamps revision and build time; the release workflow
    // stamps both explicitly. Anything else (a release tarball, an installed
    // crate) reports "none"/"unknown" like Go's `go install pkg@version`
    // builds do.
    let commit = env_nonempty("JSE_COMMIT")
        .or_else(|| head.as_deref().map(short_revision))
        .unwrap_or_else(|| "none".to_string());
    let build_date = env_nonempty("JSE_BUILD_DATE")
        .or_else(|| head.is_some().then(|| rfc3339_utc(now_unix())))
        .unwrap_or_else(|| "unknown".to_string());

    let out = std::env::var("OUT_DIR").unwrap_or_default();
    let dst = Path::new(&out).join("version_info.rs");
    let body = format!(
        "pub const VERSION: &str = {:?};\n\
         pub const COMMIT: &str = {:?};\n\
         pub const BUILD_DATE: &str = {:?};\n",
        version, commit, build_date,
    );
    // Rewriting identical content would change the OUT_DIR mtime and force a
    // crate rebuild, so only write when a stamp actually changed.
    if std::fs::read_to_string(&dst).ok().as_deref() != Some(body.as_str()) {
        if let Err(e) = std::fs::write(&dst, body) {
            panic!("build.rs: write {}: {e}", dst.display());
        }
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Declare the git state that the stamps depend on. Cargo stops scanning the
/// package once a build script prints `rerun-if-changed`, and none of these
/// paths are covered by editing `build.rs`.
fn emit_rerun_triggers() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Tracked files feed the dirty check (`git status --porcelain`).
    if let Some(files) = git_stdout(&["ls-files", "-z"]) {
        let files = String::from_utf8_lossy(&files);
        let mut dirs = std::collections::BTreeSet::new();
        for file in files.split('\0') {
            if file.is_empty() {
                continue;
            }
            println!("cargo:rerun-if-changed={file}");
            // Untracked files also count as dirty (Go's vcs.modified), and a
            // directory watch catches newly created ones.
            if let Some((dir, _)) = file.split_once('/') {
                dirs.insert(dir);
            }
        }
        for dir in dirs {
            println!("cargo:rerun-if-changed={dir}");
        }
    }
    // HEAD, the index, and the ref tree (branches/tags) move without touching
    // tracked files; tags also feed the version derivation.
    for name in ["HEAD", "index", "refs", "packed-refs"] {
        if let Some(path) = git_path(name).filter(|p| Path::new(p).exists()) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

fn git_path(name: &str) -> Option<String> {
    let out = git_stdout(&["rev-parse", "--git-path", name])?;
    let path = String::from_utf8_lossy(&out).trim().to_string();
    (!path.is_empty()).then_some(path)
}

fn git_stdout(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn head_revision() -> Option<String> {
    let out = git_stdout(&["rev-parse", "HEAD"])?;
    let rev = String::from_utf8_lossy(&out).trim().to_string();
    (rev.len() >= 12).then_some(rev)
}

fn short_revision(rev: &str) -> String {
    rev[..7].to_string()
}

/// `cmd/go`'s build-info version for a commit (see `load/pkg.go` and
/// `modfetch/coderepo.go`): highest canonical tag on HEAD, else a
/// pseudo-version based on the highest reachable tag, else `v0.0.0-...`.
fn git_version(head: &str) -> Option<String> {
    let exact = tags(&["tag", "--points-at", head])
        .into_iter()
        .filter_map(|tag| {
            let version = vcsver::Version::parse(&tag)?;
            version.is_canonical(&tag).then_some(version)
        })
        .max_by(|a, b| a.compare(b));
    if let Some(version) = exact {
        return Some(version.canonical());
    }

    let base = tags(&["tag", "--merged", head])
        .into_iter()
        .filter_map(|tag| vcsver::Version::parse(&tag))
        .max_by(|a, b| a.compare(b));
    let timestamp = commit_timestamp(head).map(compact_utc)?;
    Some(vcsver::pseudo_version(
        base.as_ref(),
        &timestamp,
        &head[..12],
    ))
}

fn tags(args: &[&str]) -> Vec<String> {
    git_stdout(args)
        .map(|out| {
            String::from_utf8_lossy(&out)
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Committer time of `rev`, the clock Go uses for `vcs.time` and pseudo
/// versions.
fn commit_timestamp(rev: &str) -> Option<u64> {
    let out = git_stdout(&["log", "-1", "--format=%ct", rev])?;
    String::from_utf8_lossy(&out).trim().parse().ok()
}

/// Go's `vcs.modified` equivalent: `git status --porcelain` reports tracked
/// edits, staged changes, and untracked files (ignored files stay out).
fn worktree_dirty() -> bool {
    git_stdout(&["status", "--porcelain"])
        .map(|out| !String::from_utf8_lossy(&out).trim().is_empty())
        .unwrap_or(false)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// RFC3339 UTC, Go's vcs.time shape: "2026-10-18T13:18:54Z".
fn rfc3339_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z",
        h = rem / 3_600,
        m = (rem % 3_600) / 60,
        s = rem % 60,
    )
}

/// Go's `PseudoVersionTimestampFormat`: UTC "yyyymmddhhmmss".
fn compact_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{y:04}{mo:02}{d:02}{h:02}{m:02}{s:02}",
        h = rem / 3_600,
        m = (rem % 3_600) / 60,
        s = rem % 60,
    )
}

/// Howard Hinnant's civil_from_days: days since 1970-01-01 → (y, m, d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}
