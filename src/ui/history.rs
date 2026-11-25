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
/// order; removals report the removed location's own path, which may no
/// longer resolve in the new document.
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
/// exist on the `from` side (removals) and on the `to` side (additions).
/// A removal reports the removed member's / element's own path even though
/// it no longer resolves in `to`: schema menus still list the row (absent
/// members render), so undo can land the cursor back on the key; menus
/// without the row fall back to the container. Container anchor renames
/// are reported at the container itself. Changes and additions come in
/// `to` document order; a container's removals are emitted right after its
/// own `to`-side walk. Paths never repeat.
pub fn diff_paths(from: &Doc, to: &Doc) -> Vec<Vec<crate::doc::jsondoc::PathSeg>> {
    diff_kinds(from, to)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// DiffKind classifies one diff location: added on the `to` side, removed
/// on the `from` side, or present on both sides with different content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Added,
    Removed,
    Modified,
}

/// diffKinds is [`diff_paths`] with a classification per location, driving
/// the colored row badges (added / removed / modified).
pub fn diff_kinds(from: &Doc, to: &Doc) -> Vec<(Vec<crate::doc::jsondoc::PathSeg>, DiffKind)> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    diff_node(from, from.root, to, to.root, &mut path, &mut out);
    out
}

/// ArrayOp is one aligned step between two array element lists.
enum ArrayOp {
    /// equal content on both sides: nothing to report
    Equal,
    /// same slot, different content: compare the pair (single leaf path);
    /// the first field is the (to-side) slot index for the reported path
    Replace(
        usize,
        crate::doc::jsondoc::NodeId,
        crate::doc::jsondoc::NodeId,
    ),
    /// exists only on the `from` side: report the (dangling) from index
    Remove(usize),
    /// exists only on the `to` side: report the to index
    Add(usize),
}

/// elem_eq reports whether two subtrees carry identical content: value
/// spelling plus the anchor/merge/raw metadata, recursively. This is
/// exactly what [`diff_node`] compares, so matched pairs are silent.
/// Containers compare member keys / element content — their `Value`
/// equality is arena-id based and meaningless across shifts.
fn elem_eq(
    from: &Doc,
    fi: crate::doc::jsondoc::NodeId,
    to: &Doc,
    ti: crate::doc::jsondoc::NodeId,
) -> bool {
    use crate::doc::jsondoc::Value;
    let (df, dt) = (from.node(fi), to.node(ti));
    if df.anchor != dt.anchor || df.merge != dt.merge || df.raw != dt.raw {
        return false;
    }
    match (&df.value, &dt.value) {
        (Value::Object(fm), Value::Object(tm)) => {
            fm.len() == tm.len()
                && fm
                    .iter()
                    .zip(tm.iter())
                    .all(|(f, t)| f.key == t.key && elem_eq(from, f.val, to, t.val))
        }
        (Value::Array(fa), Value::Array(ta)) => {
            fa.len() == ta.len()
                && fa
                    .iter()
                    .zip(ta.iter())
                    .all(|(&f, &t)| elem_eq(from, f, to, t))
        }
        _ => df.value == dt.value,
    }
}

