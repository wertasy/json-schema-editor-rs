//! Persistent `:` command history: one executed command per line, oldest
//! first, capped at [`MAX_CMD_HISTORY`]. The file lives under the jse cache
//! dir; session.rs wires the path through
//! [`Options::cmd_history_file`](crate::ui::Options), so `JSE_CACHE_DIR`
//! relocates it and tests can isolate it. An absent or unreadable file
//! simply means an empty history, and a failed write only loses
//! persistence, never the session history.
//!
//! Session-only in Go; the persistence is a jse extension.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ui::keys::MAX_CMD_HISTORY;

/// load reads the history file, oldest first. Blank lines are skipped and
/// only the newest [`MAX_CMD_HISTORY`] entries survive; any read problem
/// yields an empty history.
pub fn load(file: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    if lines.len() > MAX_CMD_HISTORY {
        lines.drain(..lines.len() - MAX_CMD_HISTORY);
    }
    lines
}

/// store rewrites the history file via temp file + rename (concurrent jse
/// instances), best-effort like the fetch cache: any failure leaves the
/// previous file untouched.
pub fn store(file: &Path, entries: &[String]) {
    let Some(dir) = file.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut text = entries.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrips_through_load_and_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("cmd-history");
        store(&file, &["r !echo one".to_string(), "w".to_string()]);
        assert_eq!(load(&file), vec!["r !echo one", "w"]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "r !echo one\nw\n");

        store(&file, &[]);
        assert_eq!(load(&file), Vec::<String>::new());
    }

    #[test]
    fn load_skips_blank_lines_and_caps_to_the_newest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cmd-history");
        let mut text = String::new();
        for i in 0..(MAX_CMD_HISTORY + 5) {
            text.push_str(&format!("!echo {i}\n"));
            if i % 10 == 0 {
                text.push('\n');
            }
        }
        text.push_str("   \n");
        std::fs::write(&file, text).unwrap();

        let lines = load(&file);
        assert_eq!(lines.len(), MAX_CMD_HISTORY);
        assert_eq!(lines[0], "!echo 5");
        assert_eq!(lines[MAX_CMD_HISTORY - 1], format!("!echo {}", MAX_CMD_HISTORY + 4));
        assert!(lines.iter().all(|l| !l.trim().is_empty()));
    }

    #[test]
    fn load_of_a_missing_or_unreadable_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("absent")), Vec::<String>::new());
        // A directory reads as EISDIR, not as history.
        assert_eq!(load(&dir.path()), Vec::<String>::new());
    }
}
