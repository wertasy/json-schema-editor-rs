use super::*;

// --- summaries -------------------------------------------------------------

/// summary renders the right-hand value column for fn at path: the current
/// doc value, or the effective default when unset, "—" when neither exists.
pub fn summary(ctx: &MenuCtx<'_>, fn_: FormNodeId, path: &[PathSeg]) -> String {
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
            let enc = vdoc.encode_value(v);
            for o in &f.enum_ {
                if o.encoded == enc {
                    return o.display.clone();
                }
            }
            let s = vdoc.scalar_text(v).unwrap_or_default().to_string();
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
                    let enc = vdoc.encode_value(v);
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
                    let s = vdoc.scalar_text(v).unwrap_or_default().to_string();
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
        Kind::Raw => format!("json {}", clip_runes(&vdoc.encode_value(v), 20)),
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
        return clip_runes(&vdoc.encode_value(n), 24);
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
