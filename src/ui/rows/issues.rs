use super::*;

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
        let p = append_key(path, key);
        out.push(Issue {
            msg: format!(
                "at '{}': unknown key {} (not in the schema)",
                path_string(&p),
                quote_key(key)
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
                            &append_key(path, &schema.nodes[c as usize].key),
                            m,
                        );
                    }
                }
                let patterns = schema.nodes[fn_ as usize].key_patterns.clone();
                if !schema.nodes[fn_ as usize].open_additional {
                    unknown_of(doc, out, val, &known, &patterns, path);
                }
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
                let mut open_additional = false;
                for &v in &variants {
                    open_additional |= schema.nodes[v as usize].open_additional;
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
                            walk(schema, doc, out, c, &append_key(path, &key), m);
                        }
                    }
                }
                if !open_additional {
                    unknown_of(doc, out, val, &known, &patterns, path);
                }
            }
            Kind::Map => {
                // every document key is defined by the map's additionalProperties
                if let Some(items) = schema.nodes[fn_ as usize].items {
                    if let Value::Object(members) = doc.value(val) {
                        for mem in members.clone() {
                            walk(
                                schema,
                                doc,
                                out,
                                items,
                                &append_key(path, &mem.key),
                                mem.val,
                            );
                        }
                    }
                }
            }
            Kind::Array => {
                if let Some(items) = schema.nodes[fn_ as usize].items {
                    if let Value::Array(arr) = doc.value(val) {
                        for (i, &item) in arr.iter().enumerate() {
                            walk(schema, doc, out, items, &append_idx(path, i), item);
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

pub(crate) fn row_scalar_value(schema: &Schema, doc: &Doc, r: &Row) -> bool {
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

pub(crate) fn row_search_text(schema: &Schema, doc: &Doc, r: &Row) -> Option<String> {
    let Some(f) = r.fn_ else {
        if r.kind != RowKind::Unknown {
            return None;
        }
        return Some(unknown_match_text(r, row_scalar_value(schema, doc, r)));
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
    if row_scalar_value(schema, doc, r) {
        let s = summary_raw(schema, doc, f, &r.path);
        if s != "—" {
            text += &format!(" {s}");
        }
    }
    Some(text)
}
