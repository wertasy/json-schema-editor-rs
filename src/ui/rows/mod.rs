//! Menuconfig row model: the pure logic behind the menu TUI — row lists per
//! menu state, value summaries, variant signatures and navigation-stack
//! reconstruction. No ratatui/crossterm imports here so it stays
//! unit-testable.
//!
//! Rust port of `internal/ui/menu_rows.go`. Go's shared `*smodel.FormNode`
//! pointers become [`FormNodeId`] arena indexes; the few places Go
//! synthesized throwaway schema nodes (map/array raw fallbacks, merged and
//! pinned oneOf menus) push real nodes into the [`Schema`] arena, memoized
//! in a [`SynthKey`] map so ids stay stable across calls the way Go's
//! pointers did.
//!
//! Deviation from the module contract: functions that only read the model
//! take the data they need directly (`&Schema`, `&Doc`) or a [`MenuCtx`];
//! the three constructors that may synthesize nodes
//! ([`menu_rows_for`], [`search_index`], [`rebuild_stack`]) take
//! `&mut Schema` because [`MenuCtx`] holds an immutable borrow.

use std::collections::{HashMap, HashSet};

use crate::doc::jsondoc::{parse_path, path_string};
use crate::doc::{Doc, NodeId, PathSeg, Value, append_idx, append_key};
use crate::icons::{ERR, WARN};
use crate::smodel::{FormNode, FormNodeId, Kind, Schema};
use crate::ui::state::{App, FileState, SynthKey, alias_at, anchor_holder_path, same_state};
use crate::ui::util::wrap_cursor;
use crate::ui::{Issue, MenuCtx, MenuKind, MenuState, Row, RowKind};

// --- merge hooks -----------------------------------------------------------

/// One key a mapping's merge pulls in. Go's `mergedKey`/`mergeSource` pair
/// (menu_merge.go) collapses to the data menu_rows needs: the key, its path
/// inside the anchor's own object, and the anchor name for the badge.
#[derive(Clone, Debug)]
pub struct MergedKey {
    pub key: String,
    pub path: Vec<PathSeg>,
    pub src_name: String,
}

/// The merge/alias lookups menu_rows makes into the merge layer
/// (menu_merge.go / menu_alias.go). Default implementations keep menus
/// merge-free; the menu wires the real implementation once those phases
/// land.
pub trait MergeView {
    /// anchor name when the node at path is a reference ("*name" occurrence)
    fn alias_at(&self, _path: &[PathSeg]) -> Option<String> {
        None
    }
    /// merged keys of the mapping at st.path in merge order
    fn merged_keys(&self, _st: &MenuState) -> Vec<MergedKey> {
        Vec::new()
    }
}

/// Merge-free default.
pub struct NoMerge;

impl MergeView for NoMerge {}

/// Per-call inputs Go read off `listModel` beyond the model itself.
pub struct RowsEnv<'a> {
    /// Go: `len(lm.stack) > 0 && lm.stack[0] == st`
    pub is_root: bool,
    pub issues_now: &'a [Issue],
    pub merge: &'a dyn MergeView,
    /// memo of synthesized schema nodes (stable ids across calls)
    pub synth: HashMap<SynthKey, FormNodeId>,
}

impl Default for RowsEnv<'_> {
    fn default() -> Self {
        RowsEnv {
            is_root: false,
            issues_now: &[],
            merge: &NoMerge,
            synth: HashMap::new(),
        }
    }
}

// --- helpers usually found on listModel ------------------------------------

fn path_eq(a: &[PathSeg], b: &[PathSeg]) -> bool {
    a == b
}

/// variantSearchMatch reports whether a variant row matches the picker's
/// search: case-insensitive substring over its title, key and signature.
fn variant_search_match(schema: &Schema, v: FormNodeId, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    let f = &schema.nodes[v as usize];
    f.title.to_lowercase().contains(&q)
        || f.key.to_lowercase().contains(&q)
        || variant_signature(schema, Some(v))
            .to_lowercase()
            .contains(&q)
}

