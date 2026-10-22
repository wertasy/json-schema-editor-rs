//! Schema → form model: converts a JSON Schema document into a tree of
//! editable form nodes consumed by the UI. Only a practical subset of
//! keywords is form-ized; anything unsupported degrades to a raw JSON editor
//! and is still enforced by save-time validation (src/validate.rs).
//!
//! Rust port of `internal/smodel/smodel.go`. Go's shared `*FormNode`/`*Node`
//! pointers become arenas: form nodes live in [`Schema::nodes`] addressed by
//! `FormNodeId`, and schema documents (plus the synthetic allOf merge nodes)
//! live in one [`Doc`] arena addressed by `NodeId`. Go's pointer-identity
//! memo map becomes a `HashMap<NodeId, FormNodeId>`.

#![allow(dead_code)] // consumed by the UI layer in Phase 4

use std::collections::{HashMap, HashSet};

use url::Url;

use crate::doc::jsondoc::{self, Doc, Member, NodeData, NodeId, Value};
use crate::fetch;
use crate::icons;

/// Kind classifies a schema node for the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    /// string/integer/number input
    Scalar,
    /// select (single value from options)
    Enum,
    /// confirm
    Boolean,
    /// section form with children
    Object,
    /// array manager
    Array,
    /// string-keyed map (additionalProperties): rows are doc keys
    Map,
    /// variant picker
    OneOf,
    /// raw JSON textarea fallback
    Raw,
}

impl Kind {
    pub fn str(self) -> &'static str {
        match self {
            Kind::Scalar => "scalar",
            Kind::Enum => "enum",
            Kind::Boolean => "boolean",
            Kind::Object => "object",
            Kind::Array => "array",
            Kind::Map => "map",
            Kind::OneOf => "oneOf",
            Kind::Raw => "raw",
        }
    }
}

/// One selectable option. `encoded` is the canonical JSON literal of the
/// value (numbers "3", booleans "true", strings "\"a\""), so mixed type
/// enums stay collision-free. `val` points into [`Schema::doc`].
#[derive(Clone, Debug)]
pub struct EnumOpt {
    pub display: String,
    pub encoded: String,
    pub val: NodeId,
}

/// One editable schema node.
#[derive(Clone, Debug, Default)]
pub struct FormNode {
    /// Title fallback: title → key; root: "$"
    pub key: String,
    pub title: String,
    pub desc: String,
    /// Title is the key fallback, not a schema title
    pub title_fallback: bool,
    /// Title came from a referenced schema (a bare `$ref` property) rather
    /// than this property's own schema. Menus still use it as the header,
    /// but object rows label the member with its key.
    pub title_from_ref: bool,
    pub kind: Kind,
    /// Scalar: "string"|"integer"|"number"
    pub json_type: String,
    /// Raw: schema admits only JSON null
    pub null_only: bool,
    /// Raw: edit in the built-in raw textarea instead of the any-type picker
    /// (the synthetic "JSON" variant of that picker)
    pub raw_editor: bool,

    pub min: Option<f64>,
    pub max: Option<f64>,
    pub multiple_of: Option<f64>,
    pub exclusive_min: bool,
    pub exclusive_max: bool,
    pub min_len: Option<usize>,
    pub max_len: Option<usize>,
    pub pattern: String,
    pub enum_: Vec<EnumOpt>,
    /// Scalar string: schema values offered as tab completions (free text
    /// stays valid)
    pub suggestions: Vec<String>,
    pub default_: Option<NodeId>,
    pub placeholder: String,
    pub required: bool,
    /// Object, in schema order
    pub children: Vec<FormNodeId>,
    /// Array elements, Map values
    pub items: Option<FormNodeId>,
    pub min_items: i64,
    pub max_items: i64,
    /// OneOf
    pub variants: Vec<FormNodeId>,
    /// x-tag-reference: tag lookup domain (e.g. "http_client")
    pub tag_ref: String,
    /// Map: document keys must match this regexp (patternProperties)
    pub key_pattern: String,
}

/// Arena index into [`Schema::nodes`].
pub type FormNodeId = u32;

/// The form model of one schema document: the normalized node tree plus the
/// schema document arena its enum/default values point into.
pub struct Schema {
    pub nodes: Vec<FormNode>,
    pub root: FormNodeId,
    /// Top-level `$defs`/`definitions` entries, normalized as selectable
    /// types. Raw fallbacks are omitted: they have no concrete shape.
    pub defs: Vec<FormNodeId>,
    /// The parsed schema JSON document (plus grafted remote $ref documents
    /// and synthetic merge nodes); enum/default values use NodeIds into it.
    pub doc: Doc,
    /// True for the fallback model built when no schema source resolves.
    /// The tree is derived from the document's runtime shape, so keys are
    /// not "unknown to the schema" and must not be flagged as such.
    pub schema_less: bool,
}

const MAX_DEPTH: usize = 32;

/// Caps the cartesian product of the oneOf/anyOf lists that meet inside an
/// allOf merge; beyond it the combo stays a plain member.
const MAX_COMBO_PRODUCT: usize = 128;

/// Tracks the document currently being normalized and its canonical
/// location (for relative $ref resolution).
#[derive(Clone)]
struct RefCtx {
    /// Root NodeId of the document inside the shared arena.
    doc_root: NodeId,
    loc: String,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct SmodelError(pub String);

#[derive(Default)]
struct Parser {
    /// canonical location → document root inside the shared arena
    docs: HashMap<String, NodeId>,
    /// Go's pointer memo: one built FormNode per schema object node
    memo: HashMap<NodeId, FormNodeId>,
    on_path: HashSet<NodeId>,
    /// Shared arena: parsed documents are grafted here, and synthetic
    /// allOf-merge nodes are appended as they are built.
    doc: Doc,
    nodes: Vec<FormNode>,
}

/// Load parses the schema at loc (local path, http(s):// or file:// URL)
/// into a form model; the raw schema document rides on `Schema::doc`.
pub fn load(loc: &str) -> Result<Schema, SmodelError> {
    let canon = fetch::canonical_loc(loc).map_err(SmodelError)?;
    let mut p = Parser::default();
    let doc_root = p.cached_load(&canon)?;
    let ctx = RefCtx {
        doc_root,
        loc: canon,
    };
    let root = p.schema(doc_root, &ctx, "$", false, 0);
    let defs = p.build_defs(doc_root, &ctx);
    Ok(Schema {
        nodes: p.nodes,
        root,
        defs,
        doc: p.doc,
        schema_less: false,
    })
}

/// untyped builds the schema-less model used when no schema source resolves:
/// the root follows the loaded document's runtime shape — objects get the
/// schema-less key menu, arrays an item manager, and every other value a
/// runtime-typed field. Saving has no validation, and unknown-key warnings
/// are suppressed (there is no schema to compare against).
pub fn untyped(doc: &Doc) -> Schema {
    let mut nodes: Vec<FormNode> = Vec::new();
    let root = match doc.value(doc.root) {
        Value::Object(_) => {
            nodes.push(FormNode {
                key: "$".into(),
                title: "$".into(),
                title_fallback: true,
                kind: Kind::Object,
                ..FormNode::default()
            });
            0
        }
        Value::Array(_) => {
            // Array items have no type information: edit each by its runtime
            // value, opening the type picker for null/missing slots.
            nodes.push(FormNode {
                title: "Item".into(),
                kind: Kind::Raw,
                ..FormNode::default()
            });
            nodes.push(FormNode {
                key: "$".into(),
                title: "$".into(),
                title_fallback: true,
                kind: Kind::Array,
                items: Some(0),
                ..FormNode::default()
            });
            1
        }
        _ => {
            nodes.push(FormNode {
                key: "$".into(),
                title: "$".into(),
                title_fallback: true,
                kind: Kind::Raw,
                ..FormNode::default()
            });
            0
        }
    };
    Schema {
        nodes,
        root,
        defs: Vec::new(),
        doc: Doc::new(),
        schema_less: true,
    }
}

/// root_has_property reports whether the normalized root object declares the
/// given member key. A root oneOf is checked through its variants. The check
/// looks at the schema's properties (form children), not at the schema
/// document's own `$schema` keyword.
pub fn root_has_property(schema: &Schema, key: &str) -> bool {
    fn walk(schema: &Schema, id: FormNodeId, key: &str, seen: &mut HashSet<FormNodeId>) -> bool {
        if !seen.insert(id) {
            return false;
        }
        let n = &schema.nodes[id as usize];
        match n.kind {
            Kind::Object | Kind::Map => n
                .children
                .iter()
                .any(|&c| schema.nodes[c as usize].key == key),
            Kind::OneOf => n.variants.iter().any(|&v| walk(schema, v, key, seen)),
            _ => false,
        }
    }
    walk(schema, schema.root, key, &mut HashSet::new())
}

impl Parser {
    /// build_defs normalizes the top-level `$defs`/`definitions` entries so
    /// the arbitrary-type picker can offer the schema's own named types
    /// alongside the JSON built-ins. Unreachable/raw shapes are skipped.
    fn build_defs(&mut self, doc_root: NodeId, ctx: &RefCtx) -> Vec<FormNodeId> {
        let mut entries: Vec<(String, NodeId)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for keyword in ["$defs", "definitions"] {
            let Some(defs) = field(&self.doc, doc_root, keyword) else {
                continue;
            };
            let Value::Object(members) = self.doc.value(defs) else {
                continue;
            };
            for m in members {
                if seen.insert(m.key.clone()) {
                    entries.push((m.key.clone(), m.val));
                }
            }
        }
        let mut out: Vec<FormNodeId> = Vec::new();
        for (key, node) in entries {
            let fid = self.schema(node, ctx, &key, false, 0);
            if self.nodes[fid as usize].kind == Kind::Raw {
                continue;
            }
            if !out.contains(&fid) {
                out.push(fid);
            }
        }
        out
    }

