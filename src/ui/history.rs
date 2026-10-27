//! Document history for the menu TUI: the key dispatcher snapshots the
//! document before each key and pushes the snapshot when the key changed
//! it, so one undo step reverts exactly one key's worth of edits.
//!
//! Rust port of `internal/ui/menu_history.go`. Go hangs these on the
//! `*listModel` receiver; here the stacks live in [`History`] and the
//! predicates are free functions over an [`Inputs`] snapshot of the
//! overlay flags, because the per-file state (`FileState`, src/ui/files.rs)
//! is owned by the menu layer.

use crate::doc::jsondoc::Doc;

/// maxHistory caps each history stack; the oldest snapshot is dropped first.
pub const MAX_HISTORY: usize = 100;

/// Snapshot pair for one file: the document states a `u` / `ctrl+r` walk
/// through. Go: `undoStack`/`redoStack` on the listModel.
#[derive(Default)]
pub struct History {
    pub undo_stack: Vec<Doc>,
    pub redo_stack: Vec<Doc>,
}

/// Outcome of an undo/redo attempt. `changed` is true when the document was
/// swapped (the caller must then `repair_stack` and refresh rows); `notice`
/// is byte-identical to the Go status-line text either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryResult {
    pub changed: bool,
    pub notice: &'static str,
}

impl History {
    /// pushUndo records `pre` (the document before an edit) and drops the
    /// oldest entry beyond the cap. A new edit forks the timeline: the redo
    /// branch dies.
    pub fn push_undo(&mut self, pre: Doc) {
        self.undo_stack.push(pre);
        if self.undo_stack.len() > MAX_HISTORY {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    /// undo restores the document as it was before the last change, keeping
    /// the current state for redo. On success `*doc` is replaced with the
    /// restored snapshot (clone preserves alias sharing).
    pub fn undo(&mut self, doc: &mut Doc) -> HistoryResult {
        let Some(pre) = self.undo_stack.pop() else {
            return HistoryResult {
                changed: false,
                notice: "nothing to undo",
            };
        };
        let cur = std::mem::replace(doc, pre);
        self.redo_stack.push(cur);
        HistoryResult {
            changed: true,
            notice: "undone",
        }
    }

    /// redo re-applies the most recently undone change.
    pub fn redo(&mut self, doc: &mut Doc) -> HistoryResult {
        let Some(next) = self.redo_stack.pop() else {
            return HistoryResult {
                changed: false,
                notice: "nothing to redo",
            };
        };
        let cur = std::mem::replace(doc, next);
        self.undo_stack.push(cur);
        HistoryResult {
            changed: true,
            notice: "redone",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::jsondoc::parse;

    fn doc(src: &str) -> Doc {
        parse(src).expect("test json")
    }

    fn push(h: &mut History, src: &str) {
        h.push_undo(doc(src));
    }

    #[test]
    fn undo_redo_notices() {
        let mut h = History::default();
        let mut d = doc(r#"{"a":1}"#);
        assert_eq!(h.undo(&mut d).notice, "nothing to undo");
        assert_eq!(h.redo(&mut d).notice, "nothing to redo");

        push(&mut h, r#"{"a":1}"#);
        let r = h.undo(&mut d);
        assert!(r.changed);
        assert_eq!(r.notice, "undone");
        let r = h.redo(&mut d);
        assert!(r.changed);
        assert_eq!(r.notice, "redone");
        let r = h.redo(&mut d);
        assert!(!r.changed);
        assert_eq!(r.notice, "nothing to redo");
    }

    #[test]
    fn undo_restores_previous_document() {
        let mut h = History::default();
        let mut d = doc(r#"{"a":2}"#);
        push(&mut h, r#"{"a":1}"#);
        h.undo(&mut d);
        assert_eq!(d.write(), "{\n  \"a\": 1\n}\n");
        h.redo(&mut d);
        assert_eq!(d.write(), "{\n  \"a\": 2\n}\n");
    }

    #[test]
    fn new_edit_kills_redo_branch() {
        let mut h = History::default();
        let mut d = doc("{}");
        push(&mut h, r#"{"a":1}"#);
        h.undo(&mut d);
        assert!(h.redo(&mut d).changed);
        h.undo(&mut d);
        // a fresh edit after undo drops the redo branch
        push(&mut h, r#"{"b":2}"#);
        assert!(h.redo_stack.is_empty());
        assert_eq!(h.redo(&mut d).notice, "nothing to redo");
    }

    #[test]
    fn undo_stack_capped_at_100() {
        let mut h = History::default();
        for i in 0..105 {
            push(&mut h, &format!(r#"{{"n":{i}}}"#));
        }
        assert_eq!(h.undo_stack.len(), MAX_HISTORY);
        // the 5 oldest are gone: first undo restores n=104, last restores n=5
        let mut d = doc(r#"{"n":105}"#);
        h.undo(&mut d);
        assert_eq!(d.write(), "{\n  \"n\": 104\n}\n");
        for _ in 0..99 {
            h.undo(&mut d);
        }
        assert_eq!(d.write(), "{\n  \"n\": 5\n}\n");
        assert_eq!(h.undo(&mut d).notice, "nothing to undo");
    }
}