pub(crate) fn is_null(doc: &Doc, id: NodeId) -> bool {
    matches!(doc.value(id), Value::Null)
}

/// is_null_opt is is_null over an optional node: no value counts as null.
pub(crate) fn is_null_opt(doc: &Doc, v: Option<NodeId>) -> bool {
    v.map_or(true, |v| is_null(doc, v))
}

/// selKey renders a path as the selection map key; the s:/i: tags keep
/// string keys and array indexes distinct.
pub(crate) fn sel_key(path: &[PathSeg]) -> String {
    let mut b = String::new();
    for seg in path {
        match seg {
            PathSeg::Key(s) => {
                b.push_str("s:");
                b.push_str(s);
            }
            PathSeg::Idx(i) => {
                b.push_str("i:");
                b.push_str(&i.to_string());
            }
        }
        b.push('\0');
    }
    b
}

/// selectableRow reports whether a row can carry a selection mark: object
/// menu key-value rows (fields, container rows, schema-less keys) and array
/// item rows (their label is "key[N]", as the item has no object key) — no
/// action or status rows.
pub(crate) fn selectable_row(r: &Row) -> bool {
    matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) && !r.path.is_empty()
}

/// findRowKey returns the key a row matches by: the schema key, or the
/// document key (last path segment) for schema-less rows. Only key-value
/// rows match — array items and action rows are skipped.
pub(crate) fn find_row_key(schema: &Schema, r: &Row) -> Option<String> {
    if !selectable_row(r) {
        return None;
    }
    if let Some(f) = r.fn_ {
        let key = &schema.nodes[f as usize].key;
        if !key.is_empty() {
            return Some(key.clone());
        }
    }
    match r.path.last() {
        Some(PathSeg::Key(k)) if !k.is_empty() => Some(k.clone()),
        _ => None,
    }
}

/// copyRowKey returns the row's object key: the last path segment when it is
/// a string. Array indexes and the root path have no object key.
pub(crate) fn copy_row_key(path: &[PathSeg]) -> Option<String> {
    match path.last() {
        Some(PathSeg::Key(k)) => Some(k.clone()),
        _ => None,
    }
}

/// docKeyAt renders the document position of a menu path as a key label.
pub(crate) fn doc_key_at(path: &[PathSeg]) -> String {
    match path.last() {
        Some(PathSeg::Key(s)) => s.clone(),
        Some(PathSeg::Idx(i)) => match path.get(path.len().saturating_sub(2)) {
            Some(PathSeg::Key(k)) => format!("{}[{}]", k, i),
            _ => format!("[{}]", i),
        },
        None => String::new(),
    }
}

/// pasteValueRowOK reports whether the cursor row is a value slot a pasted
/// single value can replace.
pub(crate) fn paste_value_row_ok(r: &Row) -> bool {
    match r.kind {
        // The root scalar document is a Field row with an empty path.
        RowKind::Field => true,
        RowKind::Unknown => !r.path.is_empty(),
        // A oneOf picker row still points at the field's value (or the root
        // document): P replaces the value and the variant is re-inferred.
        RowKind::Variant => true,
        RowKind::Drill => {
            // A oneOf/object/array key row is still one value slot: P with a
            // single value replaces that whole value (the variant is inferred
            // again from the new value on the next refresh).
            !r.path.is_empty() && r.fn_.is_some()
        }
        _ => false,
    }
}

// --- kinds -----------------------------------------------------------------

/// menuKindFor returns the menu kind a container node drills into.
pub fn menu_kind_for(schema: &Schema, fn_: FormNodeId) -> MenuKind {
    match schema.nodes[fn_ as usize].kind {
        Kind::OneOf => MenuKind::OneOf,
        Kind::Array => match schema.nodes[fn_ as usize].items {
            Some(items) if schema.nodes[items as usize].kind == Kind::Enum => MenuKind::EnumArray,
            _ => MenuKind::Items,
        },
        _ => MenuKind::Object,
    }
}

