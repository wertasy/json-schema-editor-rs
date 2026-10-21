//! `--version` output (Go cmd/jse: versionLine / withBuildInfo).
//!
//! Build metadata comes from build.rs (the ldflags/BuildInfo equivalent).

/// Constants injected by build.rs into $OUT_DIR/version_info.rs: the
/// ldflags-equivalent version stamp plus the VCS revision and build time
/// (sentinels "none"/"unknown" when built outside a git checkout).
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
