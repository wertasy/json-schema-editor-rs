#![allow(dead_code)] // wired into the menu in later phases

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
use crate::doc::{Doc, NodeId, PathSeg, Value};
use crate::icons::{ERR, WARN};
use crate::smodel::{FormNode, FormNodeId, Kind, Schema};
use crate::ui::{Issue, MenuCtx, MenuKind, MenuState, Options, Row, RowKind};

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

/// rawLabel (raw.go): the raw editor syntax label, "JSON" by default.
fn raw_label(opts: &Options) -> &'static str {
    if opts.raw.label.is_empty() {
        "JSON"
    } else {
        opts.raw.label
    }
}

/// pathFor returns a copy of path with key appended.
fn path_for(path: &[PathSeg], key: &str) -> Vec<PathSeg> {
    let mut p = path.to_vec();
    p.push(PathSeg::Key(key.to_string()));
    p
}

/// pathIdx returns a copy of path with index appended.
fn path_idx(path: &[PathSeg], idx: usize) -> Vec<PathSeg> {
    let mut p = path.to_vec();
    p.push(PathSeg::Idx(idx));
    p
}

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

/// scalarText (widgets.go): renders a current/default scalar node as text.
fn scalar_text(doc: &Doc, id: NodeId) -> String {
    match doc.value(id) {
        Value::Str(s) => s.clone(),
        Value::Num(n) => n.clone(),
        Value::Bool(b) => {
            if *b {
                "true".into()
            } else {
                "false".into()
            }
        }
        _ => String::new(),
    }
}

/// The canonical JSON literal of a value (smodel.EncodeValue).
fn encode_value(doc: &Doc, id: NodeId) -> String {
    serde_json::to_string(&doc.to_value(id)).unwrap_or_default()
}

fn is_null(doc: &Doc, id: NodeId) -> bool {
    matches!(doc.value(id), Value::Null)
}