/// menuLeaf reports whether a node edits in place (no child menu of its own).
pub fn menu_leaf(schema: &Schema, fn_: FormNodeId) -> bool {
    match schema.nodes[fn_ as usize].kind {
        Kind::Scalar | Kind::Enum | Kind::Boolean | Kind::Raw => true,
        Kind::Array => match schema.nodes[fn_ as usize].items {
            None => true,
            Some(items) => schema.nodes[items as usize].kind == Kind::Raw,
        },
        _ => false,
    }
}

// --- row construction ------------------------------------------------------

/// menuRowsFor builds the row list for one menu state: the top one in
/// App::menu_rows, any state for cursor bookkeeping during variant flips.
pub fn menu_rows_for(
    schema: &mut Schema,
    doc: &Doc,
    hide_empty: bool,
    st: &MenuState,
    env: &mut RowsEnv<'_>,
) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // A YAML alias occurrence drills into the anchored original the
    // reference points at; < Expand > tops the menu so the reference can be
    // materialized into an independent copy.
    if env.merge.alias_at(&st.path).is_some() {
        rows.push(Row {
            kind: RowKind::Expand,
            path: st.path.clone(),
            idx: -1,
            label: "< Expand >".into(),
            ..Row::default()
        });
    }
    // A menu for an array item (path ends with an index) no longer carries a
    // "< Remove this item >" action row: D removes the item from its form.
    match st.kind {
        MenuKind::Object => {
            let merged = env.merge.merged_keys(st);
            let by_key: HashMap<&str, &MergedKey> =
                merged.iter().map(|mk| (mk.key.as_str(), mk)).collect();
            match schema.nodes[st.node as usize].kind {
                Kind::Map => {
                    // string-keyed map (additionalProperties): one row per
                    // document key, values edited against the value schema
                    if let Some(cur) = doc.get(&st.path) {
                        if let Value::Object(mems) = doc.value(cur) {
                            for mem in mems {
                                if !doc.nodes[mem.val as usize].merge.is_empty() {
                                    continue; // the merge spec itself: title row owns it
                                }
                                rows.push(map_item_row(
                                    schema,
                                    &mut env.synth,
                                    st.node,
                                    &st.path,
                                    &mem.key,
                                ));
                            }
                        }
                    }
                    for mk in &merged {
                        rows.push(merged_row(schema, &mut env.synth, st, mk));
                    }
                }
                Kind::Object => {
                    let mut known: HashSet<String> = HashSet::new();
                    let children = schema.nodes[st.node as usize].children.clone();
                    // Several properties may $ref the same titled definition
                    // (Cargo profiles: dev/release/bench/test all reuse
                    // "Profile with Build Override"). Showing that title on
                    // every row hides which key is which, so duplicated
                    // titles fall back to the property keys.
                    let mut title_counts: HashMap<String, usize> = HashMap::new();
                    for &c in &children {
                        let title = &schema.nodes[c as usize].title;
                        if !title.is_empty() {
                            *title_counts.entry(title.clone()).or_default() += 1;
                        }
                    }
                    for c in children {
                        let key = schema.nodes[c as usize].key.clone();
                        known.insert(key.clone());
                        // a merged key takes its schema-declared slot
                        if let Some(mk) = by_key.get(key.as_str()) {
                            rows.push(merged_row(schema, &mut env.synth, st, mk));
                            continue;
                        }
                        let mut row = child_row(schema, &st.path, c);
                        let title = &schema.nodes[c as usize].title;
                        if !title.is_empty() && title_counts.get(title).copied().unwrap_or(0) > 1 {
                            row.label = key;
                        }
                        rows.push(row);
                    }
                    // document members the schema does not define stay visible
                    if let Some(cur) = doc.get(&st.path) {
                        if let Value::Object(mems) = doc.value(cur) {
                            for mem in mems {
                                if !doc.nodes[mem.val as usize].merge.is_empty() {
                                    continue;
                                }
                                if !known.contains(&mem.key) {
                                    rows.push(Row {
                                        kind: RowKind::Unknown,
                                        idx: -1,
                                        path: append_key(&st.path, &mem.key),
                                        label: mem.key.clone(),
                                        in_schema: schema.nodes[st.node as usize].open_additional
                                            || pattern_defined(schema, st.node, &mem.key),
                                        ..Row::default()
                                    });
                                }
                            }
                        }
                    }
                    // merged keys the schema leaves undefined follow the
                    // schema-defined rows in merge order
                    for mk in &merged {
                        if !known.contains(&mk.key) {
                            rows.push(merged_row(schema, &mut env.synth, st, mk));
                        }
                    }
                }
                _ => {
                    // scalar/enum/bool/raw root schema: the root itself is the field
                    rows.push(Row {
                        kind: RowKind::Field,
                        fn_: Some(st.node),
                        path: st.path.clone(),
                        idx: -1,
                        ..Row::default()
                    });
                }
            }
        }
        MenuKind::Items => {
            let len = match doc.get(&st.path).map(|cur| doc.value(cur)) {
                Some(Value::Array(items)) => items.len(),
                _ => 0,
            };
            // Non-empty arrays add through a/A or paste: keep the row list
            // purely items. An empty array shows the one append action.
            if len == 0 {
                rows.push(Row {
                    kind: RowKind::Add,
                    label: "< Add item >".into(),
                    idx: -1,
                    ..Row::default()
                });
            }
            for i in 0..len {
                rows.push(item_row(schema, &mut env.synth, st.node, &st.path, i));
            }
        }
        MenuKind::EnumArray => {
            let opts_list = schema.nodes[st.node as usize]
                .items
                .map(|items| schema.nodes[items as usize].enum_.clone())
                .unwrap_or_default();
            if opts_list.is_empty() {
                rows.push(Row {
                    kind: RowKind::NoItems,
                    idx: -1,
                    label: "( no options )".into(),
                    ..Row::default()
                });
            } else {
                let q = st.filter.ti.value().to_lowercase();
                for (i, o) in opts_list.iter().enumerate() {
                    if st.filter.active && !q.is_empty() && !o.display.to_lowercase().contains(&q) {
                        continue;
                    }
                    rows.push(Row {
                        kind: RowKind::Toggle,
                        fn_: Some(st.node),
                        path: st.path.clone(),
                        idx: i as i64,
                        ..Row::default()
                    });
                }
            }
        }
        MenuKind::OneOf => {
            let variants = schema.nodes[st.node as usize].variants.clone();
            for v in variants {
                if st.filter.active && !variant_search_match(schema, v, st.filter.ti.value()) {
                    continue;
                }
                rows.push(Row {
                    kind: RowKind::Variant,
                    variant: Some(v),
                    path: st.path.clone(),
                    idx: -1,
                    ..Row::default()
                });
            }
        }
    }
    if hide_empty {
        rows.retain(|r| !empty_key_row(schema, doc, r));
    }
    if rows.is_empty() {
        let (label, kind) = match st.kind {
            MenuKind::OneOf => ("( no matching types )", RowKind::NoItems),
            MenuKind::EnumArray => ("( no matching options )", RowKind::NoItems),
            _ => ("< Add Key >", RowKind::NoItems),
        };
        rows.push(Row {
            kind,
            idx: -1,
            label: label.into(),
            ..Row::default()
        });
    }
    if env.is_root && !env.issues_now.is_empty() {
        let (mut errs, mut warns) = (0usize, 0usize);
        for it in env.issues_now {
            if it.warn {
                warns += 1;
            } else {
                errs += 1;
            }
        }
        rows.insert(
            0,
            Row {
                kind: RowKind::Issues,
                idx: -1,
                label: issue_status_label(errs, warns),
                ..Row::default()
            },
        );
    }
    rows
}

