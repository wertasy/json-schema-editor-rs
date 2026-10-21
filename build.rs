//! Build metadata injection — the Rust equivalent of Go's `-ldflags -X` plus
//! `debug.ReadBuildInfo`:
//!
//! - `VERSION`: `JSE_VERSION_OVERRIDE` from the environment wins (release
//!   workflows set it, mirroring the ldflags stamp); otherwise the crate
//!   version is used, rendered with the conventional `v` prefix.
//! - `COMMIT` / `BUILD_DATE`: `JSE_COMMIT` / `JSE_BUILD_DATE` win (release
//!   workflows set them); otherwise stamped from the git checkout a VCS build
//!   is made in (7-char revision, RFC3339 UTC build time); a build without git
//!   metadata behaves like an installed Go build and reports neither, so
//!   `--version` reads just `jse v0.1.0` — see version.rs.
//!
//! The values are written to `$OUT_DIR/version_info.rs` and included by
//! `src/version.rs`.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-env-changed=JSE_VERSION_OVERRIDE");
    println!("cargo:rerun-if-env-changed=JSE_COMMIT");
    println!("cargo:rerun-if-env-changed=JSE_BUILD_DATE");
    println!("cargo:rerun-if-changed=.git/HEAD");

    let version = env_nonempty("JSE_VERSION_OVERRIDE").unwrap_or_else(|| {
        format!(
            "v{}",
            std::env::var("CARGO_PKG_VERSION").unwrap_or_default()
        )
    });

    // A VCS checkout stamps revision and build time; the release workflow
    // stamps both explicitly. Anything else (a release tarball, an installed
    // crate) reports "none"/"unknown" like Go's `go install pkg@version`
    // builds do.
    let head = head_commit();
    let commit = env_nonempty("JSE_COMMIT")
        .or_else(|| head.clone())
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
    if let Err(e) = std::fs::write(&dst, body) {
        panic!("build.rs: write {}: {e}", dst.display());
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn head_commit() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.len() < 7 {
        return None;
    }
    Some(s[..7].to_string())
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