/// array_ops aligns two element lists. Common prefixes/suffixes are matched
/// without any diffing; the (trimmed) middle gets an LCS table, so an
/// insertion reports only the inserted slot while matched followers stay
/// silent. Consecutive remove+add pairs of the same run fold into Replace,
/// which compares the pair and reports one changed leaf instead of two
/// dangling paths. Very large middles fall back to index zipping (the old
/// behavior) to bound the DP table.
fn array_ops(
    from: &Doc,
    fa: &[crate::doc::jsondoc::NodeId],
    to: &Doc,
    ta: &[crate::doc::jsondoc::NodeId],
) -> Vec<ArrayOp> {
    enum Step {
        Equal,
        Remove(usize),
        Add(usize),
    }
    let mut steps: Vec<Step> = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    while i < fa.len() && j < ta.len() && elem_eq(from, fa[i], to, ta[j]) {
        steps.push(Step::Equal);
        i += 1;
        j += 1;
    }
    let mut f_tail = fa.len();
    let mut t_tail = ta.len();
    while f_tail > i && t_tail > j && elem_eq(from, fa[f_tail - 1], to, ta[t_tail - 1]) {
        f_tail -= 1;
        t_tail -= 1;
    }
    let fm = &fa[i..f_tail];
    let tm = &ta[j..t_tail];
    if fm.is_empty() || tm.is_empty() || fm.len() * tm.len() > 1_000_000 {
        // nothing shared in the middle (or too large to diff): zip the
        // slots, report the extra tail as removals / additions
        let shared = fm.len().min(tm.len());
        for k in 0..shared {
            if elem_eq(from, fm[k], to, tm[k]) {
                steps.push(Step::Equal);
            } else {
                steps.push(Step::Remove(i + k));
                steps.push(Step::Add(j + k));
            }
        }
        for k in shared..fm.len() {
            steps.push(Step::Remove(i + k));
        }
        for k in shared..tm.len() {
            steps.push(Step::Add(j + k));
        }
    } else {
        // LCS table over the trimmed middle
        let mut lcs = vec![0u32; (fm.len() + 1) * (tm.len() + 1)];
        let at = |r: usize, c: usize| r * (tm.len() + 1) + c;
        for r in (0..fm.len()).rev() {
            for c in (0..tm.len()).rev() {
                lcs[at(r, c)] = if elem_eq(from, fm[r], to, tm[c]) {
                    lcs[at(r + 1, c + 1)] + 1
                } else {
                    lcs[at(r + 1, c)].max(lcs[at(r, c + 1)])
                };
            }
        }
        let (mut r, mut c) = (0usize, 0usize);
        while r < fm.len() && c < tm.len() {
            if elem_eq(from, fm[r], to, tm[c]) {
                steps.push(Step::Equal);
                r += 1;
                c += 1;
            } else if lcs[at(r + 1, c)] >= lcs[at(r, c + 1)] {
                steps.push(Step::Remove(i + r));
                r += 1;
            } else {
                steps.push(Step::Add(j + c));
                c += 1;
            }
        }
        while r < fm.len() {
            steps.push(Step::Remove(i + r));
            r += 1;
        }
        while c < tm.len() {
            steps.push(Step::Add(j + c));
            c += 1;
        }
    }
    // the matched suffix closes the walk
    let t_suffix = t_tail;
    let suffix_len = fa.len() - f_tail;
    for _ in 0..suffix_len {
        steps.push(Step::Equal);
    }
    let _ = t_suffix;

    // fold remove+add runs into Replace pairs (a replaced slot reports one
    // changed location, not two dangling ones)
    let mut out = Vec::with_capacity(steps.len());
    let mut k = 0usize;
    while k < steps.len() {
        match steps[k] {
            Step::Equal => {
                out.push(ArrayOp::Equal);
                k += 1;
            }
            Step::Add(a) => {
                out.push(ArrayOp::Add(a));
                k += 1;
            }
            Step::Remove(f) => {
                let mut removes = vec![f];
                k += 1;
                while k < steps.len() {
                    if let Step::Remove(f2) = steps[k] {
                        removes.push(f2);
                        k += 1;
                    } else {
                        break;
                    }
                }
                let mut adds = Vec::new();
                while k < steps.len() {
                    if let Step::Add(a2) = steps[k] {
                        adds.push(a2);
                        k += 1;
                    } else {
                        break;
                    }
                }
                let paired = removes.len().min(adds.len());
                for p in 0..paired {
                    out.push(ArrayOp::Replace(adds[p], fa[removes[p]], ta[adds[p]]));
                }
                for f2 in &removes[paired..] {
                    out.push(ArrayOp::Remove(*f2));
                }
                for a2 in &adds[paired..] {
                    out.push(ArrayOp::Add(*a2));
                }
            }
        }
    }
    out
}