/// valueRow reports whether r is a document entry whose value is present and
/// not JSON null.
pub fn value_row(ctx: &MenuCtx<'_>, r: &Row) -> bool {
    value_row_raw(ctx.schema, ctx.doc, r)
}

fn value_row_raw(schema: &Schema, doc: &Doc, r: &Row) -> bool {
    let _ = schema;
    match r.kind {
        RowKind::Field | RowKind::Drill | RowKind::Unknown => {}
        _ => return false,
    }
    match doc.get(&r.path) {
        // a nil path is the root scalar itself
        Some(v) => !is_null(doc, v),
        None => false,
    }
}

fn empty_key_row(_schema: &Schema, doc: &Doc, r: &Row) -> bool {
    if r.path.is_empty() {
        return false;
    }
    if !matches!(r.path.last(), Some(PathSeg::Key(_))) {
        return false;
    }
    match r.kind {
        RowKind::Field | RowKind::Drill | RowKind::Unknown => {}
        _ => return false,
    }
    match doc.get(&r.path) {
        None => true,
        Some(v) => is_null(doc, v),
    }
}

/// moveToValue moves the cursor to the next (delta > 0) or previous
/// (delta < 0) row with a non-null value, wrapping at both ends.
pub fn move_to_value(ctx: &MenuCtx<'_>, st: &mut MenuState, rows: &[Row], delta: i64) {
    if rows.is_empty() {
        return;
    }
    let cur = st.cursor;
    for step in 1..=rows.len() as i64 {
        let i = wrap_cursor(cur, delta * step, rows.len());
        if value_row(ctx, &rows[i]) {
            st.cursor = i;
            return;
        }
    }
}

