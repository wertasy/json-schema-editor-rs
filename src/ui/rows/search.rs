use super::*;

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
            // A null object/oneOf still has the schema-defined fields in
            // its menu (a just-created placeholder), so keep indexing them.
            // Other null containers have no actual children to index.
            if is_null(doc, n)
                && !matches!(schema.nodes[fn_ as usize].kind, Kind::Object | Kind::OneOf)
            {
                return;
            }
        }
        match schema.nodes[fn_ as usize].kind {
            Kind::Object => {
                let mut known: HashSet<String> = HashSet::new();
                let children = schema.nodes[fn_ as usize].children.clone();
                for c in children {
                    known.insert(schema.nodes[c as usize].key.clone());
                    let cpath = append_key(path, &schema.nodes[c as usize].key);
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
                        let cpath = append_key(path, &key);
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
                            let cpath = append_key(path, &mem.key);
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
                let n = doc.array_len(path);
                for i in 0..n {
                    let ipath = append_idx(path, i);
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
pub(crate) fn unknown_rows(
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
        let p = append_key(path, &mem.key);
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
pub(crate) fn pattern_defined(schema: &Schema, fn_: FormNodeId, key: &str) -> bool {
    key_matches_patterns(key, &schema.nodes[fn_ as usize].key_patterns)
}

pub(crate) fn key_matches_patterns(key: &str, patterns: &[String]) -> bool {
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
pub(crate) fn unknown_value_rows(
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
                let p = append_key(path, &mem.key);
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
                let p = append_idx(path, i);
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