    fn cached_load(&mut self, loc: &str) -> Result<NodeId, SmodelError> {
        if let Some(&d) = self.docs.get(loc) {
            return Ok(d);
        }
        let data = fetch::load(loc).map_err(SmodelError)?;
        let text = String::from_utf8(data)
            .map_err(|_| SmodelError(format!("{loc}: invalid UTF-8 in schema document")))?;
        let parsed = jsondoc::parse(&text).map_err(|e| SmodelError(format!("{loc}: {e}")))?;
        let root = graft(&mut self.doc, &parsed);
        self.docs.insert(loc.to_string(), root);
        Ok(root)
    }

    /// resolveRef resolves a $ref value against ctx, loading remote
    /// documents through the parser cache.
    fn resolve_ref(
        &mut self,
        r: &str,
        ctx: &RefCtx,
        depth: usize,
    ) -> Result<(NodeId, RefCtx), String> {
        if let Some(ptr) = r.strip_prefix('#') {
            let _ = ptr;
            return match self.pointer_at(ctx.doc_root, r) {
                Some(t) => Ok((t, ctx.clone())),
                None => Err(format!("unresolvable pointer {r}")),
            };
        }
        if depth > MAX_DEPTH {
            return Err("ref chain too deep".into());
        }
        let base = Url::parse(&doc_base(&self.doc, ctx)).map_err(|e| e.to_string())?;
        let abs = base.join(r).map_err(|e| e.to_string())?;
        let frag = abs.fragment().unwrap_or("").to_string();
        let mut doc_url = abs.clone();
        doc_url.set_fragment(None);
        let doc_loc = doc_url.to_string();
        let doc_root = self.cached_load(&doc_loc).map_err(|e| e.0)?;
        let target = self
            .pointer_at(doc_root, &frag)
            .ok_or_else(|| format!("unresolvable pointer {frag} in {doc_loc}"))?;
        Ok((
            target,
            RefCtx {
                doc_root,
                loc: doc_loc,
            },
        ))
    }

    /// JSON pointer resolution rooted at an arbitrary document root (the
    /// shared arena holds several grafted documents); mirrors
    /// `Doc::resolve_json_pointer`.
    fn pointer_at(&self, start: NodeId, ptr: &str) -> Option<NodeId> {
        let ptr = ptr.strip_prefix('#').unwrap_or(ptr);
        if ptr.is_empty() {
            return Some(start);
        }
        let decoded = percent_decode(ptr);
        let ptr: &str = decoded.as_deref().unwrap_or(ptr);
        if ptr.is_empty() {
            return Some(start);
        }
        let mut cur = start;
        for raw in ptr.strip_prefix('/').unwrap_or(ptr).split('/') {
            let tok = raw.replace("~1", "/").replace("~0", "~");
            match self.doc.value(cur) {
                Value::Object(members) => {
                    cur = members.iter().find(|m| m.key == tok)?.val;
                }
                Value::Array(items) => {
                    cur = *items.get(tok.parse::<usize>().ok()?)?;
                }
                _ => return None,
            }
        }
        Some(cur)
    }

    /// Normalizes one schema node into a FormNode.
    fn schema(
        &mut self,
        n: NodeId,
        ctx: &RefCtx,
        key: &str,
        required: bool,
        depth: usize,
    ) -> FormNodeId {
        if depth > MAX_DEPTH {
            return self.push_raw(
                key,
                &format!("{} max schema depth exceeded, edit as JSON", icons::WARN),
            );
        }
        let fid = match self.doc.value(n) {
            // Boolean schemas (true/false) and null: no form semantics.
            Value::Null => return self.push_raw(key, ""),
            Value::Bool(_) => {
                // schema `true`/`false`: anything / nothing allowed.
                return self.push_raw(key, "boolean schema, edit as JSON");
            }
            Value::Object(_) => {
                if self.on_path.contains(&n) {
                    return self.push_raw(
                        key,
                        &format!("{} recursive $ref, edit as JSON", icons::WARN),
                    );
                }
                if let Some(&m) = self.memo.get(&n) {
                    return self.fork(m, key, required);
                }
                self.on_path.insert(n);
                let fid = self.build(n, ctx, key, required, depth);
                // Memoize before applying the key fallback: the fallback
                // title belongs to the property that references this node,
                // not to the node itself. Nested $refs already applied a
                // fallback for the first referencing key, so strip it here;
                // fork names each property correctly.
                let f = &mut self.nodes[fid as usize];
                if f.title_fallback {
                    f.title.clear();
                    f.title_fallback = false;
                }
                self.memo.insert(n, fid);
                fid
            }
            _ => return self.push_raw(key, ""),
        };
        let r = self.fork(fid, key, required);
        self.on_path.remove(&n);
        r
    }

    fn push_raw(&mut self, key: &str, desc: &str) -> FormNodeId {
        self.push_form(FormNode {
            key: key.into(),
            title: title_fallback(key),
            kind: Kind::Raw,
            desc: desc.into(),
            ..FormNode::default()
        })
    }

    fn push_form(&mut self, f: FormNode) -> FormNodeId {
        let id = self.nodes.len() as FormNodeId;
        self.nodes.push(f);
        id
    }

    /// Returns a memoized node as one referencing property sees it: its
    /// key, fallback title and required flag are that property's, and a
    /// combo also re-keys its variants (nested combos and array items
    /// included) — a variant is a value of the referencing member, so it
    /// must not keep the key of the first property that happened to build
    /// the shared node.
    fn fork(&mut self, fid: FormNodeId, key: &str, required: bool) -> FormNodeId {
        let mut cp = self.nodes[fid as usize].clone();
        cp.key = key.into();
        cp.required = required;
        if cp.title.is_empty() {
            cp.title = title_fallback(key);
            cp.title_fallback = true;
        }
        cp.variants = self.rekey_variants(cp.variants.clone(), key);
        self.push_form(cp)
    }

    /// Copies the variants of a shared combo for the referencing property.
    fn rekey_variants(&mut self, vs: Vec<FormNodeId>, key: &str) -> Vec<FormNodeId> {
        if !self.variants_need_key(&vs, key) {
            return vs;
        }
        let mut out = Vec::with_capacity(vs.len());
        for &v in &vs {
            let mut c = self.nodes[v as usize].clone();
            c.key = key.into();
            c.variants = self.rekey_variants(c.variants.clone(), key);
            if let Some(it) = c.items {
                if self.nodes[it as usize].key != key {
                    let mut i2 = self.nodes[it as usize].clone();
                    i2.key = key.into();
                    c.items = Some(self.push_form(i2));
                }
            }
            out.push(self.push_form(c));
        }
        out
    }

    fn variants_need_key(&self, vs: &[FormNodeId], key: &str) -> bool {
        for &v in vs {
            let f = &self.nodes[v as usize];
            if f.key != key
                || f.items.is_some_and(|it| self.nodes[it as usize].key != key)
                || self.variants_need_key(&f.variants, key)
            {
                return true;
            }
        }
        false
    }

    fn build(
        &mut self,
        n: NodeId,
        ctx: &RefCtx,
        key: &str,
        required: bool,
        depth: usize,
    ) -> FormNodeId {
        // 1. $ref: resolve, recurse, apply sibling title/description overrides.
        let ref_str = field(&self.doc, n, "$ref").and_then(|r| match self.doc.value(r) {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        });
        if let Some(ref_str) = ref_str {
            return match self.resolve_ref(&ref_str, ctx, depth) {
                Err(err) => {
                    let desc = append_warn(
                        &desc_of(&self.doc, n),
                        &format!("{} ref unreachable: {err}", icons::WARN),
                    );
                    self.push_form(FormNode {
                        key: key.into(),
                        kind: Kind::Raw,
                        desc,
                        required,
                        ..FormNode::default()
                    })
                }
                Ok((target, tctx)) => {
                    let merged = self.schema(target, &tctx, key, required, depth + 1);
                    let t = title_of(&self.doc, n);
                    if !t.is_empty() {
                        let m = &mut self.nodes[merged as usize];
                        m.title = t;
                        m.title_fallback = false;
                        m.title_from_ref = false;
                    } else {
                        // The property has no title of its own: the row must
                        // stay identifiable by its key (Cargo profile,
                        // package, workspace, …), while the referenced
                        // title still names the submenu.
                        self.nodes[merged as usize].title_from_ref = true;
                    }
                    let d = desc_of(&self.doc, n);
                    if !d.is_empty() {
                        self.nodes[merged as usize].desc = d;
                    }
                    merged
                }
            };
        }

        // 2. allOf: shallow-merge into one synthetic schema, then normalize.
        let subs = array_of(&self.doc, n, "allOf");
        if !subs.is_empty() {
            let merged = self.merge_all_of(n, &subs, ctx, depth);
            return self.schema(merged, ctx, key, required, depth + 1);
        }

        let mut f = FormNode {
            key: key.into(),
            title: title_of(&self.doc, n),
            desc: desc_of(&self.doc, n),
            required,
            ..FormNode::default()
        };
        if let Some(d) = field(&self.doc, n, "default") {
            if !matches!(self.doc.value(d), Value::Null) {
                f.default_ = Some(d);
            }
        }
        if let Some(r) = field(&self.doc, n, "x-tag-reference") {
            if let Value::Str(s) = self.doc.value(r) {
                f.tag_ref = s.clone();
            }
        }
        apply_constraints(&self.doc, n, &mut f);

        let (t, nullable) = type_of(&self.doc, n);
        let has_props = matches!(
            field(&self.doc, n, "properties").map(|p| self.doc.value(p)),
            Some(Value::Object(_))
        );
        let has_items = field(&self.doc, n, "items").is_some();
        let has_additional = field(&self.doc, n, "additionalProperties").is_some();
        let mut combo = array_of(&self.doc, n, "oneOf");
        if combo.is_empty() {
            combo = array_of(&self.doc, n, "anyOf");
        }

        if let Some(opts) = enum_of(&self.doc, n) {
            f.kind = Kind::Enum;
            f.enum_ = opts;
        } else if t == "boolean" {
            f.kind = Kind::Boolean;
        } else if !combo.is_empty() && !has_props && t.is_empty() {
            // an open string plus a closed set of strings (workflow shell:
            // anyOf[plain string, enum of builtins]) is a text field with
            // the enum values as completions, not a variant picker
            if let Some(sugg) = self.string_combo(&combo, ctx, key, depth) {
                f.kind = Kind::Scalar;
                f.json_type = "string".into();
                f.suggestions = sugg;
            } else {
                let (kind, opts, variants) = self.build_variants(&combo, ctx, key, depth);
                f.kind = kind;
                f.enum_ = opts;
                f.variants = variants;
            }
        } else if t == "object" || has_props || has_additional {
            let ap = field(&self.doc, n, "additionalProperties");
            if !has_props && ap.is_some() && usable_additional(&self.doc, n) {
                // string-keyed map (sing-box HTTPHeader et al): rows come
                // from the document, every value edits against the value
                // schema
                f.kind = Kind::Map;
                f.items = Some(self.schema(ap.unwrap_or(n), ctx, key, false, depth + 1));
            } else if !has_props {
                // single-pattern object (GitHub workflow jobs): document
                // keys are the rows, each value edits against the pattern's
                // value schema
                if let Some((pat, val)) = pattern_map(&self.doc, n) {
                    f.kind = Kind::Map;
                    f.key_pattern = pat;
                    f.items = Some(self.schema(val, ctx, key, false, depth + 1));
                } else {
                    f.kind = Kind::Object;
                    f.children = self.build_children(n, ctx, depth);
                }
            } else {
                f.kind = Kind::Object;
                f.children = self.build_children(n, ctx, depth);
            }
        } else if t == "array" || has_items {
            f.kind = Kind::Array;
            if let Some(items) = field(&self.doc, n, "items") {
                let it = self.schema(items, ctx, key, false, depth + 1);
                if self.nodes[it as usize].kind == Kind::Array {
                    let (title, desc) = {
                        let i = &self.nodes[it as usize];
                        (i.title.clone(), i.desc.clone())
                    };
                    f.items = Some(self.push_form(FormNode {
                        key: key.into(),
                        title,
                        kind: Kind::Raw,
                        desc: append_warn(&desc, "nested arrays not supported, edit as JSON"),
                        ..FormNode::default()
                    }));
                } else {
                    f.items = Some(it);
                }
            }
            f.min_items = int_of(&self.doc, n, "minItems");
            f.max_items = int_of(&self.doc, n, "maxItems");
        } else if t == "string" || t == "integer" || t == "number" {
            f.kind = Kind::Scalar;
            f.json_type = t;
        } else {
            f.kind = Kind::Raw;
            f.null_only = null_only_type(&self.doc, n);
        }
        if nullable && f.kind != Kind::Raw {
            f.desc = append_warn(&f.desc, "may be omitted");
        }
        self.push_form(f)
    }