/// childRow maps one object child to a field or drill row.
pub fn child_row(schema: &Schema, parent_path: &[PathSeg], c: FormNodeId) -> Row {
    let p = append_key(parent_path, &schema.nodes[c as usize].key);
    let label = if schema.nodes[c as usize].title_from_ref {
        schema.nodes[c as usize].key.clone()
    } else {
        String::new()
    };
    if menu_leaf(schema, c) {
        Row {
            kind: RowKind::Field,
            fn_: Some(c),
            path: p,
            idx: -1,
            label,
            ..Row::default()
        }
    } else {
        Row {
            kind: RowKind::Drill,
            fn_: Some(c),
            path: p,
            idx: -1,
            label,
            ..Row::default()
        }
    }
}

fn fallback_raw_id(
    schema: &mut Schema,
    synth: &mut HashMap<SynthKey, FormNodeId>,
    node: FormNodeId,
) -> FormNodeId {
    let key = SynthKey::Fallback(node);
    if let Some(&id) = synth.get(&key) {
        return id;
    }
    let n = &schema.nodes[node as usize];
    let fb = FormNode {
        key: n.key.clone(),
        title: n.title.clone(),
        kind: Kind::Raw,
        ..FormNode::default()
    };
    let id = schema.nodes.len() as FormNodeId;
    schema.nodes.push(fb);
    synth.insert(key, id);
    id
}

/// mapItemRow maps one document key of a string-keyed map (KMap) to an
/// editable or drillable row against the map's value schema.
pub fn map_item_row(
    schema: &mut Schema,
    synth: &mut HashMap<SynthKey, FormNodeId>,
    node: FormNodeId,
    parent_path: &[PathSeg],
    key: &str,
) -> Row {
    let p = append_key(parent_path, key);
    let items = match schema.nodes[node as usize].items {
        Some(items) => items,
        None => fallback_raw_id(schema, synth, node),
    };
    let label = key.to_string();
    if menu_leaf(schema, items) {
        Row {
            kind: RowKind::Field,
            fn_: Some(items),
            path: p,
            idx: -1,
            label,
            ..Row::default()
        }
    } else {
        Row {
            kind: RowKind::Drill,
            fn_: Some(items),
            path: p,
            idx: -1,
            label,
            ..Row::default()
        }
    }
}

