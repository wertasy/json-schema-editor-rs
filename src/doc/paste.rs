//! Clipboard-text classification for paste (Go menu_paste.go parsePaste and
//! its classify helpers). The pasted text is decoded with the document's own
//! codec family and split into object members, array items or a single
//! replacement value; the parsed document comes back so the members' NodeIds
//! stay resolvable while the paste is applied.

use crate::doc::jsoncdoc;
use crate::doc::tomldoc;
use crate::doc::yamldoc;
use crate::doc::{Doc, Member, NodeId, Value};

/// A paste member: one key-value pair.
pub(crate) struct PasteMember {
    pub(crate) key: String,
    pub(crate) val: NodeId, // id into the parsed paste document
}

/// stripTrailingComma drops one comma sitting right before the closing
/// bracket; reports whether the text changed.
fn strip_trailing_comma(s: &str) -> Option<String> {
    if let Some(t) = s.strip_suffix(",}") {
        return Some(format!("{t}}}"));
    }
    if let Some(t) = s.strip_suffix(",]") {
        return Some(format!("{t}]"));
    }
    None
}

/// classifyParsed splits a fully parsed document into members or items; a
/// scalar root is a single value (members and items both None). The parsed
/// document is returned so the members/items' NodeIds stay resolvable.
fn classify_parsed(doc: Doc) -> (Option<Vec<PasteMember>>, Option<Vec<NodeId>>, Option<Doc>) {
    let root = doc.root;
    let members = if let Value::Object(mems) = doc.value(root) {
        Some(
            mems.iter()
                .map(|m| PasteMember {
                    key: m.key.clone(),
                    val: m.val,
                })
                .collect(),
        )
    } else {
        None
    };
    let items = if let Value::Array(items) = doc.value(root) {
        Some(items.clone())
    } else {
        None
    };
    (members, items, Some(doc))
}

/// classifyPasteYAML decodes clipboard text as YAML for YAML documents;
/// scalar roots come back as a single value.
fn classify_paste_yaml(
    s: &str,
) -> Result<(Option<Vec<PasteMember>>, Option<Vec<NodeId>>, Option<Doc>), String> {
    let (doc, _meta) =
        yamldoc::parse(s).map_err(|_| "clipboard is not JSON keys or items".to_string())?;
    Ok(classify_parsed(doc))
}

/// classifyPasteTOML decodes clipboard text as TOML for TOML documents.
/// A full document supplies members/items; otherwise a single value (scalar,
/// array, inline table) replaces the target value.
fn classify_paste_toml(
    s: &str,
) -> Result<(Option<Vec<PasteMember>>, Option<Vec<NodeId>>, Option<Doc>), String> {
    if let Ok((doc, _meta)) = tomldoc::parse(s) {
        return Ok(classify_parsed(doc));
    }
    if let Ok((doc, _meta)) = tomldoc::parse_value(s) {
        return Ok(classify_parsed(doc));
    }
    // Copying a string value writes its text without quotes (the same rule
    // every format uses), so a single unadorned line is that string.
    if !s.contains('\n') && !s.contains('=') && !s.starts_with(['[', '{', '"', '\'']) {
        let mut doc = Doc::new();
        let n = doc.new_str(s);
        doc.root = n;
        return Ok((None, None, Some(doc)));
    }
    Err("clipboard is not JSON keys or items".to_string())
}

/// parsePaste classifies the clipboard (Go menu_paste.go parsePaste).
pub(crate) fn parse_paste(
    text: &str,
    mode: &str,
) -> Result<(Option<Vec<PasteMember>>, Option<Vec<NodeId>>, Option<Doc>), String> {
    let mut s = text.trim().to_string();
    if s.is_empty() {
        return Err("clipboard is empty".to_string());
    }
    if s.starts_with('{') || s.starts_with('[') {
        match jsoncdoc::parse_strict(&s) {
            Ok(doc) => return Ok(classify_parsed(doc)),
            Err(_) => {
                // one trailing comma before the closing bracket is tolerated
                if let Some(t) = strip_trailing_comma(&s) {
                    if let Ok(doc) = jsoncdoc::parse_strict(&t) {
                        return Ok(classify_parsed(doc));
                    }
                }
                match mode {
                    "YAML" => return classify_paste_yaml(&s),
                    "TOML" => return classify_paste_toml(&s),
                    _ => {}
                }
                return Err("clipboard is not JSON keys or items".to_string());
            }
        }
    }
    // a complete JSON document that is neither {…} nor […] is one value
    if let Ok(doc) = jsoncdoc::parse_strict(&s) {
        return Ok(classify_parsed(doc));
    }
    if s.ends_with(',') {
        s = s.trim().trim_end_matches(',').trim().to_string();
    }
    if s.is_empty() {
        return Err("clipboard is empty".to_string());
    }
    if let Ok(doc) = jsoncdoc::parse_strict(&format!("{{{s}}}")) {
        if let Value::Object(mems) = doc.value(doc.root) {
            let members = mems
                .iter()
                .map(|m| PasteMember {
                    key: m.key.clone(),
                    val: m.val,
                })
                .collect();
            return Ok((Some(members), None, Some(doc)));
        }
    }
    if let Ok(doc) = jsoncdoc::parse_strict(&format!("[{s}]")) {
        if let Value::Array(items) = doc.value(doc.root) {
            return Ok((None, Some(items.clone()), Some(doc)));
        }
    }
    match mode {
        "YAML" => classify_paste_yaml(&s),
        "TOML" => classify_paste_toml(&s),
        _ => Err("clipboard is not JSON keys or items".to_string()),
    }
}

/// plain_paste_doc turns clipboard text that parsed as no structured value
/// into a single string value. `y`/`Y` copy strings without quotes, so `P`
/// on a string row must accept that text back in every format (JSON had no
/// such fallback). One surrounding pair of double quotes is stripped so a
/// hand-quoted string whose JSON spelling is invalid still lands as its text.
pub(crate) fn plain_paste_doc(text: &str) -> Option<Doc> {
    let t = text.trim_matches(['\r', '\n']);
    if t.trim().is_empty() {
        return None;
    }
    let s = if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        &t[1..t.len() - 1]
    } else {
        t
    };
    let mut doc = Doc::new();
    let n = doc.new_str(s);
    doc.root = n;
    Some(doc)
}

/// pasteEmptyNode mirrors jsondoc's own emptiness test: null or no value at
/// all converts to a container.
pub(crate) fn paste_empty_node(doc: &Doc, n: NodeId) -> bool {
    matches!(doc.value(n), Value::Null) || matches!(doc.value(n), Value::Object(m) if m.is_empty())
}

/// pasteHasKey reports whether the member list contains key.
pub(crate) fn paste_has_key(members: &[Member], key: &str) -> bool {
    members.iter().any(|m| m.key == key)
}