    /// Collapses a oneOf/anyOf whose branches are all strings into one text
    /// input: a branch must be an unconstrained string (any text stays
    /// valid) and the other branches may be enums of strings, whose options
    /// become tab completions. Strings the schema does not list stay
    /// editable — save-time validation remains authoritative.
    fn string_combo(
        &mut self,
        combo: &[NodeId],
        ctx: &RefCtx,
        key: &str,
        depth: usize,
    ) -> Option<Vec<String>> {
        let mut sugg: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut open = false;
        for &vn in combo {
            let fid = self.schema(vn, ctx, key, false, depth + 1);
            let f = self.nodes[fid as usize].clone();
            match f.kind {
                Kind::Scalar => {
                    if f.json_type != "string"
                        || !f.pattern.is_empty()
                        || f.min_len.is_some()
                        || f.max_len.is_some()
                    {
                        return None;
                    }
                    open = true;
                }
                Kind::Enum => {
                    for o in &f.enum_ {
                        if !matches!(self.doc.value(o.val), Value::Str(_)) {
                            return None; // a non-string option closes the set
                        }
                        if seen.insert(o.display.clone()) {
                            sugg.push(o.display.clone());
                        }
                    }
                }
                _ => return None,
            }
        }
        if !open {
            return None;
        }
        Some(sugg)
    }

    /// Handles oneOf/anyOf: if every variant is a const or single value
    /// enum the whole thing collapses into one enum select; otherwise the
    /// UI gets a variant picker.
    fn build_variants(
        &mut self,
        combo: &[NodeId],
        ctx: &RefCtx,
        key: &str,
        depth: usize,
    ) -> (Kind, Vec<EnumOpt>, Vec<FormNodeId>) {
        let mut opts: Vec<EnumOpt> = Vec::new();
        let mut variants: Vec<FormNodeId> = Vec::new();
        let mut all_single = true;
        for &vn in combo {
            let fid = self.schema(vn, ctx, key, false, depth + 1);
            let f = &self.nodes[fid as usize];
            if f.kind != Kind::Enum || f.enum_.len() != 1 {
                all_single = false;
                break;
            }
            opts.push(f.enum_[0].clone());
            variants.push(fid);
        }
        if all_single && !opts.is_empty() {
            return (Kind::Enum, opts, Vec::new());
        }
        let mut variants = Vec::new();
        for &vn in combo {
            let fid = self.schema(vn, ctx, key, false, depth + 1);
            if title_of(&self.doc, vn).is_empty() {
                // no schema title: clear the key fallback too — the UI
                // shows the structural signature instead of a meaningless
                // "variant N"
                let f = &mut self.nodes[fid as usize];
                f.title.clear();
                f.title_fallback = false;
            }
            variants.push(fid);
        }
        (Kind::OneOf, Vec::new(), variants)
    }

    fn build_children(&mut self, n: NodeId, ctx: &RefCtx, depth: usize) -> Vec<FormNodeId> {
        let Some(props) = field(&self.doc, n, "properties") else {
            return Vec::new();
        };
        let members = match self.doc.value(props) {
            Value::Object(ms) => ms.clone(),
            _ => return Vec::new(),
        };
        let mut req_set: HashSet<String> = HashSet::new();
        for r in array_of(&self.doc, n, "required") {
            if let Value::Str(s) = self.doc.value(r) {
                req_set.insert(s.clone());
            }
        }
        let mut children = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for m in members {
            // machine-generated schemas repeat keys inside one properties
            // object (older sing-box schemas repeat the dial-fields
            // tables); the document value always resolves to the first
            // member, so later duplicates would be dead rows — keep the
            // first occurrence only
            if !seen.insert(m.key.clone()) {
                continue;
            }
            let required = req_set.contains(&m.key);
            children.push(self.schema(m.val, ctx, &m.key, required, depth + 1));
        }
        children
    }

    /// Shallow-merges the node's own keywords with every allOf sub schema
    /// (sub $refs resolved first, nested allOf expanded) into one synthetic
    /// schema node.
    fn merge_all_of(&mut self, n: NodeId, subs: &[NodeId], ctx: &RefCtx, depth: usize) -> NodeId {
        if depth > MAX_DEPTH {
            return n;
        }
        let mut list = vec![n];
        for &sub in subs {
            let mut cur = sub;
            let mut cctx = ctx.clone();
            let mut cur_is_object = matches!(self.doc.value(cur), Value::Object(_));
            let mut hops = 0usize;
            while cur_is_object {
                if hops > MAX_DEPTH {
                    break;
                }
                let Some(ref_str) =
                    field(&self.doc, cur, "$ref").and_then(|r| match self.doc.value(r) {
                        Value::Str(s) => Some(s.clone()),
                        _ => None,
                    })
                else {
                    break;
                };
                match self.resolve_ref(&ref_str, &cctx, depth + hops) {
                    Err(_) => {
                        cur_is_object = false;
                        cur = NodeId::MAX; // sentinel: dropped below
                        break;
                    }
                    Ok((target, tctx)) => {
                        cur = target;
                        cctx = tctx;
                        cur_is_object = matches!(self.doc.value(cur), Value::Object(_));
                    }
                }
                hops += 1;
            }
            let dropped = cur == NodeId::MAX;
            if !dropped && cur_is_object {
                let inner = array_of(&self.doc, cur, "allOf");
                let cur = if !inner.is_empty() {
                    self.merge_all_of(cur, &inner, &cctx, depth + 1)
                } else {
                    cur
                };
                list.push(cur);
            }
        }
        self.merge_parts(&list, depth)
    }

