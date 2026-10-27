use super::*;

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
        return Some(append_key(&loc, &names[0]));
    }
    if let Some(name) = issue_prop_name(&detail, "additional properties '") {
        return Some(append_key(&loc, &name));
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
        return names.iter().map(|name| append_key(&loc, name)).collect();
    }
    if let Some(i) = detail.find("additional propert") {
        let names = quoted_names(&detail[i..]);
        if !names.is_empty() {
            return names.iter().map(|name| append_key(&loc, name)).collect();
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
            let enc = doc.encode_value(val);
            for o in &schema.nodes[v as usize].enum_ {
                if o.encoded == enc {
                    return o.display.clone();
                }
            }
            String::new()
        }
        _ => {
            let s = doc.scalar_text(val).unwrap_or_default().to_string();
            if !s.is_empty() {
                return value_clip(&s);
            }
            String::new()
        }
    }
}

/// goQuote mirrors Go's %q for the property names embedded in issue
/// messages (plain ASCII keys in practice).
pub(crate) fn quote_key(s: &str) -> String {
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