/// itemRow maps array element i to an editable or drillable row. The row
/// label is "<key>[i]".
pub fn item_row(
    schema: &mut Schema,
    synth: &mut HashMap<SynthKey, FormNodeId>,
    node: FormNodeId,
    parent_path: &[PathSeg],
    i: usize,
) -> Row {
    let p = append_idx(parent_path, i);
    let items = match schema.nodes[node as usize].items {
        Some(items) => items,
        None => fallback_raw_id(schema, synth, node),
    };
    let mut key = schema.nodes[node as usize].key.clone();
    if let Some(PathSeg::Key(s)) = parent_path.last() {
        key = s.clone();
    }
    let label = format!("{key}[{i}]");
    if menu_leaf(schema, items) {
        Row {
            kind: RowKind::Field,
            fn_: Some(items),
            path: p,
            idx: i as i64,
            label,
            ..Row::default()
        }
    } else {
        Row {
            kind: RowKind::Drill,
            fn_: Some(items),
            path: p,
            idx: i as i64,
            label,
            ..Row::default()
        }
    }
}

/// mergedRow builds the menu row of one merged key against the menu's schema
/// node (mergedRow + childSchemaByKey of menu_merge.go).
fn merged_row(
    schema: &mut Schema,
    synth: &mut HashMap<SynthKey, FormNodeId>,
    st: &MenuState,
    mk: &MergedKey,
) -> Row {
    let mut r = if schema.nodes[st.node as usize].kind == Kind::Map {
        let parent = &mk.path[..mk.path.len().saturating_sub(1)];
        map_item_row(schema, synth, st.node, parent, &mk.key)
    } else {
        let node = st.node;
        let fn_ = schema.nodes[node as usize]
            .children
            .iter()
            .copied()
            .find(|c| schema.nodes[*c as usize].key == mk.key);
        match fn_ {
            None => Row {
                kind: RowKind::Unknown,
                idx: -1,
                path: mk.path.clone(),
                label: mk.key.clone(),
                in_schema: schema.nodes[st.node as usize].open_additional
                    || pattern_defined(schema, st.node, &mk.key),
                ..Row::default()
            },
            Some(f) if menu_leaf(schema, f) => Row {
                kind: RowKind::Field,
                fn_: Some(f),
                path: mk.path.clone(),
                idx: -1,
                label: mk.key.clone(),
                ..Row::default()
            },
            Some(f) => Row {
                kind: RowKind::Drill,
                fn_: Some(f),
                path: mk.path.clone(),
                idx: -1,
                label: mk.key.clone(),
                ..Row::default()
            },
        }
    };
    r.merged = mk.src_name.clone();
    r
}

mod summary;
pub use summary::*;
mod variant;
pub use variant::*;
mod stack;
pub use stack::*;
mod search;
pub use search::*;
mod issues;
pub use issues::*;
mod filter;
pub use filter::*;
mod issue_parse;
pub use issue_parse::*;

// --- merge/alias lookups feeding the row model (Go menu_merge.go) ---

/// MergeLookup implements [`MergeView`] over the document's anchor state —
/// Go hung these on the listModel; they are pure document walks here.
pub struct MergeLookup<'a> {
    pub doc: &'a Doc,
}

impl MergeView for MergeLookup<'_> {
    fn alias_at(&self, path: &[PathSeg]) -> Option<String> {
        // Go's listModel.aliasAt returns ok only for reference occurrences
        // (menu_alias.go:64-67): the menu offers < Expand > on a reference,
        // not on the anchor carrier itself.
        alias_at(self.doc, path)
            .filter(|(_, is_ref)| *is_ref)
            .map(|(name, _)| name)
    }

    fn merged_keys(&self, st: &MenuState) -> Vec<MergedKey> {
        merged_keys(self.doc, st)
    }
}

/// MergeSource is one merge input: the anchor holder's path and the anchor
/// node (see `merge_defining_source`).
type MergeSource = (Vec<PathSeg>, NodeId);

/// mergeSourceFor resolves one merge reference into (name, holder path,
/// node). Unreachable references (their anchor was deleted) drop out.
fn merge_source_for(doc: &Doc, r: NodeId) -> Option<(String, Vec<PathSeg>, NodeId)> {
    let data = doc.node(r);
    if data.anchor.is_empty() || !matches!(doc.value(r), Value::Object(_)) {
        return None;
    }
    let holder = anchor_holder_path(doc, r)?;
    Some((data.anchor.clone(), holder, r))
}