    /// Shallow-merges the member maps of object schemas: properties
    /// concatenate (later parts win key conflicts), required unions,
    /// numeric bounds keep the stricter side, enums intersect, every other
    /// keyword takes the first non-empty value. oneOf/anyOf members are
    /// withheld from the plain merge and distributed over their variants
    /// when the shapes are object-compatible — rest ∧ (v1 ∨ v2 …) ≡
    /// (rest ∧ v1) ∨ (rest ∧ v2) … — which is how sing-box composes
    /// Rule = matchers allOf RuleAction; the UI flattens the resulting
    /// nested variants (dynamicVariants). A combo that cannot be
    /// distributed (scalar variant against an object rest, bare $ref
    /// variant, cartesian explosion, depth cap) stays a plain member
    /// exactly as before: save-time validation remains the backstop.
    fn merge_parts(&mut self, parts: &[NodeId], depth: usize) -> NodeId {
        let mut members: Vec<Member> = Vec::new();
        let mut combos: Vec<(String, Vec<NodeId>)> = Vec::new();
        for &part in parts {
            let ms: Vec<Member> = match self.doc.value(part) {
                Value::Object(ms) => ms.clone(),
                _ => continue,
            };
            for m in ms {
                match m.key.as_str() {
                    "allOf" | "$schema" | "$id" | "$comment" => {
                        // dropped
                    }
                    "oneOf" | "anyOf" => {
                        if let Value::Array(vs) = self.doc.value(m.val) {
                            if !vs.is_empty() {
                                combos.push((m.key, vs.clone()));
                            }
                        }
                    }
                    "properties" => {
                        let mut base: Vec<Member> = match member_index(&members, "properties") {
                            Some(i) => match self.doc.value(members[i].val) {
                                Value::Object(b) => b.clone(),
                                _ => Vec::new(),
                            },
                            None => Vec::new(),
                        };
                        if let Value::Object(pms) = self.doc.value(m.val) {
                            for pm in pms {
                                if let Some(i) = base.iter().position(|b| b.key == pm.key) {
                                    base[i].val = pm.val; // later wins
                                } else {
                                    base.push(pm.clone());
                                }
                            }
                        }
                        let nid = self.doc.push(NodeData {
                            value: Value::Object(base),
                            ..NodeData::default()
                        });
                        member_set(&mut members, "properties", nid);
                    }
                    "required" => {
                        let mut base: Vec<NodeId> = match member_index(&members, "required") {
                            Some(i) => match self.doc.value(members[i].val) {
                                Value::Array(a) => a.clone(),
                                _ => Vec::new(),
                            },
                            None => Vec::new(),
                        };
                        if let Value::Array(vs) = self.doc.value(m.val) {
                            for &rv in vs {
                                let found = base.iter().any(|&b| {
                                    matches!((self.doc.value(b), self.doc.value(rv)),
                                        (Value::Str(bs), Value::Str(rs)) if bs == rs)
                                });
                                if !found {
                                    base.push(rv);
                                }
                            }
                        }
                        let nid = self.doc.push(NodeData {
                            value: Value::Array(base),
                            ..NodeData::default()
                        });
                        member_set(&mut members, "required", nid);
                    }
                    "minimum" | "minLength" | "exclusiveMinimum" | "maximum" | "maxLength"
                    | "exclusiveMaximum" => {
                        let old = member_index(&members, &m.key).map(|i| members[i].val);
                        if let Some(merged) = self.stricter_num(old, m.val, &m.key) {
                            member_set(&mut members, &m.key, merged);
                        }
                    }
                    "enum" => {
                        let old = member_index(&members, "enum").map(|i| members[i].val);
                        match old {
                            None => member_set(&mut members, "enum", m.val),
                            Some(old) => {
                                let inter = self.intersect_enum(old, m.val);
                                member_set(&mut members, "enum", inter);
                            }
                        }
                    }
                    "type" | "title" | "description" | "default" | "examples" | "pattern" => {
                        let old_empty = member_index(&members, &m.key)
                            .map(|i| is_empty_node(&self.doc, members[i].val))
                            .unwrap_or(true);
                        if old_empty {
                            member_set(&mut members, &m.key, m.val);
                        }
                    }
                    _ => {
                        if member_index(&members, &m.key).is_none() {
                            member_set(&mut members, &m.key, m.val);
                        }
                    }
                }
            }
        }
        let base = self.doc.push(NodeData {
            value: Value::Object(members),
            ..NodeData::default()
        });
        if combos.is_empty() {
            return base;
        }
        let keep = |me: &mut Parser, base: NodeId| -> NodeId {
            let (key, vals) = (&combos[0].0, &combos[0].1);
            let present =
                matches!(me.doc.value(base), Value::Object(ms) if ms.iter().any(|m| &m.key == key));
            if !present {
                let arr = me.doc.push(NodeData {
                    value: Value::Array(vals.clone()),
                    ..NodeData::default()
                });
                if let Value::Object(ms) = &mut me.doc.node_mut(base).value {
                    ms.push(Member {
                        key: key.clone(),
                        val: arr,
                    });
                }
            }
            base
        };
        let mut distributable = !matches!(self.doc.value(base), Value::Object(ms) if
            ms.iter().any(|m| m.key == "properties")); // an unshaped rest merges into anything
        if !distributable {
            distributable = true;
            for (_, vs) in &combos {
                for &v in vs {
                    if !self.objectish(v) {
                        distributable = false;
                    }
                }
            }
        }
        if depth >= MAX_DEPTH || !distributable {
            return keep(self, base);
        }
        let mut tuples: Vec<Vec<NodeId>> = vec![Vec::new()];
        for (_, vs) in &combos {
            let mut next = Vec::new();
            for t in &tuples {
                for &v in vs {
                    let mut t2 = t.clone();
                    t2.push(v);
                    next.push(t2);
                }
            }
            tuples = next;
            if tuples.len() > MAX_COMBO_PRODUCT {
                return keep(self, base);
            }
        }
        let mut variants = Vec::with_capacity(tuples.len());
        for t in &tuples {
            let mut p2 = Vec::with_capacity(1 + t.len());
            p2.push(base);
            p2.extend(t.iter().copied());
            variants.push(self.merge_parts(&p2, depth + 1));
        }
        let arr = self.doc.push(NodeData {
            value: Value::Array(variants),
            ..NodeData::default()
        });
        self.doc.push(NodeData {
            value: Value::Object(vec![Member {
                key: combos[0].0.clone(),
                val: arr,
            }]),
            ..NodeData::default()
        })
    }

    /// Keeps the stricter of two numeric constraints (bigger min, smaller
    /// max); booleans OR together via numeric encoding 1/0.
    fn stricter_num(&self, old: Option<NodeId>, cur: NodeId, key: &str) -> Option<NodeId> {
        let Some(old) = old else {
            return Some(cur);
        };
        let (ov, ok1) = self.node_num(old);
        let (cv, ok2) = self.node_num(cur);
        if !ok1 {
            return Some(cur);
        }
        if !ok2 {
            return Some(old);
        }
        match key {
            "minimum" | "minLength" | "exclusiveMinimum" => {
                if cv > ov {
                    Some(cur)
                } else {
                    Some(old)
                }
            }
            "maximum" | "maxLength" | "exclusiveMaximum" => {
                if cv < ov {
                    Some(cur)
                } else {
                    Some(old)
                }
            }
            _ => Some(old),
        }
    }

    fn intersect_enum(&mut self, a: NodeId, b: NodeId) -> NodeId {
        let mut out: Vec<NodeId> = Vec::new();
        if let (Value::Array(as_), Value::Array(bs)) = (self.doc.value(a), self.doc.value(b)) {
            let as_ = as_.clone();
            let bs = bs.clone();
            for &av in &as_ {
                for &bv in &bs {
                    if self.encode_value(av) == self.encode_value(bv) {
                        out.push(av);
                        break;
                    }
                }
            }
        }
        self.doc.push(NodeData {
            value: Value::Array(out),
            ..NodeData::default()
        })
    }

    /// The canonical JSON literal of a value as a string.
    fn encode_value(&self, id: NodeId) -> String {
        serde_json::to_string(&self.doc.to_value(id)).unwrap_or_default()
    }

    fn node_num(&self, n: NodeId) -> (f64, bool) {
        match doc_node_num(&self.doc, n) {
            Some(f) => (f, true),
            None => (0.0, false),
        }
    }

