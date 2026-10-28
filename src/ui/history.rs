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
/// is byte-identical to the Go status-line text either way. `paths` lists
/// the locations that differ between the pre- and post-state, in document
/// order; every path resolves in the new document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryResult {
    pub changed: bool,
    pub notice: &'static str,
    pub paths: Vec<Vec<crate::doc::jsondoc::PathSeg>>,
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
                paths: Vec::new(),
            };
        };
        let cur = std::mem::replace(doc, pre);
        let paths = diff_paths(&cur, doc);
        self.redo_stack.push(cur);
        HistoryResult {
            changed: true,
            notice: "undone",
            paths,
        }
    }

    /// redo re-applies the most recently undone change.
    pub fn redo(&mut self, doc: &mut Doc) -> HistoryResult {
        let Some(next) = self.redo_stack.pop() else {
            return HistoryResult {
                changed: false,
                notice: "nothing to redo",
                paths: Vec::new(),
            };
        };
        let cur = std::mem::replace(doc, next);
        let paths = diff_paths(&cur, doc);
        self.undo_stack.push(cur);
        HistoryResult {
            changed: true,
            notice: "redone",
            paths,
        }
    }
}

/// diffPaths walks two documents of the same clone lineage and reports the
/// paths of every location whose content differs, plus paths that only
/// exist on the `to` side (additions). Removals are not reported: their
/// path no longer resolves in `to`, so it cannot be jumped to. Order is
/// `to`'s document order; paths never repeat.
pub fn diff_paths(from: &Doc, to: &Doc) -> Vec<Vec<crate::doc::jsondoc::PathSeg>> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    diff_node(from, from.root, to, to.root, &mut path, &mut out);
    out
}

fn diff_node(
    from: &Doc,
    fi: crate::doc::jsondoc::NodeId,
    to: &Doc,
    ti: crate::doc::jsondoc::NodeId,
    path: &mut Vec<crate::doc::jsondoc::PathSeg>,
    out: &mut Vec<Vec<crate::doc::jsondoc::PathSeg>>,
) {
    use crate::doc::jsondoc::PathSeg as Seg;
    use crate::doc::jsondoc::Value;
    let (fv, tv) = (from.value(fi), to.value(ti));
    match (fv, tv) {
        (Value::Object(fm), Value::Object(tm)) => {
            for m in tm {
                path.push(Seg::Key(m.key.clone()));
                match fm.iter().find(|f| f.key == m.key) {
                    Some(f) => diff_node(from, f.val, to, m.val, path, out),
                    None => out.push(path.clone()), // added on the `to` side
                }
                path.pop();
            }
        }
        (Value::Array(fa), Value::Array(ta)) => {
            for (i, &t) in ta.iter().enumerate() {
                path.push(Seg::Idx(i));
                match fa.get(i) {
                    Some(&f) => diff_node(from, f, to, t, path, out),
                    None => out.push(path.clone()),
                }
                path.pop();
            }
        }
        _ => {
            // leaves (and kind flips) compare content: value spelling plus
            // the anchor/merge/raw metadata carried beside it
            let (df, dt) = (from.node(fi), to.node(ti));
            if df.value != dt.value
                || df.anchor != dt.anchor
                || df.merge != dt.merge
                || df.raw != dt.raw
            {
                out.push(path.clone());
            }
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

    fn ks(s: &str) -> crate::doc::jsondoc::PathSeg {
        crate::doc::jsondoc::PathSeg::key(s)
    }

    #[test]
    fn diff_paths_reports_changes_and_additions_not_removals() {
        use crate::doc::jsondoc::PathSeg;
        let from = doc(r#"{"a":1,"b":{"x":1},"c":[1,2],"z":9}"#);
        let to = doc(r#"{"a":2,"b":{"x":1,"y":2},"c":[1,9]}"#);
        assert_eq!(
            diff_paths(&from, &to),
            vec![
                vec![ks("a")],                  // changed leaf
                vec![ks("b"), ks("y")],         // added key
                vec![ks("c"), PathSeg::Idx(1)], // changed element
            ]
        );
        // removals resolve nowhere in the new doc: not reported
        let from = doc(r#"{"a":1,"z":9}"#);
        let to = doc(r#"{"a":1}"#);
        assert!(diff_paths(&from, &to).is_empty());
        // identical docs produce nothing
        let d = doc(r#"{"a":{"b":[1,{"c":2}]}}"#);
        assert!(diff_paths(&d, &d).is_empty());
        // kind flips report the location once
        let from = doc(r#"{"a":{"b":1}}"#);
        let to = doc(r#"{"a":[1,2]}"#);
        assert_eq!(diff_paths(&from, &to), vec![vec![ks("a")]]);
    }

    #[test]
    fn undo_redo_paths_describe_the_applied_direction() {
        let mut h = History::default();
        let mut d = doc(r#"{"a":1,"b":2}"#);
        // snapshot of the state the undo will restore
        push(&mut h, r#"{"a":9,"b":2,"c":3}"#);
        let r = h.undo(&mut d);
        // undo applied {a:9,b:2,c:3}: a reverted to 9, the c addition came
        // back; b was never touched
        assert_eq!(r.paths, vec![vec![ks("a")], vec![ks("c")]]);
        let r = h.redo(&mut d);
        // redo re-applied the edit: a back to 1, the c addition disappeared
        // (removals are not reported — the path no longer resolves)
        assert_eq!(r.paths, vec![vec![ks("a")]]);
    }
}
