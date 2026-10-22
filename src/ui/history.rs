#![allow(dead_code)] // wired into the menu in later phases

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
use crate::ui::{MenuState, Row};

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

/// The overlay/inputs flags `typing` and `list_mode` need. The menu layer
/// fills this from the active `FileState` (Go read the listModel fields
/// directly); `Overlay`-based callers derive it from the variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub edit: bool,
    pub text: bool,
    pub search_active: bool,
    pub add: bool,
    pub write: bool,
    pub anc: bool,
    pub mrg: bool,
    pub confirm: bool,
    pub issues: bool,
    pub errs: bool,
    pub sel: bool,
    pub help: bool,
    pub file_pick: bool,
    /// oneOf picker search mode (no overlay, but text input rules)
    pub variant_search: bool,
    /// f/F fast move waits for a target character: u/q/@/. are characters
    pub fast_move: bool,
}

/// typing reports whether a text input has focus (in-place editor, raw
/// textarea, search, add-key input, anchor/merge inputs): q/@/. are ordinary
/// characters there and history keys must not fire.
pub fn typing(i: &Inputs) -> bool {
    i.edit
        || i.text
        || i.search_active
        || i.add
        || i.write
        || i.anc
        || i.mrg
        || i.variant_search
        || i.fast_move
}

/// list_mode reports whether the plain menu key table owns the keyboard.
pub fn list_mode(i: &Inputs) -> bool {
    !i.confirm && !i.issues && !i.errs && !i.sel && !i.help && !typing(i) && !i.file_pick
}

/// repairStack drops navigation levels whose path no longer resolves in the
/// restored document (undo can shrink an array or remove a container the
/// user is inside) and clamps the cursor of the surviving top level to the
/// rows `rows_for` lists for it. The root level (empty path) always stays.
pub fn repair_stack(
    stack: &mut Vec<MenuState>,
    doc: &Doc,
    rows_for: impl Fn(&MenuState) -> Vec<Row>,
) {
    while stack.len() > 1 {
        let path = stack.last().expect("len > 1").path.clone();
        if path.is_empty() {
            break;
        }
        if doc.get(&path).is_some() {
            break;
        }
        stack.pop();
    }
    // clampCursor(menuRows(m)): keep the top cursor inside its row list.
    if let Some(top) = stack.last_mut() {
        let max = rows_for(top).len().saturating_sub(1);
        top.cursor = top.cursor.min(max);
    }
}

/// applyHistoryKey handles `u` (undo) and `ctrl+r` (redo). Returns
/// `Some(result)` when the key was a history key (whether or not it changed
/// anything); `None` lets the caller route the key elsewhere.
pub fn apply_history_key(h: &mut History, doc: &mut Doc, key: &str) -> Option<HistoryResult> {
    let res = match key {
        "u" => h.undo(doc),
        "ctrl+r" => h.redo(doc),
        _ => return None,
    };
    Some(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::jsondoc::parse;

    fn doc(src: &str) -> Doc {
        parse(src).expect("test json")
    }

    fn plain_rows(_st: &MenuState) -> Vec<Row> {
        vec![Row::default(), Row::default(), Row::default()]
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

    fn st(path: &[&str], cursor: usize) -> MenuState {
        MenuState {
            kind: crate::ui::MenuKind::Object,
            node: 0,
            path: path
                .iter()
                .map(|k| crate::doc::PathSeg::Key((*k).into()))
                .collect(),
            cursor,
            offset: 0,
            variant_of: None,
            one_of: None,
            sel: Default::default(),
            visual: false,
            vanchor: 0,
            search_ti: Default::default(),
            searching: false,
            search_prefilled: false,
            search_applied: false,
        }
    }

    fn top_path(stack: &[MenuState]) -> String {
        crate::doc::jsondoc::path_string(&stack.last().expect("survivor").path)
    }

    #[test]
    fn repair_stack_drops_dead_levels_and_clamps_cursor() {
        // doc only has $.a left; stack digs into $.a.b — the dead level above
        // $.a must go, and the $.a cursor clamps to 3 rows.
        let d = doc(r#"{"a":{"x":1}}"#);
        let mut stack = vec![st(&[], 0), st(&["a"], 9), st(&["a", "b"], 1)];
        repair_stack(&mut stack, &d, plain_rows);
        assert_eq!(stack.len(), 2);
        assert_eq!(top_path(&stack), "$.a");
        assert_eq!(stack.last().expect("survivor").cursor, 2);
    }

    #[test]
    fn repair_stack_keeps_root_alive() {
        // even a single root level with an out-of-range cursor survives:
        // the root level (empty path) always stays, only the cursor clamps.
        let d = doc("{}");
        let mut stack = vec![st(&[], 42)];
        repair_stack(&mut stack, &d, |_st| vec![Row::default()]);
        assert_eq!(stack.len(), 1);
        assert_eq!(stack[0].cursor, 0);
    }

    #[test]
    fn typing_and_list_mode() {
        let base = Inputs::default();
        assert!(list_mode(&base));
        assert!(!typing(&base));

        let mut i = base;
        i.search_active = true;
        assert!(typing(&i));
        assert!(!list_mode(&i));

        for flag in [
            "edit",
            "text",
            "add",
            "write",
            "anc",
            "mrg",
            "variant_search",
        ] {
            let mut i = Inputs::default();
            match flag {
                "edit" => i.edit = true,
                "text" => i.text = true,
                "add" => i.add = true,
                "write" => i.write = true,
                "anc" => i.anc = true,
                "mrg" => i.mrg = true,
                "variant_search" => i.variant_search = true,
                "fast_move" => i.fast_move = true,
                _ => unreachable!(),
            }
            assert!(typing(&i), "{flag} must be typing");
            assert!(!list_mode(&i));
        }

        for flag in ["confirm", "issues", "errs", "sel", "help", "file_pick"] {
            let mut i = Inputs::default();
            match flag {
                "confirm" => i.confirm = true,
                "issues" => i.issues = true,
                "errs" => i.errs = true,
                "sel" => i.sel = true,
                "help" => i.help = true,
                "file_pick" => i.file_pick = true,
                _ => unreachable!(),
            }
            assert!(!typing(&i), "{flag} is not a text input");
            assert!(!list_mode(&i));
        }
    }

    #[test]
    fn apply_history_key_dispatch() {
        let mut h = History::default();
        let mut d = doc("{}");
        push(&mut h, r#"{"a":1}"#);
        assert!(apply_history_key(&mut h, &mut d, "x").is_none());
        assert_eq!(
            apply_history_key(&mut h, &mut d, "u")
                .expect("u consumed")
                .notice,
            "undone"
        );
        assert_eq!(d.write(), "{\n  \"a\": 1\n}\n");
        assert_eq!(
            apply_history_key(&mut h, &mut d, "ctrl+r")
                .expect("ctrl+r consumed")
                .notice,
            "redone"
        );
        assert_eq!(d.write(), "{}\n");
    }
}
