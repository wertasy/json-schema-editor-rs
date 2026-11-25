//! FormNode tree construction: `Parser::build` and its helpers turn a
//! resolved schema node into form nodes (objects, arrays, combos, variants).

use super::*;

impl Parser {
    /// build_defs normalizes the top-level `$defs`/`definitions` entries so
    /// the arbitrary-type picker can offer the schema's own named types
    /// alongside the JSON built-ins. Unreachable/raw shapes are skipped.
    pub(super) fn build_defs(&mut self, doc_root: NodeId, ctx: &RefCtx) -> Vec<FormNodeId> {
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
    /// Normalizes one schema node into a FormNode.
    pub(super) fn schema(
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

    pub(super) fn push_raw(&mut self, key: &str, desc: &str) -> FormNodeId {
        self.push_form(FormNode {
            key: key.into(),
            title: title_fallback(key),
            kind: Kind::Raw,
            desc: desc.into(),
            ..FormNode::default()
        })
    }

    pub(super) fn push_form(&mut self, f: FormNode) -> FormNodeId {
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
    pub(super) fn fork(&mut self, fid: FormNodeId, key: &str, required: bool) -> FormNodeId {
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
    pub(super) fn rekey_variants(&mut self, vs: Vec<FormNodeId>, key: &str) -> Vec<FormNodeId> {
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

    pub(super) fn variants_need_key(&self, vs: &[FormNodeId], key: &str) -> bool {
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

    pub(super) fn build(
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
            let open_additional = usable_additional(&self.doc, n);
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
            if f.kind == Kind::Object {
                f.open_additional = open_additional;
                if let Some(pp) = field(&self.doc, n, "patternProperties") {
                    if let Value::Object(pms) = self.doc.value(pp) {
                        for pm in pms {
                            if regex::Regex::new(&pm.key).is_ok() {
                                f.key_patterns.push(pm.key.clone());
                            }
                        }
                    }
                }
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
    pub(super) fn string_combo(
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
    pub(super) fn build_variants(
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

    pub(super) fn build_children(
        &mut self,
        n: NodeId,
        ctx: &RefCtx,
        depth: usize,
    ) -> Vec<FormNodeId> {
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
}