    /// Reports whether n shapes an object value the way build's object
    /// branch does: declared properties or an object type entry.
    fn objectish(&self, n: NodeId) -> bool {
        if !matches!(self.doc.value(n), Value::Object(_)) {
            return false;
        }
        if field(&self.doc, n, "properties").is_some() {
            return true;
        }
        let Some(t) = field(&self.doc, n, "type") else {
            return false;
        };
        match self.doc.value(t) {
            Value::Str(s) => s == "object",
            Value::Array(es) => es
                .iter()
                .any(|&e| matches!(self.doc.value(e), Value::Str(s) if s == "object")),
            _ => false,
        }
    }
}

// --- helpers ------------------------------------------------------------

fn member_index(members: &[Member], key: &str) -> Option<usize> {
    members.iter().position(|m| m.key == key)
}

fn member_set(members: &mut Vec<Member>, key: &str, val: NodeId) {
    match member_index(members, key) {
        Some(i) => members[i].val = val,
        None => members.push(Member {
            key: key.into(),
            val,
        }),
    }
}

/// Appends a parsed document into the shared arena, remapping its NodeIds
/// (alias sharing inside the document is preserved).
fn graft(doc: &mut Doc, d: &Doc) -> NodeId {
    let map: Vec<NodeId> = d.nodes.iter().map(|nd| doc.push(nd.clone())).collect();
    for (i, nd) in d.nodes.iter().enumerate() {
        let nv = match &nd.value {
            Value::Object(ms) => Value::Object(
                ms.iter()
                    .map(|m| Member {
                        key: m.key.clone(),
                        val: map[m.val as usize],
                    })
                    .collect(),
            ),
            Value::Array(vs) => Value::Array(vs.iter().map(|&v| map[v as usize]).collect()),
            other => other.clone(),
        };
        doc.node_mut(map[i]).value = nv;
    }
    map[d.root as usize]
}

/// Returns the base URI of ctx: its $id if present (resolved against the
/// fetch location), else the fetch location itself.
fn doc_base(doc: &Doc, ctx: &RefCtx) -> String {
    if let Some(id) = field(doc, ctx.doc_root, "$id") {
        if let Value::Str(s) = doc.value(id) {
            if !s.is_empty() {
                if let Ok(bu) = Url::parse(&ctx.loc) {
                    if let Ok(iu) = bu.join(s) {
                        return iu.to_string();
                    }
                }
                return s.clone();
            }
        }
    }
    ctx.loc.clone()
}

fn title_fallback(key: &str) -> String {
    if key.is_empty() || key == "$" {
        "$".into()
    } else {
        key.into()
    }
}

/// Reports whether additionalProperties shapes a map: an object schema
/// (values built against it) or schema `true` (values of any shape).
/// `false` closes the object instead — no map semantics.
fn usable_additional(doc: &Doc, n: NodeId) -> bool {
    let Some(ap) = field(doc, n, "additionalProperties") else {
        return false;
    };
    match doc.value(ap) {
        Value::Bool(b) => *b,
        Value::Object(_) => true,
        _ => false,
    }
}

/// Reports whether n is a single-pattern string-keyed map: exactly one
/// patternProperties member, additionalProperties: false closing the
/// object, and a pattern that compiles as a regexp. Returns the pattern
/// string and the value schema.
fn pattern_map(doc: &Doc, n: NodeId) -> Option<(String, NodeId)> {
    let pp = field(doc, n, "patternProperties")?;
    let members = match doc.value(pp) {
        Value::Object(ms) => ms,
        _ => return None,
    };
    if members.len() != 1 {
        return None;
    }
    let ap = field(doc, n, "additionalProperties")?;
    match doc.value(ap) {
        Value::Bool(false) => {}
        _ => return None,
    }
    let pattern = &members[0].key;
    if regex::Regex::new(pattern).is_err() {
        return None;
    }
    Some((pattern.clone(), members[0].val))
}

fn is_empty_node(doc: &Doc, n: NodeId) -> bool {
    match doc.value(n) {
        Value::Null => true,
        Value::Object(ms) => ms.is_empty(),
        Value::Array(vs) => vs.is_empty(),
        Value::Str(s) => s.is_empty(),
        Value::Num(_) | Value::Bool(_) => false,
    }
}

// --- document accessors ---------------------------------------------------

fn field(doc: &Doc, n: NodeId, key: &str) -> Option<NodeId> {
    match doc.value(n) {
        Value::Object(members) => members.iter().find(|m| m.key == key).map(|m| m.val),
        _ => None,
    }
}

fn array_of(doc: &Doc, n: NodeId, key: &str) -> Vec<NodeId> {
    match field(doc, n, key).map(|a| doc.value(a)) {
        Some(Value::Array(vs)) => vs.clone(),
        _ => Vec::new(),
    }
}

fn title_of(doc: &Doc, n: NodeId) -> String {
    match field(doc, n, "title").map(|t| doc.value(t)) {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

fn desc_of(doc: &Doc, n: NodeId) -> String {
    match field(doc, n, "description").map(|t| doc.value(t)) {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

fn bool_of(doc: &Doc, n: NodeId, key: &str) -> bool {
    match field(doc, n, key).map(|b| doc.value(b)) {
        Some(Value::Bool(b)) => *b,
        _ => false,
    }
}

fn int_of(doc: &Doc, n: NodeId, key: &str) -> i64 {
    int_field(doc, n, key).unwrap_or(0)
}

fn int_field(doc: &Doc, n: NodeId, key: &str) -> Option<i64> {
    let f = doc_node_num(doc, field(doc, n, key)?)?;
    if f != f.trunc() {
        return None;
    }
    Some(f as i64)
}

fn num_field(doc: &Doc, n: NodeId, key: &str) -> Option<f64> {
    doc_node_num(doc, field(doc, n, key)?)
}

fn doc_node_num(doc: &Doc, n: NodeId) -> Option<f64> {
    match doc.value(n) {
        Value::Num(lit) => lit.parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn apply_constraints(doc: &Doc, n: NodeId, f: &mut FormNode) {
    if let Some(v) = num_field(doc, n, "minimum") {
        f.min = Some(v);
    }
    if let Some(v) = num_field(doc, n, "maximum") {
        f.max = Some(v);
    }
    // exclusiveMinimum/Maximum: numeric (2020-12) acts as its own bound,
    // boolean (draft-07) modifies the sibling minimum/maximum.
    if let Some(v) = num_field(doc, n, "exclusiveMinimum") {
        if f.min.is_none_or(|m| v > m) {
            f.min = Some(v);
        }
        f.exclusive_min = true;
    } else if bool_of(doc, n, "exclusiveMinimum") {
        f.exclusive_min = true;
    }
    if let Some(v) = num_field(doc, n, "exclusiveMaximum") {
        if f.max.is_none_or(|m| v < m) {
            f.max = Some(v);
        }
        f.exclusive_max = true;
    } else if bool_of(doc, n, "exclusiveMaximum") {
        f.exclusive_max = true;
    }
    if let Some(v) = num_field(doc, n, "multipleOf") {
        f.multiple_of = Some(v);
    }
    if let Some(v) = int_field(doc, n, "minLength") {
        f.min_len = Some(v.max(0) as usize);
    }
    if let Some(v) = int_field(doc, n, "maxLength") {
        f.max_len = Some(v.max(0) as usize);
    }
    if let Some(s) = field(doc, n, "pattern") {
        if let Value::Str(p) = doc.value(s) {
            f.pattern = p.clone();
        }
    }
    let ex = array_of(doc, n, "examples");
    if let Some(&first) = ex.first() {
        match doc.value(first) {
            Value::Str(s) => f.placeholder = s.clone(),
            Value::Num(lit) => f.placeholder = lit.clone(),
            Value::Bool(b) => f.placeholder = if *b { "true" } else { "false" }.into(),
            _ => {}
        }
    }
}

/// Reports whether the schema's type keyword admits null and nothing else.
fn null_only_type(doc: &Doc, n: NodeId) -> bool {
    let Some(t) = field(doc, n, "type") else {
        return false;
    };
    match doc.value(t) {
        Value::Str(s) => s == "null",
        Value::Array(es) => {
            let mut found = false;
            for &e in es {
                match doc.value(e) {
                    Value::Str(s) if s == "null" => found = true,
                    _ => return false,
                }
            }
            found
        }
        _ => false,
    }
}

/// Returns the effective scalar type name (first non-null entry when type
/// is an array) and whether null is allowed.
fn type_of(doc: &Doc, n: NodeId) -> (String, bool) {
    let Some(t) = field(doc, n, "type") else {
        return (String::new(), false);
    };
    if let Value::Str(s) = doc.value(t) {
        return (s.clone(), false);
    }
    let Value::Array(es) = doc.value(t) else {
        return (String::new(), false);
    };
    let mut name = String::new();
    let mut nullable = false;
    for &e in es {
        let Value::Str(s) = doc.value(e) else {
            continue;
        };
        if s == "null" {
            nullable = true;
            continue;
        }
        if name.is_empty() {
            name = s.clone();
        }
    }
    (name, nullable)
}

/// Returns options for enum/const schemas, None if not an enum.
fn enum_of(doc: &Doc, n: NodeId) -> Option<Vec<EnumOpt>> {
    if let Some(e) = field(doc, n, "enum") {
        if let Value::Array(vs) = doc.value(e) {
            return Some(
                vs.iter()
                    .map(|&v| EnumOpt {
                        display: display_value(doc, v),
                        encoded: encode_value(doc, v),
                        val: v,
                    })
                    .collect(),
            );
        }
    }
    field(doc, n, "const")
        .map(|c| EnumOpt {
            display: display_value(doc, c),
            encoded: encode_value(doc, c),
            val: c,
        })
        .map(|o| vec![o])
}

fn display_value(doc: &Doc, v: NodeId) -> String {
    match doc.value(v) {
        Value::Str(s) => s.clone(),
        Value::Num(lit) => lit.clone(),
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        _ => encode_value(doc, v),
    }
}

/// The canonical JSON literal of a value as a string.
fn encode_value(doc: &Doc, v: NodeId) -> String {
    serde_json::to_string(&doc.to_value(v)).unwrap_or_default()
}

fn append_warn(desc: &str, warn: &str) -> String {
    if desc.is_empty() {
        warn.into()
    } else {
        format!("{desc} {warn}")
    }
}

/// Mirrors url.PathUnescape: %XX decodes, a malformed escape is an error.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = b.get(i + 1..i + 3)?;
            let hi = u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()?;
            out.push(hi);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must_load(src: &str) -> Schema {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("schema.json");
        std::fs::write(&p, src).unwrap();
        let s = load(p.to_str().unwrap()).unwrap();
        s
    }

    #[test]
    fn root_has_property_checks_declared_properties() {
        let s = must_load(
            r#"{"type":"object","properties":{"$schema":{"type":"string"},"name":{"type":"string"}}}"#,
        );
        assert!(root_has_property(&s, "$schema"));
        assert!(root_has_property(&s, "name"));
        assert!(!root_has_property(&s, "missing"));

        let s = must_load(
            r#"{"oneOf":[{"type":"object","properties":{"$schema":{"type":"string"}}}]}"#,
        );
        assert!(root_has_property(&s, "$schema"));

        let s = must_load(r#"{"type":"string"}"#);
        assert!(!root_has_property(&s, "$schema"));
    }

    fn child(s: &Schema, n: FormNodeId, key: &str) -> FormNodeId {
        find_child(s, n, key).unwrap_or_else(|| panic!("child {key:?} not found"))
    }

    fn find_child(s: &Schema, n: FormNodeId, key: &str) -> Option<FormNodeId> {
        s.nodes[n as usize]
            .children
            .iter()
            .copied()
            .find(|&c| s.nodes[c as usize].key == key)
    }

    fn kind_of(s: &Schema, n: FormNodeId) -> Kind {
        s.nodes[n as usize].kind
    }

    /// Isolates fetch's on-disk cache and per-process memo. The lock is the
    /// same one fetch_tests uses: both touch the process-global cache root.
    struct FetchIsolation {
        old_dir: Option<String>,
        _dir: tempfile::TempDir,
        _guard: parking_lot::MutexGuard<'static, ()>,
    }

    impl Drop for FetchIsolation {
        fn drop(&mut self) {
            fetch::set_cache_dir(self.old_dir.clone());
            fetch::reset_memo();
        }
    }

    fn isolate_fetch() -> FetchIsolation {
        let guard = fetch::TEST_LOCK.lock();
        let old_dir = fetch::CACHE_DIR_OVERRIDE.lock().clone();
        let dir = tempfile::tempdir().unwrap();
        fetch::set_cache_dir(Some(dir.path().to_string_lossy().into_owned()));
        fetch::reset_memo();
        FetchIsolation {
            old_dir,
            _dir: dir,
            _guard: guard,
        }
    }

    #[test]
    fn test_nested_objects() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "server": {
      "type": "object",
      "title": "Server",
      "required": ["port"],
      "properties": {
        "port": {"type": "integer", "minimum": 1024, "maximum": 65535, "title": "Port"},
        "debug": {"type": "boolean", "default": false}
      }
    },
    "name": {"type": "string", "minLength": 1, "pattern": "^[a-z]+$", "examples": ["mysrv"]}
  }
}"##,
        );
        let f = &s.nodes[s.root as usize];
        assert_eq!(f.kind, Kind::Object);
        assert_eq!(f.title, "$");
        assert_eq!(f.key, "$");
        assert_eq!(f.children.len(), 2);
        assert_eq!(s.nodes[f.children[0] as usize].key, "server");
        assert_eq!(s.nodes[f.children[1] as usize].key, "name");
        let srv = f.children[0];
        let srvf = &s.nodes[srv as usize];
        assert_eq!(srvf.kind, Kind::Object);
        assert_eq!(srvf.title, "Server");
        let port = child(&s, srv, "port");
        let pf = &s.nodes[port as usize];
        assert_eq!(pf.kind, Kind::Scalar);
        assert_eq!(pf.json_type, "integer");
        assert!(pf.required);
        assert_eq!(pf.min, Some(1024.0));
        assert_eq!(pf.max, Some(65535.0));
        let name = child(&s, s.root, "name");
        let nf = &s.nodes[name as usize];
        assert_eq!(nf.min_len, Some(1));
        assert_eq!(nf.pattern, "^[a-z]+$");
        assert_eq!(nf.placeholder, "mysrv");
        let dbg = child(&s, srv, "debug");
        let df = &s.nodes[dbg as usize];
        assert_eq!(df.kind, Kind::Boolean);
        let dv = df.default_.map(|d| s.doc.value(d));
        assert!(matches!(dv, Some(Value::Bool(false))));
    }

    #[test]
    fn test_defs_ref() {
        let s = must_load(
            r##"{
  "$defs": {"positive": {"type": "integer", "minimum": 1, "title": "Positive"}},
  "type": "object",
  "properties": {
    "count": {"$ref": "#/$defs/positive"},
    "named": {"$ref": "#/$defs/positive", "title": "Override"}
  }
}"##,
        );
        let count = child(&s, s.root, "count");
        let cf = &s.nodes[count as usize];
        assert_eq!(cf.kind, Kind::Scalar);
        assert_eq!(cf.min, Some(1.0));
        assert_eq!(cf.title, "Positive");
        let named = child(&s, s.root, "named");
        assert_eq!(s.nodes[named as usize].title, "Override");
    }

    #[test]
    fn test_recursive_ref_no_hang() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("schema.json");
        std::fs::write(
            &p,
            r##"{
  "$defs": {"node": {"type": "object", "properties": {"next": {"$ref": "#/$defs/node"}, "label": {"type": "string"}}}},
  "type": "object",
  "properties": {"root": {"$ref": "#/$defs/node"}}
}"##,
        )
        .unwrap();
        let s = load(p.to_str().unwrap()).unwrap();
        let mut n = child(&s, s.root, "root");
        while kind_of(&s, n) == Kind::Object {
            n = child(&s, n, "next");
        }
        assert_eq!(kind_of(&s, n), Kind::Raw, "expected eventual KRaw");
    }

