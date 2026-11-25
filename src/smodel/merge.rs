//! allOf merge/intersection: synthesizes merged schema nodes and tightens
//! constraints across `allOf` subschemas.

use super::*;

impl Parser {
    /// Shallow-merges the node's own keywords with every allOf sub schema
    /// (sub $refs resolved first, nested allOf expanded) into one synthetic
    /// schema node.
    pub(super) fn merge_all_of(
        &mut self,
        n: NodeId,
        subs: &[NodeId],
        ctx: &RefCtx,
        depth: usize,
    ) -> NodeId {
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
    pub(super) fn merge_parts(&mut self, parts: &[NodeId], depth: usize) -> NodeId {
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
                        if let Value::Object(pms) = self.doc.value(m.val).clone() {
                            for pm in pms {
                                if let Some(i) = base.iter().position(|b| b.key == pm.key) {
                                    // allOf intersects: a later part refines
                                    // the property, it does not replace it
                                    // (Cargo's DetailedDependency marks
                                    // version/path/git/workspace with `true`,
                                    // which must keep the real schema).
                                    base[i].val =
                                        self.intersect_schema(base[i].val, pm.val, depth + 1);
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
                    "additionalProperties" | "items" | "contains" | "propertyNames" => {
                        let old = member_index(&members, &m.key).map(|i| members[i].val);
                        match old {
                            None => member_set(&mut members, &m.key, m.val),
                            Some(old) => {
                                let merged = self.intersect_schema(old, m.val, depth + 1);
                                member_set(&mut members, &m.key, merged);
                            }
                        }
                    }
                    "patternProperties" => {
                        let mut base: Vec<Member> =
                            match member_index(&members, "patternProperties") {
                                Some(i) => match self.doc.value(members[i].val) {
                                    Value::Object(b) => b.clone(),
                                    _ => Vec::new(),
                                },
                                None => Vec::new(),
                            };
                        if let Value::Object(pms) = self.doc.value(m.val).clone() {
                            for pm in pms {
                                if let Some(i) = base.iter().position(|b| b.key == pm.key) {
                                    base[i].val =
                                        self.intersect_schema(base[i].val, pm.val, depth + 1);
                                } else {
                                    base.push(pm.clone());
                                }
                            }
                        }
                        let nid = self.doc.push(NodeData {
                            value: Value::Object(base),
                            ..NodeData::default()
                        });
                        member_set(&mut members, "patternProperties", nid);
                    }
                    "minimum" | "minLength" | "exclusiveMinimum" | "maximum" | "maxLength"
                    | "exclusiveMaximum" | "minItems" | "maxItems" | "minProperties"
                    | "maxProperties" => {
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
    pub(super) fn stricter_num(
        &self,
        old: Option<NodeId>,
        cur: NodeId,
        key: &str,
    ) -> Option<NodeId> {
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
            "minimum" | "minLength" | "exclusiveMinimum" | "minItems" | "minProperties" => {
                if cv > ov { Some(cur) } else { Some(old) }
            }
            "maximum" | "maxLength" | "exclusiveMaximum" | "maxItems" | "maxProperties" => {
                if cv < ov { Some(cur) } else { Some(old) }
            }
            _ => Some(old),
        }
    }

    pub(super) fn intersect_enum(&mut self, a: NodeId, b: NodeId) -> NodeId {
        let mut out: Vec<NodeId> = Vec::new();
        if let (Value::Array(as_), Value::Array(bs)) = (self.doc.value(a), self.doc.value(b)) {
            let as_ = as_.clone();
            let bs = bs.clone();
            for &av in &as_ {
                for &bv in &bs {
                    if self.doc.encode_value(av) == self.doc.encode_value(bv) {
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

    pub(super) fn node_num(&self, n: NodeId) -> (f64, bool) {
        match doc_node_num(&self.doc, n) {
            Some(f) => (f, true),
            None => (0.0, false),
        }
    }

    /// Reports whether n shapes an object value the way build's object
    /// branch does: declared properties or an object type entry.
    /// Reports whether a schema node constrains nothing: boolean `true` or
    /// an empty object.
    pub(super) fn unconstrained(&self, n: NodeId) -> bool {
        match self.doc.value(n) {
            Value::Bool(b) => *b,
            Value::Object(ms) => ms.is_empty(),
            _ => false,
        }
    }

    /// Intersects two property/value schemas for an allOf merge: `true`/`{}`
    /// are neutral, `false` absorbs, and two object schemas merge recursively
    /// so later parts add constraints instead of dropping earlier ones.
    /// Shapes the form layer does not model (tuple `items` arrays, invalid
    /// nodes) keep the first part, matching the old shallow behavior.
    pub(super) fn intersect_schema(&mut self, old: NodeId, new: NodeId, depth: usize) -> NodeId {
        if self.unconstrained(new) {
            return old;
        }
        if self.unconstrained(old) {
            return new;
        }
        let old_false = matches!(self.doc.value(old), Value::Bool(false));
        let new_false = matches!(self.doc.value(new), Value::Bool(false));
        if old_false || new_false {
            return if old_false { old } else { new };
        }
        let both_objects = matches!(self.doc.value(old), Value::Object(_))
            && matches!(self.doc.value(new), Value::Object(_));
        if both_objects && depth <= MAX_DEPTH {
            // Keep the pair as allOf so build() (which has the $ref context)
            // resolves refs on both sides before merging. Merging here would
            // let the first $ref win and silently drop the second schema.
            let arr = self.doc.push(NodeData {
                value: Value::Array(vec![old, new]),
                ..NodeData::default()
            });
            return self.doc.push(NodeData {
                value: Value::Object(vec![Member {
                    key: "allOf".into(),
                    val: arr,
                }]),
                ..NodeData::default()
            });
        }
        old
    }

    pub(super) fn objectish(&self, n: NodeId) -> bool {
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