/// mergedKeys lists the merged keys of the mapping at st.path in merge order
/// with YAML precedence applied: keys the mapping defines itself, and keys
/// an earlier anchor already provided, shadow the later ones.
pub fn merged_keys(doc: &Doc, st: &MenuState) -> Vec<MergedKey> {
    let Some(n) = doc.get(&st.path) else {
        return Vec::new();
    };
    let Some(Value::Object(mems)) = doc.value_opt(n) else {
        return Vec::new();
    };
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for mem in mems {
        if doc.node(mem.val).merge.is_empty() {
            seen.insert(mem.key.clone());
        }
    }
    let mut out: Vec<MergedKey> = Vec::new();
    for r in doc.merge_refs(n) {
        let Some((_, holder, node)) = merge_source_for(doc, r) else {
            continue;
        };
        // the anchor's resolved members: its own keys plus whatever its own
        // "<<" chain provides, with the anchor's own keys winning
        for mem in doc.resolved_members(node) {
            if seen.contains(&mem.key) {
                continue;
            }
            seen.insert(mem.key.clone());
            // inherited keys live in the mapping that actually defines them:
            // the badge, the breadcrumb and edits follow that source
            let mut busy = std::collections::HashSet::new();
            let (def_holder, def_node) =
                merge_defining_source(doc, (holder.clone(), node), &mem.key, &mut busy)
                    .unwrap_or((holder.clone(), node));
            let mut path = def_holder;
            path.push(PathSeg::key(mem.key.clone()));
            // Go's mergedKey src is the defining source, so the badge names
            // the anchor that actually holds the value (menu_merge.go:89).
            out.push(MergedKey {
                key: mem.key,
                path,
                src_name: doc.node(def_node).anchor.clone(),
            });
        }
    }
    out
}

/// mergeDefiningSource finds the mapping that defines key for the merge
/// source src: the mapping itself when it defines it, else the first mapping
/// its own merge keys pull the key from (YAML precedence: own keys win,
/// earlier anchors before later ones).
fn merge_defining_source(
    doc: &Doc,
    src: MergeSource,
    key: &str,
    busy: &mut std::collections::HashSet<NodeId>,
) -> Option<MergeSource> {
    let (path, node) = src;
    if !busy.insert(node) {
        return None;
    }
    let result = (|| {
        if let Some(Value::Object(mems)) = doc.value_opt(node) {
            for mem in mems {
                if doc.node(mem.val).merge.is_empty() && mem.key == key {
                    return Some((path, node));
                }
            }
        }
        for r in doc.merge_refs(node) {
            if let Some(sub) = merge_source_for(doc, r) {
                if let Some(got) = merge_defining_source(doc, (sub.1, r), key, busy) {
                    return Some(got);
                }
            }
        }
        None
    })();
    busy.remove(&node);
    result
}

impl App {
    /// menuRows builds the row list for the current (top) menu state.
    pub fn menu_rows(&mut self) -> Vec<Row> {
        let st = self.top().cloned();
        match st {
            Some(st) => self.menu_rows_state(&st),
            None => Vec::new(),
        }
    }

    /// menuRowsFor builds the row list for one menu state: the top one in
    /// menu_rows, any state for cursor bookkeeping during variant flips.
    pub fn menu_rows_state(&mut self, st: &MenuState) -> Vec<Row> {
        let hide_empty = self.hide_empty;
        let cur = self.cur;
        let f = &mut self.files[cur];
        let is_root = f.stack.first().map_or(false, |top| same_state(top, st));
        let FileState {
            schema,
            doc,
            menu_cache,
            issues_now,
            ..
        } = f;
        let lookup = MergeLookup { doc };
        let mut cache = std::mem::take(menu_cache);
        let mut env = RowsEnv {
            is_root,
            issues_now,
            merge: &lookup,
            synth: cache,
        };
        let rows = menu_rows_for(schema, doc, hide_empty, st, &mut env);
        cache = env.synth;
        *menu_cache = cache;
        rows
    }
}

#[cfg(test)]
mod tests;