    #[test]
    fn test_remote_ref() {
        let _guard = isolate_fetch();
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let tiny_http::ListenAddr::IP(addr) = server.server_addr() else {
            panic!("tcp listener expected");
        };
        let port = addr.port();
        let srv = server.clone();
        std::thread::spawn(move || {
            loop {
                match srv.recv() {
                    Ok(req) => {
                        let _ = req.respond(tiny_http::Response::from_string(
                            r##"{"t": {"type": "string", "minLength": 2}}"##,
                        ));
                    }
                    Err(_) => break,
                }
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root.json");
        std::fs::write(
            &root,
            format!(
                r##"{{"type":"object","properties":{{"x":{{"$ref":"http://127.0.0.1:{port}/types.json#/t"}}}}}}"##
            ),
        )
        .unwrap();
        let s = load(root.to_str().unwrap()).unwrap();
        drop(server); // the detached server thread unblocks once its Arc is gone
        let x = child(&s, s.root, "x");
        let xf = &s.nodes[x as usize];
        assert_eq!(xf.kind, Kind::Scalar, "remote ref child = {:?}", xf);
        assert_eq!(xf.min_len, Some(2));
    }

    #[test]
    fn test_remote_ref_unreachable() {
        let _guard = isolate_fetch();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root.json");
        std::fs::write(
            &root,
            r##"{"type":"object","properties":{"x":{"$ref":"http://127.0.0.1:1/types.json"}}}"##,
        )
        .unwrap();
        let s = load(root.to_str().unwrap()).unwrap();
        let x = child(&s, s.root, "x");
        let xf = &s.nodes[x as usize];
        assert_eq!(xf.kind, Kind::Raw, "unreachable ref kind = {:?}", xf.kind);
        assert!(xf.desc.contains("ref unreachable"), "desc = {:?}", xf.desc);
    }

    #[test]
    fn test_relative_ref_against_doc_base() {
        let _guard = isolate_fetch();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("common.json"),
            r##"{"defs": {"port": {"type": "integer", "minimum": 1}}}"##,
        )
        .unwrap();
        let root = dir.path().join("root.json");
        std::fs::write(
            &root,
            r##"{
  "$id": "https://example.com/schemas/root.json",
  "type": "object",
  "properties": {"p": {"$ref": "common.json#/defs/port"}}
}"##,
        )
        .unwrap();
        // $id base would point at example.com; this host is unreachable, so
        // the ref must degrade. The same test with no $id resolves against
        // the file directory and succeeds.
        let s = load(root.to_str().unwrap()).unwrap();
        let p = child(&s, s.root, "p");
        let pf = &s.nodes[p as usize];
        assert!(
            pf.kind == Kind::Raw && pf.desc.contains("ref unreachable"),
            "$id-relative ref should degrade: {:?}",
            pf
        );

        let root2 = dir.path().join("root2.json");
        std::fs::write(
            &root2,
            r##"{
  "type": "object",
  "properties": {"p": {"$ref": "common.json#/defs/port"}}
}"##,
        )
        .unwrap();
        let s2 = load(root2.to_str().unwrap()).unwrap();
        let p2 = child(&s2, s2.root, "p");
        let p2f = &s2.nodes[p2 as usize];
        assert_eq!(p2f.kind, Kind::Scalar, "directory-relative ref = {:?}", p2f);
        assert_eq!(p2f.min, Some(1.0));
    }