fn diff_node(
    from: &Doc,
    fi: crate::doc::jsondoc::NodeId,
    to: &Doc,
    ti: crate::doc::jsondoc::NodeId,
    path: &mut Vec<crate::doc::jsondoc::PathSeg>,
    out: &mut Vec<(Vec<crate::doc::jsondoc::PathSeg>, DiffKind)>,
) {
    use crate::doc::jsondoc::PathSeg as Seg;
    use crate::doc::jsondoc::Value;
    let (fv, tv) = (from.value(fi), to.value(ti));
    match (fv, tv) {
        (Value::Object(fm), Value::Object(tm)) => {
            // an anchor rename changes this container itself, not a child
            if from.node(fi).anchor != to.node(ti).anchor {
                out.push((path.clone(), DiffKind::Modified));
            }
            for m in tm {
                path.push(Seg::Key(m.key.clone()));
                match fm.iter().find(|f| f.key == m.key) {
                    Some(f) => diff_node(from, f.val, to, m.val, path, out),
                    None => out.push((path.clone(), DiffKind::Added)), // added on the `to` side
                }
                path.pop();
            }
            // keys removed on the `to` side report their own path (dangling
            // in `to`): menus that still list the row can land the cursor
            // back on the key, the rest fall back to this container
            for f in fm {
                if !tm.iter().any(|t| t.key == f.key) {
                    path.push(Seg::Key(f.key.clone()));
                    out.push((path.clone(), DiffKind::Removed));
                    path.pop();
                }
            }
        }
        (Value::Array(fa), Value::Array(ta)) => {
            // Align the two element lists by content (LCS): inserting or
            // removing an element must not flag every shifted follower as
            // changed — only the actually inserted / removed slots report,
            // matched pairs are silent.
            for op in array_ops(from, fa, to, ta) {
                match op {
                    ArrayOp::Equal => {}
                    ArrayOp::Replace(ti, fi, tn) => {
                        path.push(Seg::Idx(ti));
                        diff_node(from, fi, to, tn, path, out);
                        path.pop();
                    }
                    ArrayOp::Remove(fi) => {
                        path.push(Seg::Idx(fi));
                        out.push((path.clone(), DiffKind::Removed));
                        path.pop();
                    }
                    ArrayOp::Add(ti) => {
                        path.push(Seg::Idx(ti));
                        out.push((path.clone(), DiffKind::Added));
                        path.pop();
                    }
                }
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
                out.push((path.clone(), DiffKind::Modified));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::jsoncdoc::parse_strict;

    fn doc(src: &str) -> Doc {
        parse_strict(src).expect("test json")
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
    fn diff_paths_aligns_arrays_by_content() {
        use crate::doc::jsondoc::PathSeg;
        // an insertion reports only the inserted slot: matched followers
        // stay silent even though every index shifted
        let from = doc(r#"{"c":[1,2,3]}"#);
        let to = doc(r#"{"c":[1,2,9,3]}"#);
        assert_eq!(diff_paths(&from, &to), vec![vec![ks("c"), PathSeg::Idx(2)]]);
        // insertion at the head
        let to = doc(r#"{"c":[9,1,2,3]}"#);
        assert_eq!(diff_paths(&from, &to), vec![vec![ks("c"), PathSeg::Idx(0)]]);
        // a removal from the middle reports only the removed slot
        let from = doc(r#"{"c":[1,9,2]}"#);
        let to = doc(r#"{"c":[1,2]}"#);
        assert_eq!(diff_paths(&from, &to), vec![vec![ks("c"), PathSeg::Idx(1)]]);
        // same-length replacement still reports one changed slot
        let to = doc(r#"{"c":[1,9,3]}"#);
        assert_eq!(diff_paths(&from, &to), vec![vec![ks("c"), PathSeg::Idx(2)]]);
        // nested objects inside elements match by content too
        let from = doc(r#"{"c":[{"n":"a","v":1},{"n":"b","v":2}]}"#);
        let to = doc(r#"{"c":[{"n":"a","v":1},{"n":"x","v":2},{"n":"b","v":2}]}"#);
        assert_eq!(diff_paths(&from, &to), vec![vec![ks("c"), PathSeg::Idx(1)]]);
        // a truncated array still reports the removed tail indices
        let from = doc(r#"{"c":[1,2,3]}"#);
        let to = doc(r#"{"c":[1]}"#);
        assert_eq!(
            diff_paths(&from, &to),
            vec![
                vec![ks("c"), PathSeg::Idx(1)],
                vec![ks("c"), PathSeg::Idx(2)],
            ]
        );
    }

    #[test]
    fn diff_kinds_classifies_added_removed_modified() {
        use crate::doc::jsondoc::PathSeg;
        let from = doc(r#"{"a":1,"b":"keep","c":[1,2],"z":9}"#);
        let to = doc(r#"{"a":2,"b":"keep","c":[1,9,3],"d":{"y":2}}"#);
        let kinds = diff_kinds(&from, &to);
        assert!(kinds.contains(&(vec![ks("a")], DiffKind::Modified)));
        assert!(kinds.contains(&(vec![ks("c"), PathSeg::Idx(1)], DiffKind::Modified)));
        assert!(kinds.contains(&(vec![ks("c"), PathSeg::Idx(2)], DiffKind::Added)));
        assert!(kinds.contains(&(vec![ks("z")], DiffKind::Removed)));
        // `d` itself is new: one Added path for the whole subtree
        assert!(kinds.contains(&(vec![ks("d")], DiffKind::Added)));
        // matched array slots and untouched keys report nothing
        assert!(!kinds.iter().any(|(p, _)| p == &[ks("b")][..]));
        assert!(
            !kinds
                .iter()
                .any(|(p, _)| p == &[ks("c"), PathSeg::Idx(0)][..])
        );
    }

    #[test]
    fn diff_paths_reports_changes_additions_and_removed_paths() {
        use crate::doc::jsondoc::PathSeg;
        let from = doc(r#"{"a":1,"b":{"x":1},"c":[1,2],"z":9}"#);
        let to = doc(r#"{"a":2,"b":{"x":1,"y":2},"c":[1,9]}"#);
        assert_eq!(
            diff_paths(&from, &to),
            vec![
                vec![ks("a")],                  // changed leaf
                vec![ks("b"), ks("y")],         // added key
                vec![ks("c"), PathSeg::Idx(1)], // changed element
                vec![ks("z")],                  // removed member: its own path
            ]
        );
        // a removed member reports its own (dangling) path
        let from = doc(r#"{"a":1,"z":9}"#);
        let to = doc(r#"{"a":1}"#);
        assert_eq!(diff_paths(&from, &to), vec![vec![ks("z")]]);
        // several removed members each report their own path
        let from = doc(r#"{"x":{"y":1,"z":2}}"#);
        let to = doc(r#"{"x":{}}"#);
        assert_eq!(
            diff_paths(&from, &to),
            vec![vec![ks("x"), ks("y")], vec![ks("x"), ks("z")]]
        );
        // a truncated array reports the removed tail indices
        let from = doc(r#"{"c":[1,2,3]}"#);
        let to = doc(r#"{"c":[1]}"#);
        assert_eq!(
            diff_paths(&from, &to),
            vec![
                vec![ks("c"), PathSeg::Idx(1)], // changed element
                vec![ks("c"), PathSeg::Idx(2)], // removed tail index
            ]
        );
        // identical docs produce nothing
        let d = doc(r#"{"a":{"b":[1,{"c":2}]}}"#);
        assert!(diff_paths(&d, &d).is_empty());
        // kind flips report the location once
        let from = doc(r#"{"a":{"b":1}}"#);
        let to = doc(r#"{"a":[1,2]}"#);
        assert_eq!(diff_paths(&from, &to), vec![vec![ks("a")]]);
    }

    #[test]
    fn diff_paths_reports_container_anchor_changes() {
        // a mapping anchor rename changes the mapping itself, not a child
        let from = doc(r#"{"a":{"b":1}}"#);
        let mut to = doc(r#"{"a":{"b":1}}"#);
        let id = to.get(&[ks("a")]).expect("key a");
        to.node_mut(id).anchor = "mapping".into();
        assert_eq!(diff_paths(&from, &to), vec![vec![ks("a")]]);

        // an anchor rename beside a child change reports both locations
        let mut to = doc(r#"{"a":{"b":2}}"#);
        let id = to.get(&[ks("a")]).expect("key a");
        to.node_mut(id).anchor = "mapping".into();
        let from = doc(r#"{"a":{"b":1}}"#);
        assert_eq!(
            diff_paths(&from, &to),
            vec![vec![ks("a")], vec![ks("a"), ks("b")]]
        );
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
        // redo re-applied the edit: a back to 1; the removed c reports its
        // own (now dangling) path
        assert_eq!(r.paths, vec![vec![ks("a")], vec![ks("c")]]);
    }
}
