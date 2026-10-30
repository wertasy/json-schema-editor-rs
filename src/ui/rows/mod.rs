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

use regex::Regex;

use crate::doc::jsondoc::{parse_path, path_string};
use crate::doc::{Doc, NodeId, PathSeg, Value, append_idx, append_key};
use crate::icons::{ERR, WARN};
use crate::smodel::{FormNode, FormNodeId, Kind, Schema};
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

/// Identity of a synthesized schema node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SynthKey {
    /// map/array value schema unknown: raw-editing fallback for node
    Fallback(FormNodeId),
    /// union menu over node's (dynamic) variant list
    Merged(FormNodeId),
    /// variant's menu with discriminator keys unionized
    Pinned(FormNodeId, FormNodeId),
    /// arbitrary JSON type picker (unknown keys, untyped schema fields)
    AnyType,
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

fn is_null(doc: &Doc, id: NodeId) -> bool {
    matches!(doc.value(id), Value::Null)
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

#[cfg(test)]
mod tests;
