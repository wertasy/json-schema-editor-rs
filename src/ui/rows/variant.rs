use super::*;

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
pub(crate) fn variant_signature_seen(
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
            let enc = doc.encode_value(val);
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
            let enc = doc.encode_value(v);
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

pub(crate) fn push_node(schema: &mut Schema, n: FormNode) -> FormNodeId {
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

pub(crate) fn merged_one_of_menu_id(
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

pub(crate) fn pinned_one_of_menu_id(
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