/// wrapCursor advances cur by delta over n items, wrapping at both ends (a
/// non-positive list stays at 0).
fn wrap_cursor(cur: usize, delta: i64, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let n = n as i64;
    let mut c = (cur as i64 + delta) % n;
    if c < 0 {
        c += n;
    }
    c as usize
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

/// menuRows builds the row list for the current (top) menu state.
pub fn menu_rows(
    schema: &mut Schema,
    doc: &Doc,
    opts: &Options,
    hide_empty: bool,
    st: &MenuState,
    env: &mut RowsEnv,
) -> Vec<Row> {
    menu_rows_for(schema, doc, opts, hide_empty, st, env)
}

/// menuRowsFor builds the row list for one menu state: the top one in
/// menu_rows, any state for cursor bookkeeping during variant flips.
pub fn menu_rows_for(
    schema: &mut Schema,
    doc: &Doc,
    opts: &Options,
    hide_empty: bool,
    st: &MenuState,
    env: &mut RowsEnv,
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
    // A menu for an array item (path ends with an index) offers removal.
    if !st.path.is_empty()
        && st.kind != MenuKind::EnumArray
        && st.kind != MenuKind::OneOf
        && matches!(st.path.last(), Some(PathSeg::Idx(_)))
    {
        rows.push(Row {
            kind: RowKind::Remove,
            path: st.path.clone(),
            idx: -1,
            label: "< Remove this item >".into(),
            ..Row::default()
        });
    }
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
                                        path: path_for(&st.path, &mem.key),
                                        label: mem.key.clone(),
                                        in_schema: pattern_defined(schema, st.node, &mem.key),
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
            rows.push(Row {
                kind: RowKind::Add,
                label: "< Add item >".into(),
                idx: -1,
                ..Row::default()
            });
            if let Some(cur) = doc.get(&st.path) {
                if let Value::Array(items) = doc.value(cur) {
                    for i in 0..items.len() {
                        rows.push(item_row(schema, &mut env.synth, st.node, &st.path, i));
                    }
                }
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
                let q = st.search_ti.value().to_lowercase();
                for (i, o) in opts_list.iter().enumerate() {
                    if st.searching && !q.is_empty() && !o.display.to_lowercase().contains(&q) {
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
                if st.searching && !variant_search_match(schema, v, st.search_ti.value()) {
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
        rows.retain(|r| !empty_key_row_raw(schema, doc, r));
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
    if env.is_root {
        rows.push(Row {
            kind: RowKind::Save,
            label: "< Save and exit >".into(),
            idx: -1,
            ..Row::default()
        });
        rows.push(Row {
            kind: RowKind::Raw,
            label: format!("< Edit raw {} >", raw_label(opts)),
            idx: -1,
            ..Row::default()
        });
    }
    rows
}

/// valueRow reports whether r is a document entry whose value is present and
/// not JSON null.
pub fn value_row(ctx: &MenuCtx, r: &Row) -> bool {
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

/// emptyKeyRow reports whether r is an object member row with no document
/// value: absent or explicit null. A non-null default does not count.
pub fn empty_key_row(ctx: &MenuCtx, r: &Row) -> bool {
    empty_key_row_raw(ctx.schema, ctx.doc, r)
}

fn empty_key_row_raw(_schema: &Schema, doc: &Doc, r: &Row) -> bool {
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

/// withoutEmptyKeys drops the rows the z filter hides.
pub fn without_empty_keys(ctx: &MenuCtx, rows: Vec<Row>) -> Vec<Row> {
    let (schema, doc) = (ctx.schema, ctx.doc);
    let mut rows = rows;
    rows.retain(|r| !empty_key_row_raw(schema, doc, r));
    rows
}

/// moveToValue moves the cursor to the next (delta > 0) or previous
/// (delta < 0) row with a non-null value, wrapping at both ends.
pub fn move_to_value(ctx: &MenuCtx, st: &mut MenuState, rows: &[Row], delta: i64) {
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
    let p = path_for(parent_path, &schema.nodes[c as usize].key);
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
    let p = path_for(parent_path, key);
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
    let p = path_idx(parent_path, i);
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
                in_schema: pattern_defined(schema, st.node, &mk.key),
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

// --- summaries -------------------------------------------------------------

/// summary renders the right-hand value column for fn at path: the current
/// doc value, or the effective default when unset, "—" when neither exists.
pub fn summary(ctx: &MenuCtx, fn_: FormNodeId, path: &[PathSeg]) -> String {
    summary_raw(ctx.schema, ctx.doc, fn_, path)
}

pub fn summary_raw(schema: &Schema, doc: &Doc, fn_: FormNodeId, path: &[PathSeg]) -> String {
    let f = &schema.nodes[fn_ as usize];
    let mut vid = doc.get(path).filter(|&v| !is_null(doc, v));
    let mut vdoc = doc;
    if vid.is_none() {
        match f.default_ {
            Some(d) if !is_null(&schema.doc, d) => {
                vid = Some(d);
                vdoc = &schema.doc;
            }
            _ => return "—".to_string(),
        }
    }
    let v = vid.unwrap();
    match f.kind {
        Kind::Scalar => match vdoc.value(v) {
            Value::Str(s) => value_clip(s),
            Value::Num(n) => n.clone(),
            Value::Bool(b) => b.to_string(),
            _ => "—".to_string(),
        },
        Kind::Boolean => {
            if matches!(vdoc.value(v), Value::Bool(true)) {
                "[y]".to_string()
            } else {
                "[n]".to_string()
            }
        }
        Kind::Enum => {
            let enc = encode_value(vdoc, v);
            for o in &f.enum_ {
                if o.encoded == enc {
                    return o.display.clone();
                }
            }
            let s = scalar_text(vdoc, v);
            if !s.is_empty() {
                return s;
            }
            "—".to_string()
        }
        Kind::Object | Kind::Map => object_summary(schema, Some(fn_), vdoc, Some(v)),
        Kind::Array => array_summary(vdoc, Some(v)),
        Kind::OneOf => {
            // descend to the matched leaf, not the group
            let Some(variant) = matched_leaf_variant(schema, fn_, vdoc, Some(v)) else {
                return "—".to_string();
            };
            let vf = &schema.nodes[variant as usize];
            match vf.kind {
                Kind::Object | Kind::Map => {
                    if !vf.title.is_empty() {
                        return format!(
                            "{} {}",
                            vf.title,
                            object_summary(schema, Some(variant), vdoc, Some(v))
                        );
                    }
                    object_summary(schema, Some(variant), vdoc, Some(v))
                }
                Kind::Enum => {
                    let enc = encode_value(vdoc, v);
                    for o in &vf.enum_ {
                        if o.encoded == enc {
                            return o.display.clone();
                        }
                    }
                    if !vf.title.is_empty() {
                        vf.title.clone()
                    } else {
                        "—".to_string()
                    }
                }
                Kind::Boolean => {
                    if matches!(vdoc.value(v), Value::Bool(false)) {
                        "[n]".to_string()
                    } else {
                        "[y]".to_string()
                    }
                }
                Kind::Array => array_summary(vdoc, Some(v)),
                Kind::Scalar => {
                    let s = scalar_text(vdoc, v);
                    if !s.is_empty() {
                        return value_clip(&s);
                    }
                    if !vf.title.is_empty() {
                        vf.title.clone()
                    } else {
                        "—".to_string()
                    }
                }
                Kind::Raw | Kind::OneOf => {
                    if !vf.title.is_empty() {
                        vf.title.clone()
                    } else {
                        "—".to_string()
                    }
                }
            }
        }
        Kind::Raw => format!("json {}", clip_runes(&encode_value(vdoc, v), 20)),
    }
}

/// objectSummary shows the first two object members as k=v, then " +N";
/// "{}" when empty. Members render in the schema's declared order
/// (fn.Children); keys the schema does not know keep document order after
/// the known ones. fn may be None (schema-less values).
pub fn object_summary(
    schema: &Schema,
    fn_: Option<FormNodeId>,
    vdoc: &Doc,
    n: Option<NodeId>,
) -> String {
    let Some(n) = n else {
        return "{}".to_string();
    };
    if !matches!(vdoc.value(n), Value::Object(_)) {
        return clip_runes(&encode_value(vdoc, n), 24);
    }
    let mem = vdoc.resolved_members(n); // merged keys resolve in
    if mem.is_empty() {
        return "{}".to_string();
    }
    let mut rank: HashMap<&str, usize> = HashMap::new();
    if let Some(f) = fn_ {
        for (i, c) in schema.nodes[f as usize].children.iter().enumerate() {
            let key = schema.nodes[*c as usize].key.as_str();
            rank.entry(key).or_insert(i);
        }
    }
    let mut ordered = mem; // never reorder the doc
    let unknown_rank = rank.len();
    ordered.sort_by_key(|m| rank.get(m.key.as_str()).copied().unwrap_or(unknown_rank));
    let mut parts: Vec<String> = Vec::with_capacity(3);
    for m in ordered.iter().take(2) {
        let v = vdoc.value(m.val);
        match v {
            Value::Null => parts.push(format!("{}=null", m.key)),
            Value::Object(_) => parts.push(format!("{}={{…}}", m.key)),
            Value::Array(items) => parts.push(format!("{}=[{}]", m.key, items.len())),
            Value::Str(s) => parts.push(format!("{}={}", m.key, value_clip(s))),
            Value::Num(num) => parts.push(format!("{}={}", m.key, num)),
            Value::Bool(b) => parts.push(format!("{}={}", m.key, b)),
        }
    }
    let mut s = parts.join(" ");
    if ordered.len() > 2 {
        s += &format!(" +{}", ordered.len() - 2);
    }
    s
}

/// valueClip truncates a scalar value for the single-line summary column
/// (32 runes, ellipsis included). Line breaks and tab runs fold into single
/// spaces.
pub fn value_clip(s: &str) -> String {
    clip_runes(&squash_space(s), 32)
}

/// squashSpace folds every whitespace run into one space; values without
/// line breaks or tabs are returned untouched.
pub fn squash_space(s: &str) -> String {
    if !s.contains(['\r', '\n', '\t']) {
        return s.to_string();
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// arraySummary renders an array value column: non-object lists preview
/// their members ("[4] a 1 {…} +1"), pure object arrays just count
/// ("N items"), "empty" when unset.
pub fn array_summary(vdoc: &Doc, v: Option<NodeId>) -> String {
    let Some(v) = v else {
        return "—".to_string();
    };
    let arr = match vdoc.value(v) {
        Value::Array(a) => a.clone(),
        _ => return "—".to_string(),
    };
    if arr.is_empty() {
        return "empty".to_string();
    }
    let mut label = format!("{} item", arr.len());
    if arr.len() != 1 {
        label += "s";
    }
    let pure_objects = arr
        .iter()
        .all(|&it| matches!(vdoc.value(it), Value::Object(_)));
    if pure_objects {
        return label;
    }
    const PREVIEW: usize = 3;
    let parts: Vec<String> = arr
        .iter()
        .take(PREVIEW)
        .map(|&it| array_item_brief(vdoc, Some(it)))
        .collect();
    let mut s = format!("[{}] {}", arr.len(), parts.join(" "));
    if arr.len() > PREVIEW {
        s += &format!(" +{}", arr.len() - PREVIEW);
    }
    s
}

/// arrayItemBrief renders one array member for the preview column.
pub fn array_item_brief(vdoc: &Doc, n: Option<NodeId>) -> String {
    let Some(n) = n else {
        return "null".to_string();
    };
    match vdoc.value(n) {
        Value::Null => "null".to_string(),
        Value::Object(_) => "{…}".to_string(),
        Value::Array(items) => format!("[{}]", items.len()),
        Value::Str(s) => clip_runes(s, 16),
        Value::Num(num) => num.clone(),
        Value::Bool(b) => b.to_string(),
    }
}

/// clipRunes truncates s to max runes, the last being an ellipsis.
pub fn clip_runes(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let chs: Vec<char> = s.chars().collect();
    if chs.len() <= max {
        return s.to_string();
    }
    chs[..max - 1].iter().collect::<String>() + "…"
}

// --- variant signatures & matching -----------------------------------------

/// VariantSignature renders the structural signature shown on oneOf variant
/// rows and search hits: "{a, b*}" for objects, type names for scalars,
/// "a |b |c" option lists for enums, "yes/no", "list of <sig>" and "json" for
/// unconstrained values. A nested oneOf (an allOf-distributed detail branch)
/// stands in its variants' signatures, deduped.
pub fn variant_signature(schema: &Schema, fn_: Option<FormNodeId>) -> String {
    variant_signature_seen(schema, fn_, &mut Vec::new())
}

/// The recursion carries the ancestor containers because synthesized menus
/// can point back into themselves (the any-type picker's array items are the
/// picker itself); a cycle reads as an unconstrained value.
fn variant_signature_seen(
    schema: &Schema,
    fn_: Option<FormNodeId>,
    seen: &mut Vec<FormNodeId>,
) -> String {
    let Some(fn_) = fn_ else {
        return "json".to_string();
    };
    if seen.contains(&fn_) {
        return "json".to_string();
    }
    seen.push(fn_);
    let f = &schema.nodes[fn_ as usize];
    let out = match f.kind {
        Kind::Object => {
            let mut names: Vec<String> = Vec::with_capacity(f.children.len());
            for c in &f.children {
                let c = &schema.nodes[*c as usize];
                let n = if c.required && c.kind == Kind::Enum && c.enum_.len() == 1 {
                    format!("{}={}", c.key, c.enum_[0].display)
                } else if c.required {
                    format!("{}*", c.key)
                } else {
                    c.key.clone()
                };
                names.push(n);
            }
            if names.is_empty() {
                "{}".to_string()
            } else if names.len() > 4 {
                format!("{{{}, +{}}}", names[..4].join(", "), names.len() - 4)
            } else {
                format!("{{{}}}", names.join(", "))
            }
        }
        Kind::Scalar => {
            if f.json_type.is_empty() {
                "string".to_string()
            } else {
                f.json_type.clone()
            }
        }
        Kind::Enum => {
            let ds: Vec<String> = f.enum_.iter().map(|o| o.display.clone()).collect();
            if ds.is_empty() {
                "enum".to_string()
            } else if ds.len() > 3 {
                format!("{} …", ds[..3].join(" |"))
            } else {
                ds.join(" |")
            }
        }
        Kind::Boolean => "yes/no".to_string(),
        Kind::Array => match f.items {
            None => "list of json".to_string(),
            Some(items) => format!(
                "list of {}",
                variant_signature_seen(schema, Some(items), seen)
            ),
        },
        Kind::OneOf => {
            let mut sigs: Vec<String> = Vec::new();
            let mut cyclic = false;
            for &v in &f.variants {
                if seen.contains(&v) {
                    cyclic = true;
                    break;
                }
                let sig = variant_signature_seen(schema, Some(v), seen);
                if !sigs.contains(&sig) {
                    sigs.push(sig);
                }
            }
            if cyclic {
                "json".to_string()
            } else if sigs.is_empty() {
                "json".to_string()
            } else if sigs.len() > 3 {
                format!("{} …", sigs[..3].join(" | "))
            } else {
                sigs.join(" | ")
            }
        }
        Kind::Raw => {
            if f.null_only {
                "null".to_string()
            } else {
                "json".to_string()
            }
        }
        _ => "json".to_string(),
    };
    seen.pop();
    out
}

/// currentVariant returns the index of the first variant the value at path
/// matches, or -1.
pub fn current_variant(schema: &Schema, node: FormNodeId, doc: &Doc, val: Option<NodeId>) -> i64 {
    let val = match val {
        Some(v) if !is_null(doc, v) => v,
        _ => return -1,
    };
    for (i, &v) in schema.nodes[node as usize].variants.iter().enumerate() {
        if variant_matches(schema, v, doc, Some(val)) {
            return i as i64;
        }
    }
    -1
}

/// matchedLeafVariant resolves the variant matching val, following nested
/// oneOf groups down to the leaf; None when no level matches.
pub fn matched_leaf_variant(
    schema: &Schema,
    node: FormNodeId,
    doc: &Doc,
    val: Option<NodeId>,
) -> Option<FormNodeId> {
    let mut node = node;
    while schema.nodes[node as usize].kind == Kind::OneOf {
        let idx = current_variant(schema, node, doc, val);
        if idx < 0 {
            return None;
        }
        node = schema.nodes[node as usize].variants[idx as usize];
    }
    Some(node)
}

/// seedVariantCursor points a freshly built oneOf picker state at the
/// variant matching the current value (the ● row); unset values stay on
/// row 0.
pub fn seed_variant_cursor(schema: &Schema, st: &mut MenuState, doc: &Doc) {
    if st.kind != MenuKind::OneOf {
        return;
    }
    let Some(v) = doc.get(&st.path) else {
        return;
    };
    let idx = current_variant(schema, st.node, doc, Some(v));
    if idx >= 0 {
        st.cursor = idx as usize;
    }
}

/// displayTitle is the header/label for a node whose schema gave it no
/// title (untitled oneOf variants): the structural signature stands in.
pub fn display_title(schema: &Schema, fn_: FormNodeId) -> String {
    let f = &schema.nodes[fn_ as usize];
    if !f.title.is_empty() {
        return f.title.clone();
    }
    variant_signature(schema, Some(fn_))
}

/// dynamicVariants flattens oneOf variant groups (nested oneOfs) into their
/// object leaves; None when any leaf is not an object — the classic variant
/// picker applies instead.
pub fn dynamic_variants(schema: &Schema, node: FormNodeId) -> Option<Vec<FormNodeId>> {
    {
        let n = &schema.nodes[node as usize];
        if n.kind != Kind::OneOf || n.variants.is_empty() {
            return None;
        }
    }
    fn add(schema: &Schema, vs: &[FormNodeId], out: &mut Vec<FormNodeId>) -> bool {
        for &v in vs {
            match schema.nodes[v as usize].kind {
                Kind::Object => out.push(v),
                Kind::OneOf => {
                    let sub = schema.nodes[v as usize].variants.clone();
                    if !add(schema, &sub, out) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }
    let mut out = Vec::new();
    let top = schema.nodes[node as usize].variants.clone();
    if add(schema, &top, &mut out) {
        Some(out)
    } else {
        None
    }
}

/// matchingVariants returns the candidate variants for val: all variants
/// while the value is unset or an empty object, otherwise those the value
/// conforms to (see variant_matches).
pub fn matching_variants(
    schema: &Schema,
    node: FormNodeId,
    doc: &Doc,
    val: Option<NodeId>,
) -> Vec<FormNodeId> {
    if schema.nodes[node as usize].kind != Kind::OneOf {
        return Vec::new();
    }
    let variants = schema.nodes[node as usize].variants.clone();
    match_variant_list(schema, &variants, doc, val)
}

/// matchVariantList is matchingVariants over an explicit variant list
/// (dynamic menus pass the flattened leaves).
pub fn match_variant_list(
    schema: &Schema,
    vs: &[FormNodeId],
    doc: &Doc,
    val: Option<NodeId>,
) -> Vec<FormNodeId> {
    if vs.is_empty() {
        return Vec::new();
    }
    let empty_or_unset = match val {
        None => true,
        Some(v) if is_null(doc, v) => true,
        Some(v) => match doc.value(v) {
            Value::Object(_) => doc.resolved_members(v).is_empty(),
            _ => false,
        },
    };
    if empty_or_unset {
        return vs.to_vec();
    }
    // keys defined by only some variants are evidence for those variants;
    // keys every variant defines are not. Rank by that evidence.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for &v in vs {
        for c in &schema.nodes[v as usize].children {
            *counts
                .entry(schema.nodes[*c as usize].key.as_str())
                .or_insert(0) += 1;
        }
    }
    let unique = |key: &str| counts.get(key).copied().unwrap_or(0) < vs.len();
    let mut best: i64 = -1;
    let mut out: Vec<FormNodeId> = Vec::new();
    for &v in vs {
        let (ok, score) = variant_match(schema, v, doc, val, Some(&unique));
        if !ok {
            continue;
        }
        if score as i64 > best {
            best = score as i64;
            out.clear();
        }
        if score as i64 == best {
            out.push(v);
        }
    }
    out
}

/// variantMatches reports whether val fits one variant (see variant_match).
pub fn variant_matches(schema: &Schema, v: FormNodeId, doc: &Doc, val: Option<NodeId>) -> bool {
    variant_match(schema, v, doc, val, None).0
}

/// variantMatch reports whether val fits one variant and how many of its
/// overlapping keys are evidence for it.
pub fn variant_match(
    schema: &Schema,
    v: FormNodeId,
    doc: &Doc,
    val: Option<NodeId>,
    unique: Option<&dyn Fn(&str) -> bool>,
) -> (bool, usize) {
    match schema.nodes[v as usize].kind {
        Kind::OneOf => {
            let variants = schema.nodes[v as usize].variants.clone();
            (
                !match_variant_list(schema, &variants, doc, val).is_empty(),
                0,
            )
        }
        Kind::Enum => {
            let Some(val) = val else { return (false, 0) };
            let enc = encode_value(doc, val);
            for o in &schema.nodes[v as usize].enum_ {
                if o.encoded == enc {
                    return (true, 0);
                }
            }
            (false, 0)
        }
        Kind::Object | Kind::Map => {
            // KMap defines no required keys, so any object fits it; KObject
            // needs a key overlap once it has required keys.
            let Some(val) = val else { return (false, 0) };
            if !matches!(doc.value(val), Value::Object(_)) {
                return (false, 0);
            }
            let mut defs: HashMap<&str, FormNodeId> = HashMap::new();
            let mut required = false;
            for &c in &schema.nodes[v as usize].children {
                defs.insert(schema.nodes[c as usize].key.as_str(), c);
                if schema.nodes[c as usize].required {
                    required = true;
                }
            }
            if !required {
                return (true, 0);
            }
            // merged keys identify the variant just like own keys do
            let members = doc.resolved_members(val);
            let mut overlap = false;
            let mut evidence = 0usize;
            for m in members {
                let Some(&def) = defs.get(m.key.as_str()) else {
                    continue;
                };
                overlap = true;
                if unique.map(|u| u(&m.key)).unwrap_or(false) {
                    evidence += 1;
                }
                if !value_conforms(schema, Some(def), doc, Some(m.val)) {
                    return (false, 0);
                }
            }
            (overlap, evidence)
        }
        Kind::Scalar | Kind::Boolean | Kind::Array | Kind::Raw => {
            // leaf variants (e.g. a plain string alternative) match by shape
            (value_conforms(schema, Some(v), doc, val), 0)
        }
    }
}

/// valueConforms reports whether the value fits the field definition at the
/// kind level. Raw definitions accept anything.
pub fn value_conforms(
    schema: &Schema,
    def: Option<FormNodeId>,
    doc: &Doc,
    v: Option<NodeId>,
) -> bool {
    let Some(def) = def else {
        return true;
    };
    let d = &schema.nodes[def as usize];
    if d.kind == Kind::Raw && d.null_only {
        return matches!(v, Some(v) if is_null(doc, v));
    }
    match d.kind {
        Kind::Enum => {
            let Some(v) = v else { return false };
            let enc = encode_value(doc, v);
            d.enum_.iter().any(|o| o.encoded == enc)
        }
        Kind::Boolean => matches!(v, Some(v) if matches!(doc.value(v), Value::Bool(_))),
        Kind::Object | Kind::Map => {
            matches!(v, Some(v) if matches!(doc.value(v), Value::Object(_)))
        }
        Kind::Array => matches!(v, Some(v) if matches!(doc.value(v), Value::Array(_))),
        Kind::Scalar => {
            let Some(v) = v else { return false };
            match d.json_type.as_str() {
                "integer" | "number" => matches!(doc.value(v), Value::Num(_)),
                "string" => matches!(doc.value(v), Value::Str(_)),
                _ => matches!(doc.value(v), Value::Str(_) | Value::Num(_) | Value::Bool(_)),
            }
        }
        _ => true,
    }
}

/// schemaAcceptsValue reports whether v's runtime shape satisfies def's
/// schema. Completion lists use it to offer only anchors that can legally
/// fill the field.
pub fn schema_accepts_value(
    schema: &Schema,
    def: Option<FormNodeId>,
    doc: &Doc,
    v: Option<NodeId>,
) -> bool {
    let Some(def) = def else {
        return true;
    };
    match schema.nodes[def as usize].kind {
        Kind::OneOf => schema.nodes[def as usize]
            .variants
            .iter()
            .any(|&variant| schema_accepts_value(schema, Some(variant), doc, v)),
        Kind::Raw => {
            if schema.nodes[def as usize].null_only {
                matches!(v, Some(v) if is_null(doc, v))
            } else {
                true
            }
        }
        _ => value_conforms(schema, Some(def), doc, v),
    }
}

/// variantFieldDefs collects every field key's definitions across the
/// variant list, in first-appearance order.
pub fn variant_field_defs(
    schema: &Schema,
    variants: &[FormNodeId],
) -> (Vec<String>, HashMap<String, Vec<FormNodeId>>) {
    let mut order: Vec<String> = Vec::new();
    let mut defs: HashMap<String, Vec<FormNodeId>> = HashMap::new();
    for &v in variants {
        for &c in &schema.nodes[v as usize].children {
            let key = schema.nodes[c as usize].key.clone();
            if !defs.contains_key(&key) {
                order.push(key.clone());
            }
            defs.entry(key).or_default().push(c);
        }
    }
    (order, defs)
}

/// unionField merges one key's definitions across variants. Enum fields with
/// differing option sets become a union enum marked as a discriminator;
/// otherwise same-kind definitions take the first, and kind conflicts drop
/// the key (None). The union covers multi-option enums as well as consts:
/// sing-box's RuleSet `type` is `enum: ["inline", ""]` in one variant and
/// `const: "local"` / `const: "remote"` in the others, so all four options
/// must stay selectable.
pub fn union_field(
    schema: &mut Schema,
    key: &str,
    ds: &[FormNodeId],
) -> Option<(FormNodeId, bool)> {
    let all_enum = ds
        .iter()
        .all(|&d| schema.nodes[d as usize].kind == Kind::Enum);
    let differs = all_enum
        && ds.iter().skip(1).any(|&d| {
            let a = &schema.nodes[ds[0] as usize].enum_;
            let b = &schema.nodes[d as usize].enum_;
            a.len() != b.len() || a.iter().zip(b).any(|(x, y)| x.encoded != y.encoded)
        });
    if differs {
        let mut union = FormNode {
            kind: Kind::Enum,
            key: key.to_string(),
            required: true,
            ..FormNode::default()
        };
        union.title = schema.nodes[ds[0] as usize].title.clone();
        let mut seen: HashSet<String> = HashSet::new();
        for &d in ds {
            let dn = &schema.nodes[d as usize];
            for o in &dn.enum_ {
                if seen.insert(o.encoded.clone()) {
                    union.enum_.push(o.clone());
                }
            }
            union.required = union.required && dn.required;
        }
        let id = schema.nodes.len() as FormNodeId;
        schema.nodes.push(union);
        return Some((id, true));
    }
    for &d in &ds[1..] {
        if schema.nodes[d as usize].kind != schema.nodes[ds[0] as usize].kind {
            return None;
        }
    }
    Some((ds[0], false))
}

fn push_node(schema: &mut Schema, n: FormNode) -> FormNodeId {
    let id = schema.nodes.len() as FormNodeId;
    schema.nodes.push(n);
    id
}

/// anyTypeMenu builds the variant picker for a value with no concrete schema
/// type (a document key the schema does not define, or a field that degrades
/// to "json"). It offers the JSON built-ins, the schema's own named types
/// (`$defs`/`definitions`), and a JSON escape hatch that opens the built-in
/// raw textarea. The array variant's items use the picker itself, so
/// arbitrary nested arrays keep the same type-choice flow.
pub fn any_type_menu(schema: &mut Schema, synth: &mut HashMap<SynthKey, FormNodeId>) -> FormNodeId {
    if let Some(&id) = synth.get(&SynthKey::AnyType) {
        return id;
    }
    let schema_defs = schema.defs.clone();
    let mut variants = vec![
        push_node(
            schema,
            FormNode {
                title: "String".into(),
                kind: Kind::Scalar,
                json_type: "string".into(),
                ..FormNode::default()
            },
        ),
        push_node(
            schema,
            FormNode {
                title: "Number".into(),
                kind: Kind::Scalar,
                json_type: "number".into(),
                ..FormNode::default()
            },
        ),
        push_node(
            schema,
            FormNode {
                title: "Boolean".into(),
                kind: Kind::Boolean,
                ..FormNode::default()
            },
        ),
        push_node(
            schema,
            FormNode {
                title: "Object".into(),
                kind: Kind::Object,
                ..FormNode::default()
            },
        ),
        push_node(
            schema,
            FormNode {
                title: "Array".into(),
                kind: Kind::Array,
                ..FormNode::default()
            },
        ),
        push_node(
            schema,
            FormNode {
                title: "Null".into(),
                kind: Kind::Raw,
                null_only: true,
                ..FormNode::default()
            },
        ),
    ];
    variants.extend(schema_defs);
    variants.push(push_node(
        schema,
        FormNode {
            title: "JSON".into(),
            kind: Kind::Raw,
            raw_editor: true,
            ..FormNode::default()
        },
    ));
    let array = variants[4];
    let id = push_node(
        schema,
        FormNode {
            kind: Kind::OneOf,
            variants,
            ..FormNode::default()
        },
    );
    schema.nodes[array as usize].items = Some(id);
    synth.insert(SynthKey::AnyType, id);
    id
}

/// mergedOneOfMenu synthesizes the union-menu node over the variant list:
/// the merge of every variant's fields (discriminator keys as union enums,
/// same-kind keys as the first definition, conflicting keys dropped).
/// Returns the id of the pushed node.
pub fn merged_one_of_menu(
    schema: &mut Schema,
    node: FormNodeId,
    variants: &[FormNodeId],
) -> FormNodeId {
    let (key, title) = {
        let n = &schema.nodes[node as usize];
        (n.key.clone(), n.title.clone())
    };
    let mut merged = FormNode {
        kind: Kind::Object,
        key,
        title,
        ..FormNode::default()
    };
    let (order, defs) = variant_field_defs(schema, variants);
    for k in &order {
        if let Some((fld, _)) = union_field(schema, k, &defs[k]) {
            merged.children.push(fld);
        }
    }
    push_node(schema, merged)
}

/// pinnedOneOfMenu returns the variant's node with discriminator keys
/// replaced by the union enum, so the variant can be switched in place.
/// Returns the id of the pushed node.
pub fn pinned_one_of_menu(
    schema: &mut Schema,
    variants: &[FormNodeId],
    variant: FormNodeId,
) -> FormNodeId {
    let (_, defs) = variant_field_defs(schema, variants);
    let (key, title) = {
        let v = &schema.nodes[variant as usize];
        (v.key.clone(), v.title.clone())
    };
    let mut out = FormNode {
        kind: Kind::Object,
        key,
        title,
        ..FormNode::default()
    };
    for &c in &schema.nodes[variant as usize].children.clone() {
        let ckey = schema.nodes[c as usize].key.clone();
        match union_field(schema, &ckey, &defs[&ckey]) {
            Some((fld, true)) => out.children.push(fld),
            _ => out.children.push(c),
        }
    }
    push_node(schema, out)
}

fn merged_one_of_menu_id(
    schema: &mut Schema,
    synth: &mut HashMap<SynthKey, FormNodeId>,
    node: FormNodeId,
    variants: &[FormNodeId],
) -> FormNodeId {
    if let Some(&id) = synth.get(&SynthKey::Merged(node)) {
        return id;
    }
    let id = merged_one_of_menu(schema, node, variants);
    synth.insert(SynthKey::Merged(node), id);
    id
}

fn pinned_one_of_menu_id(
    schema: &mut Schema,
    synth: &mut HashMap<SynthKey, FormNodeId>,
    node: FormNodeId,
    variant: FormNodeId,
) -> FormNodeId {
    if let Some(&id) = synth.get(&SynthKey::Pinned(node, variant)) {
        return id;
    }
    let variants = dynamic_variants(schema, node)
        .unwrap_or_else(|| schema.nodes[node as usize].variants.clone());
    let id = pinned_one_of_menu(schema, &variants, variant);
    synth.insert(SynthKey::Pinned(node, variant), id);
    id
}

// --- navigation stack -------------------------------------------------------

/// rebuildStack reconstructs the navigation stack from the root down to the
/// menu that contains the row for path (leaf segments are not pushed: the
/// row lives in its parent menu).
pub fn rebuild_stack(schema: &mut Schema, doc: &Doc, path: &[PathSeg]) -> Vec<MenuState> {
    fn state(kind: MenuKind, node: FormNodeId, path: Vec<PathSeg>) -> MenuState {
        MenuState {
            kind,
            node,
            path,
            cursor: 0,
            offset: 0,
            variant_of: None,
            one_of: None,
            sel: HashSet::new(),
            visual: false,
            vanchor: 0,
            search_ti: Default::default(),
            searching: false,
            search_cursor: 0,
        }
    }
    let root = schema.root;
    let mut synth: HashMap<SynthKey, FormNodeId> = HashMap::new();
    let mut stack = vec![state(menu_kind_for(schema, root), root, Vec::new())];
    let mut cur = root;
    let mut cur_path: Vec<PathSeg> = Vec::new();

    /// raw_runtime reports whether Enter on this raw row opens the runtime
    /// editor (any-type menus) rather than the raw textarea.
    fn raw_runtime(schema: &Schema, f: FormNodeId) -> bool {
        let n = &schema.nodes[f as usize];
        n.kind == Kind::Raw && !n.null_only && !n.raw_editor
    }

    /// runtime_descend pushes the schema-less Object/Array editor for the
    /// document value at path — the menu Enter would open for an unknown
    /// key, a raw leaf, or an array item without a schema type.
    fn runtime_descend(
        schema: &mut Schema,
        synth: &mut HashMap<SynthKey, FormNodeId>,
        doc: &Doc,
        stack: &mut Vec<MenuState>,
        path: &[PathSeg],
    ) -> Option<FormNodeId> {
        let want = match doc.get(path).map(|v| doc.value(v)) {
            Some(Value::Object(_)) => Kind::Object,
            Some(Value::Array(_)) => Kind::Array,
            _ => return None,
        };
        let any = any_type_menu(schema, synth);
        let node = schema.nodes[any as usize]
            .variants
            .iter()
            .copied()
            .find(|&v| {
                let n = &schema.nodes[v as usize];
                n.key.is_empty() && n.kind == want
            })?;
        let kind = menu_kind_for(schema, node);
        stack.push(state(kind, node, path.to_vec()));
        Some(node)
    }

    for seg in path {
        match seg {
            PathSeg::Key(s) => {
                if schema.nodes[cur as usize].kind == Kind::OneOf {
                    let v = doc.get(&cur_path);
                    let idx = current_variant(schema, cur, doc, v);
                    if idx < 0 {
                        return stack;
                    }
                    let variant = schema.nodes[cur as usize].variants[idx as usize];
                    if menu_leaf(schema, variant) {
                        // A raw variant still opens the runtime editor for a
                        // container value; scalars edit in place.
                        if !raw_runtime(schema, variant) {
                            return stack;
                        }
                        match runtime_descend(schema, &mut synth, doc, &mut stack, &cur_path) {
                            Some(node) => cur = node,
                            None => return stack,
                        }
                    } else {
                        let kind = menu_kind_for(schema, variant);
                        let mut st = state(kind, variant, cur_path.clone());
                        st.one_of = Some(cur);
                        stack.push(st);
                        cur = variant;
                    }
                }
                let node_kind = schema.nodes[cur as usize].kind;
                let cpath = path_for(&cur_path, s);
                let child = if node_kind == Kind::Object {
                    schema.nodes[cur as usize]
                        .children
                        .iter()
                        .copied()
                        .find(|c| schema.nodes[*c as usize].key == *s)
                } else if node_kind == Kind::Map {
                    // any doc key resolves against the map's value schema
                    let items = schema.nodes[cur as usize].items;
                    match (items, doc.get(&cpath)) {
                        (Some(items), Some(_)) => Some(items),
                        _ => None,
                    }
                } else {
                    None
                };
                let Some(child) = child else {
                    // A document key with no schema node (schema-less file or
                    // a key the schema does not define): Enter on its row
                    // opens the runtime editor, so the jump descends the same
                    // way through objects and arrays.
                    if node_kind == Kind::Object {
                        if let Some(node) =
                            runtime_descend(schema, &mut synth, doc, &mut stack, &cpath)
                        {
                            cur = node;
                            cur_path = cpath;
                            continue;
                        }
                        if doc.get(&cpath).is_some() {
                            return stack; // scalar row lives in this menu
                        }
                    }
                    continue; // extra key without schema: skip the segment
                };
                if schema.nodes[child as usize].kind == Kind::OneOf {
                    // an existing value edits its variant directly; the
                    // picker stays one t away
                    if let Some(v) = doc.get(&cpath) {
                        if !is_null(doc, v) {
                            let vi = current_variant(schema, child, doc, Some(v));
                            if vi >= 0 {
                                let variant = schema.nodes[child as usize].variants[vi as usize];
                                if !menu_leaf(schema, variant) {
                                    let kind = menu_kind_for(schema, variant);
                                    let mut st = state(kind, variant, cpath.clone());
                                    st.one_of = Some(child);
                                    stack.push(st);
                                    cur = variant;
                                    cur_path = cpath;
                                    continue;
                                }
                                // A raw leaf variant opens the runtime editor
                                // for a container value; other leaf variants
                                // keep their row in the current menu.
                                if raw_runtime(schema, variant) {
                                    if let Some(node) =
                                        runtime_descend(schema, &mut synth, doc, &mut stack, &cpath)
                                    {
                                        cur = node;
                                        cur_path = cpath;
                                        continue;
                                    }
                                }
                                return stack;
                            }
                        }
                    }
                }
                if menu_leaf(schema, child) {
                    if raw_runtime(schema, child) {
                        if let Some(node) =
                            runtime_descend(schema, &mut synth, doc, &mut stack, &cpath)
                        {
                            cur = node;
                            cur_path = cpath;
                            continue;
                        }
                    }
                    return stack; // leaf row lives in the current top menu
                }
                let kind = menu_kind_for(schema, child);
                stack.push(state(kind, child, cpath.clone()));
                cur = child;
                cur_path = cpath;
            }
            PathSeg::Idx(s) => {
                let ipath = path_idx(&cur_path, *s);
                let items = match schema.nodes[cur as usize].kind {
                    Kind::Array => schema.nodes[cur as usize].items,
                    _ => None,
                };
                let Some(items) = items else {
                    // No element schema: a container item's row opens the
                    // runtime editor, so descend the same way.
                    if let Some(node) = runtime_descend(schema, &mut synth, doc, &mut stack, &ipath)
                    {
                        cur = node;
                        cur_path = ipath;
                        continue;
                    }
                    return stack;
                };
                if menu_leaf(schema, items) {
                    if raw_runtime(schema, items) {
                        if let Some(node) =
                            runtime_descend(schema, &mut synth, doc, &mut stack, &ipath)
                        {
                            cur = node;
                            cur_path = ipath;
                            continue;
                        }
                    }
                    return stack; // scalar item rows live in the array menu
                }
                if schema.nodes[items as usize].kind == Kind::OneOf {
                    if let Some(variants) = dynamic_variants(schema, items) {
                        let ival = doc.get(&ipath);
                        let inode = if match_variant_list(schema, &variants, doc, ival).len() == 1 {
                            let cands = match_variant_list(schema, &variants, doc, ival);
                            let mut synth = HashMap::new();
                            pinned_one_of_menu_id(schema, &mut synth, items, cands[0])
                        } else {
                            let mut synth = HashMap::new();
                            merged_one_of_menu_id(schema, &mut synth, items, &variants)
                        };
                        let mut st = state(MenuKind::Object, inode, ipath.clone());
                        st.variant_of = Some(items);
                        stack.push(st);
                        cur = inode;
                        cur_path = ipath;
                        continue;
                    }
                    // classic oneOf item: follow the matched leaf variant
                    // (oneOf variants may themselves be variant groups). A
                    // leaf variant's row lives in the array Items menu; an
                    // object variant gets its own editor menu.
                    let ival = doc.get(&ipath);
                    if matches!(ival, Some(v) if !is_null(doc, v)) {
                        if let Some(variant) = matched_leaf_variant(schema, items, doc, ival) {
                            if menu_leaf(schema, variant) {
                                if raw_runtime(schema, variant) {
                                    if let Some(node) =
                                        runtime_descend(schema, &mut synth, doc, &mut stack, &ipath)
                                    {
                                        cur = node;
                                        cur_path = ipath;
                                        continue;
                                    }
                                }
                                return stack;
                            }
                            let kind = menu_kind_for(schema, variant);
                            let mut st = state(kind, variant, ipath.clone());
                            st.one_of = Some(items);
                            stack.push(st);
                            cur = variant;
                            cur_path = ipath;
                            continue;
                        }
                    }
                    // unset item: its row is in the array Items menu
                    return stack;
                }
                let kind = menu_kind_for(schema, items);
                stack.push(state(kind, items, ipath.clone()));
                cur = items;
                cur_path = ipath;
            }
        }
    }
    for st in stack.iter_mut() {
        seed_variant_cursor(schema, st, doc);
    }
    stack
}

// --- search index -----------------------------------------------------------

/// searchIndex walks the schema subtree below st — the menu the breadcrumb
/// points at — collecting object children, the actual array items and oneOf
/// variant subtrees, plus the document keys the schema does not define.
pub fn search_index(schema: &mut Schema, doc: &Doc, st: &MenuState) -> Vec<Row> {
    let mut synth: HashMap<SynthKey, FormNodeId> = HashMap::new();
    let mut out: Vec<Row> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new(); // unknown doc paths, deduped
    walk(
        schema, doc, &mut synth, &mut out, &mut seen, st.node, &st.path,
    );
    return out;

    fn walk(
        schema: &mut Schema,
        doc: &Doc,
        synth: &mut HashMap<SynthKey, FormNodeId>,
        out: &mut Vec<Row>,
        seen: &mut HashSet<String>,
        fn_: FormNodeId,
        path: &[PathSeg],
    ) {
        // Do not synthesize paths below a container the document does not
        // actually have (especially array items past the document length).
        if !path.is_empty() {
            let Some(n) = doc.get(path) else {
                return;
            };
            if is_null(doc, n) {
                return;
            }
        }
        match schema.nodes[fn_ as usize].kind {
            Kind::Object => {
                let mut known: HashSet<String> = HashSet::new();
                let children = schema.nodes[fn_ as usize].children.clone();
                for c in children {
                    known.insert(schema.nodes[c as usize].key.clone());
                    let cpath = path_for(path, &schema.nodes[c as usize].key);
                    out.push(Row {
                        kind: RowKind::Field,
                        fn_: Some(c),
                        path: cpath.clone(),
                        idx: -1,
                        label: format!(
                            "{}  {}",
                            path_string(&cpath),
                            summary_raw(schema, doc, c, &cpath)
                        ),
                        ..Row::default()
                    });
                    walk(schema, doc, synth, out, seen, c, &cpath);
                }
                let patterns = schema.nodes[fn_ as usize].key_patterns.clone();
                unknown_rows(schema, doc, path, &known, &patterns, seen, out);
            }
            Kind::OneOf => {
                // Index the variant fields, but not one row per variant for
                // the same key: the key's own row already represents an
                // unset value, and duplicate type rows only add noise.
                let val = doc.get(path);
                let has_val = matches!(val, Some(v) if !is_null(doc, v));
                let mut variants = schema.nodes[fn_ as usize].variants.clone();
                let mut matched: Vec<FormNodeId> = Vec::new();
                let mut pinned: Option<FormNodeId> = None;
                // A dynamic menu (every leaf an object, eventually after
                // nested oneOf groups) flattens the leaves: the merged menu
                // indexes those, while a top-level group node has no
                // children and would make every document key look unknown.
                if let Some(cands) = dynamic_variants(schema, fn_) {
                    if has_val {
                        matched = match_variant_list(schema, &cands, doc, val);
                    }
                    if matched.len() == 1 {
                        pinned = Some(matched[0]); // pinned variant menu
                    } else {
                        variants = cands; // merged menu
                    }
                } else if has_val {
                    matched = match_variant_list(schema, &variants, doc, val);
                    if matched.len() == 1 {
                        pinned = Some(matched[0]);
                    }
                }
                if let Some(v) = pinned {
                    // The value fits exactly one variant: walk it as the
                    // value at this path. An object variant indexes its
                    // fields; a container variant (anyOf string|array, as in
                    // sing-box route.rules[].rule_set) indexes its elements
                    // instead of pretending to have fields of its own.
                    walk(schema, doc, synth, out, seen, v, path);
                    return;
                }
                let mut known: HashSet<String> = HashSet::new();
                let mut patterns: Vec<String> = Vec::new();
                for &v in &variants {
                    for p in &schema.nodes[v as usize].key_patterns {
                        if !patterns.contains(p) {
                            patterns.push(p.clone());
                        }
                    }
                    let children = schema.nodes[v as usize].children.clone();
                    for c in children {
                        let key = schema.nodes[c as usize].key.clone();
                        if known.contains(&key) {
                            continue;
                        }
                        known.insert(key.clone());
                        let cpath = path_for(path, &key);
                        out.push(Row {
                            kind: RowKind::Field,
                            fn_: Some(c),
                            path: cpath.clone(),
                            idx: -1,
                            label: format!(
                                "{}  {}",
                                path_string(&cpath),
                                summary_raw(schema, doc, c, &cpath)
                            ),
                            ..Row::default()
                        });
                        walk(schema, doc, synth, out, seen, c, &cpath);
                    }
                }
                unknown_rows(schema, doc, path, &known, &patterns, seen, out);
                // Matched variants that are containers have no fields to
                // union; an array value still has elements. Walk the first
                // matched array variant (the candidates describe the same
                // value, so one walk is enough) to keep them searchable.
                if let Some(v) = matched
                    .iter()
                    .copied()
                    .find(|&v| schema.nodes[v as usize].kind == Kind::Array)
                {
                    walk(schema, doc, synth, out, seen, v, path);
                }
            }
            Kind::Map => {
                // every document key is a known member; values index
                // against the map's value schema
                if let Some(cur) = doc.get(path) {
                    if matches!(doc.value(cur), Value::Object(_)) {
                        let vn = match schema.nodes[fn_ as usize].items {
                            Some(vn) => vn,
                            None => fallback_raw_id(schema, synth, fn_),
                        };
                        let members = match doc.value(cur) {
                            Value::Object(m) => m.clone(),
                            _ => unreachable!(),
                        };
                        for mem in members {
                            if !doc.nodes[mem.val as usize].merge.is_empty() {
                                continue; // a merge-key spec is not a searchable key
                            }
                            let cpath = path_for(path, &mem.key);
                            out.push(Row {
                                kind: RowKind::Field,
                                fn_: Some(vn),
                                path: cpath.clone(),
                                idx: -1,
                                label: format!(
                                    "{}  {}",
                                    path_string(&cpath),
                                    summary_raw(schema, doc, vn, &cpath)
                                ),
                                ..Row::default()
                            });
                            walk(schema, doc, synth, out, seen, vn, &cpath);
                        }
                    }
                }
            }
            Kind::Array => {
                let items = schema.nodes[fn_ as usize].items;
                // Only actual items are searchable; schema-only indexes
                // would create paths whose parent does not exist. Enum items
                // are document values too (the toggle menu edits the array,
                // not the schema), and an item without its own schema —
                // missing `items`, or a nested array degraded to Raw — is
                // still a value: index it (and everything below it) raw.
                let n = doc_array_len(doc, path);
                for i in 0..n {
                    let ipath = path_idx(path, i);
                    let (item_fn, raw) = match items {
                        Some(it) => (it, schema.nodes[it as usize].kind == Kind::Raw),
                        None => (fallback_raw_id(schema, synth, fn_), true),
                    };
                    let kind = if menu_leaf(schema, item_fn) {
                        RowKind::Field
                    } else {
                        RowKind::Drill
                    };
                    let overview = if raw {
                        raw_value_summary(schema, doc, &ipath)
                    } else {
                        summary_raw(schema, doc, item_fn, &ipath)
                    };
                    out.push(Row {
                        kind,
                        fn_: Some(item_fn),
                        path: ipath.clone(),
                        idx: -1,
                        label: format!("{}  {}", path_string(&ipath), overview),
                        ..Row::default()
                    });
                    walk(schema, doc, synth, out, seen, item_fn, &ipath);
                }
            }
            // A node the form layer could not model (boolean schema,
            // recursive $ref, unsupported shape) still has a document value:
            // index everything below it raw, like an unknown key's value.
            Kind::Raw => unknown_value_rows(schema, doc, path, doc.get(path), false, seen, out),
            _ => {}
        }
    }
}

/// unknownRows indexes the members of the object at path that the schema
/// does not define (known), as rowUnknown with raw value overviews.
fn unknown_rows(
    schema: &Schema,
    doc: &Doc,
    path: &[PathSeg],
    known: &HashSet<String>,
    patterns: &[String],
    seen: &mut HashSet<String>,
    out: &mut Vec<Row>,
) {
    let Some(cur) = doc.get(path) else {
        return;
    };
    let members = match doc.value(cur) {
        Value::Object(m) => m.clone(),
        _ => return,
    };
    for mem in members {
        if !doc.nodes[mem.val as usize].merge.is_empty() {
            continue; // a merge-key spec is not a searchable key
        }
        if known.contains(&mem.key) {
            continue;
        }
        let p = path_for(path, &mem.key);
        let k = path_string(&p);
        if !seen.contains(&k) {
            seen.insert(k);
            let in_schema = key_matches_patterns(&mem.key, patterns);
            let mut row = unknown_row(schema, doc, &p);
            row.in_schema = in_schema;
            out.push(row);
            unknown_value_rows(schema, doc, &p, Some(mem.val), in_schema, seen, out);
        }
    }
}

/// patternDefined reports whether key matches a patternProperties regexp of
/// an object fallback node, making it schema-defined even though the form
/// cannot map it to a typed field.
fn pattern_defined(schema: &Schema, fn_: FormNodeId, key: &str) -> bool {
    key_matches_patterns(key, &schema.nodes[fn_ as usize].key_patterns)
}

fn key_matches_patterns(key: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| {
        regex::Regex::new(p)
            .map(|re| re.is_match(key))
            .unwrap_or(false)
    })
}

/// unknownRow builds one schema-less index row.
pub fn unknown_row(schema: &Schema, doc: &Doc, p: &[PathSeg]) -> Row {
    Row {
        kind: RowKind::Unknown,
        idx: -1,
        path: p.to_vec(),
        label: format!("{}  {}", path_string(p), raw_value_summary(schema, doc, p)),
        ..Row::default()
    }
}

/// unknownValueRows indexes everything nested inside a schema-less value.
fn unknown_value_rows(
    schema: &Schema,
    doc: &Doc,
    path: &[PathSeg],
    v: Option<NodeId>,
    in_schema: bool,
    seen: &mut HashSet<String>,
    out: &mut Vec<Row>,
) {
    let Some(v) = v else {
        return;
    };
    match doc.value(v) {
        Value::Object(members) => {
            for mem in members.clone() {
                if !doc.nodes[mem.val as usize].merge.is_empty() {
                    continue;
                }
                let p = path_for(path, &mem.key);
                let k = path_string(&p);
                if !seen.contains(&k) {
                    seen.insert(k);
                    let mut row = unknown_row(schema, doc, &p);
                    row.in_schema = in_schema;
                    out.push(row);
                    unknown_value_rows(schema, doc, &p, Some(mem.val), in_schema, seen, out);
                }
            }
        }
        Value::Array(items) => {
            for (i, &item) in items.iter().enumerate() {
                let p = path_idx(path, i);
                let k = path_string(&p);
                if !seen.contains(&k) {
                    seen.insert(k);
                    let mut row = unknown_row(schema, doc, &p);
                    row.in_schema = in_schema;
                    out.push(row);
                    unknown_value_rows(schema, doc, &p, Some(item), in_schema, seen, out);
                }
            }
        }
        _ => {}
    }
}

/// rawValueSummary renders a document value with no schema behind it
/// (unknown keys): scalars as text, containers compactly.
pub fn raw_value_summary(schema: &Schema, doc: &Doc, path: &[PathSeg]) -> String {
    let Some(v) = doc.get(path) else {
        return String::new();
    };
    match doc.value(v) {
        Value::Null => "—".to_string(),
        Value::Str(s) => value_clip(s),
        Value::Num(n) => n.clone(),
        Value::Bool(b) => {
            if *b {
                "yes".into()
            } else {
                "no".into()
            }
        }
        Value::Object(_) => {
            let s = object_summary(schema, None, doc, Some(v));
            if !s.is_empty() { s } else { "{}".to_string() }
        }
        Value::Array(items) => format!("{} items", items.len()),
    }
}

// --- unknown-key issues -----------------------------------------------------

/// unknownKeyIssues collects the document keys no schema node defines —
/// the same keys the menus flag "(not in schema)" — as editor-level
/// warnings for the issues list.
pub fn unknown_key_issues(schema: &Schema, doc: &Doc) -> Vec<Issue> {
    // Without a schema every key is "unknown"; that is the editing model,
    // not a problem worth flagging.
    if schema.schema_less {
        return Vec::new();
    }
    let mut out: Vec<Issue> = Vec::new();
    let model = schema.root;
    walk(schema, doc, &mut out, model, &[], doc.root);
    return out;

    fn emit(out: &mut Vec<Issue>, path: &[PathSeg], key: &str) {
        let p = path_for(path, key);
        out.push(Issue {
            msg: format!(
                "at '{}': unknown key {} (not in the schema)",
                path_string(&p),
                go_quote(key)
            ),
            path: Some(p),
            ..Issue::default()
        });
        // warn flag set below to keep Default concise
    }
    fn set_warn(i: &mut Issue) {
        i.warn = true;
    }

    fn unknown_of(
        doc: &Doc,
        out: &mut Vec<Issue>,
        val: NodeId,
        known: &HashSet<String>,
        patterns: &[String],
        path: &[PathSeg],
    ) {
        let members = match doc.value(val) {
            Value::Object(m) => m.clone(),
            _ => return,
        };
        for mem in members {
            if !doc.nodes[mem.val as usize].merge.is_empty() {
                continue; // a merge-key spec is YAML machinery, not a key
            }
            if path.is_empty() && mem.key == "$schema" {
                continue; // the editor's own schema pointer
            }
            if known.contains(&mem.key) || key_matches(&mem.key, patterns) {
                continue;
            }
            emit(out, path, &mem.key);
            set_warn(out.last_mut().unwrap());
        }
    }

    fn key_matches(key: &str, patterns: &[String]) -> bool {
        patterns.iter().any(|p| {
            regex::Regex::new(p)
                .map(|re| re.is_match(key))
                .unwrap_or(false)
        })
    }

    fn walk(
        schema: &Schema,
        doc: &Doc,
        out: &mut Vec<Issue>,
        fn_: FormNodeId,
        path: &[PathSeg],
        val: NodeId,
    ) {
        match schema.nodes[fn_ as usize].kind {
            Kind::Object => {
                let mut known: HashSet<String> = HashSet::new();
                let children = schema.nodes[fn_ as usize].children.clone();
                for c in children {
                    known.insert(schema.nodes[c as usize].key.clone());
                    if let Some(m) = member_of(doc, val, &schema.nodes[c as usize].key) {
                        walk(
                            schema,
                            doc,
                            out,
                            c,
                            &path_for(path, &schema.nodes[c as usize].key),
                            m,
                        );
                    }
                }
                let patterns = schema.nodes[fn_ as usize].key_patterns.clone();
                unknown_of(doc, out, val, &known, &patterns, path);
            }
            Kind::OneOf => {
                let variants = schema.nodes[fn_ as usize].variants.clone();
                let cands = match_variant_list(schema, &variants, doc, Some(val));
                if cands.len() == 1 {
                    walk(schema, doc, out, cands[0], path, val);
                    return;
                }
                // value unset, ambiguous or unmatched: any key any variant
                // defines counts as known; nested oneOf groups flatten to
                // their object leaves first
                let variants = match dynamic_variants(schema, fn_) {
                    Some(v) => v,
                    None => variants,
                };
                let mut known: HashSet<String> = HashSet::new();
                let mut patterns: Vec<String> = Vec::new();
                for &v in &variants {
                    for p in &schema.nodes[v as usize].key_patterns {
                        if !patterns.contains(p) {
                            patterns.push(p.clone());
                        }
                    }
                    let children = schema.nodes[v as usize].children.clone();
                    for c in children {
                        let key = schema.nodes[c as usize].key.clone();
                        if known.contains(&key) {
                            continue;
                        }
                        known.insert(key.clone());
                        if let Some(m) = member_of(doc, val, &key) {
                            walk(schema, doc, out, c, &path_for(path, &key), m);
                        }
                    }
                }
                unknown_of(doc, out, val, &known, &patterns, path);
            }
            Kind::Map => {
                // every document key is defined by the map's additionalProperties
                if let Some(items) = schema.nodes[fn_ as usize].items {
                    if let Value::Object(members) = doc.value(val) {
                        for mem in members.clone() {
                            walk(schema, doc, out, items, &path_for(path, &mem.key), mem.val);
                        }
                    }
                }
            }
            Kind::Array => {
                if let Some(items) = schema.nodes[fn_ as usize].items {
                    if let Value::Array(arr) = doc.value(val) {
                        for (i, &item) in arr.iter().enumerate() {
                            walk(schema, doc, out, items, &path_idx(path, i), item);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// memberOf returns the value of key in an object node, None when absent.
pub fn member_of(doc: &Doc, n: NodeId, key: &str) -> Option<NodeId> {
    match doc.value(n) {
        Value::Object(members) => members.iter().find(|m| m.key == key).map(|m| m.val),
        _ => None,
    }
}

/// rowScalarValue reports whether the row's own value is a scalar: its
/// document value, or the schema default when the key is unset.
pub fn row_scalar_value(ctx: &MenuCtx, r: &Row) -> bool {
    row_scalar_value_raw(ctx.schema, ctx.doc, r)
}

fn row_scalar_value_raw(schema: &Schema, doc: &Doc, r: &Row) -> bool {
    let mut n = doc.get(&r.path).filter(|&v| !is_null(doc, v));
    let mut vdoc = doc;
    if n.is_none() {
        match r.fn_.and_then(|f| schema.nodes[f as usize].default_) {
            Some(d) => {
                n = Some(d);
                vdoc = &schema.doc;
            }
            None => return false,
        }
    }
    matches!(
        vdoc.value(n.unwrap()),
        Value::Str(_) | Value::Num(_) | Value::Bool(_)
    )
}

/// rowSearchText is the text-mode match target of a row.
pub fn row_search_text(ctx: &MenuCtx, r: &Row) -> Option<String> {
    row_search_text_raw(ctx.schema, ctx.doc, r)
}

fn row_search_text_raw(schema: &Schema, doc: &Doc, r: &Row) -> Option<String> {
    let Some(f) = r.fn_ else {
        if r.kind != RowKind::Unknown {
            return None;
        }
        return Some(unknown_match_text(r, row_scalar_value_raw(schema, doc, r)));
    };
    let node = &schema.nodes[f as usize];
    let mut text = format!("{} {}", node.title, node.key);
    // Map rows (additionalProperties/patternProperties) edit against the
    // value schema, whose key is the map's key, not the document member's.
    // The member key is the row's identity, so it must be searchable.
    if let Some(PathSeg::Key(k)) = r.path.last() {
        if *k != node.key {
            text += &format!(" {k}");
        }
    }
    if row_scalar_value_raw(schema, doc, r) {
        let s = summary_raw(schema, doc, f, &r.path);
        if s != "—" {
            text += &format!(" {s}");
        }
    }
    Some(text)
}

// --- search filtering --------------------------------------------------------

/// filterSearch narrows the index to rows matching q. A query starting with
/// "$" is a document-path query; a query starting with "." matches the
/// keyword against path segments from any node; any other query is ordinary
/// text (title, key and scalar value).
pub fn filter_search(
    ctx: &MenuCtx,
    all: &[Row],
    q: &str,
    fold: bool,
    re: Option<&Regex>,
) -> Vec<Row> {
    let (schema, doc) = (ctx.schema, ctx.doc);
    if q.is_empty() {
        return all.to_vec();
    }
    if is_path_query(q) {
        let mut picked = vec![false; all.len()];
        let mut out: Vec<Row> = Vec::new();
        if let Some(re) = re {
            for (i, r) in all.iter().enumerate() {
                let body = path_body(&r.path);
                if re.find(&body).map(|m| m.start()) == Some(0) {
                    picked[i] = true;
                    out.push(r.clone());
                }
            }
            out.sort_by_key(|r| {
                let body = path_body(&r.path);
                !matches!(re.find(&body), Some(m) if m.start() == 0 && m.end() == body.len())
            });
        } else {
            let needle = if fold {
                q.to_lowercase()
            } else {
                q.to_string()
            };
            for (i, r) in all.iter().enumerate() {
                let mut p = path_string(&r.path);
                if fold {
                    p = p.to_lowercase();
                }
                if p.starts_with(&needle) {
                    picked[i] = true;
                    out.push(r.clone());
                }
            }
            out.sort_by_key(|r| {
                let mut p = path_string(&r.path);
                if fold {
                    p = p.to_lowercase();
                }
                p != needle
            });
        }
        out.extend(text_search_rows_raw(
            schema,
            doc,
            all,
            q,
            fold,
            None,
            Some(&picked),
            true,
        ));
        return out;
    }
    if is_dot_query(q) {
        let mut picked = vec![false; all.len()];
        let mut out = dot_query_rows(all, &mut picked, q, fold);
        out.extend(text_search_rows_raw(
            schema,
            doc,
            all,
            q,
            fold,
            None,
            Some(&picked),
            true,
        ));
        return out;
    }
    text_search_rows_raw(schema, doc, all, q, fold, re, None, false)
}

/// isPathQuery reports whether q is a document-path query: "$", "$.", or
/// "$[" prefix.
pub fn is_path_query(q: &str) -> bool {
    q == "$" || q.starts_with("$.") || q.starts_with("$[")
}

/// isDotQuery reports whether q is an any-node path query: "." plus at
/// least one more character.
pub fn is_dot_query(q: &str) -> bool {
    q.len() > 1 && q.starts_with('.')
}

/// dotQueryRows ranks rows whose path matches a leading-dot query from any
/// node (dotQueryRank); ties keep document order. Rows it lists are marked
/// in picked so the text fallback skips them.
pub fn dot_query_rows(all: &[Row], picked: &mut [bool], q: &str, fold: bool) -> Vec<Row> {
    let mut hits: Vec<(Row, usize)> = Vec::new();
    for (i, r) in all.iter().enumerate() {
        if let Some(rank) = dot_query_rank(&r.path, &q[1..], fold) {
            picked[i] = true;
            hits.push((r.clone(), rank));
        }
    }
    hits.sort_by_key(|h| h.1);
    hits.into_iter().map(|h| h.0).collect()
}

/// dotQueryRank scores one row path against the keyword after the leading
/// dot; rank 0+ for a run ending in the leaf segment, 16+ for one ending
/// higher up; None when nothing matches.
pub fn dot_query_rank(path: &[PathSeg], keyword: &str, fold: bool) -> Option<usize> {
    let mut parts: Vec<String> = keyword.split('.').map(String::from).collect();
    if fold {
        parts = parts.into_iter().map(|p| p.to_lowercase()).collect();
    }
    let segs: Vec<String> = path
        .iter()
        .map(|seg| match seg {
            PathSeg::Key(k) => k.clone(),
            PathSeg::Idx(i) => i.to_string(),
        })
        .map(|s| if fold { s.to_lowercase() } else { s })
        .collect();
    let mut best: Option<usize> = None;
    if parts.len() <= segs.len() {
        for s in 0..=segs.len() - parts.len() {
            let mut tier = 0u8;
            for (k, part) in parts.iter().enumerate() {
                let t = seg_match(&segs[s + k], part);
                if t == 0 {
                    tier = 0;
                    break;
                }
                if t > tier {
                    tier = t;
                }
            }
            if tier == 0 {
                continue;
            }
            // the run ends above the leaf: a descendant hit
            let r = if s + parts.len() == segs.len() {
                (tier - 1) as usize // ends in the leaf segment: the node itself
            } else {
                16 + (tier - 1) as usize
            };
            if best.is_none() || r < best.unwrap() {
                best = Some(r);
            }
        }
    }
    best
}

/// segMatch scores keyword against one path segment: 1 = segment prefix,
/// 2 = infix, 3 = fuzzy subsequence, 0 = no match.
pub fn seg_match(seg: &str, keyword: &str) -> u8 {
    if seg.starts_with(keyword) {
        return 1;
    }
    if seg.contains(keyword) {
        return 2;
    }
    if subsequence_match(seg, keyword) {
        return 3;
    }
    0
}

/// subsequenceMatch reports whether keyword's runes appear in seg in order.
pub fn subsequence_match(seg: &str, keyword: &str) -> bool {
    let rs: Vec<char> = keyword.chars().collect();
    let mut i = 0;
    for r in seg.chars() {
        if i < rs.len() && r == rs[i] {
            i += 1;
        }
    }
    i == rs.len()
}

/// textSearchRows is the ordinary title/key/value match shared by plain
/// text queries and the value fallback of path queries.
pub fn text_search_rows(
    ctx: &MenuCtx,
    all: &[Row],
    q: &str,
    fold: bool,
    re: Option<&Regex>,
    picked: Option<&[bool]>,
    raw: bool,
) -> Vec<Row> {
    text_search_rows_raw(ctx.schema, ctx.doc, all, q, fold, re, picked, raw)
}

fn text_search_rows_raw(
    schema: &Schema,
    doc: &Doc,
    all: &[Row],
    q: &str,
    fold: bool,
    re: Option<&Regex>,
    picked: Option<&[bool]>,
    raw: bool,
) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    if let Some(re) = re {
        for (i, r) in all.iter().enumerate() {
            if picked.map(|p| p[i]).unwrap_or(false) {
                continue;
            }
            let Some(text) = (if raw {
                row_fallback_text_raw(schema, doc, r)
            } else {
                row_search_text_raw(schema, doc, r)
            }) else {
                continue;
            };
            if re.is_match(&text) {
                out.push(r.clone());
            }
        }
        out.sort_by_key(|r| {
            let target = row_key(schema, r);
            !matches!(re.find(&target), Some(m) if m.start() == 0 && m.end() == target.len())
        });
        return out;
    }
    let needle = if fold {
        q.to_lowercase()
    } else {
        q.to_string()
    };
    for (i, r) in all.iter().enumerate() {
        if picked.map(|p| p[i]).unwrap_or(false) {
            continue;
        }
        let Some(mut text) = (if raw {
            row_fallback_text_raw(schema, doc, r)
        } else {
            row_search_text_raw(schema, doc, r)
        }) else {
            continue;
        };
        if fold {
            text = text.to_lowercase();
        }
        if text.contains(&needle) {
            out.push(r.clone());
        }
    }
    let eq = |a: &str, b: &str| {
        if fold {
            a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
        } else {
            a == b
        }
    };
    out.sort_by_key(|r| if eq(&row_key(schema, r), q) { 0 } else { 1 });
    out
}

/// rowFallbackText is the path-query value fallback's match target: like
/// rowSearchText, but the row's own scalar value enters the match
/// unclipped.
pub fn row_fallback_text(ctx: &MenuCtx, r: &Row) -> Option<String> {
    row_fallback_text_raw(ctx.schema, ctx.doc, r)
}

fn row_fallback_text_raw(schema: &Schema, doc: &Doc, r: &Row) -> Option<String> {
    let Some(f) = r.fn_ else {
        if r.kind != RowKind::Unknown {
            return None;
        }
        let key = match r.path.last() {
            Some(PathSeg::Key(k)) => k.clone(),
            Some(PathSeg::Idx(i)) => i.to_string(),
            None => String::new(),
        };
        if let Some(s) = row_raw_scalar_raw(schema, doc, r) {
            return Some(format!("{key} {s}"));
        }
        return Some(key);
    };
    let node = &schema.nodes[f as usize];
    let mut text = format!("{} {}", node.title, node.key);
    if let Some(PathSeg::Key(k)) = r.path.last() {
        if *k != node.key {
            text += &format!(" {k}");
        }
    }
    if let Some(s) = row_raw_scalar_raw(schema, doc, r) {
        text += &format!(" {s}");
    }
    Some(text)
}

/// rowRawScalar returns the row's own scalar value — the document value,
/// else the schema default — in its text form.
pub fn row_raw_scalar(ctx: &MenuCtx, r: &Row) -> Option<String> {
    row_raw_scalar_raw(ctx.schema, ctx.doc, r)
}

fn row_raw_scalar_raw(schema: &Schema, doc: &Doc, r: &Row) -> Option<String> {
    let mut n = doc.get(&r.path).filter(|&v| !is_null(doc, v));
    let mut vdoc = doc;
    if n.is_none() {
        match r.fn_.and_then(|f| schema.nodes[f as usize].default_) {
            Some(d) => {
                n = Some(d);
                vdoc = &schema.doc;
            }
            None => return None,
        }
    }
    match vdoc.value(n.unwrap()) {
        Value::Str(s) => Some(s.clone()),
        Value::Num(num) => Some(num.clone()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// pathBody is a row path as a path-query match subject: the rendered path
/// without its leading "$".
pub fn path_body(path: &[PathSeg]) -> String {
    path_string(path)[1..].to_string()
}

/// pathPattern turns a document-path query into a regex over pathBody: the
/// leading "$" is dropped and JSON-path index brackets are escaped.
pub fn path_pattern(q: &str) -> String {
    let body = q.strip_prefix('$').unwrap_or(q);
    let b = body.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'[' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        match body[i + 1..].find(']') {
            Some(k0) => {
                let k = i + 1 + k0;
                if all_digits(&body[i + 1..k]) {
                    out.extend_from_slice(format!("\\[{}\\]", &body[i + 1..k]).as_bytes());
                    i = k + 1;
                    continue;
                }
            }
            None => {
                let rest = &body[i + 1..];
                if rest.is_empty() || all_digits(rest) {
                    // an index still being typed: escape so the regex stays valid
                    out.extend_from_slice(format!("\\[{rest}").as_bytes());
                    i = b.len();
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// allDigits reports whether s consists of ASCII digits and is non-empty.
pub fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// rowKey returns a row's leaf key for exact-key ranking: the last path
/// segment when it is a string, else the schema node's key.
pub fn row_key(schema: &Schema, r: &Row) -> String {
    if let Some(PathSeg::Key(s)) = r.path.last() {
        return s.clone();
    }
    if let Some(f) = r.fn_ {
        return schema.nodes[f as usize].key.clone();
    }
    String::new()
}

/// unknownMatchText is the match text of a schema-less row: its key (the
/// last path segment) plus, for a scalar value, the value overview carried
/// in the label.
pub fn unknown_match_text(r: &Row, with_value: bool) -> String {
    let key = match r.path.last() {
        Some(PathSeg::Key(k)) => k.clone(),
        Some(PathSeg::Idx(i)) => i.to_string(),
        None => String::new(),
    };
    if !with_value {
        return key;
    }
    let overview = match r.label.find("  ") {
        Some(i) => r.label[i + 2..].to_string(),
        None => r.label.clone(),
    };
    format!("{key} {overview}")
}

/// replaceAll replaces every occurrence of old in s with rep, scanning left
/// to right without re-scanning replacements. fold=true matches
/// case-insensitively. With re the query is a regular expression instead.
pub fn replace_all(
    s: &str,
    old: &str,
    rep: &str,
    fold: bool,
    re: Option<&Regex>,
) -> (String, usize) {
    if let Some(re) = re {
        return (re.replace_all(s, rep).into_owned(), re.find_iter(s).count());
    }
    if old.is_empty() {
        return (s.to_string(), 0);
    }
    if !fold {
        let n = s.matches(old).count();
        if n == 0 {
            return (s.to_string(), 0);
        }
        return (s.replace(old, rep), n);
    }
    let sr: Vec<char> = s.chars().collect();
    let or: Vec<char> = old.chars().collect();
    let mut b = String::new();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < sr.len() {
        if i + or.len() <= sr.len() && runes_fold_equal(&sr[i..i + or.len()], &or) {
            b.push_str(rep);
            n += 1;
            i += or.len();
            continue;
        }
        b.push(sr[i]);
        i += 1;
    }
    (b, n)
}

fn runes_fold_equal(a: &[char], b: &[char]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| x.to_lowercase().eq(y.to_lowercase()))
}

/// replaceHits narrows the index to rows whose document value is a string
/// containing q (per fold), relabeling each hit
/// "<path>  <old> → <new>" so the result list doubles as the replacement
/// preview.
pub fn replace_hits(
    ctx: &MenuCtx,
    all: &[Row],
    q: &str,
    rep: &str,
    fold: bool,
    re: Option<&Regex>,
) -> Vec<Row> {
    let doc = ctx.doc;
    let mut out: Vec<Row> = Vec::new();
    let mut seen: Vec<Vec<PathSeg>> = Vec::new();
    for r in all {
        if seen.iter().any(|p| path_eq(p, &r.path)) {
            continue;
        }
        let Some(n) = doc.get(&r.path) else {
            continue;
        };
        let Value::Str(s) = doc.value(n) else {
            continue;
        };
        let s = s.clone();
        seen.push(r.path.clone());
        let mut r2 = r.clone();
        if !q.is_empty() {
            let (new, c) = replace_all(&s, q, rep, fold, re);
            if c == 0 {
                continue;
            }
            r2.label = format!(
                "{}  {} → {}",
                path_string(&r.path),
                value_clip(&s),
                value_clip(&new)
            );
        }
        out.push(r2);
    }
    out
}

// --- issue parsing ----------------------------------------------------------

/// splitIssue extracts the quoted document location from a validator message
/// like "at '$.inner.x': got number, want string" and returns the detail
/// after it.
pub fn split_issue(msg: &str) -> Option<(Vec<PathSeg>, String)> {
    const AT: &str = "at '";
    let i = msg.find(AT)?;
    let rest = &msg[i + AT.len()..];
    let j = rest.find('\'')?;
    let loc = parse_path(&rest[..j])?;
    Some((loc, rest[j..].to_string()))
}

/// parseIssue extracts the jump target from a validator message: the
/// location itself, extended by the property name for "missing property"
/// and "additional properties" details.
pub fn parse_issue(msg: &str) -> Option<Vec<PathSeg>> {
    let (loc, detail) = split_issue(msg)?;
    let names = missing_properties(&detail);
    if !names.is_empty() {
        return Some(path_for(&loc, &names[0]));
    }
    if let Some(name) = issue_prop_name(&detail, "additional properties '") {
        return Some(path_for(&loc, &name));
    }
    Some(loc)
}

/// issueFlagPaths returns every location a validator message refers to.
pub fn issue_flag_paths(msg: &str) -> Vec<Vec<PathSeg>> {
    let (loc, detail) = match split_issue(msg) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let names = missing_properties(&detail);
    if !names.is_empty() {
        return names.iter().map(|name| path_for(&loc, name)).collect();
    }
    if let Some(i) = detail.find("additional propert") {
        let names = quoted_names(&detail[i..]);
        if !names.is_empty() {
            return names.iter().map(|name| path_for(&loc, name)).collect();
        }
    }
    vec![loc]
}

/// missingProperties reads the names from a missing-required-property
/// detail, singular or plural.
pub fn missing_properties(detail: &str) -> Vec<String> {
    const PLURAL: &str = "missing properties ";
    if let Some(k) = detail.find(PLURAL) {
        return quoted_names(&detail[k + PLURAL.len()..]);
    }
    issue_prop_name(detail, "missing property '")
        .map(|n| vec![n])
        .unwrap_or_default()
}

/// issuePropName reads the property name after marker (which includes its
/// opening quote) up to the closing unescaped quote.
pub fn issue_prop_name(detail: &str, marker: &str) -> Option<String> {
    let k = detail.find(marker)?;
    let s = detail[k + marker.len()..].as_bytes();
    let mut b: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\\' && i + 1 < s.len() && s[i + 1] == b'\'' {
            b.push(b'\'');
            i += 2;
            continue;
        }
        if s[i] == b'\'' {
            return Some(String::from_utf8_lossy(&b).into_owned());
        }
        b.push(s[i]);
        i += 1;
    }
    None
}

/// quotedNames returns every single-quoted token in s, resolving the \'
/// escape the library uses inside property names.
pub fn quoted_names(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\'' {
            i += 1;
            continue;
        }
        let mut b: Vec<u8> = Vec::new();
        let mut j = i + 1;
        while j < bytes.len() {
            if bytes[j] == b'\\' && j + 1 < bytes.len() && bytes[j + 1] == b'\'' {
                b.push(b'\'');
                j += 1;
                continue;
            }
            if bytes[j] == b'\'' {
                break;
            }
            b.push(bytes[j]);
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        out.push(String::from_utf8_lossy(&b).into_owned());
        i = j + 1;
    }
    out
}

/// docArrayLen returns the current length of the array at path (0 if absent).
pub fn doc_array_len(doc: &Doc, path: &[PathSeg]) -> usize {
    match doc.get(path) {
        Some(n) => match doc.value(n) {
            Value::Array(items) => items.len(),
            _ => 0,
        },
        None => 0,
    }
}

/// issueStatusLabel composes the root status row label.
pub fn issue_status_label(errs: usize, warns: usize) -> String {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    if errs > 0 && warns > 0 {
        format!(
            "{} {} validation error{} · {} {} warning{}",
            ERR,
            errs,
            plural(errs),
            WARN,
            warns,
            plural(warns)
        )
    } else if errs > 0 {
        format!("{} {} validation error{}", ERR, errs, plural(errs))
    } else {
        format!("{} {} warning{}", WARN, warns, plural(warns))
    }
}

/// variantValueSummary is the short value hint shown next to the signature
/// of the current variant.
pub fn variant_value_summary(
    schema: &Schema,
    v: FormNodeId,
    doc: &Doc,
    val: Option<NodeId>,
) -> String {
    let Some(val) = val else {
        return String::new();
    };
    if schema.nodes[v as usize].kind == Kind::OneOf {
        // nested variant group (classic picker): show the matched leaf's value
        if let Some(leaf) = matched_leaf_variant(schema, v, doc, Some(val)) {
            return variant_value_summary(schema, leaf, doc, Some(val));
        }
        return String::new();
    }
    match schema.nodes[v as usize].kind {
        Kind::Object | Kind::Map => {
            if matches!(doc.value(val), Value::Object(_)) {
                return object_summary(schema, Some(v), doc, Some(val));
            }
            String::new()
        }
        Kind::Array => array_summary(doc, Some(val)),
        Kind::Enum => {
            let enc = encode_value(doc, val);
            for o in &schema.nodes[v as usize].enum_ {
                if o.encoded == enc {
                    return o.display.clone();
                }
            }
            String::new()
        }
        _ => {
            let s = scalar_text(doc, val);
            if !s.is_empty() {
                return value_clip(&s);
            }
            String::new()
        }
    }
}

/// goQuote mirrors Go's %q for the property names embedded in issue
/// messages (plain ASCII keys in practice).
fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use regex::Regex;
    use tempfile::tempdir;

    use super::*;
    use crate::doc::jsondoc::{Member, NodeData};
    use crate::doc::{Doc, NodeId, Value};
    use crate::smodel::{EnumOpt, FormNode, Kind, Schema};
    use crate::ui::{MenuCtx, MenuKind, MenuState, NodeRef, Options, RawCodec};

    // --- fixtures -----------------------------------------------------------

    /// Builds a Schema arena plus the Doc arena its enum/default values
    /// point into (the Go tests build loose *FormNode graphs).
    struct TB {
        nodes: Vec<FormNode>,
        doc: Doc,
        root: FormNodeId,
    }

    impl TB {
        fn new() -> TB {
            TB {
                nodes: Vec::new(),
                doc: Doc::new(),
                root: 0,
            }
        }
        fn id(&mut self, n: FormNode) -> FormNodeId {
            let id = self.nodes.len() as FormNodeId;
            self.nodes.push(n);
            id
        }
        fn s(&mut self, v: &str) -> NodeId {
            self.doc.new_str(v)
        }
        fn n(&mut self, v: &str) -> NodeId {
            self.doc.new_num(v)
        }
        fn b(&mut self, v: bool) -> NodeId {
            self.doc.new_bool(v)
        }
        fn finish(self) -> Schema {
            Schema {
                nodes: self.nodes,
                root: self.root,
                defs: Vec::new(),
                doc: self.doc,
                schema_less: false,
            }
        }
    }

    /// Builds a document arena (the Go tobj/tstr/tnum helpers). Methods
    /// take `&self` so nested expressions like
    /// `db.obj(vec![("a", db.s("x"))])` compile.
    #[derive(Clone)]
    struct DB {
        nodes: std::rc::Rc<std::cell::RefCell<Vec<NodeData>>>,
    }

    impl DB {
        fn new() -> DB {
            DB {
                nodes: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            }
        }
        fn push(&self, v: Value) -> NodeId {
            let mut n = self.nodes.borrow_mut();
            let id = n.len() as NodeId;
            n.push(NodeData {
                value: v,
                ..NodeData::default()
            });
            id
        }
        fn s(&self, v: &str) -> NodeId {
            self.push(Value::Str(v.into()))
        }
        fn n(&self, v: &str) -> NodeId {
            self.push(Value::Num(v.into()))
        }
        fn b(&self, v: bool) -> NodeId {
            self.push(Value::Bool(v))
        }
        fn obj(&self, ms: Vec<(&str, NodeId)>) -> NodeId {
            self.push(Value::Object(
                ms.into_iter()
                    .map(|(k, v)| Member {
                        key: k.into(),
                        val: v,
                    })
                    .collect(),
            ))
        }
        fn arr(&self, xs: Vec<NodeId>) -> NodeId {
            self.push(Value::Array(xs))
        }
        fn finish(self, root: NodeId) -> Doc {
            Doc {
                nodes: self.nodes.borrow().clone(),
                root,
            }
        }
    }

    /// dbuild builds a whole document in one closure.
    fn dbuild(f: impl FnOnce(&DB) -> NodeId) -> Doc {
        let db = DB::new();
        let root = f(&db);
        db.finish(root)
    }

    fn tenum(b: &mut TB, vals: &[&str]) -> Vec<EnumOpt> {
        vals.iter()
            .map(|&v| {
                let val = b.s(v);
                let encoded = encode_value(&b.doc, val);
                EnumOpt {
                    display: v.into(),
                    encoded,
                    val,
                }
            })
            .collect()
    }

    fn fnode(key: &str, title: &str, kind: Kind) -> FormNode {
        FormNode {
            key: key.into(),
            title: title.into(),
            kind,
            ..FormNode::default()
        }
    }

    fn scalar(key: &str, title: &str, jt: &str) -> FormNode {
        FormNode {
            key: key.into(),
            title: title.into(),
            kind: Kind::Scalar,
            json_type: jt.into(),
            ..FormNode::default()
        }
    }

    /// testModel mirrors examples/server.schema.json.
    fn test_model(b: &mut TB) -> FormNodeId {
        let mut name = scalar("name", "Name", "string");
        name.required = true;
        let name = b.id(name);
        let mut mode = fnode("mode", "Mode", Kind::Enum);
        mode.enum_ = tenum(b, &["http", "grpc", "tcp"]);
        mode.default_ = Some(b.s("http"));
        let mode = b.id(mode);

        let mut port = scalar("port", "Port", "integer");
        port.required = true;
        port.min = Some(1024.0);
        port.max = Some(65535.0);
        port.default_ = Some(b.n("8080"));
        let port = b.id(port);
        let mut host = scalar("host", "Host", "string");
        host.default_ = Some(b.s("0.0.0.0"));
        let host = b.id(host);
        let mut debug = fnode("debug", "Debug", Kind::Boolean);
        debug.default_ = Some(b.b(false));
        let debug = b.id(debug);
        let ratio = b.id(scalar("ratio", "Ratio", "number"));
        let server = b.id(FormNode {
            key: "server".into(),
            title: "Server".into(),
            kind: Kind::Object,
            required: true,
            children: vec![port, host, debug, ratio],
            ..FormNode::default()
        });

        let mut levels_items = fnode("levels", "", Kind::Enum);
        levels_items.enum_ = tenum(b, &["debug", "info", "warn", "error"]);
        let levels_items = b.id(levels_items);
        let levels = b.id(FormNode {
            key: "levels".into(),
            title: "Levels".into(),
            kind: Kind::Array,
            min_items: 1,
            items: Some(levels_items),
            ..FormNode::default()
        });
        let tags_items = b.id(scalar("tags", "", "string"));
        let tags = b.id(FormNode {
            key: "tags".into(),
            title: "Tags".into(),
            kind: Kind::Array,
            items: Some(tags_items),
            ..FormNode::default()
        });
        let logging = b.id(FormNode {
            key: "logging".into(),
            title: "Logging".into(),
            kind: Kind::Object,
            children: vec![levels, tags],
            ..FormNode::default()
        });

        let mut src = scalar("src", "Source", "string");
        src.required = true;
        let src = b.id(src);
        let dst = b.id(scalar("dst", "Target", "string"));
        let mount = b.id(FormNode {
            key: "mounts".into(),
            title: "Mount".into(),
            kind: Kind::Object,
            children: vec![src, dst],
            ..FormNode::default()
        });
        let mounts = b.id(FormNode {
            key: "mounts".into(),
            title: "Mounts".into(),
            kind: Kind::Array,
            items: Some(mount),
            ..FormNode::default()
        });

        let mut none_v = fnode("auth", "None", Kind::Enum);
        none_v.enum_ = tenum(b, &["none"]);
        let none_v = b.id(none_v);
        let mut token = scalar("token", "Token", "string");
        token.required = true;
        let token = b.id(token);
        let token_v = b.id(FormNode {
            key: "auth".into(),
            title: "Token".into(),
            kind: Kind::Object,
            children: vec![token],
            ..FormNode::default()
        });
        let auth = b.id(FormNode {
            key: "auth".into(),
            title: "Auth".into(),
            kind: Kind::OneOf,
            variants: vec![none_v, token_v],
            ..FormNode::default()
        });

        b.id(FormNode {
            key: "$".into(),
            title: "Server Configuration".into(),
            kind: Kind::Object,
            children: vec![name, mode, server, logging, mounts, auth],
            ..FormNode::default()
        })
    }

    fn test_schema() -> Schema {
        let mut b = TB::new();
        let root = test_model(&mut b);
        b.root = root;
        b.finish()
    }

    fn test_doc() -> Doc {
        dbuild(|db| {
            db.obj(vec![
                ("name", db.s("webapp")),
                (
                    "server",
                    db.obj(vec![
                        ("port", db.n("8080")),
                        ("host", db.s("0.0.0.0")),
                        ("debug", db.b(true)),
                    ]),
                ),
                (
                    "mounts",
                    db.arr(vec![
                        db.obj(vec![("src", db.s("/data")), ("dst", db.s("/mnt"))]),
                    ]),
                ),
            ])
        })
    }

    fn test_opts() -> Options {
        Options {
            accessible: false,
            force: false,
            raw: RawCodec {
                label: "",
                ext: ".json",
                marshal_doc: Arc::new(|_: &Doc| Ok("{}".into())),
                unmarshal_doc: Arc::new(|_: &str| Err("unused".into())),
                adopt_doc: None,
                marshal_value: Arc::new(|_: NodeRef<'_>| Ok("null".into())),
                unmarshal_value: Arc::new(|_: &str| Err("unused".into())),
            },
            validate: None,
            save: None,
            path: String::new(),
            write_to: None,
            copy: None,
            paste: None,
            replace_schema_location: None,
            comment_at: None,
            set_comment: None,
            status: String::new(),
            startup_notice: String::new(),
            load_errors: vec![],
        }
    }

    /// A ready-to-use (schema, doc) pair over the test fixture with owned
    /// opts/cache, so MenuCtx can borrow from it.
    struct Fx {
        schema: Schema,
        doc: Doc,
        opts: Options,
        cache: HashMap<FormNodeId, FormNodeId>,
    }

    impl Fx {
        fn new() -> Fx {
            Fx {
                schema: test_schema(),
                doc: test_doc(),
                opts: test_opts(),
                cache: HashMap::new(),
            }
        }
        fn ctx(&self) -> MenuCtx<'_> {
            MenuCtx {
                schema: &self.schema,
                doc: &self.doc,
                opts: &self.opts,
                menu_cache: &self.cache,
                hide_empty: false,
            }
        }
    }

    fn st(kind: MenuKind, node: FormNodeId, path: Vec<PathSeg>) -> MenuState {
        MenuState {
            kind,
            node,
            path,
            cursor: 0,
            offset: 0,
            variant_of: None,
            one_of: None,
            sel: Default::default(),
            visual: false,
            vanchor: 0,
            search_ti: Default::default(),
            searching: false,
            search_cursor: 0,
        }
    }

    fn root_state(schema: &Schema) -> MenuState {
        st(menu_kind_for(schema, schema.root), schema.root, Vec::new())
    }

    fn child_of(schema: &Schema, n: FormNodeId, key: &str) -> FormNodeId {
        schema.nodes[n as usize]
            .children
            .iter()
            .copied()
            .find(|c| schema.nodes[*c as usize].key == key)
            .unwrap_or_else(|| panic!("child {key:?} not found"))
    }

    /// menuRows over a stack whose top is `states.last()`; is_root mirrors
    /// Go's `lm.stack[0] == st` check.
    fn rows_for(fx: &mut Fx, states: Vec<MenuState>, is_root: bool) -> Vec<Row> {
        let opts = test_opts();
        let top = states.last().unwrap();
        let mut env = RowsEnv::default();
        env.is_root = is_root;
        menu_rows_for(&mut fx.schema, &fx.doc, &opts, false, top, &mut env)
    }

    fn kinds(rows: &[Row]) -> Vec<RowKind> {
        rows.iter().map(|r| r.kind).collect()
    }

    fn paths(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| path_string(&r.path)).collect()
    }

    fn must_schema(src: &str) -> Schema {
        let dir = tempdir().unwrap();
        let p = dir.path().join("schema.json");
        std::fs::write(&p, src).unwrap();
        crate::smodel::load(p.to_str().unwrap()).unwrap()
    }

    // --- summary ------------------------------------------------------------

    #[test]
    fn test_summary_rules() {
        let fx = Fx::new();
        let root = fx.schema.root;
        let name = child_of(&fx.schema, root, "name");
        let mode = child_of(&fx.schema, root, "mode");
        let server = child_of(&fx.schema, root, "server");
        let port = child_of(&fx.schema, server, "port");
        let host = child_of(&fx.schema, server, "host");
        let debug = child_of(&fx.schema, server, "debug");
        let ratio = child_of(&fx.schema, server, "ratio");
        let mounts = child_of(&fx.schema, root, "mounts");
        let auth = child_of(&fx.schema, root, "auth");
        let ctx = fx.ctx();
        let cases: Vec<(FormNodeId, Vec<PathSeg>, &str)> = vec![
            (name, vec![PathSeg::key("name")], "webapp"),
            (mode, vec![PathSeg::key("mode")], "http"), // default displayed
            (
                port,
                vec![PathSeg::key("server"), PathSeg::key("port")],
                "8080",
            ),
            (
                host,
                vec![PathSeg::key("server"), PathSeg::key("host")],
                "0.0.0.0",
            ),
            (
                debug,
                vec![PathSeg::key("server"), PathSeg::key("debug")],
                "[y]",
            ),
            (
                ratio,
                vec![PathSeg::key("server"), PathSeg::key("ratio")],
                "—",
            ),
            (
                server,
                vec![PathSeg::key("server")],
                "port=8080 host=0.0.0.0 +1",
            ),
            (mounts, vec![PathSeg::key("mounts")], "1 item"),
            (auth, vec![PathSeg::key("auth")], "—"), // unset oneOf
        ];
        for (f, path, want) in cases {
            assert_eq!(
                summary(&ctx, f, &path),
                want,
                "summary({})",
                fx.schema.nodes[f as usize].key
            );
        }

        // oneOf value summaries: enum/leaf variants show the value itself,
        // object variants keep the title for context
        let d2 = dbuild(|db| db.obj(vec![("auth", db.s("none"))]));
        let ctx2 = MenuCtx {
            schema: &fx.schema,
            doc: &d2,
            opts: &fx.opts,
            menu_cache: &fx.cache,
            hide_empty: false,
        };
        assert_eq!(summary(&ctx2, auth, &[PathSeg::key("auth")]), "none");

        let d3 = dbuild(|db| db.obj(vec![("auth", db.obj(vec![("token", db.s("secret123"))]))]));
        let ctx3 = MenuCtx {
            schema: &fx.schema,
            doc: &d3,
            opts: &fx.opts,
            menu_cache: &fx.cache,
            hide_empty: false,
        };
        assert_eq!(
            summary(&ctx3, auth, &[PathSeg::key("auth")]),
            "Token token=secret123"
        );

        // untitled object variant: members only, no synthesized fallback label
        let mut b = TB::new();
        let action = b.id(fnode("action", "", Kind::Scalar));
        let v = b.id(FormNode {
            key: "".into(),
            kind: Kind::Object,
            children: vec![action],
            ..FormNode::default()
        });
        let untitled = b.id(FormNode {
            key: "rules".into(),
            kind: Kind::OneOf,
            variants: vec![v],
            ..FormNode::default()
        });
        let untitled_schema = b.finish();
        let d3b = dbuild(|db| db.obj(vec![("rules", db.obj(vec![("action", db.s("sniff"))]))]));
        let ctx3b = MenuCtx {
            schema: &untitled_schema,
            doc: &d3b,
            opts: &fx.opts,
            menu_cache: &fx.cache,
            hide_empty: false,
        };
        assert_eq!(
            summary(&ctx3b, untitled, &[PathSeg::key("rules")]),
            "action=sniff"
        );

        // empty and long arrays
        let d4 = dbuild(|db| db.obj(vec![("mounts", db.arr(vec![]))]));
        let ctx4 = MenuCtx {
            schema: &fx.schema,
            doc: &d4,
            opts: &fx.opts,
            menu_cache: &fx.cache,
            hide_empty: false,
        };
        assert_eq!(summary(&ctx4, mounts, &[PathSeg::key("mounts")]), "empty");

        let d5 = dbuild(|db| {
            db.obj(vec![(
                "mounts",
                db.arr(vec![db.obj(vec![]), db.obj(vec![])]),
            )])
        });
        let ctx5 = MenuCtx {
            schema: &fx.schema,
            doc: &d5,
            opts: &fx.opts,
            menu_cache: &fx.cache,
            hide_empty: false,
        };
        assert_eq!(summary(&ctx5, mounts, &[PathSeg::key("mounts")]), "2 items");

        // long string clipped to 32 runes including ellipsis
        let long: String = "a".repeat(40);
        let d6 = dbuild(|db| db.obj(vec![("name", db.s(&long))]));
        let ctx6 = MenuCtx {
            schema: &fx.schema,
            doc: &d6,
            opts: &fx.opts,
            menu_cache: &fx.cache,
            hide_empty: false,
        };
        assert_eq!(
            summary(&ctx6, name, &[PathSeg::key("name")]),
            format!("{}…", &long[..31])
        );

        // string lists preview their members: [N] v0 v1 v2 +rest
        let mut b = TB::new();
        let tags = b.id(FormNode {
            key: "tags".into(),
            kind: Kind::Array,
            ..FormNode::default()
        });
        let tags_schema = b.finish();
        let p = [PathSeg::key("tags")];
        macro_rules! tags_summary {
            ($build:expr) => {{
                let doc = dbuild($build);
                let ctx = MenuCtx {
                    schema: &tags_schema,
                    doc: &doc,
                    opts: &fx.opts,
                    menu_cache: &fx.cache,
                    hide_empty: false,
                };
                summary(&ctx, tags, &p)
            }};
        }
        assert_eq!(
            tags_summary!(|db: &DB| db.obj(vec![(
                "tags",
                db.arr(vec![db.s("a"), db.s("b"), db.s("c"), db.s("d")])
            )])),
            "[4] a b c +1"
        );
        assert_eq!(
            tags_summary!(|db: &DB| db.obj(vec![("tags", db.arr(vec![db.s("a"), db.s("b")]))])),
            "[2] a b"
        );
        assert_eq!(
            tags_summary!(|db: &DB| db.obj(vec![("tags", db.arr(vec![db.s("legacy")]))])),
            "[1] legacy"
        );
        assert_eq!(
            tags_summary!(|db: &DB| db.obj(vec![("tags", db.arr(vec![db.s("a"), db.n("1")]))])),
            "[2] a 1"
        );
        // numbers preview like strings; objects in a mixed list render as {…}
        assert_eq!(
            tags_summary!(|db: &DB| db.obj(vec![(
                "tags",
                db.arr(vec![db.n("1"), db.n("22"), db.n("3.5")])
            )])),
            "[3] 1 22 3.5"
        );
        assert_eq!(
            tags_summary!(|db: &DB| db.obj(vec![(
                "tags",
                db.arr(vec![
                    db.s("a"),
                    db.n("1"),
                    db.obj(vec![("k", db.s("v"))]),
                    db.b(true)
                ])
            )])),
            "[4] a 1 {…} +1"
        );
        // pure object arrays keep the plain count
        assert_eq!(
            tags_summary!(
                |db: &DB| db.obj(vec![("tags", db.arr(vec![db.obj(vec![]), db.obj(vec![])]))])
            ),
            "2 items"
        );
        assert_eq!(
            tags_summary!(|db: &DB| db.obj(vec![("tags", db.arr(vec![db.s(&long)]))])),
            format!("[1] {}…", &long[..15])
        );

        // raw nodes
        let mut b = TB::new();
        let raw = b.id(fnode("extra", "Extra", Kind::Raw));
        let raw_schema = b.finish();
        let d7 = dbuild(|db| db.obj(vec![("extra", db.obj(vec![("a", db.n("1"))]))]));
        let ctx7 = MenuCtx {
            schema: &raw_schema,
            doc: &d7,
            opts: &fx.opts,
            menu_cache: &fx.cache,
            hide_empty: false,
        };
        assert_eq!(
            summary(&ctx7, raw, &[PathSeg::key("extra")]),
            r#"json {"a":1}"#
        );
    }

    /// TestSummaryNestedOneOfGroups covers the sing-box DNSRule shape: the
    /// item schema is a oneOf over nested oneOf groups, so the summary must
    /// descend to the matched leaf instead of falling through to "—".
    /// (The Go test's second half asserts rendered View output; rendering
    /// lands in a later phase and is covered by the pty tests.)
    #[test]
    fn test_summary_nested_one_of_groups() {
        let mut b = TB::new();
        let mut type_default = fnode("type", "", Kind::Enum);
        type_default.enum_ = tenum(&mut b, &["default"]);
        type_default.required = true;
        let type_default = b.id(type_default);
        let ds_items = b.id(scalar("domain_suffix", "", "string"));
        let domain_suffix = b.id(FormNode {
            key: "domain_suffix".into(),
            kind: Kind::Array,
            items: Some(ds_items),
            ..FormNode::default()
        });
        let server = b.id(scalar("server", "", "string"));
        let matcher = b.id(FormNode {
            key: "rules".into(),
            kind: Kind::Object,
            children: vec![type_default, domain_suffix, server],
            ..FormNode::default()
        });
        let g1 = b.id(FormNode {
            key: "rules".into(),
            kind: Kind::OneOf,
            variants: vec![matcher],
            ..FormNode::default()
        });

        let mut type_logical = fnode("type", "", Kind::Enum);
        type_logical.enum_ = tenum(&mut b, &["logical"]);
        type_logical.required = true;
        let type_logical = b.id(type_logical);
        let mut mode = fnode("mode", "", Kind::Enum);
        mode.enum_ = tenum(&mut b, &["and", "or"]);
        mode.required = true;
        let mode = b.id(mode);
        let rules_arr = b.id(FormNode {
            key: "rules".into(),
            kind: Kind::Array,
            required: true,
            ..FormNode::default()
        });
        let logical = b.id(FormNode {
            key: "rules".into(),
            kind: Kind::Object,
            children: vec![type_logical, mode, rules_arr],
            ..FormNode::default()
        });
        let g2 = b.id(FormNode {
            key: "rules".into(),
            kind: Kind::OneOf,
            variants: vec![logical],
            ..FormNode::default()
        });
        let rules = b.id(FormNode {
            key: "rules".into(),
            kind: Kind::OneOf,
            variants: vec![g1, g2],
            ..FormNode::default()
        });
        let schema = b.finish();

        let doc = dbuild(|db| {
            db.obj(vec![(
                "rules",
                db.arr(vec![
                    db.obj(vec![
                        ("type", db.s("default")),
                        ("domain_suffix", db.arr(vec![db.s("a.com"), db.s("b.com")])),
                        ("server", db.s("dns-local")),
                    ]),
                    db.obj(vec![
                        ("type", db.s("logical")),
                        ("mode", db.s("or")),
                        ("rules", db.arr(vec![])),
                    ]),
                ]),
            )])
        });
        let opts = test_opts();
        let cache = HashMap::new();
        let cx = MenuCtx {
            schema: &schema,
            doc: &doc,
            opts: &opts,
            menu_cache: &cache,
            hide_empty: false,
        };
        assert_eq!(
            summary(&cx, rules, &[PathSeg::key("rules"), PathSeg::Idx(0)]),
            "type=default domain_suffix=[2] +1"
        );
        assert_eq!(
            summary(&cx, rules, &[PathSeg::key("rules"), PathSeg::Idx(1)]),
            "type=logical mode=or +1"
        );
        // classic picker rows: a nested group variant shows its matched
        // leaf's value
        let logical_val = doc.get(&[PathSeg::key("rules"), PathSeg::Idx(1)]).unwrap();
        assert_eq!(
            variant_value_summary(
                &schema,
                schema.nodes[rules as usize].variants[1],
                &doc,
                Some(logical_val)
            ),
            "type=logical mode=or +1"
        );
    }

    #[test]
    fn test_object_summary() {
        let fx = Fx::new();
        let plain = |build: &dyn Fn(&DB) -> NodeId| -> String {
            let doc = dbuild(|db| build(db));
            let rootv = doc.root;
            object_summary(&fx.schema, None, &doc, Some(rootv))
        };
        assert_eq!(object_summary(&fx.schema, None, &fx.doc, None), "{}");
        assert_eq!(plain(&|db: &DB| db.obj(vec![])), "{}");
        assert_eq!(plain(&|db: &DB| db.obj(vec![("a", db.n("1"))])), "a=1");
        assert_eq!(
            plain(&|db: &DB| db.obj(vec![("a", db.n("1")), ("b", db.b(true))])),
            "a=1 b=true"
        );
        assert_eq!(
            plain(&|db: &DB| db.obj(vec![("a", db.n("1")), ("b", db.b(true)), ("c", db.s("x"))])),
            "a=1 b=true +1"
        );
        assert_eq!(
            plain(&|db: &DB| db.obj(vec![("a", db.obj(vec![])), ("b", db.arr(vec![db.n("1")]))])),
            "a={…} b=[1]"
        );
        // null value needs a Null node; dbuild wraps a root of any shape
        let doc = dbuild(|db| db.obj(vec![("a", db.push(Value::Null))]));
        assert_eq!(
            object_summary(&fx.schema, None, &doc, Some(doc.root)),
            "a=null"
        );

        // members render in schema order even when the document lists them
        // differently: two list rows summarize alike (type before tag)
        let mut b = TB::new();
        let t = b.id(fnode("type", "", Kind::Scalar));
        let tg = b.id(fnode("tag", "", Kind::Scalar));
        let li = b.id(fnode("listen", "", Kind::Scalar));
        let po = b.id(fnode("port", "", Kind::Scalar));
        let schema = b.id(FormNode {
            key: "inbound".into(),
            kind: Kind::Object,
            children: vec![t, tg, li, po],
            ..FormNode::default()
        });
        let inbound_schema = b.finish();
        let doc = dbuild(|db| {
            db.obj(vec![
                ("tag", db.s("in-proxy")),
                ("type", db.s("mixed")),
                ("listen", db.s("::")),
                ("port", db.n("1080")),
            ])
        });
        assert_eq!(
            object_summary(&inbound_schema, Some(schema), &doc, Some(doc.root)),
            "type=mixed tag=in-proxy +2"
        );
        let doc_reordered = dbuild(|db| {
            db.obj(vec![
                ("port", db.n("1080")),
                ("listen", db.s("::")),
                ("type", db.s("mixed")),
                ("tag", db.s("in-proxy")),
            ])
        });
        assert_eq!(
            object_summary(
                &inbound_schema,
                Some(schema),
                &doc_reordered,
                Some(doc_reordered.root)
            ),
            "type=mixed tag=in-proxy +2"
        );
        // keys the schema does not know keep document order after the known ones
        let doc_unknown = dbuild(|db| {
            db.obj(vec![
                ("zz", db.n("1")),
                ("tag", db.s("t")),
                ("type", db.s("x")),
            ])
        });
        assert_eq!(
            object_summary(
                &inbound_schema,
                Some(schema),
                &doc_unknown,
                Some(doc_unknown.root)
            ),
            "type=x tag=t +1"
        );
    }

    #[test]
    fn test_variant_signature() {
        let mut b = TB::new();
        let mut cases: Vec<(FormNodeId, &str)> = Vec::new();
        cases.push((b.id(scalar("", "", "integer")), "integer"));
        cases.push((b.id(scalar("", "", "string")), "string"));
        cases.push((b.id(scalar("", "", "")), "string"));
        let mut e3 = fnode("", "", Kind::Enum);
        e3.enum_ = tenum(&mut b, &["http", "grpc", "tcp"]);
        cases.push((b.id(e3), "http |grpc |tcp"));
        let mut e4 = fnode("", "", Kind::Enum);
        e4.enum_ = tenum(&mut b, &["a", "b", "c", "d"]);
        cases.push((b.id(e4), "a |b |c …"));
        cases.push((b.id(fnode("", "", Kind::Boolean)), "yes/no"));
        cases.push((b.id(fnode("", "", Kind::Raw)), "json"));
        let s_item = b.id(scalar("", "", "string"));
        cases.push((
            b.id(FormNode {
                key: "".into(),
                kind: Kind::Array,
                items: Some(s_item),
                ..FormNode::default()
            }),
            "list of string",
        ));
        cases.push((
            b.id(FormNode {
                key: "".into(),
                kind: Kind::Array,
                ..FormNode::default()
            }),
            "list of json",
        ));
        cases.push((
            b.id(FormNode {
                key: "".into(),
                kind: Kind::Object,
                ..FormNode::default()
            }),
            "{}",
        ));
        let src = b.id(fnode("src", "", Kind::Scalar));
        b.nodes[src as usize].required = true;
        let dst = b.id(fnode("dst", "", Kind::Scalar));
        cases.push((
            b.id(FormNode {
                key: "".into(),
                kind: Kind::Object,
                children: vec![src, dst],
                ..FormNode::default()
            }),
            "{src*, dst}",
        ));
        let mut five: Vec<FormNodeId> = Vec::new();
        for k in ["a", "b", "c", "d", "e", "f"] {
            let c = b.id(fnode(k, "", Kind::Scalar));
            if k == "b" {
                b.nodes[c as usize].required = true;
            }
            five.push(c);
        }
        cases.push((
            b.id(FormNode {
                key: "".into(),
                kind: Kind::Object,
                children: five,
                ..FormNode::default()
            }),
            "{a, b*, c, d, +2}",
        ));
        // required const child shows its fixed value (sing-box style
        // discriminated variants: {type: {const: "direct"}})
        let mut ctype = fnode("type", "", Kind::Enum);
        ctype.enum_ = tenum(&mut b, &["direct"]);
        ctype.required = true;
        let ctype = b.id(ctype);
        let tag = b.id(fnode("tag", "", Kind::Scalar));
        let listen = b.id(fnode("listen", "", Kind::Scalar));
        cases.push((
            b.id(FormNode {
                key: "".into(),
                kind: Kind::Object,
                children: vec![ctype, tag, listen],
                ..FormNode::default()
            }),
            "{type=direct, tag, listen}",
        ));
        // optional const and multi-option required keep the plain/star forms
        let mut cotype = fnode("type", "", Kind::Enum);
        cotype.enum_ = tenum(&mut b, &["direct"]);
        let cotype = b.id(cotype);
        let mut cmode = fnode("mode", "", Kind::Enum);
        cmode.enum_ = tenum(&mut b, &["a", "b"]);
        cmode.required = true;
        let cmode = b.id(cmode);
        cases.push((
            b.id(FormNode {
                key: "".into(),
                kind: Kind::Object,
                children: vec![cotype, cmode],
                ..FormNode::default()
            }),
            "{type, mode*}",
        ));
        // a nested oneOf (allOf-distributed detail branch in Cargo.toml)
        // stands in its variants' signatures, not "json"; identical object
        // leaves (has version / path / git / workspace) dedupe to one
        let v1 = b.id(scalar("version", "", "string"));
        let o1 = b.id(FormNode {
            key: "".into(),
            kind: Kind::Object,
            children: vec![v1],
            ..FormNode::default()
        });
        let v2 = b.id(scalar("version", "", "string"));
        let o2 = b.id(FormNode {
            key: "".into(),
            kind: Kind::Object,
            children: vec![v2],
            ..FormNode::default()
        });
        let detailed = b.id(FormNode {
            key: "".into(),
            kind: Kind::OneOf,
            variants: vec![o1, o2],
            ..FormNode::default()
        });
        cases.push((detailed, "{version}"));
        let s1 = b.id(scalar("", "", "string"));
        let s2 = b.id(scalar("", "", "integer"));
        let distinct = b.id(FormNode {
            key: "".into(),
            kind: Kind::OneOf,
            variants: vec![s1, s2],
            ..FormNode::default()
        });
        cases.push((distinct, "string | integer"));

        let schema = b.finish();
        for (f, want) in cases {
            assert_eq!(
                variant_signature(&schema, Some(f)),
                want,
                "VariantSignature"
            );
        }
    }

    #[test]
    fn test_current_variant() {
        let schema = test_schema();
        let root = schema.root;
        let auth = child_of(&schema, root, "auth");
        let cases: Vec<(Option<NodeId>, i64)> = Vec::new();
        let _ = cases;
        let check = |doc: &Doc, want: i64, desc: &str| {
            assert_eq!(
                current_variant(&schema, auth, doc, Some(doc.root)),
                want,
                "{}",
                desc
            );
        };
        // nil value
        assert_eq!(current_variant(&schema, auth, &test_doc(), None), -1);
        // explicit null
        let dn = dbuild(|db| db.push(Value::Null));
        check(&dn, -1, "null");
        let ds = dbuild(|db| db.s("none"));
        check(&ds, 0, "enum variant match");
        let d_obj = dbuild(|db| db.obj(vec![("token", db.s("x"))]));
        check(&d_obj, 1, "key intersection");
        let d_empty = dbuild(|db| db.obj(vec![]));
        check(&d_empty, -1, "token is required → no key hit");
        let d_other = dbuild(|db| db.s("other"));
        check(&d_other, -1, "no variant matches");

        // object variant without required children matches any object
        let mut b = TB::new();
        let a = b.id(fnode("a", "", Kind::Scalar));
        let v = b.id(FormNode {
            key: "".into(),
            kind: Kind::Object,
            children: vec![a],
            ..FormNode::default()
        });
        let free = b.id(FormNode {
            key: "".into(),
            kind: Kind::OneOf,
            variants: vec![v],
            ..FormNode::default()
        });
        let free_schema = b.finish();
        let d = dbuild(|db| db.obj(vec![]));
        assert_eq!(
            current_variant(&free_schema, free, &d, Some(d.root)),
            0,
            "required-free object variant should match"
        );
    }

    // --- rows & stack --------------------------------------------------------

    #[test]
    fn test_menu_rows_root_order() {
        let mut fx = Fx::new();
        let root = fx.schema.root;
        let rows = rows_for(&mut fx, vec![st(MenuKind::Object, root, vec![])], true);
        let want = vec![
            RowKind::Field,
            RowKind::Field,
            RowKind::Drill,
            RowKind::Drill,
            RowKind::Drill,
            RowKind::Drill,
            RowKind::Save,
            RowKind::Raw,
        ];
        assert_eq!(kinds(&rows), want, "root rows: {rows:?}");
        assert_eq!(fx.schema.nodes[rows[2].fn_.unwrap() as usize].key, "server");
        assert_eq!(fx.schema.nodes[rows[5].fn_.unwrap() as usize].key, "auth");
        assert_eq!(rows[6].label, "< Save and exit >");
        assert_eq!(rows[7].label, "< Edit raw JSON >");
    }

    #[test]
    fn test_menu_rows_duplicate_child_titles_fall_back_to_keys() {
        // Cargo.toml: dev/release/bench/test all $ref the same titled
        // definition; their rows must still name the actual profile keys.
        let schema = must_schema(
            r##"{
              "type": "object",
              "properties": {"profile": {"$ref": "#/definitions/Profiles"}},
              "definitions": {
                "Profile": {
                  "title": "Profile with Build Override",
                  "type": "object",
                  "properties": {"opt-level": {"type": "integer"}}
                },
                "Special": {
                  "title": "Special profile",
                  "type": "object",
                  "properties": {"x": {"type": "string"}}
                },
                "Profiles": {
                  "title": "Profiles",
                  "type": "object",
                  "properties": {
                    "bench": {"$ref": "#/definitions/Profile"},
                    "dev": {"$ref": "#/definitions/Profile"},
                    "release": {"$ref": "#/definitions/Profile"},
                    "test": {"$ref": "#/definitions/Profile"},
                    "special": {"$ref": "#/definitions/Special"},
                    "documented": {
                      "title": "Documented field",
                      "type": "object",
                      "properties": {"y": {"type": "string"}}
                    }
                  },
                  "additionalProperties": {"$ref": "#/definitions/Profile"}
                }
              }
            }"##,
        );
        let doc = dbuild(|db| db.obj(vec![("profile", db.obj(vec![]))]));
        let mut fx = Fx {
            schema,
            doc,
            opts: test_opts(),
            cache: HashMap::new(),
        };
        let root_st = root_state(&fx.schema);
        let root_rows = rows_for(&mut fx, vec![root_st], true);
        let profile_row = root_rows
            .iter()
            .find(|r| path_string(&r.path) == "$.profile")
            .expect("profile row");
        assert_eq!(
            profile_row.label, "profile",
            "a bare $ref property must keep its key as the row label"
        );

        let profile = child_of(&fx.schema, fx.schema.root, "profile");
        let rows = rows_for(
            &mut fx,
            vec![st(MenuKind::Object, profile, vec![PathSeg::key("profile")])],
            false,
        );
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["bench", "dev", "release", "test", "special", ""],
            "ref-derived children use keys; an own title still wins: {rows:?}"
        );
    }

    #[test]
    fn test_menu_rows_item_menus() {
        let mut fx = Fx::new();
        let root = fx.schema.root;
        let mounts = child_of(&fx.schema, root, "mounts");
        let levels = child_of(&fx.schema, child_of(&fx.schema, root, "logging"), "levels");
        let mounts_items = fx.schema.nodes[mounts as usize].items.unwrap();

        // array manager: add + one drill row per item, no save rows (not root)
        let rows = rows_for(
            &mut fx,
            vec![st(MenuKind::Items, mounts, vec![PathSeg::key("mounts")])],
            false,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, RowKind::Add);
        assert_eq!(rows[1].kind, RowKind::Drill);
        assert_eq!(rows[1].idx, 0);

        // object item menu: remove row on top, then fields
        let rows2 = rows_for(
            &mut fx,
            vec![st(
                MenuKind::Object,
                mounts_items,
                vec![PathSeg::key("mounts"), PathSeg::Idx(0)],
            )],
            false,
        );
        assert_eq!(rows2.len(), 3);
        assert_eq!(rows2[0].kind, RowKind::Remove);
        assert_eq!(fx.schema.nodes[rows2[1].fn_.unwrap() as usize].key, "src");
        assert_eq!(fx.schema.nodes[rows2[2].fn_.unwrap() as usize].key, "dst");

        // scalar item menu: add + field rows
        let tags = child_of(&fx.schema, child_of(&fx.schema, root, "logging"), "tags");
        let mut fx3 = Fx::new();
        fx3.doc = dbuild(|db| {
            db.obj(vec![(
                "logging",
                db.obj(vec![("tags", db.arr(vec![db.s("prod"), db.s("dev")]))]),
            )])
        });
        let rows3 = rows_for(
            &mut fx3,
            vec![st(
                MenuKind::Items,
                tags,
                vec![PathSeg::key("logging"), PathSeg::key("tags")],
            )],
            false,
        );
        assert_eq!(rows3.len(), 3);
        assert_eq!(rows3[0].kind, RowKind::Add);
        assert_eq!(rows3[1].kind, RowKind::Field);
        assert_eq!(rows3[1].idx, 0);
        assert_eq!(rows3[2].idx, 1);

        // enum array: one toggle row per option
        let rows4 = rows_for(
            &mut fx,
            vec![st(
                MenuKind::EnumArray,
                levels,
                vec![PathSeg::key("logging"), PathSeg::key("levels")],
            )],
            false,
        );
        assert_eq!(rows4.len(), 4, "levels rows");
        for (i, r) in rows4.iter().enumerate() {
            assert_eq!(r.kind, RowKind::Toggle);
            assert_eq!(r.idx, i as i64);
        }
    }

    #[test]
    fn test_menu_rows_one_of() {
        let mut fx = Fx::new();
        let root = fx.schema.root;
        let auth = child_of(&fx.schema, root, "auth");
        let rows = rows_for(
            &mut fx,
            vec![st(MenuKind::OneOf, auth, vec![PathSeg::key("auth")])],
            false,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, RowKind::Variant);
        assert_eq!(rows[1].kind, RowKind::Variant);
        let v0 = &fx.schema.nodes[rows[0].variant.unwrap() as usize];
        let v1 = &fx.schema.nodes[rows[1].variant.unwrap() as usize];
        assert_eq!(v0.title, "None");
        assert_eq!(v1.title, "Token");
    }

    #[test]
    fn test_rebuild_stack() {
        let mut fx = Fx::new();
        let _root = fx.schema.root;

        let stx = rebuild_stack(&mut fx.schema, &fx.doc, &[PathSeg::key("server")]);
        assert_eq!(stx.len(), 2);
        assert_eq!(stx[1].kind, MenuKind::Object);
        assert_eq!(fx.schema.nodes[stx[1].node as usize].key, "server");

        // leaf field stops at the containing menu
        let stx = rebuild_stack(
            &mut fx.schema,
            &fx.doc,
            &[PathSeg::key("server"), PathSeg::key("port")],
        );
        assert_eq!(stx.len(), 2, "port stack depth");

        // array item menu
        let stx = rebuild_stack(
            &mut fx.schema,
            &fx.doc,
            &[PathSeg::key("mounts"), PathSeg::Idx(0), PathSeg::key("src")],
        );
        assert_eq!(stx.len(), 3);
        assert_eq!(stx[1].kind, MenuKind::Items);
        assert_eq!(stx[2].kind, MenuKind::Object);
        assert_eq!(fx.schema.nodes[stx[2].node as usize].key, "mounts");
        assert_eq!(stx[2].path[1], PathSeg::Idx(0), "item menu path");

        // scalar array item: row lives in the array menu
        let tags_doc = dbuild(|db| {
            db.obj(vec![(
                "logging",
                db.obj(vec![("tags", db.arr(vec![db.s("x")]))]),
            )])
        });
        let stx = rebuild_stack(
            &mut fx.schema,
            &tags_doc,
            &[
                PathSeg::key("logging"),
                PathSeg::key("tags"),
                PathSeg::Idx(0),
            ],
        );
        assert_eq!(stx.len(), 3);
        assert_eq!(stx[2].kind, MenuKind::Items);

        // oneOf descent follows the current variant; an existing value edits
        // it directly (oneOf carried for the t type switch)
        let doc_auth = dbuild(|db| db.obj(vec![("auth", db.obj(vec![("token", db.s("x"))]))]));
        let stx = rebuild_stack(
            &mut fx.schema,
            &doc_auth,
            &[PathSeg::key("auth"), PathSeg::key("token")],
        );
        assert_eq!(stx.len(), 2);
        assert_eq!(fx.schema.nodes[stx[1].node as usize].title, "Token");
        assert!(stx[1].one_of.is_some());

        // unset oneOf cannot descend past the picker
        let stx = rebuild_stack(
            &mut fx.schema,
            &fx.doc,
            &[PathSeg::key("auth"), PathSeg::key("token")],
        );
        assert_eq!(stx.len(), 2);
        assert_eq!(stx[1].kind, MenuKind::OneOf);

        // unknown keys are skipped
        let stx = rebuild_stack(&mut fx.schema, &fx.doc, &[PathSeg::key("nope")]);
        assert_eq!(stx.len(), 1);
    }

    #[test]
    fn test_search_index_and_filter() {
        let mut fx = Fx::new();
        let _root = fx.schema.root;
        let rst = root_state(&fx.schema);
        let all = search_index(&mut fx.schema, &fx.doc, &rst);
        let ctx = fx.ctx();
        let find = |title: &str| -> String {
            all.iter()
                .find(|r| r.fn_.map(|f| fx.schema.nodes[f as usize].title.as_str()) == Some(title))
                .map(|r| r.label.clone())
                .unwrap_or_default()
        };
        // labels are breadcrumb-style paths plus the menu value overview
        assert_eq!(find("Source"), "$.mounts[0].src  /data");
        assert_eq!(find("Server"), "$.server  port=8080 host=0.0.0.0 +1");
        // no phantom child rows below an absent oneOf parent
        assert_eq!(find("Token"), "");
        assert!(
            !all.iter()
                .any(|r| path_eq(&r.path, &[PathSeg::key("auth"), PathSeg::key("token")]))
        );
        assert!(
            all.iter().any(|r| {
                path_eq(&r.path, &[PathSeg::key("auth")])
                    && r.kind == RowKind::Field
                    && r.label.ends_with("—")
            }),
            "unset oneOf key row must be searchable"
        );
        for r in &all {
            assert!(
                !r.label.contains("variant "),
                "label leaks a fallback variant title: {:?}",
                r.label
            );
        }
        // filter by title and key, case-insensitive
        assert_eq!(filter_search(&ctx, &all, "token", true, None).len(), 0); // parent absent
        assert_eq!(filter_search(&ctx, &all, "SRC", true, None).len(), 1); // Source row only
        // fold=false demands the exact case
        assert_eq!(filter_search(&ctx, &all, "SRC", false, None).len(), 0);
        assert_eq!(filter_search(&ctx, &all, "src", false, None).len(), 1); // $.mounts[0].src
        assert_eq!(filter_search(&ctx, &all, "", true, None).len(), all.len());
        // a row matches with its own value: the container above it only
        // rolls its child values up, so the roll-up is no match target
        assert_eq!(filter_search(&ctx, &all, "8080", true, None).len(), 1); // $.server.port
        assert_eq!(filter_search(&ctx, &all, "port=8080", true, None).len(), 0);
        // values with a slash stay ordinary text: only a "$" prefix switches
        // to path mode
        assert_eq!(filter_search(&ctx, &all, "mnt", true, None).len(), 1); // $.mounts[0].dst
        assert_eq!(filter_search(&ctx, &all, "/mnt", true, None).len(), 1);

        // scope: searching from inside Server stays below the breadcrumb
        let mut inner_fx = Fx::new();
        let server = child_of(&inner_fx.schema, inner_fx.schema.root, "server");
        let inner_all = search_index(
            &mut inner_fx.schema,
            &inner_fx.doc,
            &st(MenuKind::Object, server, vec![PathSeg::key("server")]),
        );
        assert_eq!(inner_all.len(), 4, "scoped index"); // port, host, debug, ratio
        let inner_ctx = inner_fx.ctx();
        assert_eq!(
            filter_search(&inner_ctx, &inner_all, "name", true, None).len(),
            0,
            "scoped search must not leave the current menu"
        );
    }

    /// TestSearchDotQuery: a "."-prefixed query matches the keyword against
    /// path segments from any node — the node itself first, its descendants
    /// after — and both path queries fall back to values carrying the query
    /// text (with its "$"/"." prefix) after their path hits.
    #[test]
    fn test_search_dot_query() {
        let schema = must_schema(
            r#"{
	  "type": "object",
	  "properties": {
	    "jobs": {"type": "object", "properties": {
	      "build_apple_app_store": {"type": "object", "properties": {
	        "runs_on": {"type": "string"},
	        "timeout": {"type": "number"}
	      }},
	      "lint": {"type": "object", "properties": {
	        "runs_on": {"type": "string"}
	      }}
	    }},
	    "notes": {"type": "string"}
	  }
	}"#,
        );
        let doc = dbuild(|db| {
            db.obj(vec![
                (
                    "jobs",
                    db.obj(vec![
                        (
                            "build_apple_app_store",
                            db.obj(vec![("runs_on", db.s("macos-14")), ("timeout", db.n("30"))]),
                        ),
                        ("lint", db.obj(vec![("runs_on", db.s("ubuntu-24.04"))])),
                    ]),
                ),
                ("notes", db.s("release: see $.jobs.build_apple_app_store")),
            ])
        });
        let mut schema = schema;
        let rst = root_state(&schema);
        let all = search_index(&mut schema, &doc, &rst);
        let opts = test_opts();
        let cache = HashMap::new();
        let ctx = MenuCtx {
            schema: &schema,
            doc: &doc,
            opts: &opts,
            menu_cache: &cache,
            hide_empty: false,
        };
        let join = |rows: &[Row]| -> String {
            rows.iter()
                .map(|r| path_string(&r.path))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let subtree = "$.jobs.build_apple_app_store $.jobs.build_apple_app_store.runs_on $.jobs.build_apple_app_store.timeout";

        // the node and its subtree, node first — the keyword need not be the
        // whole key ("build_app_store" lands on "build_apple_app_store");
        // the notes value carries no ".build_app_store" text, so no fallback
        assert_eq!(
            join(&filter_search(&ctx, &all, ".build_app_store", true, None)),
            subtree
        );
        // an exact key prefix hits the same rows plus the value fallback:
        // the notes value contains ".build_app"
        assert_eq!(
            join(&filter_search(&ctx, &all, ".build_app", true, None)),
            format!("{subtree} $.notes")
        );
        // dotted parts match a run of consecutive segments from any node
        assert_eq!(
            join(&filter_search(&ctx, &all, ".lint.runs_on", true, None)),
            "$.jobs.lint.runs_on"
        );
        // fold off demands the exact case
        assert_eq!(
            filter_search(&ctx, &all, ".BUILD_APP", false, None).len(),
            0
        );
        // regex mode keeps the tiered literal matcher on the path (the re
        // only feeds ordinary text queries), and the fallback stays a
        // literal containment: identical results to the literal query above
        let re = Regex::new(&format!("(?i){}", path_pattern(".build_app"))).unwrap();
        assert_eq!(
            join(&filter_search(&ctx, &all, ".build_app", true, Some(&re))),
            format!("{subtree} $.notes")
        );

        // "$" query: root-anchored path hits first (exact path on top), then
        // the value fallback — the notes value carries the queried path text
        let want = "$.jobs.build_apple_app_store $.jobs.build_apple_app_store.runs_on $.jobs.build_apple_app_store.timeout $.notes";
        assert_eq!(
            join(&filter_search(
                &ctx,
                &all,
                "$.jobs.build_apple_app_store",
                true,
                None
            )),
            want
        );
    }

    #[test]
    fn test_search_index_variant_rows_only_without_value() {
        let key_row = |schema: &mut Schema, doc: &Doc| -> Row {
            let st = root_state(schema);
            search_index(schema, doc, &st)
                .into_iter()
                .find(|r| {
                    r.fn_.map(|f| schema.nodes[f as usize].key.as_str()) == Some("auth")
                        && path_eq(&r.path, &[PathSeg::key("auth")])
                })
                .expect("auth key row")
        };

        // Unset oneOf: exactly one key row, no per-variant duplicates.
        let mut fx = Fx::new();
        let st = root_state(&fx.schema);
        let all = search_index(&mut fx.schema, &fx.doc, &st);
        assert!(
            all.iter()
                .filter(|r| r.kind == RowKind::Variant && path_eq(&r.path, &[PathSeg::key("auth")]))
                .count()
                == 0,
            "no variant rows for an unset key"
        );
        let row = key_row(&mut fx.schema, &fx.doc);
        assert!(row.label.ends_with("—"), "unset key label: {}", row.label);

        // Set oneOf: the key row stays searchable, still without duplicates.
        let mut fx2 = Fx::new();
        let tok = fx2.doc.new_str("secret");
        let obj = fx2.doc.new_object();
        if let Value::Object(m) = &mut fx2.doc.node_mut(obj).value {
            m.push(Member {
                key: "token".into(),
                val: tok,
            });
        }
        fx2.doc.set(&[PathSeg::key("auth")], obj).unwrap();
        let st = root_state(&fx2.schema);
        let all = search_index(&mut fx2.schema, &fx2.doc, &st);
        assert!(
            all.iter()
                .filter(|r| r.kind == RowKind::Variant && path_eq(&r.path, &[PathSeg::key("auth")]))
                .count()
                == 0,
            "no duplicate variant rows for a set key"
        );
        let row = key_row(&mut fx2.schema, &fx2.doc);
        assert_ne!(row.label, "");
    }

    #[test]
    fn test_search_index_only_matched_variant_fields() {
        // an array of discriminated variants (sing-box inbounds): each
        // item's current type defines its fields
        let mut b = TB::new();
        let mut direct_type = fnode("type", "Type", Kind::Enum);
        direct_type.enum_ = tenum(&mut b, &["direct"]);
        direct_type.required = true;
        let direct_type = b.id(direct_type);
        let listen_port = b.id(scalar("listen_port", "Listen port", "string"));
        let direct = b.id(FormNode {
            key: "inbounds".into(),
            title: "Direct".into(),
            kind: Kind::Object,
            children: vec![direct_type, listen_port],
            ..FormNode::default()
        });
        let mut tun_type = fnode("type", "Type", Kind::Enum);
        tun_type.enum_ = tenum(&mut b, &["tun"]);
        tun_type.required = true;
        let tun_type = b.id(tun_type);
        let ip_scalar = b.id(scalar("include_package", "Include package", "string"));
        let ip_item = b.id(scalar("include_package", "Include package", "string"));
        let ip_array = b.id(FormNode {
            key: "include_package".into(),
            title: "Include package".into(),
            kind: Kind::Array,
            items: Some(ip_item),
            ..FormNode::default()
        });
        let include_package = b.id(FormNode {
            key: "include_package".into(),
            title: "Include package".into(),
            kind: Kind::OneOf,
            variants: vec![ip_scalar, ip_array],
            ..FormNode::default()
        });
        let tun = b.id(FormNode {
            key: "inbounds".into(),
            title: "Tun".into(),
            kind: Kind::Object,
            children: vec![tun_type, include_package],
            ..FormNode::default()
        });
        let inbounds_items = b.id(FormNode {
            key: "inbounds".into(),
            kind: Kind::OneOf,
            variants: vec![direct, tun],
            ..FormNode::default()
        });
        let inbounds = b.id(FormNode {
            key: "inbounds".into(),
            title: "Inbounds".into(),
            kind: Kind::Array,
            items: Some(inbounds_items),
            ..FormNode::default()
        });
        let proot = b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![inbounds],
            ..FormNode::default()
        });
        b.root = proot;
        let mut schema = b.finish();

        let doc = dbuild(|db| {
            db.obj(vec![(
                "inbounds",
                db.arr(vec![
                    db.obj(vec![
                        ("type", db.s("direct")),
                        ("listen_port", db.s("1080")),
                    ]),
                    db.obj(vec![
                        ("type", db.s("tun")),
                        ("include_package", db.arr(vec![db.s("com.foo")])),
                    ]),
                ]),
            )])
        });

        let rst = root_state(&schema);
        let all = search_index(&mut schema, &doc, &rst);
        let foreign = all
            .iter()
            .filter(|r| {
                path_eq(
                    &r.path,
                    &[
                        PathSeg::key("inbounds"),
                        PathSeg::Idx(0),
                        PathSeg::key("include_package"),
                    ],
                )
            })
            .count();
        let own = all
            .iter()
            .filter(|r| {
                path_eq(
                    &r.path,
                    &[
                        PathSeg::key("inbounds"),
                        PathSeg::Idx(1),
                        PathSeg::key("include_package"),
                    ],
                )
            })
            .count();
        assert_eq!(foreign, 0, "direct inbound indexed tun fields");
        assert_eq!(own, 1, "tun include_package rows");

        // the direct inbound's own field survives the restriction
        let opts = test_opts();
        let cache = HashMap::new();
        let ctx = MenuCtx {
            schema: &schema,
            doc: &doc,
            opts: &opts,
            menu_cache: &cache,
            hide_empty: false,
        };
        let hits = filter_search(&ctx, &all, "listen_port", true, None);
        assert_eq!(hits.len(), 1);
        assert!(path_eq(
            &hits[0].path,
            &[
                PathSeg::key("inbounds"),
                PathSeg::Idx(0),
                PathSeg::key("listen_port")
            ]
        ));
    }

    #[test]
    fn test_search_rank_and_path_queries() {
        let mut fx = Fx::new();
        let rst = root_state(&fx.schema);
        let all = search_index(&mut fx.schema, &fx.doc, &rst);
        let ctx = fx.ctx();
        let eq = |got: &[String], want: &[&str]| {
            let w: Vec<String> = want.iter().map(|s| s.to_string()).collect();
            assert_eq!(*got, w, "paths");
        };

        // exact-key priority: the row whose key IS the query outranks rows
        // that merely mention it
        let got = filter_search(&ctx, &all, "port", true, None);
        assert!(
            !got.is_empty() && paths(&got)[0] == "$.server.port",
            "port first hit = {:?}",
            paths(&got)
        );
        let got = filter_search(&ctx, &all, "src", true, None);
        assert!(
            !got.is_empty() && paths(&got)[0] == "$.mounts[0].src",
            "src first hit"
        );
        // a value only its own row carries matches that row alone
        let got = filter_search(&ctx, &all, "8080", true, None);
        assert_eq!(got.len(), 1);
        assert_eq!(paths(&got)[0], "$.server.port");
        // the tier is stable: without an exact key the order is unchanged
        eq(
            &paths(&filter_search(&ctx, &all, "0", true, None)),
            &["$.server.port", "$.server.host"],
        );

        // path queries: "$"-prefixed, literal prefix match from the root
        eq(
            &paths(&filter_search(&ctx, &all, "$.server.port", true, None)),
            &["$.server.port"],
        );
        let anch = filter_search(&ctx, &all, "$.server", true, None);
        for p in paths(&anch) {
            assert!(
                p.starts_with("$.server"),
                "path hit {p:?} lacks the $.server prefix"
            );
        }
        assert_eq!(paths(&anch)[0], "$.server"); // exact full path ranks first
        assert_eq!(
            paths(&filter_search(&ctx, &all, "$.serv", true, None))[0],
            "$.server"
        );
        // array indexes use the rendered [N] syntax
        eq(
            &paths(&filter_search(&ctx, &all, "$.mounts[0]", true, None)),
            &["$.mounts[0]", "$.mounts[0].src", "$.mounts[0].dst"],
        );
        eq(
            &paths(&filter_search(&ctx, &all, "$.mounts[0].src", true, None)),
            &["$.mounts[0].src"],
        );

        // dotted and slashed values remain ordinary text queries
        assert_eq!(filter_search(&ctx, &all, "0.0.0.0", true, None).len(), 1);
        assert_eq!(filter_search(&ctx, &all, "/mnt", true, None).len(), 1);
    }

    // --- search & replace helpers ----------------------------------------------

    #[test]
    fn test_filter_search_regex() {
        let mut fx = Fx::new();
        let rst = root_state(&fx.schema);
        let all = search_index(&mut fx.schema, &fx.doc, &rst);
        let eq = |got: &[String], want: &[&str]| {
            let w: Vec<String> = want.iter().map(|s| s.to_string()).collect();
            assert_eq!(*got, w, "paths");
        };
        let matchq = |fx: &Fx, all: &[Row], q: &str| -> Vec<String> {
            let pat = if is_path_query(q) {
                path_pattern(q)
            } else {
                q.to_string()
            };
            let re = Regex::new(&pat).unwrap();
            let ctx = fx.ctx();
            paths(&filter_search(&ctx, all, q, false, Some(&re)))
        };

        // the regex targets the same text literal mode matches: title/key
        // plus the row's own scalar value; a full leaf-key match ranks first
        eq(&matchq(&fx, &all, r"(?i)token"), &[]); // parent is absent
        eq(&matchq(&fx, &all, r"\bport\b"), &["$.server.port"]); // $.server's roll-up is no target
        eq(&matchq(&fx, &all, r"0\.0\.0\.0"), &["$.server.host"]);
        // a "$."-prefixed query targets the document path, regex included
        eq(&matchq(&fx, &all, "$.server.port"), &["$.server.port"]);
        eq(
            &matchq(&fx, &all, "$.server"),
            &[
                "$.server",
                "$.server.port",
                "$.server.host",
                "$.server.debug",
                "$.server.ratio",
            ],
        );
        // [N] index brackets in a pasted path stay literal in regex mode
        eq(&matchq(&fx, &all, "$.mounts[0].src"), &["$.mounts[0].src"]);
        eq(
            &matchq(&fx, &all, r"$.mounts\[0\].src"),
            &["$.mounts[0].src"],
        ); // explicit escape
        // the pattern matches original-case text: folding is the caller's (?i)
        let re = Regex::new(r"(?i)SRC").unwrap();
        eq(
            &paths(&filter_search(
                &fx.ctx(),
                &all,
                r"(?i)SRC",
                false,
                Some(&re),
            )),
            &["$.mounts[0].src"],
        );
        assert_eq!(matchq(&fx, &all, "SRC").len(), 0, "exact-case SRC");
        // a pattern matching only a container roll-up finds nothing
        assert_eq!(matchq(&fx, &all, "port=8080").len(), 0);
    }

    #[test]
    fn test_replace_all() {
        let cases: Vec<(&str, &str, &str, bool, &str, usize)> = vec![
            ("Hello world HELLO", "hello", "hi", true, "hi world hi", 2),
            ("Hello", "hello", "x", true, "x", 1),
            ("Hello", "hello", "x", false, "Hello", 0),
            ("Grösse GRÖSSE", "grösse", "S", true, "S S", 2),
            ("Größe", "GRÖSSE", "X", true, "Größe", 0), // simple fold: ß is not ss
            ("aaa", "aa", "b", true, "ba", 1),          // non-overlapping scan
            ("ab", "a", "aa", true, "aab", 1),          // rep containing old: no rescan
            ("abc", "", "x", true, "abc", 0),           // empty old
            ("abc", "z", "x", false, "abc", 0),         // no match
            ("a§b A§B", "§", "§§", true, "a§§b A§§B", 2), // multibyte-safe
        ];
        for (s, old, rep, fold, want, want_n) in cases {
            let (got, n) = replace_all(s, old, rep, fold, None);
            assert_eq!(
                (got.as_str(), n),
                (want, want_n),
                "replaceAll({s:?}, {old:?}, {rep:?}, {fold})"
            );
        }
    }

    #[test]
    fn test_replace_all_regex() {
        let cases: Vec<(&str, &str, &str, &str, usize)> = vec![
            ("hello", "l+", "x", "hexo", 1),
            ("foo1bar2", "[0-9]+", "#", "foo#bar#", 2),
            ("abc123", "(b)(c)", "[$2$1]", "a[cb]123", 1), // $N groups expand
            ("HELLO hello", "(?i)hello", "x", "x x", 2),   // folding is the caller's (?i)
            ("bbb", "a*", "x", "xbxbxbx", 4),              // empty match at each boundary
        ];
        for (s, pat, rep, want, want_n) in cases {
            let re = Regex::new(pat).unwrap();
            let (got, n) = replace_all(s, pat, rep, false, Some(&re));
            assert_eq!(
                (got.as_str(), n),
                (want, want_n),
                "replaceAll({s:?}, {pat:?}, regexp)"
            );
        }
    }

    #[test]
    fn test_replace_hits() {
        let mut fx = Fx::new();
        let rst = root_state(&fx.schema);
        let all = search_index(&mut fx.schema, &fx.doc, &rst);
        let ctx = fx.ctx();

        // non-empty query: only string values containing q, relabeled
        let hits = replace_hits(&ctx, &all, "/data", "/opt", true, None);
        assert_eq!(hits.len(), 1, "hits");
        assert_eq!(hits[0].path.len(), 3, "hit path");
        assert_eq!(hits[0].label, "$.mounts[0].src  /data → /opt");

        // fold=false demands the exact case
        assert_eq!(
            replace_hits(&ctx, &all, "/DATA", "/opt", false, None).len(),
            0
        );
        assert_eq!(
            replace_hits(&ctx, &all, "/DATA", "/opt", true, None).len(),
            1
        );

        // empty query: every string value, original labels, numbers excluded
        let strs = replace_hits(&ctx, &all, "", "", true, None);
        assert_eq!(strs.len(), 4); // name, server/host, mounts/0/src, mounts/0/dst
        for r in &strs {
            if let Some(f) = r.fn_ {
                assert_ne!(
                    fx.schema.nodes[f as usize].key, "port",
                    "the number port must not be a replace hit"
                );
            }
            assert!(
                !r.label.is_empty(),
                "empty-query hits keep their original labels"
            );
        }

        // duplicate paths (oneOf variants walking the same location) are
        // kept once
        let mut b = TB::new();
        let va = b.id(scalar("auth", "A", "string"));
        let vb = b.id(scalar("auth", "B", "string"));
        let auth = b.id(FormNode {
            key: "auth".into(),
            title: "Auth".into(),
            kind: Kind::OneOf,
            variants: vec![va, vb],
            ..FormNode::default()
        });
        let proot = b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![auth],
            ..FormNode::default()
        });
        b.root = proot;
        let mut fx2 = Fx {
            schema: b.finish(),
            doc: dbuild(|db| db.obj(vec![("auth", db.s("secret"))])),
            opts: test_opts(),
            cache: HashMap::new(),
        };
        let rst = root_state(&fx2.schema);
        let all2 = search_index(&mut fx2.schema, &fx2.doc, &rst);
        let ctx2 = fx2.ctx();
        let got = replace_hits(&ctx2, &all2, "secret", "x", true, None);
        assert_eq!(got.len(), 1, "duplicate variant paths");
    }

    #[test]
    fn test_search_index_unknown_keys() {
        // schema-less document keys (with nested values) are indexed,
        // matched and offered to replace like any other row
        let mut fx = Fx::new();
        {
            let note = fx.doc.new_str("alpha");
            let beta = fx.doc.new_str("beta");
            let tags = fx.doc.new_array();
            fx.doc.node_mut(tags).value = Value::Array(vec![beta]);
            let extra = fx.doc.new_object();
            fx.doc.node_mut(extra).value = Value::Object(vec![
                Member {
                    key: "note".into(),
                    val: note,
                },
                Member {
                    key: "tags".into(),
                    val: tags,
                },
            ]);
            fx.doc.set(&[PathSeg::key("extra")], extra).unwrap();
            let legacy = fx.doc.new_str("gamma");
            fx.doc
                .set(&[PathSeg::key("server"), PathSeg::key("legacy")], legacy)
                .unwrap();
        }
        let rst = root_state(&fx.schema);
        let all = search_index(&mut fx.schema, &fx.doc, &rst);
        let ctx = fx.ctx();

        let labels: std::collections::HashSet<&str> = all
            .iter()
            .filter(|r| r.kind == RowKind::Unknown)
            .map(|r| r.label.as_str())
            .collect();
        for want in [
            "$.extra.note  alpha",
            "$.extra.tags[0]  beta",
            "$.server.legacy  gamma",
        ] {
            assert!(
                labels.contains(want),
                "unknown row {want:?} missing from index: {labels:?}"
            );
        }

        // search matches the unknown key name and its value overview
        for q in ["legacy", "gamma", "extra"] {
            assert!(
                !filter_search(&ctx, &all, q, true, None).is_empty(),
                "filter {q:?} found no unknown row"
            );
        }

        // replace previews target unknown string values, nested included
        let hits = replace_hits(&ctx, &all, "a", "A", true, None);
        let hit_paths: std::collections::HashSet<String> =
            hits.iter().map(|h| path_string(&h.path)).collect();
        for want in ["$.extra.note", "$.extra.tags[0]", "$.server.legacy"] {
            assert!(
                hit_paths.contains(want),
                "replace hit {want:?} missing: {hit_paths:?}"
            );
        }
    }

    #[test]
    fn test_search_index_container_variant_elements() {
        // `anyOf: [string, array[string]]` is one value with two shapes: the
        // matched array variant must be walked as the value itself, so its
        // elements stay searchable (sing-box route.rules[].rule_set)
        let schema_src = r#"{
          "type": "object",
          "properties": {
            "rule_set": {"anyOf": [
              {"type": "string"},
              {"type": "array", "items": {"type": "string"}}
            ]}
          }
        }"#;
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("schema.json");
        std::fs::write(&p, schema_src).expect("schema write");
        let mut schema = crate::smodel::load(p.to_str().expect("path")).expect("schema load");
        let doc = dbuild(|db| {
            db.obj(vec![(
                "rule_set",
                db.arr(vec![db.s("ruleset-proxy-ai"), db.s("ruleset-proxy-github")]),
            )])
        });
        let st = root_state(&schema);
        let all = search_index(&mut schema, &doc, &st);
        let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();
        assert!(
            labels.contains(&"$.rule_set[0]  ruleset-proxy-ai")
                && labels.contains(&"$.rule_set[1]  ruleset-proxy-github"),
            "array variant elements missing: {labels:?}"
        );
        let opts = test_opts();
        let cache = HashMap::new();
        let ctx = MenuCtx {
            schema: &schema,
            doc: &doc,
            opts: &opts,
            menu_cache: &cache,
            hide_empty: false,
        };
        assert_eq!(
            filter_search(&ctx, &all, "ruleset-proxy-github", true, None).len(),
            1,
            "the element value must match"
        );
    }

    #[test]
    fn test_search_index_array_elements() {
        // every document element is searchable, whatever shape its schema
        // has: enum items, missing items, nested arrays (degraded to Raw) and
        // plain object items
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("schema.json");
        std::fs::write(
            &p,
            r#"{
              "type": "object",
              "properties": {
                "levels": {"type": "array", "items": {"type": "string", "enum": ["debug", "info"]}},
                "free": {"type": "array"},
                "matrix": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}},
                "mounts": {"type": "array", "items": {"type": "object", "properties": {"src": {"type": "string"}}}}
              }
            }"#,
        )
        .expect("schema write");
        let mut schema = crate::smodel::load(p.to_str().expect("path")).expect("schema load");
        let doc = dbuild(|db| {
            db.obj(vec![
                ("levels", db.arr(vec![db.s("debug"), db.s("info")])),
                (
                    "free",
                    db.arr(vec![db.s("alpha"), db.obj(vec![("k", db.n("7"))])]),
                ),
                ("matrix", db.arr(vec![db.arr(vec![db.n("1"), db.n("2")])])),
                ("mounts", db.arr(vec![db.obj(vec![("src", db.s("/data"))])])),
            ])
        });
        let st = root_state(&schema);
        let all = search_index(&mut schema, &doc, &st);
        let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();

        // enum items are document values, not schema-only options
        assert!(
            labels.contains(&"$.levels[0]  debug") && labels.contains(&"$.levels[1]  info"),
            "enum array elements missing: {labels:?}"
        );
        // no items schema: element rows plus nested values, indexed raw
        assert!(
            labels.contains(&"$.free[0]  alpha")
                && labels.contains(&"$.free[1]  k=7")
                && labels.contains(&"$.free[1].k  7"),
            "schema-less array elements missing: {labels:?}"
        );
        // nested arrays degrade to Raw; the inner elements stay searchable
        assert!(
            labels.contains(&"$.matrix[0]  2 items")
                && labels.contains(&"$.matrix[0][0]  1")
                && labels.contains(&"$.matrix[0][1]  2"),
            "nested array elements missing: {labels:?}"
        );
        // object items keep their per-field rows
        assert!(
            labels.contains(&"$.mounts[0].src  /data"),
            "object item fields missing: {labels:?}"
        );

        // the new rows are ordinary text matches
        let opts = test_opts();
        let cache = HashMap::new();
        let ctx = MenuCtx {
            schema: &schema,
            doc: &doc,
            opts: &opts,
            menu_cache: &cache,
            hide_empty: false,
        };
        for q in ["info", "alpha", "7", "/data"] {
            assert!(
                !filter_search(&ctx, &all, q, true, None).is_empty(),
                "filter {q:?} found no array element row"
            );
        }
    }

    #[test]
    fn test_search_index_raw_value_subtrees() {
        // a node the form layer degrades to Raw is edited as one blob, but
        // its nested values must still be searchable: object children and
        // map values include everything below the raw node
        let mut b = TB::new();
        let extra = b.id(fnode("extra", "Extra", Kind::Raw));
        let env = b.id(FormNode {
            key: "env".into(),
            title: "Env".into(),
            kind: Kind::Map,
            ..FormNode::default()
        });
        let root = b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![extra, env],
            ..FormNode::default()
        });
        b.root = root;
        let mut schema = b.finish();
        let doc = dbuild(|db| {
            db.obj(vec![
                (
                    "extra",
                    db.obj(vec![
                        ("note", db.s("needle")),
                        ("items", db.arr(vec![db.obj(vec![("name", db.s("deep"))])])),
                    ]),
                ),
                (
                    "env",
                    db.obj(vec![("A", db.obj(vec![("deep", db.s("hidden"))]))]),
                ),
            ])
        });
        let st = root_state(&schema);
        let all = search_index(&mut schema, &doc, &st);
        let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();
        assert!(
            labels.contains(&"$.extra.note  needle")
                && labels.contains(&"$.extra.items[0].name  deep"),
            "raw child subtree missing: {labels:?}"
        );
        assert!(
            labels.contains(&"$.env.A.deep  hidden"),
            "raw map value subtree missing: {labels:?}"
        );
        let opts = test_opts();
        let cache = HashMap::new();
        let ctx = MenuCtx {
            schema: &schema,
            doc: &doc,
            opts: &opts,
            menu_cache: &cache,
            hide_empty: false,
        };
        for q in ["needle", "deep", "hidden"] {
            assert!(
                !filter_search(&ctx, &all, q, true, None).is_empty(),
                "filter {q:?} found no raw subtree row"
            );
        }
    }

    #[test]
    fn test_search_index_recursive_raw_field() {
        // a recursive $ref degrades the inner field to Raw ("edit as JSON");
        // values inside that raw subtree must still be searchable
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("schema.json");
        std::fs::write(
            &p,
            r##"{
              "type": "object",
              "properties": {"node": {"$ref": "#/$defs/Node"}},
              "$defs": {
                "Node": {
                  "type": "object",
                  "properties": {
                    "name": {"type": "string"},
                    "child": {"$ref": "#/$defs/Node"}
                  }
                }
              }
            }"##,
        )
        .expect("schema write");
        let mut schema = crate::smodel::load(p.to_str().expect("path")).expect("schema load");
        let doc = dbuild(|db| {
            db.obj(vec![(
                "node",
                db.obj(vec![
                    ("name", db.s("root")),
                    ("child", db.obj(vec![("name", db.s("needle"))])),
                ]),
            )])
        });
        let st = root_state(&schema);
        let all = search_index(&mut schema, &doc, &st);
        let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();
        assert!(
            labels.contains(&"$.node.child.name  needle"),
            "recursive raw subtree missing: {labels:?}"
        );
    }

    #[test]
    fn test_search_index_multi_match_array_variants() {
        // two array variants both match the value (a tie): the union has no
        // object fields to merge, so the elements must still be indexed
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("schema.json");
        std::fs::write(
            &p,
            r#"{
              "type": "object",
              "properties": {
                "ids": {"anyOf": [
                  {"type": "array", "items": {"type": "string"}},
                  {"type": "array", "items": {"type": "string"}, "minItems": 1}
                ]}
              }
            }"#,
        )
        .expect("schema write");
        let mut schema = crate::smodel::load(p.to_str().expect("path")).expect("schema load");
        let doc = dbuild(|db| db.obj(vec![("ids", db.arr(vec![db.s("x"), db.s("y")]))]));
        let st = root_state(&schema);
        let all = search_index(&mut schema, &doc, &st);
        let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();
        assert!(
            labels.contains(&"$.ids[0]  x") && labels.contains(&"$.ids[1]  y"),
            "multi-match array variant elements missing: {labels:?}"
        );
    }

    #[test]
    fn test_rebuild_stack_schema_less() {
        // The runtime menus Enter opens for unknown values are the ones a
        // jump must rebuild, or a schema-less search hit lands only on the
        // top-level key.
        let doc = dbuild(|db| {
            db.obj(vec![
                ("server", db.obj(vec![("port", db.n("8080"))])),
                ("tags", db.arr(vec![db.s("alpha"), db.s("beta")])),
                ("arr", db.arr(vec![db.obj(vec![("name", db.s("deep"))])])),
            ])
        });
        let mut schema = crate::smodel::untyped(&doc);

        // object member: jump target is the member row in the value's menu
        let stx = rebuild_stack(
            &mut schema,
            &doc,
            &[PathSeg::key("server"), PathSeg::key("port")],
        );
        assert_eq!(stx.len(), 2);
        assert_eq!(stx[1].kind, MenuKind::Object);
        assert_eq!(stx[1].path, vec![PathSeg::key("server")]);
        assert_eq!(schema.nodes[stx[1].node as usize].kind, Kind::Object);

        // array item: the runtime items menu
        let stx = rebuild_stack(&mut schema, &doc, &[PathSeg::key("tags"), PathSeg::Idx(1)]);
        assert_eq!(stx.len(), 2);
        assert_eq!(stx[1].kind, MenuKind::Items);
        assert_eq!(stx[1].path, vec![PathSeg::key("tags")]);

        // object nested in an array item: item menu, then its field menu
        let stx = rebuild_stack(
            &mut schema,
            &doc,
            &[PathSeg::key("arr"), PathSeg::Idx(0), PathSeg::key("name")],
        );
        assert_eq!(stx.len(), 3);
        assert_eq!(stx[2].kind, MenuKind::Object);
        assert_eq!(stx[2].path, vec![PathSeg::key("arr"), PathSeg::Idx(0)]);
    }

    /// Map rows edit against the value schema, so the schema node's key is
    /// the map's own key ("dependencies"), not the document member's. The
    /// member key must still be searchable (Cargo.toml `ratatui`).
    #[test]
    fn test_search_map_member_key() {
        let mut schema = must_schema(
            r#"{
              "type": "object",
              "properties": {
                "dependencies": {
                  "type": "object",
                  "additionalProperties": {
                    "oneOf": [
                      {"type": "string"},
                      {"type": "object", "properties": {"version": {"type": "string"}}}
                    ]
                  }
                }
              }
            }"#,
        );
        let doc = dbuild(|db| {
            db.obj(vec![(
                "dependencies",
                db.obj(vec![("ratatui", db.s("=0.30.2"))]),
            )])
        });
        let rst = root_state(&schema);
        let all = search_index(&mut schema, &doc, &rst);
        let ctx = MenuCtx {
            schema: &schema,
            doc: &doc,
            opts: &test_opts(),
            menu_cache: &HashMap::new(),
            hide_empty: false,
        };
        let hits = filter_search(&ctx, &all, "ratatui", true, None);
        assert_eq!(
            hits.iter().map(|r| r.label.clone()).collect::<Vec<_>>(),
            ["$.dependencies.ratatui  =0.30.2"],
            "map member key must be a text-search target"
        );
        assert_eq!(
            filter_search(&ctx, &all, "RATATUI", false, None).len(),
            0,
            "case-sensitive search must not match a differently cased key"
        );
    }

    #[test]
    fn test_search_multi_pattern_keys_in_schema() {
        let schema = must_schema(
            r##"{
              "type": "object",
              "properties": {
                "jobs": {
                  "type": "object",
                  "patternProperties": {
                    "^a": {"type": "string"},
                    "^b": {"type": "integer"}
                  },
                  "additionalProperties": false
                }
              }
            }"##,
        );
        let doc = dbuild(|db| {
            db.obj(vec![(
                "jobs",
                db.obj(vec![
                    ("a1", db.obj(vec![("nested", db.s("x"))])),
                    ("b1", db.n("2")),
                    ("c1", db.b(true)),
                ]),
            )])
        });
        let jobs = child_of(&schema, schema.root, "jobs");
        let mut schema = schema;
        let st = st(MenuKind::Object, jobs, vec![PathSeg::key("jobs")]);
        let all = search_index(&mut schema, &doc, &st);
        let find = |key: &str| {
            all.iter()
                .find(|r| r.path == vec![PathSeg::key("jobs"), PathSeg::key(key)])
                .unwrap_or_else(|| panic!("{key} search row missing"))
        };
        assert!(find("a1").in_schema);
        assert!(find("b1").in_schema);
        assert!(!find("c1").in_schema);
        let nested = all
            .iter()
            .find(|r| {
                r.path
                    == vec![
                        PathSeg::key("jobs"),
                        PathSeg::key("a1"),
                        PathSeg::key("nested"),
                    ]
            })
            .expect("nested row");
        assert!(
            nested.in_schema,
            "rows below a pattern-defined key inherit its schema standing"
        );
    }
}