    // Machine-generated schemas repeat keys inside one properties object
    // (e.g. the dial-fields tables of older sing-box schemas). jsondoc
    // keeps every member, but the document value always resolves to the
    // first member, so later duplicates are dead rows — they must be
    // dropped, keeping the first occurrence.
    #[test]
    fn test_duplicate_property_keys() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "a": {"type": "string", "title": "First A"},
    "b": {"type": "integer"},
    "a": {"type": "boolean"},
    "c": {"type": "string"},
    "b": {"type": "number", "title": "Second B"}
  }
}"##,
        );
        let keys: Vec<&str> = s.nodes[s.root as usize]
            .children
            .iter()
            .map(|&c| s.nodes[c as usize].key.as_str())
            .collect();
        assert_eq!(keys, ["a", "b", "c"], "children keys = {:?}", keys);
        let a = child(&s, s.root, "a");
        let af = &s.nodes[a as usize];
        assert_ne!(
            af.kind,
            Kind::Boolean,
            "first a must win, got kind={:?}",
            af.kind
        );
    }

    #[test]
    fn test_all_of_merge() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "merged": {
      "allOf": [
        {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]},
        {"properties": {"b": {"type": "integer"}}, "required": ["b"], "minimum": 5}
      ]
    }
  }
}"##,
        );
        let m = child(&s, s.root, "merged");
        let mf = &s.nodes[m as usize];
        assert_eq!(mf.kind, Kind::Object);
        assert_eq!(mf.children.len(), 2);
        assert_eq!(s.nodes[mf.children[0] as usize].key, "a");
        assert_eq!(s.nodes[mf.children[1] as usize].key, "b");
        assert!(
            s.nodes[mf.children[0] as usize].required && s.nodes[mf.children[1] as usize].required
        );
    }

    #[test]
    fn test_all_of_numeric_stricter() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "n": {"allOf": [{"type": "integer", "minimum": 1}, {"minimum": 10}, {"maximum": 99}]}
  }
}"##,
        );
        let n = child(&s, s.root, "n");
        let nf = &s.nodes[n as usize];
        assert_eq!(nf.kind, Kind::Scalar);
        assert_eq!(nf.json_type, "integer");
        assert_eq!(nf.min, Some(10.0), "stricter minimum");
        assert_eq!(nf.max, Some(99.0), "maximum");
    }

    #[test]
    fn test_one_of_const_merge() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "mode": {"oneOf": [{"const": "fast"}, {"const": "slow"}, {"const": "auto"}]}
  }
}"##,
        );
        let m = child(&s, s.root, "mode");
        let mf = &s.nodes[m as usize];
        assert_eq!(mf.kind, Kind::Enum);
        assert_eq!(mf.enum_.len(), 3);
        assert_eq!(mf.enum_[0].encoded, "\"fast\"");
        assert_eq!(mf.enum_[0].display, "fast");
    }

    #[test]
    fn test_one_of_variants() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "conn": {
      "oneOf": [
        {"title": "TCP", "type": "object", "properties": {"host": {"type": "string"}, "port": {"type": "integer"}}, "required": ["host"]},
        {"title": "Unix", "type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
      ]
    }
  }
}"##,
        );
        let c = child(&s, s.root, "conn");
        let cf = &s.nodes[c as usize];
        assert_eq!(cf.kind, Kind::OneOf);
        assert_eq!(cf.variants.len(), 2);
        assert_eq!(s.nodes[cf.variants[0] as usize].title, "TCP");
        assert_eq!(s.nodes[cf.variants[1] as usize].title, "Unix");
        assert_eq!(kind_of(&s, cf.variants[0]), Kind::Object);
    }

    #[test]
    fn test_variant_tag_reference() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "http_client": {"anyOf": [
      {"type": "string", "x-tag-reference": "http_client"},
      {"type": "object", "properties": {"engine": {"type": "string"}}}
    ]}
  }
}"##,
        );
        let c = child(&s, s.root, "http_client");
        let cf = &s.nodes[c as usize];
        assert_eq!(cf.kind, Kind::OneOf);
        assert_eq!(cf.variants.len(), 2);
        assert_eq!(
            s.nodes[cf.variants[0] as usize].tag_ref, "http_client",
            "string variant TagRef"
        );
        assert_eq!(
            s.nodes[cf.variants[1] as usize].tag_ref, "",
            "object variant TagRef"
        );
    }

    #[test]
    fn test_variant_untitled_carries_no_label() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "v": {"anyOf": [{"type": "string"}, {"type": "integer"}]}
  }
}"##,
        );
        let v = child(&s, s.root, "v");
        let vf = &s.nodes[v as usize];
        assert_eq!(vf.kind, Kind::OneOf);
        assert_eq!(vf.variants.len(), 2);
        // no synthesized "variant N": the UI shows the structural signature
        assert_eq!(s.nodes[vf.variants[0] as usize].title, "");
        assert_eq!(s.nodes[vf.variants[1] as usize].title, "");
    }

    #[test]
    fn test_enum_encoding() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "mix": {"enum": [3, "3", true, null, 1.5]}
  }
}"##,
        );
        let m = child(&s, s.root, "mix");
        let mf = &s.nodes[m as usize];
        assert_eq!(mf.kind, Kind::Enum);
        assert_eq!(mf.enum_.len(), 5);
        let enc: Vec<&str> = mf.enum_.iter().map(|o| o.encoded.as_str()).collect();
        let mut uniq = enc.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), enc.len(), "duplicate encoding in {enc:?}");
        for w in ["3", "\"3\"", "true", "null", "1.5"] {
            assert!(enc.contains(&w), "missing encoding {w:?} in {enc:?}");
        }
    }

    #[test]
    fn test_arrays() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 5},
    "levels": {"type": "array", "items": {"enum": ["debug", "info", "warn"]}},
    "mounts": {"type": "array", "items": {"type": "object", "properties": {"src": {"type": "string"}}}},
    "matrix": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}}
  }
}"##,
        );
        let tags = child(&s, s.root, "tags");
        let tf = &s.nodes[tags as usize];
        assert_eq!(tf.kind, Kind::Array);
        let ti = tf.items.expect("tags items");
        assert_eq!(s.nodes[ti as usize].kind, Kind::Scalar);
        assert_eq!(s.nodes[ti as usize].json_type, "string");
        assert_eq!((tf.min_items, tf.max_items), (1, 5), "tags bounds");
        let levels = child(&s, s.root, "levels");
        let li = s.nodes[levels as usize].items.expect("levels items");
        let lif = &s.nodes[li as usize];
        assert_eq!(lif.kind, Kind::Enum);
        assert_eq!(lif.enum_.len(), 3);
        let mounts = child(&s, s.root, "mounts");
        let mi = s.nodes[mounts as usize].items.expect("mounts items");
        assert_eq!(kind_of(&s, mi), Kind::Object);
        let matrix = child(&s, s.root, "matrix");
        let xi = s.nodes[matrix as usize].items.expect("matrix items");
        assert_eq!(
            kind_of(&s, xi),
            Kind::Raw,
            "nested array items should be KRaw"
        );
    }

    #[test]
    fn test_property_order_preserved() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "zulu": {"type": "string"},
    "alpha": {"type": "string"},
    "Mike": {"type": "string"},
    "1num": {"type": "string"}
  }
}"##,
        );
        let got: Vec<&str> = s.nodes[s.root as usize]
            .children
            .iter()
            .map(|&c| s.nodes[c as usize].key.as_str())
            .collect();
        assert_eq!(got, ["zulu", "alpha", "Mike", "1num"]);
    }

    #[test]
    fn test_unknown_type_degrades() {
        let s = must_load(
            r##"{"type": "object", "properties": {"weird": {}, "anyConst": {"const": {"a": 1}}}}"##,
        );
        let w = child(&s, s.root, "weird");
        assert_eq!(kind_of(&s, w), Kind::Raw, "empty schema kind");
        let ac = child(&s, s.root, "anyConst");
        let acf = &s.nodes[ac as usize];
        assert_eq!(acf.kind, Kind::Enum);
        assert_eq!(acf.enum_.len(), 1);
        assert_eq!(acf.enum_[0].display, "{\"a\":1}");
    }

    #[test]
    fn test_root_non_object() {
        let s = must_load(r##"{"type": "string", "minLength": 1}"##);
        let sf = &s.nodes[s.root as usize];
        assert_eq!(sf.kind, Kind::Scalar);
        assert_eq!(sf.title, "$");
        let a = must_load(r##"{"type": "array", "items": {"type": "integer"}}"##);
        let af = &a.nodes[a.root as usize];
        assert_eq!(af.kind, Kind::Array);
        assert!(af.items.is_some());
        assert_eq!(a.nodes[af.items.unwrap() as usize].kind, Kind::Scalar);
        let o = must_load(r##"{"oneOf": [{"type": "string"}, {"type": "integer"}]}"##);
        let of = &o.nodes[o.root as usize];
        assert_eq!(of.kind, Kind::OneOf);
        assert_eq!(of.variants.len(), 2);
    }

    // Several properties $ref-ing the same untitled definition must each
    // fall back to their own key, not the first key that touched the node
    // (sing-box schema shares duration/address defs across many fields).
    #[test]
    fn test_shared_ref_target_keeps_own_title() {
        let s = must_load(
            r##"{
  "type": "object",
  "$defs": {"duration": {"type": "string"}},
  "properties": {
    "connect_timeout": {"$ref": "#/$defs/duration"},
    "udp_timeout":     {"$ref": "#/$defs/duration"},
    "fallback_delay":  {"$ref": "#/$defs/duration"}
  }
}"##,
        );
        for key in ["connect_timeout", "udp_timeout", "fallback_delay"] {
            let c = child(&s, s.root, key);
            let cf = &s.nodes[c as usize];
            assert_eq!(
                cf.title, key,
                "child {key:?}: title = {:?}, want own key",
                cf.title
            );
        }
    }

    // Combo variants are values of the referencing property: a $ref'd anyOf
    // definition reached from several properties must hand each one its own
    // key, or value completion would look under the first property's member.
    #[test]
    fn test_shared_combo_variants_keep_referencing_key() {
        let s = must_load(
            r##"{
	  "type": "object",
	  "$defs": {"duration": {"anyOf": [{"type": "integer"}, {"type": "string"}]}},
	  "properties": {
	    "connect_timeout": {"$ref": "#/$defs/duration"},
	    "udp_timeout":     {"$ref": "#/$defs/duration"}
	  }
	}"##,
        );
        for key in ["connect_timeout", "udp_timeout"] {
            let c = child(&s, s.root, key);
            let cf = &s.nodes[c as usize];
            assert_eq!(cf.key, key, "{key}: node key = {:?}", cf.key);
            assert_eq!(
                cf.variants.len(),
                2,
                "{key}: {} variants, want 2",
                cf.variants.len()
            );
            for (i, &v) in cf.variants.iter().enumerate() {
                assert_eq!(s.nodes[v as usize].key, key, "{key} variant {i}: key");
            }
        }
    }

    #[test]
    fn test_load_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path().join("nope.json").to_str().unwrap()).is_err());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{invalid").unwrap();
        assert!(
            load(bad.to_str().unwrap()).is_err(),
            "invalid JSON should error"
        );
    }

    #[test]
    fn test_all_of_distributes_combo() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "rule": {
      "allOf": [
        {"type": "object", "properties": {"domain": {"type": "string"}, "invert": {"type": "boolean"}}},
        {"$ref": "#/$defs/act"}
      ]
    }
  },
  "$defs": {"act": {"oneOf": [
    {"type": "object", "properties": {"action": {"const": "route"}, "outbound": {"type": "string"}}, "required": ["action"]},
    {"type": "object", "properties": {"action": {"const": "reject"}, "method": {"type": "integer"}}, "required": ["action"]}
  ]}}
}"##,
        );
        let r = child(&s, s.root, "rule");
        let rf = &s.nodes[r as usize];
        assert_eq!(rf.kind, Kind::OneOf);
        assert_eq!(rf.variants.len(), 2);
        for (i, want) in ["route", "reject"].into_iter().enumerate() {
            let v = rf.variants[i];
            assert_eq!(kind_of(&s, v), Kind::Object, "variant {i}");
            // matcher fields from the object part ride along on every variant
            assert!(
                find_child(&s, v, "domain").is_some() && find_child(&s, v, "invert").is_some(),
                "variant {i} lost the allOf base fields"
            );
            let a =
                find_child(&s, v, "action").unwrap_or_else(|| panic!("variant {i} action missing"));
            let af = &s.nodes[a as usize];
            assert_eq!(af.kind, Kind::Enum, "variant {i} action");
            assert_eq!(af.enum_.len(), 1);
            assert_eq!(af.enum_[0].encoded, format!("\"{want}\""));
            assert!(af.required, "variant {i} action must stay required");
        }
        assert!(
            find_child(&s, rf.variants[0], "outbound").is_some(),
            "route variant lost outbound"
        );
        assert!(
            find_child(&s, rf.variants[1], "method").is_some(),
            "reject variant lost method"
        );
    }

    // A scalar combo against an object rest cannot distribute: the combo
    // stays a plain member and build drops it, exactly as before.
    #[test]
    fn test_all_of_combo_scalar_fallback() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "v": {"allOf": [
      {"type": "object", "properties": {"a": {"type": "string"}}},
      {"oneOf": [{"type": "string"}, {"type": "integer"}]}
    ]}
  }
}"##,
        );
        let v = child(&s, s.root, "v");
        let vf = &s.nodes[v as usize];
        assert_eq!(vf.kind, Kind::Object);
        assert!(vf.variants.is_empty());
        assert!(find_child(&s, v, "a").is_some(), "object fields lost");
    }

    // Nothing object-shaped outside the combo: the variants pass through.
    #[test]
    fn test_all_of_pure_combo() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "v": {"allOf": [{"oneOf": [{"type": "string"}, {"type": "integer"}]}]}
  }
}"##,
        );
        let v = child(&s, s.root, "v");
        let vf = &s.nodes[v as usize];
        assert_eq!(vf.kind, Kind::OneOf);
        assert_eq!(vf.variants.len(), 2);
    }

    // An allOf nested inside another allOf sub merges instead of being dropped.
    #[test]
    fn test_all_of_nested_expansion() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "v": {"allOf": [
      {"allOf": [{"properties": {"a": {"type": "string"}}}, {"properties": {"b": {"type": "integer"}}}]},
      {"properties": {"c": {"type": "boolean"}}}
    ]}
  }
}"##,
        );
        let v = child(&s, s.root, "v");
        assert_eq!(kind_of(&s, v), Kind::Object);
        for k in ["a", "b", "c"] {
            assert!(find_child(&s, v, k).is_some(), "field {k:?} lost");
        }
    }

    // Sing-box HTTPHeader: a string-keyed map whose values are string or
    // []string; the map builds as KMap, the value schema as its Items.
    #[test]
    fn test_additional_properties_map() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "headers": {"$ref": "#/$defs/hdr"},
    "free": {"type": "object", "additionalProperties": true},
    "closed": {"type": "object", "additionalProperties": false},
    "mixed": {"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": {"type": "string"}}
  },
  "$defs": {"hdr": {"type": "object", "additionalProperties": {
    "anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]
  }}}
}"##,
        );
        let h = child(&s, s.root, "headers");
        let hf = &s.nodes[h as usize];
        assert_eq!(hf.kind, Kind::Map, "headers = {:?}", hf);
        let items = hf.items.expect("headers.Items");
        let itf = &s.nodes[items as usize];
        assert_eq!(itf.kind, Kind::OneOf, "headers.Items");
        assert_eq!(itf.variants.len(), 2);
        let v0 = &s.nodes[itf.variants[0] as usize];
        assert_eq!(v0.kind, Kind::Scalar);
        assert_eq!(v0.json_type, "string");
        let v1 = &s.nodes[itf.variants[1] as usize];
        assert_eq!(v1.kind, Kind::Array);
        let v1i = v1.items.expect("array variant items");
        assert_eq!(kind_of(&s, v1i), Kind::Scalar);

        let f = child(&s, s.root, "free");
        let ff = &s.nodes[f as usize];
        assert_eq!(ff.kind, Kind::Map, "free = {:?}", ff);
        let fi = ff.items.expect("free items");
        assert_eq!(
            kind_of(&s, fi),
            Kind::Raw,
            "free items should be raw values"
        );

        let c = child(&s, s.root, "closed");
        assert_ne!(
            kind_of(&s, c),
            Kind::Map,
            "closed should be a plain KObject (additionalProperties: false)"
        );

        let m = child(&s, s.root, "mixed");
        assert_ne!(
            kind_of(&s, m),
            Kind::Map,
            "mixed should be a plain KObject (properties take precedence)"
        );
    }

    // GitHub workflow on.push: several properties share a definition
    // through a chain of $refs. Every row keeps its own key-derived title
    // instead of the first referencing property's title ("branches" for
    // all four).
    #[test]
    fn test_shared_ref_property_titles() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "push": {"type": "object", "properties": {
      "branches": {"$ref": "#/definitions/branch"},
      "branches-ignore": {"$ref": "#/definitions/branch"},
      "tags": {"$ref": "#/definitions/branch"},
      "tags-ignore": {"$ref": "#/definitions/branch"},
      "paths": {"$ref": "#/definitions/path"},
      "paths-ignore": {"$ref": "#/definitions/path"}
    }}
  },
  "definitions": {
    "branch": {"$ref": "#/definitions/globs", "description": "branch filter"},
    "path": {"$ref": "#/definitions/globs", "description": "path filter"},
    "globs": {"type": "array", "items": {"type": "string"}, "minItems": 1}
  }
}"##,
        );
        let push = child(&s, s.root, "push");
        for key in [
            "branches",
            "branches-ignore",
            "tags",
            "tags-ignore",
            "paths",
            "paths-ignore",
        ] {
            let c = child(&s, push, key);
            let cf = &s.nodes[c as usize];
            assert!(
                cf.key == key && cf.title == key,
                "{key}: Key={:?} Title={:?}",
                cf.key,
                cf.title
            );
        }

        // An explicit title on the shared target is preserved for every reference.
        let titled = must_load(
            r##"{
  "type": "object",
  "properties": {
    "a": {"$ref": "#/$defs/globs"},
    "b": {"$ref": "#/$defs/globs"}
  },
  "$defs": {"globs": {"title": "Glob List", "type": "array", "items": {"type": "string"}}}
}"##,
        );
        for key in ["a", "b"] {
            let c = child(&titled, titled.root, key);
            let cf = &titled.nodes[c as usize];
            assert!(
                cf.key == key && cf.title == "Glob List",
                "{key}: Key={:?} Title={:?}",
                cf.key,
                cf.title
            );
        }
    }

    // Pins the single-pattern map gate: one patternProperties member plus
    // additionalProperties: false form-izes the object as a KMap whose keys
    // must match the pattern; every other shape keeps today's behavior
    // (KObject with no children for pattern-only objects,
    // additionalProperties winning when it is schema-valued).
    #[test]
    fn test_pattern_properties_map() {
        let jobs_src = r##"{
  "type": "object",
  "properties": {
    "jobs": {
      "type": "object",
      "patternProperties": {
        "^[_a-zA-Z][a-zA-Z0-9_-]*$": {
          "type": "object",
          "properties": {"runs-on": {"type": "string"}}
        }
      },
      "additionalProperties": false
    }
  }
}"##;
        let s = must_load(jobs_src);
        let j = child(&s, s.root, "jobs");
        let jf = &s.nodes[j as usize];
        assert_eq!(jf.kind, Kind::Map, "jobs Kind");
        assert_eq!(jf.key_pattern, "^[_a-zA-Z][a-zA-Z0-9_-]*$");
        let ji = jf.items.expect("jobs items");
        assert_eq!(
            kind_of(&s, ji),
            Kind::Object,
            "jobs items should be an object value schema"
        );
        let ro = child(&s, ji, "runs-on");
        assert_eq!(kind_of(&s, ro), Kind::Scalar);

        // Two patterns is out of scope: unknown rows as before.
        let two = must_load(
            r##"{
  "type": "object",
  "properties": {
    "jobs": {
      "type": "object",
      "patternProperties": {
        "^a": {"type": "string"},
        "^b": {"type": "string"}
      },
      "additionalProperties": false
    }
  }
}"##,
        );
        let j2 = child(&two, two.root, "jobs");
        let j2f = &two.nodes[j2 as usize];
        assert!(
            j2f.kind == Kind::Object && j2f.children.is_empty() && j2f.key_pattern.is_empty(),
            "two patterns: Kind={:?} children={} pattern={:?}",
            j2f.kind,
            j2f.children.len(),
            j2f.key_pattern
        );

        // A schema-valued additionalProperties still wins (existing map
        // support), so the pattern branch must not fire even when a pattern
        // exists.
        let scoped = must_load(
            r##"{
  "type": "object",
  "properties": {
    "headers": {
      "type": "object",
      "patternProperties": {"^X-": {"type": "number"}},
      "additionalProperties": {"type": "string"}
    }
  }
}"##,
        );
        let h = child(&scoped, scoped.root, "headers");
        let hf = &scoped.nodes[h as usize];
        let hi = hf.items.expect("headers items");
        assert!(
            hf.kind == Kind::Map
                && hf.key_pattern.is_empty()
                && kind_of(&scoped, hi) == Kind::Scalar
                && scoped.nodes[hi as usize].json_type == "string",
            "schema-valued additionalProperties: Kind={:?} pattern={:?}",
            hf.kind,
            hf.key_pattern
        );

        // A pattern the regexp engine cannot compile (ECMA-262 lookahead)
        // is unsupported.
        let bad = must_load(
            r##"{
  "type": "object",
  "properties": {
    "jobs": {
      "type": "object",
      "patternProperties": {"(?=x)": {"type": "string"}},
      "additionalProperties": false
    }
  }
}"##,
        );
        let jb = child(&bad, bad.root, "jobs");
        let jbf = &bad.nodes[jb as usize];
        assert!(
            jbf.kind == Kind::Object && jbf.children.is_empty(),
            "uncompilable pattern: Kind={:?} children={}",
            jbf.kind,
            jbf.children.len()
        );
    }

    // A oneOf/anyOf mixing an open string with a closed set of strings
    // (workflow shell) is a text field whose completions are the listed
    // values; closed sets, constrained strings and non-string branches
    // keep the variant picker.
    #[test]
    fn test_string_combo_collapses_to_text() {
        let s = must_load(
            r##"{
  "type": "object",
  "properties": {
    "shell": {"anyOf": [
      {"type": "string"},
      {"type": "string", "enum": ["bash", "pwsh", "python", "sh", "cmd", "powershell"]}
    ]}
  }
}"##,
        );
        let sh = child(&s, s.root, "shell");
        let shf = &s.nodes[sh as usize];
        assert_eq!(shf.kind, Kind::Scalar, "shell Kind");
        assert_eq!(shf.json_type, "string");
        assert_eq!(
            shf.suggestions,
            ["bash", "pwsh", "python", "sh", "cmd", "powershell"],
            "shell suggestions"
        );

        // A pattern-restricted branch means not every string is valid: keep
        // the picker.
        let pat = must_load(
            r##"{"type": "object", "properties": {"shell": {"anyOf": [
		{"type": "string", "pattern": "^/"},
		{"type": "string", "enum": ["a"]}]}}}"##,
        );
        let p = child(&pat, pat.root, "shell");
        assert_eq!(kind_of(&pat, p), Kind::OneOf, "pattern branch");

        // Two closed sets have no open branch: typing anything would be
        // invalid.
        let two = must_load(
            r##"{"type": "object", "properties": {"shell": {"oneOf": [
		{"type": "string", "enum": ["a"]},
		{"type": "string", "enum": ["b", "c"]}]}}}"##,
        );
        let c = child(&two, two.root, "shell");
        assert_eq!(kind_of(&two, c), Kind::OneOf, "no open branch");

        // A non-string branch is not free text either.
        let num = must_load(
            r##"{"type": "object", "properties": {"v": {"anyOf": [
		{"type": "string"}, {"type": "integer"}]}}}"##,
        );
        let v = child(&num, num.root, "v");
        assert_eq!(kind_of(&num, v), Kind::OneOf, "mixed types");
    }
}
