//! Key handling for the menu TUI, plus its rendering (the Go menu_keys.go /
//! menu_copy.go / menu_find.go / menu_merge.go / menu_alias.go /
//! menu_paste.go / menu_select.go port, and menu.go's View layer).
//!
//! Every edit writes through to the document immediately; overlays are
//! handled by the [`App`] methods `dispatch_key` routes to. Each overlay
//! handler returns the overlay to keep (None closes it), matching menu.rs's
//! `self.overlay = match ov { … }` dispatch.

use std::collections::HashSet;
use std::sync::Arc;

use regex::Regex;

use crate::doc::jsondoc;
use crate::doc::{Doc, Member, NodeData, NodeId, PathSeg, Value};
use crate::icons::{ERR, WARN};
use crate::smodel::{EnumOpt, FormNode, FormNodeId, Kind, Schema};
use crate::ui::files::wrap_cursor;
use crate::ui::menu::{
    AddState, AnchorMode, AnchorState, App, CommentEditState, CommentState, ConfirmState,
    DESC_BLOCK_PREVIEW_LINES, DescLine, DescSource, EditState, ErrsState, ExternalEdit,
    FastMoveState, FilePickState, IssuesState, MergeState, Msg, Overlay, RefPickState, SearchState,
    SelState, TextState, WriteState, anchor_defs, anchor_holder, anchor_holder_path,
    anchor_merge_use_paths, anchor_role, copy_detached_from_doc, copy_from_doc, count_anchor_uses,
    cursor_ancestor, cursor_by_path, cursor_for_row, cursor_prefix, is_path_prefix, merge_ref_uses,
    nodes_equal, nodes_equal_doc, page_delta, read_clipboard, ref_would_cycle, remap_state_cursor,
    vertical_delta, write_clipboard,
};
use crate::ui::rows::{
    SynthKey, any_type_menu, clip_runes, current_variant, display_title, doc_array_len,
    dynamic_variants, filter_search, is_dot_query, is_path_query, match_variant_list,
    matched_leaf_variant, path_pattern, raw_value_summary, rebuild_stack, replace_all,
    replace_hits, search_index, seed_variant_cursor, summary, variant_field_defs,
    variant_signature, variant_value_summary,
};
use crate::ui::widgets::{Key, RawExt, TextArea, TextInput, apply_scalar, scalar_text};
use crate::ui::{Issue, MenuKind, MenuState, Row, RowKind};

// ---------------------------------------------------------------------------
// free helpers

/// isDigitKey reports whether key is a single decimal digit.
fn is_digit_key(key: &str) -> bool {
    let b = key.as_bytes();
    b.len() == 1 && b[0] >= b'0' && b[0] <= b'9'
}

/// isCountBuf reports whether the pending buffer is digits only — a count
/// for N gg. A buffer holding the armed g (or ga/gr remnants) is not.
fn is_count_buf(buf: &[String]) -> bool {
    !buf.is_empty() && buf.iter().all(|k| is_digit_key(k))
}

/// bufCount folds the digit prefix of a pending buffer into its number.
fn buf_count(buf: &[String]) -> (usize, bool) {
    let mut n = 0usize;
    let mut digits = 0;
    for k in buf {
        if !is_digit_key(k) {
            break;
        }
        n = n * 10 + (k.as_bytes()[0] - b'0') as usize;
        digits += 1;
    }
    (n, digits > 0)
}

/// page_cursor moves a list cursor by whole screenfuls, clamped to the first
/// and last entry. Unlike j/k it does not wrap: a page step that runs past an
/// end stops there.
fn page_cursor(cur: usize, delta: i64, page: usize, n: usize) -> usize {
    let last = n.saturating_sub(1);
    (cur as i64 + delta * page as i64).clamp(0, last as i64) as usize
}

/// selKey renders a path as the selection map key; the s:/i: tags keep
/// string keys and array indexes distinct.
fn sel_key(path: &[PathSeg]) -> String {
    let mut b = String::new();
    for seg in path {
        match seg {
            PathSeg::Key(s) => {
                b.push_str("s:");
                b.push_str(s);
            }
            PathSeg::Idx(i) => {
                b.push_str("i:");
                b.push_str(&i.to_string());
            }
        }
        b.push('\0');
    }
    b
}

/// selectableRow reports whether a row can carry a selection mark: object
/// menu key-value rows (fields, container rows, schema-less keys) and array
/// item rows (their label is "key[N]", as the item has no object key) — no
/// action or status rows.
fn selectable_row(r: &Row) -> bool {
    matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) && !r.path.is_empty()
}

/// findRowKey returns the key a row matches by: the schema key, or the
/// document key (last path segment) for schema-less rows. Only key-value
/// rows match — array items and action rows are skipped.
fn find_row_key(schema: &Schema, r: &Row) -> Option<String> {
    if !selectable_row(r) {
        return None;
    }
    if let Some(f) = r.fn_ {
        let key = &schema.nodes[f as usize].key;
        if !key.is_empty() {
            return Some(key.clone());
        }
    }
    match r.path.last() {
        Some(PathSeg::Key(k)) if !k.is_empty() => Some(k.clone()),
        _ => None,
    }
}

/// completionCandidate reports whether v can be offered as a completion:
/// empty text never is, and multiline values are skipped.
fn completion_candidate(v: &str) -> bool {
    !v.is_empty() && !v.contains('\r') && !v.contains('\n')
}

/// filterCompletions keeps candidates starting with prefix
/// (case-insensitive); an empty prefix keeps everything.
fn filter_completions(cands: &[String], prefix: &str) -> Vec<String> {
    let lp = prefix.to_lowercase();
    cands
        .iter()
        .filter(|c| completion_candidate(c) && (lp.is_empty() || c.to_lowercase().starts_with(&lp)))
        .cloned()
        .collect()
}

/// encodeValue mirrors Go smodel.EncodeValue over an arena document.
fn encode_value(doc: &Doc, v: NodeId) -> String {
    serde_json::to_string(&doc.to_value(v)).unwrap_or_default()
}

/// enumIndex returns the cursor position of the current (or default) value
/// in fn's option list.
fn enum_index(schema: &Schema, fn_: FormNodeId, vdoc: &Doc, v: Option<NodeId>) -> usize {
    let f = &schema.nodes[fn_ as usize];
    let mut enc = String::new();
    if let Some(v) = v {
        if !matches!(vdoc.value(v), Value::Null) {
            enc = encode_value(vdoc, v);
        }
    }
    if !enc.is_empty() {
        for (i, o) in f.enum_.iter().enumerate() {
            if o.encoded == enc {
                return i;
            }
        }
    }
    if let Some(d) = f.default_ {
        let de = encode_value(&schema.doc, d);
        for (i, o) in f.enum_.iter().enumerate() {
            if o.encoded == de {
                return i;
            }
        }
    }
    0
}

/// selMatches iterates the enum picker's options allowed by its search
/// query: a case-insensitive substring over the display label. An empty
/// query keeps every option.
fn sel_matches(s: &SelState) -> impl Iterator<Item = &EnumOpt> {
    let q = s.search_ti.value().to_lowercase();
    s.opts
        .iter()
        .filter(move |o| q.is_empty() || o.display.to_lowercase().contains(&q))
}

/// listSearchMatch reports whether text contains the query
/// (case-insensitive); an empty query matches everything.
fn list_search_match(q: &str, text: &str) -> bool {
    q.is_empty() || text.to_lowercase().contains(&q.to_lowercase())
}

/// issueIndices lists the issues matching the overlay's search query.
fn issue_indices(s: &IssuesState) -> impl Iterator<Item = usize> + '_ {
    let q = s.search_ti.value();
    s.items
        .iter()
        .enumerate()
        .filter(move |(_, it)| {
            list_search_match(q, &it.msg)
                || it.path.as_ref().map_or(false, |p| {
                    list_search_match(q, &crate::doc::jsondoc::path_string(p))
                })
        })
        .map(|(i, _)| i)
}

/// errIndices lists the save-error lines matching the search query.
fn err_indices(s: &ErrsState) -> impl Iterator<Item = usize> + '_ {
    let q = s.search_ti.value();
    s.lines
        .iter()
        .enumerate()
        .filter(move |(_, line)| list_search_match(q, line))
        .map(|(i, _)| i)
}

/// selMatchCount is the number of options the picker currently lists.
fn sel_match_count(s: &SelState) -> usize {
    sel_matches(s).count()
}

/// selOptionAt resolves the option under the picker cursor in the filtered
/// list.
fn sel_option_at(s: &SelState, cursor: usize) -> Option<&EnumOpt> {
    sel_matches(s).nth(cursor)
}

/// variantContains reports whether target is one of the oneOf node's
/// variants (nested groups included).
fn variant_contains(schema: &Schema, node: FormNodeId, target: FormNodeId) -> bool {
    if schema.nodes[node as usize].kind != Kind::OneOf {
        return false;
    }
    schema.nodes[node as usize].variants.iter().any(|&v| {
        v == target
            || (schema.nodes[v as usize].kind == Kind::OneOf && variant_contains(schema, v, target))
    })
}

/// pathFor appends a key segment.
fn path_for(path: &[PathSeg], key: &str) -> Vec<PathSeg> {
    let mut p = path.to_vec();
    p.push(PathSeg::key(key));
    p
}

/// pathIdx appends an index segment.
fn path_idx(path: &[PathSeg], idx: usize) -> Vec<PathSeg> {
    let mut p = path.to_vec();
    p.push(PathSeg::Idx(idx));
    p
}

/// docKeyAt renders the document position of a menu path as a key label.
fn doc_key_at(path: &[PathSeg]) -> String {
    match path.last() {
        Some(PathSeg::Key(s)) => s.clone(),
        Some(PathSeg::Idx(i)) => match path.get(path.len().saturating_sub(2)) {
            Some(PathSeg::Key(k)) => format!("{}[{}]", k, i),
            _ => format!("[{}]", i),
        },
        None => String::new(),
    }
}

/// tagOfNode returns an object's "tag" string value.
fn tag_of_node(doc: &Doc, n: NodeId) -> String {
    if let Value::Object(mems) = doc.value(n) {
        for m in mems {
            if m.key == "tag" {
                if let Value::Str(s) = doc.value(m.val) {
                    return s.clone();
                }
            }
        }
    }
    String::new()
}

/// refWordsAbove reports whether every word appears as a whole member key in
/// the NUL-wrapped ancestor path.
fn ref_words_above(words: &[&str], ancestors: &str) -> bool {
    words
        .iter()
        .all(|w| ancestors.contains(&format!("\0{w}\0")))
}

/// copyRowKey returns the row's object key: the last path segment when it is
/// a string. Array indexes and the root path have no object key.
fn copy_row_key(path: &[PathSeg]) -> Option<String> {
    match path.last() {
        Some(PathSeg::Key(k)) => Some(k.clone()),
        _ => None,
    }
}

/// isMergeSeparator reports whether r separates merge-key tokens: spaces and
/// commas (both the "a b" and "[*a, *b]" reading work).
fn is_merge_separator(r: char) -> bool {
    r == ' ' || r == ',' || r == '\t'
}

/// splitLastToken splits the input into everything up to the final token and
/// that token, keeping the separator before it in rest.
fn split_last_token(v: &str) -> (String, String) {
    if let Some(i) = v.rfind(is_merge_separator) {
        return (v[..=i].to_string(), v[i + 1..].to_string());
    }
    (String::new(), v.to_string())
}

/// mergeTokenList splits an input fragment into bare anchor names: tokens
/// separated by spaces or commas, an optional "*" prefix stripped, empty
/// tokens dropped.
fn merge_token_list(v: &str) -> Vec<String> {
    v.split(is_merge_separator)
        .map(|t| t.trim().trim_start_matches('*').to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// mergeFiltered lists the candidates for the token being typed.
fn merge_filtered(s: &MergeState) -> Vec<String> {
    let (rest, last) = split_last_token(s.ti.value());
    let used: HashSet<String> = merge_token_list(&rest).into_iter().collect();
    let last = last.trim_start_matches('*').to_lowercase();
    s.cands
        .iter()
        .filter(|c| !used.contains(*c) && (last.is_empty() || c.to_lowercase().starts_with(&last)))
        .cloned()
        .collect()
}

fn is_null_val(doc: &Doc, v: NodeId) -> bool {
    matches!(doc.value(v), Value::Null)
}

/// bump_number_literal adds delta to a numeric literal. Integer spellings
/// stay exact (i128, no f64 round trip); fractional/exponent forms fall back
/// to the shortest JSON float form.
fn bump_number_literal(lit: &str, delta: i64) -> Option<String> {
    if !lit.contains(['.', 'e', 'E']) {
        // Integer spellings stay exact; an integer outside i128 is left
        // untouched rather than rounded through f64.
        let v = lit.parse::<i128>().ok()?;
        return v.checked_add(delta as i128).map(|n| n.to_string());
    }
    let f = lit.parse::<f64>().ok()?;
    let sum = f + delta as f64;
    if !sum.is_finite() {
        return None;
    }
    Some(jsondoc::go_json_float(sum))
}

fn is_null_opt(doc: &Doc, v: Option<NodeId>) -> bool {
    v.map_or(true, |v| is_null_val(doc, v))
}

/// variantLabel renders a variant for the picker's bottom information line:
/// its type name (when it has one) plus the structural signature.
fn variant_label(schema: &Schema, v: FormNodeId) -> String {
    let f = &schema.nodes[v as usize];
    let sig = variant_signature(schema, Some(v));
    if f.title.is_empty() {
        sig
    } else if f.title.eq_ignore_ascii_case(&sig) {
        f.title.clone()
    } else {
        format!("{} {}", f.title, sig)
    }
}

/// completionCandidate text extraction: the value form the field accepts —
/// strings for string fields, the numeric literal for numeric fields.
fn value_text(doc: &Doc, v: Option<NodeId>, want_num: bool) -> Option<String> {
    let v = v?;
    match doc.value(v) {
        Value::Str(s) if !want_num => Some(s.clone()),
        Value::Num(n) if want_num => Some(n.clone()),
        _ => None,
    }
}

// --- terminal-cell text helpers (Go ansi.StringWidthWc / ansi.TruncateWc) ---

/// displayWidth returns the terminal cell width of s (ANSI escapes are
/// skipped; wide runes count two cells).
pub(crate) fn display_width(s: &str) -> usize {
    let mut w = 0usize;
    let mut esc = false;
    for c in s.chars() {
        if esc {
            if c == 'm' {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            esc = true;
            continue;
        }
        w += char_width(c);
    }
    w
}

pub(crate) fn char_width(c: char) -> usize {
    let cp = c as u32;
    // wide: CJK, Hangul, full-width forms, emoji ranges (wcwidth semantics,
    // approximated)
    if (0x1100..=0x115F).contains(&cp)
        || (0x2E80..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7A3).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE10..=0xFE19).contains(&cp)
        || (0xFE30..=0xFE6F).contains(&cp)
        || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x1F300..=0x1F64F).contains(&cp)
        || (0x20000..=0x3FFFD).contains(&cp)
    {
        2
    } else {
        1
    }
}

/// clipDisplay truncates s to max terminal cells, ANSI-aware. A reset closes
/// any style left open by the cut.
pub(crate) fn clip_display(s: &str, max: usize) -> String {
    if max < 1 {
        return String::new();
    }
    if display_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    let mut styled = false;
    let mut esc = false;
    'outer: for c in s.chars() {
        if esc {
            out.push(c);
            if c == 'm' {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            out.push(c);
            esc = true;
            styled = true;
            continue;
        }
        let cw = char_width(c);
        if w + cw > max {
            break 'outer;
        }
        out.push(c);
        w += cw;
    }
    if styled {
        out.push_str("\x1b[0m");
    }
    out
}

/// clipLeft keeps the tail of s within max runes, marking the cut with "…".
fn clip_left(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let r: Vec<char> = s.chars().collect();
    if r.len() <= max {
        return s.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }
    "…".to_string() + &r[r.len() - (max - 1)..].iter().collect::<String>()
}

/// wrapText wraps s to max display cells per line, preserving explicit
/// newlines. It breaks at the last space when that leaves a non-empty prefix;
/// otherwise it breaks between runes.
fn wrap_text(s: &str, max: usize) -> Vec<String> {
    if max < 1 {
        return vec![s.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        for r in para.chars() {
            let mut cand = line.clone();
            cand.push(r);
            if display_width(&cand) <= max {
                line = cand;
                continue;
            }
            if line.is_empty() {
                line = r.to_string();
                continue;
            }
            if let Some(i) = line.rfind(' ') {
                if i > 0 {
                    out.push(line[..i].to_string());
                    line = line[i + 1..].to_string();
                    line.push(r);
                    continue;
                }
            }
            out.push(line);
            line = r.to_string();
        }
        out.push(line);
    }
    out
}

/// fold_desc_block caps one collapsed source block at
/// [`DESC_BLOCK_PREVIEW_LINES`] lines, leaving room for the trailing `…`.
fn fold_desc_block(mut lines: Vec<String>, width: usize) -> Vec<String> {
    if lines.len() <= DESC_BLOCK_PREVIEW_LINES {
        return lines;
    }
    lines.truncate(DESC_BLOCK_PREVIEW_LINES);
    if let Some(last) = lines.last_mut() {
        // one column for the space and one for the `…`
        *last = clip_display(last, width.saturating_sub(2));
    }
    lines
}

/// desc_line_view styles one description line; a folded block ends with an
/// emphasized `…`.
fn desc_line_view(line: &DescLine) -> String {
    let body = match line.source {
        DescSource::Schema => style::desc(&line.text),
        DescSource::Comment => style::comment(&line.text),
    };
    if line.folded {
        format!("{body} {}", style::accent("…"))
    } else {
        body
    }
}
use crate::ui::menu::style;

/// worstIssueIcon picks the severity glyph for a set of issues: any real
/// validation error outranks the warnings.
fn worst_issue_icon(items: &[Issue]) -> &'static str {
    if items.iter().any(|it| !it.warn) {
        ERR
    } else {
        WARN
    }
}

/// A paste member: one key-value pair.
struct PasteMember {
    key: String,
    val: NodeId, // id into the parsed paste document
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
    let (doc, _meta) = crate::doc::yamldoc::parse(s)
        .map_err(|_| "clipboard is not JSON keys or items".to_string())?;
    Ok(classify_parsed(doc))
}

/// classifyPasteTOML decodes clipboard text as TOML for TOML documents.
/// A full document supplies members/items; otherwise a single value (scalar,
/// array, inline table) replaces the target value.
fn classify_paste_toml(
    s: &str,
) -> Result<(Option<Vec<PasteMember>>, Option<Vec<NodeId>>, Option<Doc>), String> {
    if let Ok((doc, _meta)) = crate::doc::tomldoc::parse(s) {
        return Ok(classify_parsed(doc));
    }
    if let Ok((doc, _meta)) = crate::doc::tomldoc::parse_value(s) {
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
fn parse_paste(
    text: &str,
    mode: &str,
) -> Result<(Option<Vec<PasteMember>>, Option<Vec<NodeId>>, Option<Doc>), String> {
    let mut s = text.trim().to_string();
    if s.is_empty() {
        return Err("clipboard is empty".to_string());
    }
    if s.starts_with('{') || s.starts_with('[') {
        match jsondoc::parse(&s) {
            Ok(doc) => return Ok(classify_parsed(doc)),
            Err(_) => {
                // one trailing comma before the closing bracket is tolerated
                if let Some(t) = strip_trailing_comma(&s) {
                    if let Ok(doc) = jsondoc::parse(&t) {
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
    if let Ok(doc) = jsondoc::parse(&s) {
        return Ok(classify_parsed(doc));
    }
    if s.ends_with(',') {
        s = s.trim().trim_end_matches(',').trim().to_string();
    }
    if s.is_empty() {
        return Err("clipboard is empty".to_string());
    }
    if let Ok(doc) = jsondoc::parse(&format!("{{{s}}}")) {
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
    if let Ok(doc) = jsondoc::parse(&format!("[{s}]")) {
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
fn plain_paste_doc(text: &str) -> Option<Doc> {
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

/// schema_accepts_string reports whether a schema node can hold an arbitrary
/// string: a text scalar (typed or unconstrained), an enum, an unconstrained
/// raw/any-JSON field, or a oneOf group with such a leaf. `seen` breaks
/// recursive schema references.
fn schema_accepts_string(
    schema: &Schema,
    node: FormNodeId,
    seen: &mut HashSet<FormNodeId>,
) -> bool {
    if !seen.insert(node) {
        return false;
    }
    let n = &schema.nodes[node as usize];
    match n.kind {
        Kind::Scalar => n.json_type.is_empty() || n.json_type == "string",
        Kind::Enum => true,
        // A raw field takes any JSON value; null-only raw does not.
        Kind::Raw => !n.null_only,
        Kind::OneOf => n
            .variants
            .iter()
            .any(|&v| schema_accepts_string(schema, v, seen)),
        _ => false,
    }
}

/// pasteValueRowOK reports whether the cursor row is a value slot a pasted
/// single value can replace.
fn paste_value_row_ok(r: &Row) -> bool {
    match r.kind {
        // The root scalar document is a Field row with an empty path.
        RowKind::Field => true,
        RowKind::Unknown => !r.path.is_empty(),
        // A oneOf picker row still points at the field's value (or the root
        // document): P replaces the value and the variant is re-inferred.
        RowKind::Variant => true,
        RowKind::Drill => {
            // A oneOf/object/array key row is still one value slot: P with a
            // single value replaces that whole value (the variant is inferred
            // again from the new value on the next refresh).
            !r.path.is_empty() && r.fn_.is_some()
        }
        _ => false,
    }
}

/// pasteEmptyNode mirrors jsondoc's own emptiness test: null or no value at
/// all converts to a container.
fn paste_empty_node(doc: &Doc, n: NodeId) -> bool {
    matches!(doc.value(n), Value::Null) || matches!(doc.value(n), Value::Object(m) if m.is_empty())
}

/// pasteHasKey reports whether the member list contains key.
fn paste_has_key(members: &[Member], key: &str) -> bool {
    members.iter().any(|m| m.key == key)
}

/// Map a Msg to the reduced widget Key (Go feeds the bubbles widgets the
/// tea.KeyMsg directly; this covers the operations they exercise).
pub(crate) fn msg_key(msg: &Msg) -> Option<Key> {
    Some(match msg {
        Msg::Char(c, false) => Key::Char(*c),
        // shell-style word motion inside every text input
        Msg::Char('f', true) | Msg::Char('F', true) => Key::WordForward,
        Msg::Char('b', true) | Msg::Char('B', true) => Key::WordBackward,
        Msg::Enter => Key::Enter,
        Msg::Backspace => Key::Backspace,
        Msg::Delete => Key::Delete,
        Msg::Left => Key::Left,
        Msg::Right => Key::Right,
        Msg::Up => Key::Up,
        Msg::Down => Key::Down,
        Msg::Home => Key::Home,
        Msg::End => Key::End,
        Msg::Ctrl('a') => Key::CtrlA,
        Msg::Ctrl('e') => Key::CtrlE,
        Msg::Ctrl('k') => Key::CtrlK,
        Msg::Ctrl('u') => Key::CtrlU,
        Msg::Ctrl('w') => Key::CtrlW,
        _ => return None,
    })
}

/// inputLine renders `prompt + value` with a reverse-video caret, scrolling
/// horizontally so the caret stays visible (the schema picker's search row).
fn search_input_line(prompt: &str, ti: &TextInput, width: usize) -> String {
    let avail = width.saturating_sub(display_width(prompt) + 2); // caret + margin
    let value = ti.value();
    let chars: Vec<char> = value.chars().collect();
    let cursor = ti.cursor().min(chars.len());
    let cell_at = |n: usize| chars[..n].iter().map(|&c| char_width(c)).sum::<usize>();
    let cursor_cell = cell_at(cursor);
    let mut start = 0usize;
    let window = avail.max(1);
    while start < cursor && cursor_cell - cell_at(start) >= window {
        start += 1;
    }
    let mut end = start;
    let mut used = 0usize;
    while end < chars.len() {
        let w = char_width(chars[end]);
        if used + w > avail {
            break;
        }
        used += w;
        end += 1;
    }
    let before: String = chars[start..cursor].iter().collect();
    let mut out = String::from(prompt);
    out.push_str(&before);
    if cursor < end {
        out.push_str("\x1b[7m");
        out.push(chars[cursor]);
        out.push_str("\x1b[27m");
        out.push_str(&chars[cursor + 1..end].iter().collect::<String>());
    } else {
        out.push_str("\x1b[7m \x1b[27m");
    }
    out
}

/// search_field_line renders one overlay input row. Only the focused input
/// shows the reverse-video caret; the inactive row is dimmed and caret-less
/// so tabbing between search and replace is visible.
fn search_field_line(label: &str, ti: &TextInput, focused: bool) -> String {
    if focused {
        format!("{label}: {}", ti_view(ti))
    } else {
        style::faint(&format!("{label}: {}", ti.value()))
    }
}

/// Render a TextInput like bubbles textinput.View(): the value with the
/// cursor cell reverse-video.
fn ti_view(ti: &TextInput) -> String {
    let v = ti.value();
    let pos = ti.cursor();
    let chs: Vec<char> = v.chars().collect();
    let byte_pos = v.char_indices().nth(pos).map(|(b, _)| b).unwrap_or(v.len());
    let before = &v[..byte_pos];
    let (cell, after) = if pos < chs.len() {
        let c = chs[pos];
        (c.to_string(), v[byte_pos + c.len_utf8()..].to_string())
    } else {
        (" ".to_string(), String::new())
    };
    format!("{before}\x1b[7m{cell}\x1b[0m{after}")
}

/// Render a TextArea viewport with a reverse-video cursor cell. The window
/// follows the cursor row (bubbles textarea behavior); a long line scrolls
/// horizontally so the cursor stays visible.
fn ta_view(ta: &TextArea) -> String {
    let (top, h) = ta.window();
    let w = (ta.width() as usize).max(1);
    let (cursor_row, cursor_col) = ta.cursor();
    let mut out = String::new();
    let end = (top + h).min(ta.line_count());
    for i in top..end {
        if i > top {
            out.push('\n');
        }
        let line = ta.line(i).unwrap_or("");
        let shift = if i == cursor_row && cursor_col >= w {
            cursor_col + 1 - w
        } else {
            0
        };
        let chars: Vec<char> = line.chars().skip(shift).take(w).collect();
        if i != cursor_row {
            out.extend(chars);
            continue;
        }
        let col = cursor_col.saturating_sub(shift).min(w - 1);
        let upto = col.min(chars.len());
        out.extend(chars.iter().take(upto));
        let cell = chars.get(col).copied().unwrap_or(' ');
        out.push_str("\x1b[7m");
        out.push(cell);
        out.push_str("\x1b[0m");
        if col < chars.len() {
            out.extend(chars.iter().skip(col + 1));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// App key handling

impl App {
    // --- input bursts -------------------------------------------------------

    /// update_repeat applies `n` identical keys collected from one terminal
    /// input burst. Consecutive menu cursor moves collapse into one step;
    /// everything else (typing, overlays, macro recording) replays the key so
    /// each press keeps its own semantics.
    pub fn update_repeat(&mut self, msg: Msg, n: usize) {
        if n <= 1 {
            self.update(msg);
            return;
        }
        let page = page_delta(&msg);
        let delta = vertical_delta(&msg);
        if page.is_none() && delta.is_none() {
            for _ in 0..n {
                self.update(msg.clone());
            }
            return;
        }
        // Recording, text inputs and overlays keep one event per press.
        if self.recording || !self.list_mode() || self.overlay.is_some() {
            for _ in 0..n {
                self.update(msg.clone());
            }
            return;
        }
        self.notice.clear();
        self.fs_mut().startup.clear();
        if let Some(page) = page {
            self.move_page(page * n as i64);
        } else if let Some(delta) = delta {
            self.move_plain(delta * n as i64);
        }
    }

    /// move_page moves the plain menu cursor one screenful, clamped at the
    /// first/last row. The visible row count is the same one listView uses.
    fn move_page(&mut self, delta: i64) {
        let rows = self.menu_rows();
        self.move_page_rows(delta, &rows);
    }

    fn move_page_rows(&mut self, delta: i64, rows: &[Row]) {
        self.clamp_cursor(rows);
        if rows.is_empty() {
            return;
        }
        let page = match self.top().cloned() {
            Some(st) => self.list_page_size(&st, rows),
            None => return,
        };
        let cur = self.top().map(|st| st.cursor).unwrap_or(0);
        let last = rows.len().saturating_sub(1);
        let next = (cur as i64 + delta * page as i64).clamp(0, last as i64) as usize;
        if let Some(st) = self.top_mut() {
            st.cursor = next;
        }
        self.cmd_buf.clear(); // a jump drops a pending count
        self.visual_sync(rows);
    }

    /// move_plain is the plain menu's vertical movement (key_list's j/k and
    /// arrow branch) for a whole run of repeats. The row list does not depend
    /// on the cursor, so one build serves the whole run.
    fn move_plain(&mut self, delta: i64) {
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
        if rows.is_empty() {
            return; // key_list leaves a pending command untouched too
        }
        let n = rows.len();
        if let Some(st) = self.top_mut() {
            st.cursor = wrap_cursor(st.cursor, delta, n);
        }
        self.cmd_buf.clear(); // jump_keys drops a pending count on a plain move
        self.visual_sync(&rows);
    }

    /// list_page_size is the number of list rows on screen. It mirrors the
    /// height listView assigns to write_rows so a page move lands exactly one
    /// visible screen away.
    fn list_page_size(&self, st: &MenuState, rows: &[Row]) -> usize {
        let (_title, summary) = self.menu_title(st);
        let desc_lines = self.desc_lines_for(st, rows, !self.desc_expanded).len();
        let mut summary_lines = if summary.is_empty() {
            0
        } else {
            wrap_text(
                &format!("variant: {}", summary),
                self.width.saturating_sub(1),
            )
            .len()
        };
        if st.kind == MenuKind::OneOf && st.searching {
            summary_lines += 1;
        }
        self.height
            .saturating_sub(
                2 + 1
                    + desc_lines
                    + summary_lines
                    + self.status_lines().len()
                    + usize::from(!self.notice.is_empty())
                    + usize::from(self.hide_empty),
            )
            .max(1)
    }

    // --- schema switching (S) ----------------------------------------------

    /// openSchemaPicker searches the SchemaStore catalog for the active file
    /// and switches the form model to the chosen schema. The document body is
    /// untouched apart from the recorded schema location.
    fn open_schema_picker(&mut self) {
        if self.opts().accessible {
            return; // no key events in the line protocol
        }
        if self.opts().replace_schema_location.is_none() {
            self.notice = "schema switching unavailable".to_string();
            return;
        }
        let loc = crate::catalog::loc();
        let entries = match crate::catalog::load(&loc) {
            Ok(entries) => entries,
            Err(e) => {
                self.notice = format!("schema catalog: {e}");
                return;
            }
        };
        let base = crate::file_base(&self.opts().path);
        let chosen = crate::ui::schema_picker::pick(
            &entries,
            &crate::ui::schema_picker::PickOptions {
                accessible: false,
                filename: base.to_string(),
                query: crate::search_stem(base),
            },
        );
        // The picker owns raw mode + the alternate screen while it runs; the
        // menu session must get them back or later keys sit in line-buffered
        // input and the next frame has nowhere to draw.
        let _ = crossterm::terminal::enable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::event::EnableBracketedPaste,
            crossterm::cursor::Hide
        );
        let Some(chosen) = chosen else {
            self.notice = "schema unchanged".to_string();
            return;
        };
        self.apply_schema(&chosen);
    }

    /// applySchema installs a chosen catalog schema: normalize it, rebuild
    /// the validator, overwrite the recorded schema location, and reset the
    /// menu stack. The live document becomes the input to the new form.
    fn apply_schema(&mut self, chosen: &crate::catalog::Entry) {
        let loc = chosen.url.clone();
        let model = match crate::smodel::load(&loc) {
            Ok(model) => model,
            Err(e) => {
                self.notice = format!("load schema: {e}");
                return;
            }
        };
        let mut compile_warning = String::new();
        let validate: Option<Arc<dyn Fn(&Doc) -> Vec<String> + Send + Sync>> =
            match crate::validate::new(&loc) {
                Err(e) => {
                    compile_warning = format!(
                        "{} schema compile failed, save-time validation disabled: {}",
                        crate::icons::WARN,
                        e.replace('\n', " ")
                    );
                    None
                }
                Ok(v) => {
                    let v = Arc::new(v);
                    Some(Arc::new(move |doc: &Doc| v.check(doc)))
                }
            };
        let schema_member = crate::smodel::root_has_property(&model, "$schema");
        if let Some(replace) = self.opts().replace_schema_location.clone() {
            replace(&mut self.fs_mut().doc, &loc, schema_member);
        }

        let root = model.root;
        let kind = crate::ui::rows::menu_kind_for(&model, root);
        let f = self.fs_mut();
        f.schema = model;
        f.root = root;
        f.menu_cache.clear();
        f.opts.validate = validate;
        f.opts.status = compile_warning;
        f.issues_parked = None;
        f.issues_return = 0;
        f.issues_origin = None;
        f.search_parked = None;
        f.jump_origin = None;
        f.jump_return = 0;
        let mut st = MenuState {
            kind,
            node: root,
            path: Vec::new(),
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
        };
        crate::ui::rows::seed_variant_cursor(&f.schema, &mut st, &f.doc);
        f.stack = vec![st];

        self.nav = Default::default();
        self.hide_empty = false;
        self.cmd_buf.clear();
        self.fast_move = None;
        self.notice = format!("schema from SchemaStore: {}", chosen.name);
        if self.opts().validate.is_some() {
            self.refresh_issues();
        } else {
            let items = self.live_issues(Vec::new());
            self.set_issues(items);
        }
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
    }

    // --- list mode ---------------------------------------------------------

    /// keyList is the plain menu's key table (Go keyList).
    pub fn key_list(&mut self, msg: Msg) {
        let st_kind = self.top().map(|st| st.kind);
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
        if self.menu_search_key(&msg) {
            return;
        }
        if rows.is_empty() {
            return;
        }
        if self.fast_move.is_some() && self.fast_move_key(&msg) {
            return;
        }
        self.fast_move = None; // any other key cancels fast move and works as usual
        let cursor = self.top().map(|st| st.cursor).unwrap_or(0);
        let r = rows[cursor].clone();
        if self.g_anchor_keys(&msg, &r) {
            return;
        }
        let cur = self.top().map(|st| st.cursor).unwrap_or(0);
        let mut moved_cursor = cur;
        if self.jump_keys(&msg, &mut moved_cursor, rows.len()) {
            let moved = moved_cursor != cur;
            if let Some(st) = self.top_mut() {
                st.cursor = moved_cursor;
            }
            let visual = self.top().map(|st| st.visual).unwrap_or(false);
            if visual && moved {
                self.visual_jump(&rows, cur);
            }
            return;
        }
        match msg.key_str().as_str() {
            "up" | "k" => {
                if let Some(st) = self.top_mut() {
                    st.cursor = wrap_cursor(st.cursor, -1, rows.len());
                }
                // A cursor move leaves the row set unchanged: reuse it.
                self.visual_sync(&rows);
            }
            "down" | "j" => {
                if let Some(st) = self.top_mut() {
                    st.cursor = wrap_cursor(st.cursor, 1, rows.len());
                }
                self.visual_sync(&rows);
            }
            "pgdown" | "ctrl+f" => self.move_page_rows(1, &rows),
            "pgup" | "ctrl+b" => self.move_page_rows(-1, &rows),
            "ctrl+j" | "n" => self.move_to_value(&rows, 1),
            "ctrl+k" | "N" => self.move_to_value(&rows, -1),
            "p" => self.paste_clipboard(&r, false),
            "P" => self.paste_clipboard(&r, true),
            "y" => self.copy_row_or_selection(&r),
            "Y" => self.copy_row(&r, false),
            "x" => {
                if st_kind == Some(MenuKind::Items) {
                    self.cut_row_or_selection(&r, false);
                } else {
                    self.cut_row_or_selection(&r, true);
                }
            }
            "X" => self.cut_row_or_selection(&r, false),
            "m" => self.toggle_select(&r),
            "v" => {
                let notice;
                {
                    let Some(st) = self.top_mut() else { return };
                    st.visual = !st.visual;
                    if st.visual {
                        st.vanchor = st.cursor;
                        notice = "visual select on — j/k/gg/G mark rows, v stops".to_string();
                    } else {
                        notice = "visual select off".to_string();
                    }
                }
                let rows = self.menu_rows();
                self.visual_sync(&rows);
                self.notice = notice;
            }
            "f" => self.fast_move = Some(FastMoveState { backward: false }),
            "F" => self.fast_move = Some(FastMoveState { backward: true }),
            "z" => self.toggle_empty_keys(),
            "e" => self.edit_value_external_row(&r),
            "E" => self.edit_raw_current(&r),
            "ctrl+e" => {
                if !r.merged.is_empty() {
                    self.expand_merged(&r);
                } else if let Some(path) = self.expand_target(&r) {
                    self.expand_alias(&path);
                }
            }
            "&" => self.open_anchor(&r, AnchorMode::AnchorSet),
            "*" => self.open_anchor(&r, AnchorMode::RefSet),
            "<" => self.open_merge(),
            "enter" | "right" | "l" => {
                let is_enum_cycle = (msg.key_str() == "right" || msg.key_str() == "l")
                    && r.kind == RowKind::Field
                    && r.fn_.map_or(false, |f| {
                        self.fs().schema.nodes[f as usize].kind == Kind::Enum
                    });
                if is_enum_cycle {
                    self.cycle_enum(&r, 1);
                } else {
                    self.activate(r);
                }
            }
            "left" | "h" => {
                let is_enum_cycle = r.kind == RowKind::Field
                    && r.fn_.map_or(false, |f| {
                        self.fs().schema.nodes[f as usize].kind == Kind::Enum
                    });
                if is_enum_cycle {
                    self.cycle_enum(&r, -1);
                } else if self.fs().stack.len() <= 1 {
                    // the root menu has no parent: only esc/q leave it
                } else {
                    self.back();
                }
            }
            "esc" => {
                // esc/q cancels a live multi-selection at any menu depth
                // before it navigates: marks and visual select both count.
                let (visual, selected) = self
                    .top()
                    .map_or((false, false), |st| (st.visual, !st.sel.is_empty()));
                if visual || selected {
                    if let Some(st) = self.top_mut() {
                        st.visual = false;
                        st.sel.clear();
                    }
                    self.notice = if visual {
                        "visual select off"
                    } else {
                        "selection cleared"
                    }
                    .to_string();
                } else {
                    self.back();
                }
            }
            "ctrl+o" => self.nav_go(true),
            "tab" | "ctrl+i" => self.nav_go(false),
            " " => self.space_key(r),
            "d" | "delete" => self.delete_row(&r),
            "D" => self.remove_item_shortcut(&r),
            "-" => self.adjust_number(&r, -1),
            "=" | "+" => self.adjust_number(&r, 1),
            "J" => self.move_item(&r, 1),
            "K" => self.move_item(&r, -1),
            "t" => {
                let variant_key = match r.fn_ {
                    Some(f) if self.fs().schema.nodes[f as usize].kind == Kind::OneOf => Some(f),
                    _ => None,
                };
                if st_kind != Some(MenuKind::OneOf) {
                    if let Some(f) = variant_key {
                        // t on a variant key opens its picker without
                        // entering the current scalar/object editor
                        self.push(f, r.path.clone());
                    } else if let Some(one_of) = self.top().and_then(|st| st.one_of) {
                        let path = self.top().map(|st| st.path.clone()).unwrap_or_default();
                        self.fs_mut().stack.pop();
                        self.push(one_of, path);
                    } else {
                        match r.fn_ {
                            Some(f) => {
                                let n = &self.fs().schema.nodes[f as usize];
                                if n.kind == Kind::Raw && !n.null_only && !n.raw_editor {
                                    self.open_any_type(r.path.clone());
                                }
                            }
                            None if r.kind == RowKind::Unknown => {
                                self.open_any_type(r.path.clone());
                            }
                            None => {}
                        }
                    }
                }
            }
            "a" => {
                if st_kind == Some(MenuKind::Items) {
                    self.insert_item(&r, false);
                } else {
                    self.add_key(&r, false);
                }
            }
            "A" => {
                if st_kind == Some(MenuKind::Items) {
                    self.insert_item(&r, true);
                } else {
                    self.add_key(&r, true);
                }
            }
            "c" => self.toggle_desc(&r, &rows),
            "C" => self.edit_comment_row(&r),
            "?" => {
                self.help = true;
                self.help_scroll = 0;
            }
            "/" => {
                if matches!(st_kind, Some(MenuKind::OneOf) | Some(MenuKind::EnumArray)) {
                    self.start_menu_search();
                } else {
                    self.open_search(false);
                }
            }
            "r" => self.open_search(true),
            "s" => {
                if self.files.len() > 1 {
                    self.file_pick = Some(FilePickState {
                        cursor: self.cur,
                        search_ti: TextInput::default(),
                        searching: false,
                        search_cursor: self.cur,
                    });
                } else {
                    self.notice = "only one file is open".to_string();
                }
            }
            "S" => self.open_schema_picker(),
            "ctrl+s" | "w" => self.save(),
            "W" => self.open_write_to(),
            _ => {}
        }
    }

    /// toggleEmptyKeys flips the z filter for every stacked menu, remapping
    /// each saved cursor onto the same row (Go toggleEmptyKeys).
    pub fn toggle_empty_keys(&mut self) {
        let stack: Vec<MenuState> = self.fs().stack.clone();
        let mut levels: Vec<(MenuState, Vec<Row>)> = Vec::with_capacity(stack.len());
        for mut st in stack {
            let rows = self.menu_rows_state(&st);
            let max = rows.len().saturating_sub(1);
            st.cursor = st.cursor.min(max);
            levels.push((st, rows));
        }
        self.hide_empty = !self.hide_empty;
        for (mut st, before) in levels {
            let after = self.menu_rows_state(&st);
            remap_state_cursor(&mut st, &before, &after);
            // write the remapped cursor back onto the stack entry
            let idx = self
                .fs()
                .stack
                .iter()
                .position(|s| same_state_eq(s, &st))
                .unwrap_or(usize::MAX);
            if idx != usize::MAX {
                self.files[self.cur].stack[idx].cursor = st.cursor;
            }
        }
        if self.hide_empty {
            self.notice = "empty fields hidden · z shows them".to_string();
        } else {
            self.notice = "empty fields shown".to_string();
        }
    }

    /// moveToValue moves the cursor to the next/previous row with a
    /// non-null value (rows.rs move_to_value over the top state).
    fn move_to_value(&mut self, rows: &[Row], delta: i64) {
        let Some(mut st) = self.top().cloned() else {
            return;
        };
        crate::ui::rows::move_to_value(&self.ctx(), &mut st, rows, delta);
        if let Some(t) = self.top_mut() {
            t.cursor = st.cursor;
        }
    }

    /// activate performs the enter action for a row (Go activate).
    fn activate(&mut self, r: Row) {
        match r.kind {
            RowKind::Field => {
                let f = r.fn_.expect("field row without schema node");
                self.open_field(f, r.path.clone(), false);
            }
            RowKind::Drill => {
                let f = r.fn_.expect("drill row without schema node");
                self.push_child_menu(f, r.path.clone());
            }
            RowKind::Add => self.add_item(),
            RowKind::Variant => self.enter_variant(&r),
            RowKind::Toggle => self.toggle_enum_member(&r),
            RowKind::Unknown => self.activate_unknown(r.path.clone()),
            RowKind::Expand => self.expand_alias(&r.path),
            RowKind::NoItems => {
                if !matches!(
                    self.top().map(|st| st.kind),
                    Some(MenuKind::OneOf) | Some(MenuKind::EnumArray)
                ) {
                    self.add_key(&r, false);
                }
            }
            RowKind::Issues => self.open_issues(),
        }
    }

    /// space toggles booleans/enum members, else activates (Go space).
    fn space_key(&mut self, r: Row) {
        match r.kind {
            RowKind::Toggle => self.toggle_enum_member(&r),
            RowKind::Field => {
                let kind = r
                    .fn_
                    .map(|f| self.fs().schema.nodes[f as usize].kind)
                    .unwrap_or_default();
                if kind == Kind::Boolean {
                    let f = r.fn_.expect("field row without schema node");
                    self.toggle_bool(f, &r.path);
                } else {
                    self.activate(r);
                }
            }
            RowKind::Variant => {
                let is_bool = r
                    .variant
                    .map(|v| self.fs().schema.nodes[v as usize].kind == Kind::Boolean)
                    .unwrap_or(false);
                if is_bool {
                    let v = r.variant.expect("variant row without node");
                    self.toggle_bool(v, &r.path);
                } else {
                    self.activate(r);
                }
            }
            RowKind::Unknown => {
                if self.unknown_bool(&r.path) {
                    self.toggle_unknown_bool(&r.path);
                } else {
                    self.activate(r);
                }
            }
            _ => self.activate(r),
        }
    }

    /// back pops one menu level; on the root screen it ends the session
    /// directly when the document is unchanged and otherwise asks whether
    /// to save first (Go back).
    pub fn back(&mut self) {
        // parked search: esc at the landing depth reopens it
        let search_back = self
            .fs()
            .search_parked
            .as_ref()
            .map_or(false, |s| self.fs().stack.len() == s.return_depth);
        if search_back {
            self.search_return_to();
            return;
        }
        // parked issues: esc at the landing depth returns to the list
        if self.fs().issues_parked.is_some() && self.fs().stack.len() == self.fs().issues_return {
            self.issues_return_to();
            return;
        }
        // ga/gr return: esc at the landing depth restores the captured stack
        if self.fs().jump_origin.is_some() && self.fs().stack.len() == self.fs().jump_return {
            let leaving = self.nav_snapshot();
            let origin = self.fs_mut().jump_origin.take().unwrap_or_default();
            self.fs_mut().jump_return = 0;
            self.fs_mut().stack = origin;
            self.nav_back_step(leaving);
            return;
        }
        if self.fs().stack.len() > 1 {
            let leaving = self.nav_snapshot();
            self.fs_mut().stack.pop();
            if let Some(st) = self.top_mut() {
                st.visual = false; // navigation ends a visual select
            }
            self.nav_back_step(leaving);
            return;
        }
        let dirty = self.dirty_files();
        if dirty.is_empty() {
            self.exiting = true;
            return;
        }
        let title = if dirty.len() > 1 {
            format!("Save changes in {} files? (y/n)", dirty.len())
        } else {
            "Save changes? (y/n)".to_string()
        };
        self.overlay = Some(Overlay::Confirm(ConfirmState {
            title,
            hint: "y/enter/space save & exit · n discard & exit · esc stay".to_string(),
            on_yes: Some(Box::new(|m| m.save_dirty_files_and_exit())),
            on_no: Some(Box::new(|m| {
                m.aborted = true;
            })),
        }));
    }

    /// push drills into fn's menu (Go push).
    pub fn push(&mut self, fn_: FormNodeId, path: Vec<PathSeg>) {
        self.nav_forward();
        if let Some(top) = self.fs_mut().stack.last_mut() {
            top.visual = false;
        }
        let kind = crate::ui::rows::menu_kind_for(&self.fs().schema, fn_);
        let mut st = MenuState {
            kind,
            node: fn_,
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
        };
        {
            let f = self.fs_mut();
            seed_variant_cursor(&f.schema, &mut st, &f.doc);
        }
        self.fs_mut().stack.push(st);
    }

    /// pushChildMenu drills into a child node (Go pushChildMenu).
    fn push_child_menu(&mut self, fn_: FormNodeId, path: Vec<PathSeg>) {
        let kind = self.fs().schema.nodes[fn_ as usize].kind;
        if kind == Kind::OneOf && dynamic_variants(&self.fs().schema, fn_).is_some() {
            self.nav_forward();
            let st = self.dynamic_menu(fn_, path);
            self.fs_mut().stack.push(st);
            return;
        }
        if kind == Kind::OneOf {
            let v = self.doc().get(&path);
            if !is_null_opt(self.doc(), v) {
                let idx = current_variant(&self.fs().schema, fn_, self.doc(), v);
                if idx >= 0 {
                    let variant = self.fs().schema.nodes[fn_ as usize].variants[idx as usize];
                    self.enter_variant_editor(variant, path, Some(fn_));
                    return;
                }
            }
        }
        self.push(fn_, path);
    }

    // --- jump keys ---------------------------------------------------------

    /// jumpKeys applies the gg/G/Home/End jumps shared by the menu list and
    /// the pure-list overlays (Go jumpKeys).
    pub fn jump_keys(&mut self, msg: &Msg, cursor: &mut usize, n: usize) -> bool {
        let key = msg.key_str();
        if !self.cmd_buf.is_empty() {
            // a g-family combo or count is pending
            if is_digit_key(&key) && is_count_buf(&self.cmd_buf) {
                self.cmd_buf.push(key);
                return true;
            }
            if key == "g" && is_count_buf(&self.cmd_buf) {
                self.cmd_buf.push(key);
                return true;
            }
            let (count, has_count) = buf_count(&self.cmd_buf);
            self.cmd_buf.clear();
            match key.as_str() {
                "g" => {
                    if self.list_mode() {
                        self.nav_forward();
                    }
                    if has_count && self.jump_item(cursor, count) {
                        return true;
                    }
                    *cursor = 0;
                    return true;
                }
                "esc" => return true, // esc cancels the pending combo only
                _ => return false,
            }
        }
        if is_digit_key(&key) && self.count_armed() {
            self.cmd_buf.push(key);
            return true;
        }
        match key.as_str() {
            "g" => {
                self.cmd_buf.push(key);
                true
            }
            "G" | "end" => {
                if self.list_mode() {
                    self.nav_forward();
                }
                *cursor = n.saturating_sub(1);
                true
            }
            "home" => {
                if self.list_mode() {
                    self.nav_forward();
                }
                *cursor = 0;
                true
            }
            _ => false,
        }
    }

    /// countArmed reports whether a digit may start a count: the menu list
    /// of an array.
    fn count_armed(&self) -> bool {
        self.list_mode() && self.top().map(|st| st.kind) == Some(MenuKind::Items)
    }

    /// jumpItem moves the cursor to the item row with the given array index,
    /// clamped to the last item (Go jumpItem).
    fn jump_item(&mut self, cursor: &mut usize, idx: usize) -> bool {
        let rows = self.menu_rows();
        let mut last: Option<usize> = None;
        for (i, r) in rows.iter().enumerate() {
            if r.idx < 0 || !matches!(r.kind, RowKind::Field | RowKind::Drill) {
                continue;
            }
            if r.idx == idx as i64 {
                *cursor = i;
                return true;
            }
            last = Some(i);
        }
        if let Some(l) = last {
            *cursor = l; // past-the-end counts clamp to the last item
            return true;
        }
        false // empty array: only < Add item >
    }

    // --- fast move (f/F) ---------------------------------------------------

    /// fastMoveKey handles a key while fast move waits for a character.
    pub fn fast_move_key(&mut self, msg: &Msg) -> bool {
        match msg {
            Msg::Esc => {
                self.fast_move = None;
                self.notice = "fast move cancelled".to_string();
                true
            }
            Msg::Char(ch, false) => {
                let backward = self.fast_move.as_ref().map(|f| f.backward).unwrap_or(false);
                self.fast_move = None;
                self.fast_move_to(*ch, backward);
                true
            }
            _ => false,
        }
    }

    /// fastMoveTo moves the cursor to the nearest row after (or, backward,
    /// before) the cursor whose key starts with ch (Go findMove).
    fn fast_move_to(&mut self, ch: char, backward: bool) {
        let Some(st) = self.top() else { return };
        let cursor = st.cursor;
        let rows = self.menu_rows();
        if rows.is_empty() {
            return;
        }
        let want = ch.to_string();
        for i in 1..=rows.len() {
            let mut idx = cursor as i64;
            if backward {
                idx -= i as i64;
            } else {
                idx += i as i64;
            }
            idx %= rows.len() as i64;
            if idx < 0 {
                idx += rows.len() as i64;
            }
            let idx = idx as usize;
            if let Some(key) = find_row_key(&self.fs().schema, &rows[idx]) {
                if key.starts_with(&want) {
                    if let Some(st) = self.top_mut() {
                        st.cursor = idx;
                    }
                    return;
                }
            }
        }
        self.notice = format!("no key starts with {:?}", ch.to_string());
    }

    // --- selection ---------------------------------------------------------

    /// toggleSelect flips the cursor row's membership in the selection.
    fn toggle_select(&mut self, r: &Row) {
        if !selectable_row(r) {
            self.notice = "m marks key-value or item rows".to_string();
            return;
        }
        let Some(st) = self.top_mut() else { return };
        let k = sel_key(&r.path);
        if !st.sel.insert(k) {
            // was already present: remove
            let k = sel_key(&r.path);
            st.sel.remove(&k);
        }
    }

    /// visualSync rebuilds the selection while visual select is on.
    fn visual_sync(&mut self, rows: &[Row]) {
        let Some(st) = self.top_mut() else { return };
        if !st.visual {
            return;
        }
        let (mut lo, mut hi) = (st.vanchor, st.cursor);
        if lo > hi {
            (lo, hi) = (hi, lo);
        }
        let mut sel = HashSet::new();
        for i in lo..=hi.min(rows.len().saturating_sub(1)) {
            if selectable_row(&rows[i]) {
                sel.insert(sel_key(&rows[i].path));
            }
        }
        st.sel = sel;
    }

    /// visualJump lands a list jump while visual select is on and extends
    /// the marked range (Go visualJump).
    fn visual_jump(&mut self, rows: &[Row], from: usize) {
        let Some(st) = self.top_mut() else { return };
        if st.cursor < rows.len() && !selectable_row(&rows[st.cursor]) {
            let mut dest: Option<usize> = None;
            let mut i = st.cursor as i64 - 1;
            while i >= 0 && dest.is_none() {
                if selectable_row(&rows[i as usize]) {
                    dest = Some(i as usize);
                }
                i -= 1;
            }
            let mut i = st.cursor + 1;
            while i < rows.len() && dest.is_none() {
                if selectable_row(&rows[i]) {
                    dest = Some(i);
                }
                i += 1;
            }
            if let Some(d) = dest {
                st.cursor = d;
            }
        }
        let (lo, hi) = (
            st.vanchor.min(from).min(st.cursor),
            st.vanchor.max(from).max(st.cursor),
        );
        if st.cursor == lo {
            st.vanchor = hi;
        } else {
            st.vanchor = lo;
        }
        self.visual_sync(rows);
    }

    /// rowSelected reports whether the row is in the selection.
    fn row_selected(&self, r: &Row) -> bool {
        match self.top() {
            Some(st) => !r.path.is_empty() && st.sel.contains(&sel_key(&r.path)),
            None => false,
        }
    }

    // --- field editing -----------------------------------------------------

    /// adjustNumber implements - / = / + on a numeric field: bump the current
    /// number by delta, or write delta ("-1"/"1") when a schema-typed field
    /// has no value. Numbers the schema does not define (unknown keys,
    /// schema-less documents, raw fields) work by their runtime value.
    fn adjust_number(&mut self, r: &Row, delta: i64) {
        // Field: a scalar row. Drill: an array item/schema-less value whose
        // schema node is a container or oneOf but whose runtime value is a
        // number (generic arrays). Unknown: a schema-less object member.
        if !matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            return;
        }
        let typed_numeric = r.fn_.is_some_and(|f| {
            let node = &self.fs().schema.nodes[f as usize];
            node.kind == Kind::Scalar && matches!(node.json_type.as_str(), "integer" | "number")
        });
        let path = r.path.clone();
        let literal = match self.doc().get(&path).map(|n| self.doc().value(n)) {
            Some(Value::Num(lit)) => match bump_number_literal(lit, delta) {
                Some(next) => next,
                None => return, // unrepresentable literal: leave it untouched
            },
            // Only a schema-typed numeric field can seed a missing value: for
            // a runtime-typed key there is nothing that says it is a number.
            None | Some(Value::Null) if typed_numeric => delta.to_string(),
            _ => return,
        };
        let n = self.fs_mut().doc.new_num(literal);
        let _ = self.fs_mut().doc.set(&path, n);
    }

    /// editValueExternalRow opens $EDITOR on the value under the cursor.
    fn edit_value_external_row(&mut self, r: &Row) {
        if matches!(r.kind, RowKind::Field | RowKind::Unknown | RowKind::Drill) {
            self.edit_value_external(r.path.clone(), -1);
        } else {
            self.notice = "no value to edit".to_string();
        }
    }

    /// activateUnknown edits a schema-less key like a oneOf value: a known
    /// runtime type edits directly, while an absent/null value has no type
    /// to infer and opens the type picker.
    fn activate_unknown(&mut self, path: Vec<PathSeg>) {
        self.open_runtime_value(path);
    }

    /// openRuntimeValue opens the editor for a value with no schema type,
    /// dispatching on its JSON runtime type. Null/missing values cannot
    /// determine a type, so they open the picker instead.
    fn open_runtime_value(&mut self, path: Vec<PathSeg>) {
        let Some(n) = self.doc().get(&path) else {
            self.open_any_type(path);
            return;
        };
        match self.doc().value(n) {
            Value::Null => self.open_any_type(path),
            Value::Bool(_) => self.toggle_unknown_bool(&path),
            Value::Num(_) => self.open_unknown_scalar(path, "number"),
            Value::Str(_) => self.open_unknown_scalar(path, "string"),
            Value::Object(_) => self.open_generic_container(path, Kind::Object),
            Value::Array(_) => self.open_generic_container(path, Kind::Array),
        }
    }

    /// openGenericContainer pushes the schema-less Object/Array editor from
    /// the any-type picker: members/items are edited like unknown keys.
    fn open_generic_container(&mut self, path: Vec<PathSeg>, kind: Kind) {
        let f = self.fs_mut();
        let mut cache = std::mem::take(&mut f.menu_cache);
        let any = any_type_menu(&mut f.schema, &mut cache);
        let variant = f.schema.nodes[any as usize]
            .variants
            .iter()
            .copied()
            .find(|v| {
                let n = &f.schema.nodes[*v as usize];
                n.key.is_empty() && n.kind == kind
            });
        f.menu_cache = cache;
        if let Some(v) = variant {
            self.push(v, path);
        }
    }

    /// openUnknownScalar opens the in-place input for a schema-less scalar.
    fn open_unknown_scalar(&mut self, path: Vec<PathSeg>, json_type: &str) {
        let key = match path.last() {
            Some(PathSeg::Key(k)) => k.clone(),
            _ => String::new(),
        };
        let synth = FormNode {
            key: key.clone(),
            title: key,
            kind: Kind::Scalar,
            json_type: json_type.to_string(),
            ..FormNode::default()
        };
        self.open_field_synth(synth, path, false, None);
    }

    /// startMenuSearch arms the inline "/"-search of a oneOf picker or an
    /// enum-array toggle list. The query always starts empty and the cursor
    /// keeps the position it had (the current value's row when the picker
    /// opened on a value).
    fn start_menu_search(&mut self) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        if !matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray) {
            return;
        }
        if let Some(t) = self.top_mut() {
            t.searching = true;
            t.search_ti = TextInput::default();
            t.search_cursor = st.cursor;
        }
    }

    /// menuSearchKey consumes keys while the inline list search is active.
    /// It supports the schema picker's editing keys; the query filters the
    /// rows as it is typed.
    fn menu_search_key(&mut self, msg: &Msg) -> bool {
        let (inline_search, searching) = self
            .top()
            .map(|st| {
                (
                    matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray),
                    st.searching,
                )
            })
            .unwrap_or((false, false));
        if !inline_search || !searching {
            return false;
        }
        match msg {
            Msg::Esc => {
                if let Some(st) = self.top_mut() {
                    st.searching = false;
                    st.search_ti = TextInput::default();
                }
                let n = self.menu_rows().len();
                if let Some(st) = self.top_mut() {
                    st.cursor = st.search_cursor.min(n.saturating_sub(1));
                }
                true
            }
            Msg::Enter => {
                let cur = self.top().map(|st| st.cursor).unwrap_or(0);
                let row = self.menu_rows().get(cur).cloned();
                let Some(r) = row else { return true };
                // no match: stay in the search mode so backspace undoes it
                if r.kind == RowKind::NoItems {
                    return true;
                }
                // toggling an enum-array member keeps the filter: enter is
                // the only toggle key while the query is being typed
                if self.top().map(|st| st.kind) == Some(MenuKind::EnumArray) {
                    self.activate(r);
                    return true;
                }
                if let Some(st) = self.top_mut() {
                    st.searching = false;
                    st.search_ti = TextInput::default();
                }
                self.activate(r);
                true
            }
            Msg::Up | Msg::BackTab | Msg::Ctrl('k') => {
                self.menu_search_move(-1);
                true
            }
            Msg::Down | Msg::Tab | Msg::Ctrl('j') => {
                self.menu_search_move(1);
                true
            }
            _ => {
                let Some(key) = msg_key(msg) else {
                    return true; // search mode swallows unbound keys
                };
                let Some(st) = self.top_mut() else {
                    return true;
                };
                let before = st.search_ti.value().to_string();
                st.search_ti.handle_key(key);
                if st.search_ti.value() != before {
                    st.cursor = 0;
                    st.offset = 0;
                }
                true
            }
        }
    }

    fn menu_search_move(&mut self, delta: i64) {
        let n = self.menu_rows().len();
        let cur = self.top().map(|st| st.cursor).unwrap_or(0);
        if let Some(st) = self.top_mut() {
            st.cursor = wrap_cursor(cur, delta, n);
        }
    }

    /// openAnyType pushes the shared arbitrary-JSON type picker at path.
    fn open_any_type(&mut self, path: Vec<PathSeg>) {
        let f = self.fs_mut();
        let mut cache = std::mem::take(&mut f.menu_cache);
        let node = any_type_menu(&mut f.schema, &mut cache);
        f.menu_cache = cache;
        self.push(node, path);
    }

    fn toggle_unknown_bool(&mut self, path: &[PathSeg]) {
        let synth = FormNode {
            kind: Kind::Boolean,
            ..FormNode::default()
        };
        self.toggle_bool_synth(&synth, path);
    }

    fn unknown_bool(&self, path: &[PathSeg]) -> bool {
        self.doc()
            .get(path)
            .map_or(false, |n| matches!(self.doc().value(n), Value::Bool(_)))
    }

    /// openField starts the appropriate editor for a field row.
    fn open_field(&mut self, fn_: FormNodeId, path: Vec<PathSeg>, is_new: bool) {
        let f = self.fs().schema.nodes[fn_ as usize].clone();
        self.open_field_synth(f, path, is_new, Some(fn_))
    }

    /// openField with an inlined FormNode: schema node or the synthetic one
    /// of a schema-less scalar edit.
    fn open_field_synth(
        &mut self,
        f: FormNode,
        path: Vec<PathSeg>,
        is_new: bool,
        fn_id: Option<FormNodeId>,
    ) {
        match f.kind {
            Kind::Scalar => {
                let cur = self.doc().get(&path);
                if let Some(n) = cur {
                    if let Value::Str(s) = self.doc().value(n) {
                        if s.contains('\r') || s.contains('\n') {
                            // multiline text cannot live in the one-line input
                            self.edit_value_external(path, -1);
                            return;
                        }
                    }
                }
                let mut ti = TextInput::new();
                let mut pre = scalar_text(self.doc(), cur);
                if pre.is_empty() {
                    if let Some(d) = f.default_ {
                        pre = scalar_text(&self.fs().schema.doc, Some(d));
                    }
                }
                ti.set_value(pre);
                ti.cursor_end();
                let cands = self.completions(&f, &path);
                self.overlay = Some(Overlay::Edit(EditState {
                    // a schema field keeps its real node id: the inline
                    // renderer matches rows by it (Go editState.fn); only a
                    // schema-less scalar edit carries a synthetic node
                    fn_: fn_id,
                    synth: if fn_id.is_none() { Some(f) } else { None },
                    path,
                    ti,
                    err: String::new(),
                    is_new,
                    cands,
                    sugs: Vec::new(),
                    sug_idx: -1,
                }));
                self.refresh_edit_suggestions();
            }
            Kind::Enum => {
                let cur = self.doc().get(&path);
                let Some(fn_id) = fn_id else {
                    // synthetic nodes are never enums; fall back to raw edit
                    self.open_raw_at(None, Some(path));
                    return;
                };
                let cursor = enum_index(&self.fs().schema, fn_id, self.doc(), cur);
                self.overlay = Some(Overlay::Sel(SelState {
                    fn_: fn_id,
                    path,
                    opts: f.enum_,
                    cursor,
                    search_ti: TextInput::default(),
                    searching: false,
                    search_cursor: 0,
                    one_of: None,
                }));
            }
            Kind::Boolean => {
                // resolve against a schema id when possible (default lookup)
                if let Some(id) = fn_id {
                    self.toggle_bool(id, &path);
                } else {
                    self.toggle_bool_synth(&f, &path);
                }
            }
            // a field whose schema has no concrete JSON type edits by its
            // runtime type; only a null/missing value opens the type picker
            Kind::Raw if !f.null_only && !f.raw_editor => {
                self.open_runtime_value(path);
            }
            _ => {
                self.open_raw_at(fn_id, Some(path));
            }
        }
    }

    /// editRawCurrent opens the external $EDITOR with the raw JSON/YAML/TOML
    /// of the current node: the row under the cursor when it is a value row,
    /// otherwise (root scalar, action/status row) the whole document. E.
    /// The built-in raw textarea remains for raw-degraded fields and the
    /// any-type JSON variant.
    fn edit_raw_current(&mut self, r: &Row) {
        if !r.path.is_empty()
            && matches!(
                r.kind,
                RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Variant
            )
        {
            self.edit_value_external(r.path.clone(), -1);
        } else {
            self.edit_raw_external();
        }
    }

    /// openRawAt arms the built-in raw textarea overlay.
    fn open_raw_at(&mut self, fn_: Option<FormNodeId>, path: Option<Vec<PathSeg>>) {
        let mut ta = TextArea::new();
        let w = self.width.saturating_sub(4).clamp(20, 100);
        let h = self.height.saturating_sub(6).clamp(3, 24);
        ta.set_width(w as u16);
        ta.set_height(h as u16);
        let raw = self.opts().raw.clone();
        let mut verbatim_str = false;
        match &path {
            Some(p) => {
                // A string value is text, not a document: encoding it as YAML
                // turned multiline strings into "|" blocks whose alignment
                // spaces landed in the editor and quoted values into quotes.
                if let Some(cur) = self.doc().get(p) {
                    if !is_null_val(self.doc(), cur) {
                        match self.doc().value(cur) {
                            Value::Str(text) => {
                                ta.set_value(text);
                                verbatim_str = true;
                            }
                            _ => {
                                if let Ok(s) = raw.encode_raw_value(self.doc(), cur) {
                                    ta.set_value(&s);
                                }
                            }
                        }
                    }
                }
            }
            None => {
                if let Ok(s) = raw.encode_raw_doc(self.doc()) {
                    ta.set_value(&s);
                }
            }
        }
        self.overlay = Some(Overlay::Text(TextState {
            path,
            fn_,
            ta,
            err: String::new(),
            verbatim_str,
        }));
    }

    /// toggleBool flips a boolean field between its current/effective value
    /// and the opposite, writing through.
    fn toggle_bool(&mut self, fn_: FormNodeId, path: &[PathSeg]) {
        let f = self.fs().schema.nodes[fn_ as usize].clone();
        self.toggle_bool_synth(&f, path);
    }

    fn toggle_bool_synth(&mut self, f: &FormNode, path: &[PathSeg]) {
        let mut b = false;
        if let Some(n) = self.doc().get(path) {
            if let Value::Bool(v) = self.doc().value(n) {
                b = *v;
            } else if let Some(d) = f.default_ {
                if let Value::Bool(v) = self.fs().schema.doc.value(d) {
                    b = *v;
                }
            }
        } else if let Some(d) = f.default_ {
            if let Value::Bool(v) = self.fs().schema.doc.value(d) {
                b = *v;
            }
        }
        let nb = !b;
        let n = self.fs_mut().doc.new_bool(nb);
        let _ = self.fs_mut().doc.set(path, n);
    }

    /// cycleEnum steps an enum field through its options.
    fn cycle_enum(&mut self, r: &Row, delta: i64) {
        let Some(f) = r.fn_ else { return };
        let opts = self.fs().schema.nodes[f as usize].enum_.clone();
        if opts.is_empty() {
            return;
        }
        let cur = self.doc().get(&r.path);
        let idx = enum_index(&self.fs().schema, f, self.doc(), cur);
        let next = ((idx as i64 + delta + opts.len() as i64) % opts.len() as i64) as usize;
        let fi = self.cur;
        let f = &mut self.files[fi];
        let n = copy_from_doc(&mut f.doc, &f.schema.doc, opts[next].val);
        let _ = f.doc.set(&r.path, n);
    }

    // --- arrays ------------------------------------------------------------

    /// addItem appends a new array item (< Add item > row).
    fn add_item(&mut self) {
        let path = self.top().map(|st| st.path.clone()).unwrap_or_default();
        let idx = doc_array_len(self.doc(), &path);
        self.insert_item_at(idx, false);
    }

    /// insertItem inserts a fresh item before (after=false) or after the
    /// item under the cursor (Go insertItem).
    fn insert_item(&mut self, r: &Row, after: bool) {
        let mut idx = 0usize;
        let path = self.top().map(|st| st.path.clone()).unwrap_or_default();
        if matches!(r.kind, RowKind::Field | RowKind::Drill) && r.idx >= 0 {
            idx = r.idx as usize;
            if after {
                idx += 1;
            }
        } else if after {
            idx = doc_array_len(self.doc(), &path);
        }
        self.insert_item_at(idx, true);
    }

    /// insertItemAt splices a fresh item at idx and opens its editor or
    /// drill (Go insertItemAt).
    fn insert_item_at(&mut self, idx: usize, splice: bool) {
        let st = self.top().cloned();
        let Some(st) = st else { return };
        let items = match self.fs().schema.nodes[st.node as usize].items {
            Some(i) => i,
            None => return,
        };
        let ipath = path_idx(&st.path, idx);
        let items_kind = self.fs().schema.nodes[items as usize].kind;
        let ins_null = |m: &mut App, splice: bool| {
            if splice {
                let n = m.fs_mut().doc.new_null();
                let _ = m.fs_mut().doc.insert(&st.path, idx, n);
            }
        };
        match items_kind {
            Kind::OneOf => {
                // keep a null placeholder even for < Add item >: the type
                // picker edits this slot, and the inline editor needs a row
                let n = self.fs_mut().doc.new_null();
                if splice {
                    let _ = self.fs_mut().doc.insert(&st.path, idx, n);
                } else {
                    let _ = self.fs_mut().doc.set(&ipath, n);
                }
                if dynamic_variants(&self.fs().schema, items).is_some() {
                    self.nav_forward();
                    let st2 = self.dynamic_menu(items, ipath);
                    self.fs_mut().stack.push(st2);
                } else {
                    self.push(items, ipath);
                }
            }
            Kind::Object => {
                let n = self.fs_mut().doc.new_object();
                if splice {
                    let _ = self.fs_mut().doc.insert(&st.path, idx, n);
                }
                self.push(items, ipath);
            }
            Kind::Array => {
                let n = self.fs_mut().doc.new_array();
                if splice {
                    let _ = self.fs_mut().doc.insert(&st.path, idx, n);
                }
                self.push(items, ipath);
            }
            Kind::Boolean => {
                let mut b = false;
                if let Some(d) = self.fs().schema.nodes[items as usize].default_ {
                    if let Value::Bool(v) = self.fs().schema.doc.value(d) {
                        b = *v;
                    }
                }
                let n = self.fs_mut().doc.new_bool(b);
                if splice {
                    let _ = self.fs_mut().doc.insert(&st.path, idx, n);
                } else {
                    let _ = self.fs_mut().doc.set(&ipath, n);
                }
            }
            _ => {
                // scalar / enum / raw item
                ins_null(self, splice);
                self.open_field(items, ipath.clone(), true);
                if splice {
                    // the inserted item is the first row when the array was
                    // empty, otherwise row `idx` in the item-only list
                    if let Some(t) = self.top_mut() {
                        t.cursor = idx;
                    }
                }
            }
        }
    }

    /// deleteRow batch-deletes the current selection; without one it is the
    /// plain cursor-row delete.
    fn delete_row(&mut self, r: &Row) {
        let has_sel = self.top().map(|st| !st.sel.is_empty()).unwrap_or(false);
        if !has_sel {
            self.delete_pair(r);
            return;
        }
        let rows = self.menu_rows();
        let mut keys: Vec<Vec<PathSeg>> = rows
            .iter()
            .filter(|sel| {
                self.top()
                    .map(|st| st.sel.contains(&sel_key(&sel.path)))
                    .unwrap_or(false)
            })
            .map(|sel| sel.path.clone())
            .collect();
        // array items must go last-to-first: each delete splices the list and
        // would shift the still-marked higher indexes otherwise
        keys.sort_by(|a, b| match (a.last(), b.last()) {
            (Some(PathSeg::Idx(x)), Some(PathSeg::Idx(y))) => y.cmp(x),
            _ => std::cmp::Ordering::Equal,
        });
        for p in keys {
            let tmp = Row {
                path: p,
                ..Row::default()
            };
            self.delete_pair(&tmp);
        }
        if let Some(st) = self.top_mut() {
            st.sel.clear();
        }
    }

    /// deletePair removes the selected key-value pair (Go deletePair).
    fn delete_pair(&mut self, r: &Row) -> bool {
        if !matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            return false;
        }
        if r.path.is_empty() {
            return false;
        }
        if matches!(r.path.last(), Some(PathSeg::Idx(_))) {
            self.delete_item(r);
            return true;
        }
        // a merged row deletes the anchor mapping's field; required ones refuse
        let (required, merged, title) = r
            .fn_
            .map(|f| {
                let n = &self.fs().schema.nodes[f as usize];
                (n.required, r.merged.clone(), n.title.clone())
            })
            .unwrap_or((false, r.merged.clone(), String::new()));
        if required && !merged.is_empty() {
            self.notice = format!("{} is required", title);
            return false;
        }
        // deleting the carrier of an anchor a merge key references would
        // leave the merge dangling
        if let Some(n) = self.doc().get(&r.path) {
            let anchor = self.doc().node(n).anchor.clone();
            if !anchor.is_empty()
                && count_anchor_uses(self.doc(), n) <= 1
                && merge_ref_uses(self.doc(), n) > 0
            {
                self.notice = format!("delete the << merge key referencing &{} first", anchor);
                return false;
            }
        }
        self.fs_mut().doc.remove(&r.path);
        true
    }

    /// removeItemShortcut is D: remove the current array item, both from the
    /// items list (cursor on an item row) and from the item's own form (the
    /// menu path itself is an index). It confirms like the old removal row.
    fn remove_item_shortcut(&mut self, r: &Row) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        let (path, pop) = if matches!(st.path.last(), Some(PathSeg::Idx(_))) {
            // inside an array item's form: remove that item and go back
            (st.path.clone(), true)
        } else if st.kind == MenuKind::Items
            && matches!(r.kind, RowKind::Field | RowKind::Drill)
            && r.idx >= 0
        {
            (r.path.clone(), false)
        } else {
            self.notice = "D removes an array item".to_string();
            return;
        };
        self.overlay = Some(Overlay::Confirm(ConfirmState {
            title: "Remove this item? (y/n)".to_string(),
            hint: String::new(),
            on_yes: Some(Box::new(move |m| {
                let _ = m.fs_mut().doc.remove(&path);
                if pop {
                    m.fs_mut().stack.pop(); // back to the items menu
                }
            })),
            on_no: None,
        }));
    }

    /// deleteItem removes an array entry row.
    fn delete_item(&mut self, r: &Row) {
        let ok = self
            .top()
            .map(|st| st.kind == MenuKind::Items)
            .unwrap_or(false);
        if !ok || r.idx < 0 || !matches!(r.kind, RowKind::Field | RowKind::Drill) {
            return;
        }
        self.fs_mut().doc.remove(&r.path);
    }

    /// moveItem shifts the array item under the cursor by delta.
    fn move_item(&mut self, r: &Row, delta: i64) {
        let (kind, path) = match self.top() {
            Some(st) => (st.kind, st.path.clone()),
            None => return,
        };
        if kind != MenuKind::Items
            || r.idx < 0
            || !matches!(r.kind, RowKind::Field | RowKind::Drill)
        {
            return;
        }
        let from = r.idx as usize;
        let to = (r.idx + delta) as usize;
        if !self.fs_mut().doc.move_(&path, from, to) {
            self.notice = if delta < 0 {
                "already first item".to_string()
            } else {
                "already last item".to_string()
            };
            return;
        }
        if let Some(st) = self.top_mut() {
            st.cursor = (st.cursor as i64 + delta).max(0) as usize;
        }
    }

    /// addKey opens the key-name input for a schema-less member (Go addKey).
    fn add_key(&mut self, r: &Row, after: bool) {
        let (kind, node_kind, path) = match self.top() {
            Some(st) => (
                st.kind,
                self.fs().schema.nodes[st.node as usize].kind,
                st.path.clone(),
            ),
            None => return,
        };
        if kind != MenuKind::Object || !matches!(node_kind, Kind::Object | Kind::Map) {
            self.notice = "a adds a custom key under an object menu".to_string();
            return;
        }
        let idx = self.member_insert_index(r, after);
        let ti = TextInput::new();
        self.overlay = Some(Overlay::Add(AddState {
            path,
            node: self.top().map(|st| st.node).unwrap(),
            idx,
            ti,
            err: String::new(),
        }));
    }

    /// memberInsertIndex returns the document member index a new key gets.
    fn member_insert_index(&self, r: &Row, after: bool) -> usize {
        let Some(st) = self.top() else { return 0 };
        let mut mem_len = 0usize;
        if let Some(cur) = self.doc().get(&st.path) {
            if let Value::Object(mems) = self.doc().value(cur) {
                mem_len = mems.len();
            }
        }
        if matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown)
            && !r.path.is_empty()
        {
            if let Some(PathSeg::Key(key)) = r.path.last() {
                if let Some(cur) = self.doc().get(&st.path) {
                    if let Value::Object(mems) = self.doc().value(cur) {
                        for (i, m) in mems.iter().enumerate() {
                            if &m.key == key {
                                return if after { i + 1 } else { i };
                            }
                        }
                    }
                }
            }
        }
        mem_len
    }

    /// toggleEnumMember flips one option of an enum-valued array (Go
    /// toggleEnumMember).
    fn toggle_enum_member(&mut self, r: &Row) {
        let Some(f) = r.fn_ else { return };
        let Some(items) = self.fs().schema.nodes[f as usize].items else {
            return;
        };
        let opts = self.fs().schema.nodes[items as usize].enum_.clone();
        if r.idx < 0 || (r.idx as usize) >= opts.len() {
            return;
        }
        let required = self.fs().schema.nodes[f as usize].required;
        let mut on: HashSet<String> = HashSet::new();
        if let Some(cur) = self.doc().get(&r.path) {
            if let Value::Array(arr) = self.doc().value(cur) {
                for &it in arr {
                    on.insert(encode_value(self.doc(), it));
                }
            }
        }
        let enc = opts[r.idx as usize].encoded.clone();
        if on.contains(&enc) {
            on.remove(&enc);
        } else {
            on.insert(enc);
        }
        if on.is_empty() && !required {
            self.fs_mut().doc.remove(&r.path);
            return;
        }
        let fi = self.cur;
        let f = &mut self.files[fi];
        let mut items_v: Vec<NodeId> = Vec::new();
        for o in &opts {
            if on.contains(&o.encoded) {
                items_v.push(copy_from_doc(&mut f.doc, &f.schema.doc, o.val));
            }
        }
        let arr = f.doc.push(NodeData {
            value: Value::Array(items_v),
            anchor: String::new(),
            merge: Vec::new(),
            raw: String::new(),
        });
        let _ = f.doc.set(&r.path, arr);
    }

    // --- oneOf -------------------------------------------------------------

    fn enter_variant(&mut self, r: &Row) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        // the any-type picker's JSON variant edits the existing value raw;
        // it is not a type switch, so it must not replace the value
        if let Some(v) = r.variant {
            if self.fs().schema.nodes[v as usize].raw_editor {
                self.enter_variant_editor(v, r.path.clone(), Some(st.node));
                return;
            }
        }
        let v = self.doc().get(&r.path);
        let cur = current_variant(&self.fs().schema, st.node, self.doc(), v);
        if cur >= 0 {
            let matched = self.fs().schema.nodes[st.node as usize].variants[cur as usize];
            if Some(matched) == r.variant {
                self.enter_variant_editor(matched, r.path.clone(), Some(st.node));
                return;
            }
        }
        if !is_null_opt(self.doc(), v) {
            let (variant, path, one_of) = (r.variant, r.path.clone(), st.node);
            // the any-type picker replaces a value's type in place: a null
            // placeholder keeps the object-member order / array slot
            let keep_slot = self.fs().menu_cache.get(&SynthKey::AnyType) == Some(&st.node);
            self.overlay = Some(Overlay::Confirm(ConfirmState {
                title: "Replace current value? (y/n)".to_string(),
                hint: String::new(),
                on_yes: Some(Box::new(move |m| {
                    let has_default = variant
                        .map(|vv| m.fs().schema.nodes[vv as usize].default_)
                        .flatten();
                    match has_default {
                        Some(d) if !is_null_val(&m.fs().schema.doc, d) => {
                            let fi = m.cur;
                            let f = &mut m.files[fi];
                            let n = copy_from_doc(&mut f.doc, &f.schema.doc, d);
                            let _ = f.doc.set(&path, n);
                        }
                        _ if keep_slot || matches!(path.last(), Some(PathSeg::Idx(_))) => {
                            // keep the slot while the new type's editor
                            // fills it in
                            let n = m.fs_mut().doc.new_null();
                            let _ = m.fs_mut().doc.set(&path, n);
                        }
                        _ => {
                            m.fs_mut().doc.remove(&path);
                        }
                    }
                    m.enter_variant_editor(variant.expect("variant row"), path, Some(one_of));
                })),
                on_no: None,
            }));
            return;
        }
        let variant = r.variant.expect("variant row without node");
        self.enter_variant_editor(variant, r.path.clone(), Some(st.node));
    }

    fn enter_variant_editor(
        &mut self,
        variant: FormNodeId,
        path: Vec<PathSeg>,
        one_of: Option<FormNodeId>,
    ) {
        let (kind, null_only) = {
            let n = &self.fs().schema.nodes[variant as usize];
            (n.kind, n.null_only)
        };
        // Every variant editor replaces the picker it was chosen from, so the
        // picker never lingers above the parent menu. A root oneOf picker has
        // no parent and stays.
        if self.top().map(|st| st.kind) == Some(MenuKind::OneOf) && self.fs().stack.len() > 1 {
            self.fs_mut().stack.pop();
        }
        match kind {
            // choosing the null type writes JSON null; there is nothing to
            // edit, so return to the parent menu
            Kind::Raw if null_only => {
                let n = self.fs_mut().doc.new_null();
                let _ = self.fs_mut().doc.set(&path, n);
            }
            Kind::OneOf => {
                // a schema-defined type may itself be a variant group
                if dynamic_variants(&self.fs().schema, variant).is_some() {
                    self.nav_forward();
                    let st = self.dynamic_menu(variant, path);
                    self.fs_mut().stack.push(st);
                } else {
                    self.push(variant, path);
                }
                if let Some(st) = self.top_mut() {
                    st.one_of = one_of;
                }
            }
            Kind::Object | Kind::Map | Kind::Array => {
                self.push(variant, path);
                if let Some(st) = self.top_mut() {
                    st.one_of = one_of;
                }
            }
            Kind::Boolean => self.toggle_bool(variant, &path),
            Kind::Enum => {
                let cur = self.doc().get(&path);
                let cursor = enum_index(&self.fs().schema, variant, self.doc(), cur);
                let opts = self.fs().schema.nodes[variant as usize].enum_.clone();
                self.overlay = Some(Overlay::Sel(SelState {
                    fn_: variant,
                    path,
                    opts,
                    cursor,
                    search_ti: TextInput::default(),
                    searching: false,
                    search_cursor: 0,
                    one_of,
                }));
            }
            _ => {
                // scalar / raw: edit at the field row in the parent menu
                self.open_field(variant, path.clone(), false);
            }
        }
    }

    // --- value completion ---------------------------------------------------

    /// completions collects the candidate values for fn at path (Go
    /// completions).
    fn completions(&self, fn_node: &FormNode, path: &[PathSeg]) -> Vec<String> {
        if fn_node.kind == Kind::Scalar && !fn_node.tag_ref.is_empty() {
            return self.tag_ref_completions(&fn_node.tag_ref);
        }
        let want_num = match fn_node.json_type.as_str() {
            "string" | "" => false,
            "integer" | "number" => true,
            _ => return Vec::new(),
        };
        let doc = self.doc();
        let mut same: Vec<String> = Vec::new();
        let mut other: Vec<String> = Vec::new();
        let mut same_seen: HashSet<String> = HashSet::new();
        let mut other_seen: HashSet<String> = HashSet::new();
        fn walk(
            doc: &Doc,
            n: Option<NodeId>,
            p: &[PathSeg],
            key: &str,
            fn_key: &str,
            path: &[PathSeg],
            want_num: bool,
            same: &mut Vec<String>,
            same_seen: &mut HashSet<String>,
            other: &mut Vec<String>,
            other_seen: &mut HashSet<String>,
        ) {
            let Some(n) = n else { return };
            match doc.value(n) {
                Value::Object(mems) => {
                    for m in mems {
                        if p.is_empty() && m.key == "$schema" {
                            continue; // the editor's own schema pointer
                        }
                        let mp = path_for(p, &m.key);
                        if let Some(s) = value_text(doc, Some(m.val), want_num) {
                            if mp != path {
                                if m.key == fn_key {
                                    if completion_candidate(&s) && same_seen.insert(s.clone()) {
                                        same.push(s);
                                    }
                                } else if completion_candidate(&s) && other_seen.insert(s.clone()) {
                                    other.push(s);
                                }
                            }
                            continue;
                        }
                        walk(
                            doc,
                            Some(m.val),
                            &mp,
                            &m.key,
                            fn_key,
                            path,
                            want_num,
                            same,
                            same_seen,
                            other,
                            other_seen,
                        );
                    }
                }
                Value::Array(items) => {
                    for (i, &it) in items.iter().enumerate() {
                        let ip = path_idx(p, i);
                        if let Some(s) = value_text(doc, Some(it), want_num) {
                            if ip != path {
                                if key == fn_key {
                                    if completion_candidate(&s) && same_seen.insert(s.clone()) {
                                        same.push(s);
                                    }
                                } else if completion_candidate(&s) && other_seen.insert(s.clone()) {
                                    other.push(s);
                                }
                            }
                            continue;
                        }
                        walk(
                            doc,
                            Some(it),
                            &ip,
                            key,
                            fn_key,
                            path,
                            want_num,
                            same,
                            same_seen,
                            other,
                            other_seen,
                        );
                    }
                }
                v => {
                    if let Some(s) = value_text(doc, Some(n), want_num) {
                        if p != path {
                            let _ = v;
                            if key == fn_key {
                                if completion_candidate(&s) && same_seen.insert(s.clone()) {
                                    same.push(s);
                                }
                            } else if completion_candidate(&s) && other_seen.insert(s.clone()) {
                                other.push(s);
                            }
                        }
                    }
                }
            }
        }
        let fn_key = fn_node.key.clone();
        walk(
            doc,
            Some(doc.root),
            &[],
            "",
            &fn_key,
            path,
            want_num,
            &mut same,
            &mut same_seen,
            &mut other,
            &mut other_seen,
        );
        let mut out = same.clone();
        for v in other {
            if !same_seen.contains(&v) {
                out.push(v);
            }
        }
        out
    }

    /// tagRefCompletions collects the "tag" values of the structures a
    /// reference domain names (Go tagRefCompletions).
    fn tag_ref_completions(&self, ref_: &str) -> Vec<String> {
        let words: Vec<&str> = ref_.split('_').collect();
        let (leads, tail) = words.split_at(words.len() - 1);
        let leads: Vec<&str> = leads.to_vec();
        let tail = tail[0];
        let doc = self.doc();
        let mut out: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        fn collect(doc: &Doc, n: NodeId, out: &mut Vec<String>, seen: &mut HashSet<String>) {
            if let Value::Array(items) = doc.value(n) {
                for &it in items {
                    collect(doc, it, out, seen);
                }
                return;
            }
            let tag = tag_of_node(doc, n);
            if !tag.is_empty() && seen.insert(tag.clone()) {
                out.push(tag);
            }
        }
        fn walk(
            doc: &Doc,
            n: NodeId,
            ancestors: &str,
            ref_: &str,
            leads: &[&str],
            tail: &str,
            out: &mut Vec<String>,
            seen: &mut HashSet<String>,
        ) {
            match doc.value(n) {
                Value::Object(mems) => {
                    for m in mems {
                        if m.key == ref_
                            || m.key == format!("{}s", ref_)
                            || (!leads.is_empty()
                                && (m.key == tail || m.key == format!("{}s", tail))
                                && ref_words_above(leads, ancestors))
                        {
                            collect(doc, m.val, out, seen);
                        }
                        let child_anc = format!("{ancestors}\0{}\0", m.key);
                        walk(doc, m.val, &child_anc, ref_, leads, tail, out, seen);
                    }
                }
                Value::Array(items) => {
                    for &it in items {
                        walk(doc, it, ancestors, ref_, leads, tail, out, seen);
                    }
                }
                _ => {}
            }
        }
        walk(doc, doc.root, "\0", ref_, &leads, tail, &mut out, &mut seen);
        out
    }

    /// refreshEditSuggestions re-filters the candidates by the current
    /// input (Go refreshEditSuggestions).
    fn refresh_edit_suggestions(&mut self) {
        // schema suggestions win over document candidates (Go reads the
        // field's own node; synthetic nodes only for schema-less edits)
        let src = match &self.overlay {
            Some(Overlay::Edit(e)) => match e.fn_ {
                Some(id) => Some(self.fs().schema.nodes[id as usize].clone()),
                None => e.synth.clone(),
            },
            _ => None,
        };
        let Some(Overlay::Edit(e)) = &mut self.overlay else {
            return;
        };
        let prefix = e.ti.value().to_string();
        if let Some(src) = src {
            if src.tag_ref.is_empty() && !src.suggestions.is_empty() {
                let opts = filter_completions(&src.suggestions, &prefix);
                if !opts.is_empty() {
                    e.sugs = opts;
                    e.sug_idx = -1;
                    return;
                }
            }
        }
        e.sugs = filter_completions(&e.cands, &prefix);
        e.sug_idx = -1;
    }

    // --- copy / cut --------------------------------------------------------

    /// copyRow copies the row under the cursor (Go copyRow).
    fn copy_row(&mut self, r: &Row, pair: bool) {
        let (text, ok, refusal) = self.row_copy_text(r, pair, "y copies object keys only");
        if !ok {
            self.notice = refusal;
            return;
        }
        if let Err(e) = self.write_clipboard_text(&text) {
            self.notice = format!("copy failed: {e}");
            return;
        }
        self.notice = if pair {
            "Copied key-value pair".to_string()
        } else {
            "Copied value".to_string()
        };
    }

    fn write_clipboard_text(&self, text: &str) -> Result<(), String> {
        match &self.opts().copy {
            Some(w) => w(text),
            None => write_clipboard(text),
        }
    }

    pub(crate) fn read_clipboard_text(&self) -> Result<String, String> {
        match &self.opts().paste {
            Some(r) => r(),
            None => read_clipboard(),
        }
    }

    /// pasteIntoFocusedInput inserts paste text into the text input that owns
    /// the keyboard (ctrl+v and terminal bracketed paste). Reports whether an
    /// input consumed it; list-mode keys are left alone.
    pub(crate) fn paste_into_focused_input(&mut self, text: &str) -> bool {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        // The file switcher and the oneOf/enum-array search live outside
        // `overlay`; their inputs follow the same reset-on-change rule as
        // typed characters.
        if let Some(p) = &mut self.file_pick {
            if p.searching {
                let before = p.search_ti.value().to_string();
                p.search_ti.insert_str(&text);
                if p.search_ti.value() != before {
                    p.cursor = 0;
                }
                return true;
            }
        }
        if let Some(st) = self.top_mut() {
            if st.searching && matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray) {
                let before = st.search_ti.value().to_string();
                st.search_ti.insert_str(&text);
                if st.search_ti.value() != before {
                    st.cursor = 0;
                    st.offset = 0;
                }
                return true;
            }
        }
        let Some(ov) = self.overlay.take() else {
            return false;
        };
        match ov {
            Overlay::Edit(mut e) => {
                e.ti.insert_str(&text);
                self.overlay = Some(Overlay::Edit(e));
                self.refresh_edit_suggestions();
            }
            Overlay::Text(mut s) => {
                s.ta.insert_paste(&text);
                self.overlay = Some(Overlay::Text(s));
            }
            Overlay::CommentEdit(mut s) => {
                s.ta.insert_paste(&text);
                self.overlay = Some(Overlay::CommentEdit(s));
            }
            Overlay::Add(mut s) => {
                s.ti.insert_str(&text);
                self.overlay = Some(Overlay::Add(s));
            }
            Overlay::Write(mut s) => {
                s.ti.insert_str(&text);
                self.overlay = Some(Overlay::Write(s));
                self.refresh_write_suggestions();
            }
            Overlay::Anc(mut s) => {
                s.ti.insert_str(&text);
                self.overlay = Some(Overlay::Anc(s));
            }
            Overlay::Mrg(mut s) => {
                s.ti.insert_str(&text);
                s.sug = -1;
                self.overlay = Some(Overlay::Mrg(s));
            }
            Overlay::Search(mut s) => {
                if s.replace && s.replace_focused {
                    s.rti.insert_str(&text);
                } else {
                    s.ti.insert_str(&text);
                }
                self.refilter(&mut s);
                self.overlay = Some(Overlay::Search(s));
            }
            Overlay::Sel(mut s) if s.searching => {
                let before = s.search_ti.value().to_string();
                s.search_ti.insert_str(&text);
                if s.search_ti.value() != before {
                    s.cursor = 0;
                }
                self.overlay = Some(Overlay::Sel(s));
            }
            Overlay::Issues(mut s) if s.searching => {
                let before = s.search_ti.value().to_string();
                s.search_ti.insert_str(&text);
                if s.search_ti.value() != before {
                    s.cursor = 0;
                }
                self.overlay = Some(Overlay::Issues(s));
            }
            Overlay::Errs(mut s) if s.searching => {
                let before = s.search_ti.value().to_string();
                s.search_ti.insert_str(&text);
                if s.search_ti.value() != before {
                    s.cursor = 0;
                }
                self.overlay = Some(Overlay::Errs(s));
            }
            Overlay::RefPick(mut s) if s.searching => {
                let before = s.search_ti.value().to_string();
                s.search_ti.insert_str(&text);
                if s.search_ti.value() != before {
                    s.cursor = 0;
                }
                self.overlay = Some(Overlay::RefPick(s));
            }
            other => {
                self.overlay = Some(other);
                return false;
            }
        }
        true
    }

    /// rowCopyText builds the y/X clipboard text for the row (Go
    /// rowCopyText).
    fn row_copy_text(&self, r: &Row, pair: bool, key_refusal: &str) -> (String, bool, String) {
        if !matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            return (String::new(), false, "nothing to copy".to_string());
        }
        let Some(v) = self.doc().get(&r.path) else {
            return (String::new(), false, "no value to copy".to_string());
        };
        if pair {
            match copy_row_key(&r.path) {
                Some(key) => (self.copy_pair_text(&key, v), true, String::new()),
                None => (String::new(), false, key_refusal.to_string()),
            }
        } else {
            (self.copy_value_text(v), true, String::new())
        }
    }

    /// copyValueText is the Y text (Go copyValueText).
    fn copy_value_text(&self, v: NodeId) -> String {
        if let Value::Str(s) = self.doc().value(v) {
            return s.clone();
        }
        let raw = self.opts().raw.clone();
        if let Ok(s) = raw.encode_raw_value(self.doc(), v) {
            return s.trim_end_matches('\n').to_string();
        }
        self.doc().compact(v)
    }

    /// copyPairText is the y text (Go copyPairText).
    fn copy_pair_text(&self, key: &str, v: NodeId) -> String {
        let raw = self.opts().raw.clone();
        // a synthetic one-member object goes through the session codec
        let mut tmp = Doc::new();
        let val = copy_from_doc(&mut tmp, self.doc(), v);
        let obj = tmp.new_object();
        if let Value::Object(m) = &mut tmp.node_mut(obj).value {
            m.push(Member {
                key: key.to_string(),
                val,
            });
        }
        if let Ok(s) = (raw.marshal_value)(crate::ui::NodeRef { doc: &tmp, id: obj }) {
            return s.trim_end_matches('\n').to_string();
        }
        let raw_key = serde_json::to_string(key).unwrap_or_default();
        format!("{}: {}", raw_key, self.doc().compact(v))
    }

    /// selectionMembers collects the marked rows' key-value pairs in display
    /// order.
    fn selection_members(&mut self) -> Vec<(String, NodeId)> {
        let Some(st) = self.top().cloned() else {
            return Vec::new();
        };
        let rows = self.menu_rows_snapshot();
        let mut members = Vec::new();
        for sel in rows {
            if sel.path.is_empty() || !st.sel.contains(&sel_key(&sel.path)) {
                continue;
            }
            let Some(PathSeg::Key(key)) = sel.path.last() else {
                continue;
            };
            if let Some(v) = self.doc().get(&sel.path) {
                members.push((key.clone(), v));
            }
        }
        members
    }

    /// selectionValues collects the marked rows' document values in display
    /// order — the array-menu counterpart of selectionMembers (array items
    /// have no object key to copy).
    fn selection_values(&mut self) -> Vec<NodeId> {
        let Some(st) = self.top().cloned() else {
            return Vec::new();
        };
        let rows = self.menu_rows_snapshot();
        let doc = self.doc();
        rows.iter()
            .filter(|r| !r.path.is_empty() && st.sel.contains(&sel_key(&r.path)))
            .filter_map(|r| doc.get(&r.path))
            .collect()
    }

    /// menuRows for the current (top) state (read-only flows reuse the same
    /// warm cache path as menu_rows).
    fn menu_rows_snapshot(&mut self) -> Vec<Row> {
        self.menu_rows()
    }

    /// copyRowOrSelection is the y handler (Go copyRowOrSelection).
    fn copy_row_or_selection(&mut self, r: &Row) {
        let has_sel = self.top().map(|st| !st.sel.is_empty()).unwrap_or(false);
        // array pages copy element values, not key-value pairs
        let items = self.top().map(|st| st.kind) == Some(MenuKind::Items);
        if !has_sel {
            self.copy_row(r, !items);
            return;
        }
        if items {
            let values = self.selection_values();
            if values.is_empty() {
                self.notice = "no value to copy".to_string();
                return;
            }
            let text = values
                .iter()
                .map(|v| self.copy_value_text(*v))
                .collect::<Vec<_>>()
                .join("\n");
            if let Err(e) = self.write_clipboard_text(&text) {
                self.notice = format!("copy failed: {e}");
                return;
            }
            self.notice = format!("Copied {} values", values.len());
            if let Some(st) = self.top_mut() {
                st.sel.clear();
            }
            return;
        }
        let members = self.selection_members();
        if members.is_empty() {
            self.notice = "no value to copy".to_string();
            return;
        }
        let text = self.copy_value_text_of_members(&members);
        if let Err(e) = self.write_clipboard_text(&text) {
            self.notice = format!("copy failed: {e}");
            return;
        }
        self.notice = format!("Copied {} key-value pairs", members.len());
        if let Some(st) = self.top_mut() {
            st.sel.clear();
        }
    }

    fn copy_value_text_of_members(&self, members: &[(String, NodeId)]) -> String {
        let mut tmp = Doc::new();
        let obj = tmp.new_object();
        let mut vals: Vec<(String, NodeId)> = Vec::new();
        for (k, v) in members {
            let val = copy_from_doc(&mut tmp, self.doc(), *v);
            vals.push((k.clone(), val));
        }
        if let Value::Object(m) = &mut tmp.node_mut(obj).value {
            for (k, val) in vals {
                m.push(Member { key: k, val });
            }
        }
        let raw = self.opts().raw.clone();
        if let Ok(s) = (raw.marshal_value)(crate::ui::NodeRef { doc: &tmp, id: obj }) {
            return s.trim_end_matches('\n').to_string();
        }
        String::new()
    }

    /// cutRowOrSelection is the x/X handler (Go cutRowOrSelection).
    fn cut_row_or_selection(&mut self, r: &Row, pair: bool) {
        let batch = self.top().map(|st| !st.sel.is_empty()).unwrap_or(false);
        let items = self.top().map(|st| st.kind) == Some(MenuKind::Items);
        let (text, count) = if batch {
            if items {
                let vals = self.selection_values();
                if vals.is_empty() {
                    self.notice = "no value to copy".to_string();
                    return;
                }
                let values: Vec<String> = vals.iter().map(|v| self.copy_value_text(*v)).collect();
                (values.join("\n"), vals.len())
            } else if pair {
                let members = self.selection_members();
                if members.is_empty() {
                    self.notice = "no value to copy".to_string();
                    return;
                }
                let t = self.copy_value_text_of_members(&members);
                (t, members.len())
            } else {
                let members = self.selection_members();
                let mut values: Vec<String> = Vec::new();
                for (_, v) in &members {
                    values.push(self.copy_value_text(*v));
                }
                (values.join("\n"), members.len())
            }
        } else {
            let (t, ok, refusal) = self.row_copy_text(r, pair, "x cuts object keys only");
            if !ok {
                self.notice = refusal;
                return;
            }
            (t, 1)
        };
        if let Err(e) = self.write_clipboard_text(&text) {
            self.notice = format!("cut failed: {e}");
            return;
        }
        self.delete_row(r); // clears the selection; refusals surface their notice
        if self.notice.is_empty() {
            if !batch && pair {
                self.notice = "Cut key-value pair".to_string();
            } else if !batch {
                self.notice = "Cut value".to_string();
            } else if pair {
                self.notice = format!("Cut {} key-value pairs", count);
            } else {
                self.notice = format!("Cut {} values", count);
            }
        }
    }

    // --- paste -------------------------------------------------------------

    /// pastePlainStringDoc accepts unstructured clipboard text as a string
    /// when P targets a string value (or a selection containing one). Other
    /// targets keep the parse error: this is not a general plain-text paste.
    fn paste_plain_string_doc(&mut self, text: &str, r: &Row, under_cursor: bool) -> Option<Doc> {
        if !under_cursor || !self.paste_has_plain_string_target(r) {
            return None;
        }
        plain_paste_doc(text)
    }

    /// pasteHasPlainStringTarget reports whether the cursor row (or one of
    /// the marked rows) is a string value slot.
    fn paste_has_plain_string_target(&mut self, r: &Row) -> bool {
        if self.top().map(|st| !st.sel.is_empty()).unwrap_or(false) {
            let sel = self.top().map(|st| st.sel.clone()).unwrap_or_default();
            let rows = self.menu_rows();
            rows.iter()
                .any(|row| sel.contains(&sel_key(&row.path)) && self.paste_plain_string_target(row))
        } else {
            self.paste_plain_string_target(r)
        }
    }

    /// pastePlainStringTarget reports whether a row's value can take plain
    /// clipboard text as a string: the current runtime value is a string, or
    /// the row's schema admits an arbitrary string (text scalar, enum, raw
    /// any-JSON, oneOf with such a leaf). Schema-less keys have no type, so
    /// only a string (or missing) value counts.
    fn paste_plain_string_target(&self, r: &Row) -> bool {
        if !paste_value_row_ok(r) {
            return false;
        }
        if let Some(n) = self.doc().get(&r.path) {
            if matches!(self.doc().value(n), Value::Str(_)) {
                return true;
            }
        }
        match r.fn_ {
            Some(f) => schema_accepts_string(&self.fs().schema, f, &mut HashSet::new()),
            // A oneOf picker row has no node of its own; the group is the
            // menu's schema node.
            None if r.kind == RowKind::Variant => self
                .top()
                .filter(|st| st.kind == MenuKind::OneOf)
                .is_some_and(|st| {
                    schema_accepts_string(&self.fs().schema, st.node, &mut HashSet::new())
                }),
            // A schema-less row has no declared type: only a string (handled
            // above) or a missing value is a safe plain-text target.
            None => matches!(
                self.doc().get(&r.path).map(|n| self.doc().value(n)),
                None | Some(Value::Null)
            ),
        }
    }

    /// pasteClipboard implements p/P smart paste (Go pasteClipboard).
    fn paste_clipboard(&mut self, r: &Row, under_cursor: bool) {
        let text = match self.read_clipboard_text() {
            Ok(t) => t,
            Err(e) => {
                self.notice = format!("paste failed: {e}");
                return;
            }
        };
        let mode = self.opts().raw.raw_label();
        let (members, items, doc) = match parse_paste(&text, &mode) {
            Ok(x) => x,
            Err(e) => match self.paste_plain_string_doc(&text, r, under_cursor) {
                Some(doc) => (None, None, Some(doc)),
                None => {
                    self.notice = e;
                    return;
                }
            },
        };
        // a single value replaces values
        if members.is_none() && items.is_none() {
            let Some(pdoc) = doc.as_ref() else {
                self.notice = "paste failed: empty clipboard".to_string();
                return;
            };
            let single = pdoc.root;
            if under_cursor {
                let has_sel = self.top().map(|st| !st.sel.is_empty()).unwrap_or(false);
                let mut targets: Vec<Row> = Vec::new();
                if has_sel {
                    let rows = self.menu_rows();
                    for sel in rows {
                        if self
                            .top()
                            .map(|st| st.sel.contains(&sel_key(&sel.path)))
                            .unwrap_or(false)
                            && paste_value_row_ok(&sel)
                        {
                            targets.push(sel);
                        }
                    }
                } else if paste_value_row_ok(r) {
                    targets.push(r.clone());
                }
                if !targets.is_empty() {
                    let pre = self.doc().clone();
                    for t in &targets {
                        let n = copy_detached_from_doc(&mut self.fs_mut().doc, pdoc, single);
                        let _ = self.fs_mut().doc.set(&t.path, n);
                    }
                    if has_sel {
                        if let Some(st) = self.top_mut() {
                            st.sel.clear();
                        }
                    }
                    if nodes_equal_doc(&pre, &self.fs().doc) {
                        self.notice = "paste made no changes".to_string();
                    } else if targets.len() == 1 {
                        self.notice = format!(
                            "pasted value into {}",
                            crate::doc::jsondoc::path_string(&targets[0].path)
                        );
                    } else {
                        self.notice = format!("pasted value into {} keys", targets.len());
                    }
                    return;
                }
            }
            self.notice = "a single value replaces a row's value — press P on the row".to_string();
            return;
        }
        let (path, is_array, notice) = self.paste_target(r, under_cursor);
        if !notice.is_empty() {
            self.notice = notice;
            return;
        }
        let st_kind = self.top().map(|st| st.kind);
        if members.is_some() && is_array {
            // a complete mapping lands in an items page as ONE object item
            if st_kind != Some(MenuKind::Items) {
                self.notice = "clipboard holds key-value pairs, the target is an array".to_string();
                return;
            }
            let members = members.unwrap();
            let pre = self.doc().clone();
            let idx = self.paste_insert_index(r, &path, true, under_cursor);
            let item = self.members_as_item(doc.as_ref().unwrap(), &members);
            let _ = self.fs_mut().doc.insert(&path, idx, item);
            if nodes_equal_doc(&pre, &self.fs().doc) {
                self.notice = "paste made no changes".to_string();
                return;
            }
            self.notice = format!(
                "pasted 1 items into {}",
                crate::doc::jsondoc::path_string(&path)
            );
            return;
        }
        if items.is_some() && !is_array {
            self.notice = "clipboard holds array items, the target is a mapping".to_string();
            return;
        }
        let pre = self.doc().clone();
        if let Some(members) = members {
            let idx = self.paste_insert_index(r, &path, false, under_cursor);
            self.paste_into_object(doc.as_ref().unwrap(), &path, &members, idx);
            if nodes_equal_doc(&pre, &self.fs().doc) {
                self.notice = "paste made no changes".to_string();
                return;
            }
            self.notice = format!(
                "pasted {} keys into {}",
                members.len(),
                crate::doc::jsondoc::path_string(&path)
            );
            return;
        }
        let items = items.unwrap();
        let idx = self.paste_insert_index(r, &path, true, under_cursor);
        self.paste_into_array(&path, doc.as_ref().unwrap(), &items, idx);
        if nodes_equal_doc(&pre, &self.fs().doc) {
            self.notice = "paste made no changes".to_string();
            return;
        }
        self.notice = format!(
            "pasted {} items into {}",
            items.len(),
            crate::doc::jsondoc::path_string(&path)
        );
    }

    /// pasteTarget resolves the container the paste merges into.
    fn paste_target(&self, r: &Row, under_cursor: bool) -> (Vec<PathSeg>, bool, String) {
        let Some(st) = self.top() else {
            return (Vec::new(), false, "nothing to paste into here".to_string());
        };
        if under_cursor && r.kind == RowKind::Drill {
            if let Some(f) = r.fn_ {
                // a drill row inside an items menu is an array item: paste
                // targets the parent array
                if st.kind == MenuKind::Items && !r.path.is_empty() {
                    if matches!(r.path.last(), Some(PathSeg::Idx(_))) {
                        let parent = r.path[..r.path.len() - 1].to_vec();
                        if !self.paste_container_ok(&parent, true) {
                            return (
                                Vec::new(),
                                false,
                                "the value under the cursor is not an array".to_string(),
                            );
                        }
                        return (parent, true, String::new());
                    }
                }
                let kind = self.fs().schema.nodes[f as usize].kind;
                match kind {
                    Kind::Object | Kind::Map => {
                        if !self.paste_container_ok(&r.path, false) {
                            return (
                                Vec::new(),
                                false,
                                "the value under the cursor is not a map".to_string(),
                            );
                        }
                        return (r.path.clone(), false, String::new());
                    }
                    Kind::Array => {
                        if !self.paste_container_ok(&r.path, true) {
                            return (
                                Vec::new(),
                                false,
                                "the value under the cursor is not an array".to_string(),
                            );
                        }
                        return (r.path.clone(), true, String::new());
                    }
                    // A oneOf row's container is whatever the current
                    // variant holds: merge keys/items into that value.
                    Kind::OneOf => {
                        if let Some(n) = self.doc().get(&r.path) {
                            match self.doc().value(n) {
                                Value::Object(_) => return (r.path.clone(), false, String::new()),
                                Value::Array(_) => return (r.path.clone(), true, String::new()),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                // a scalar/unset oneOf drill row falls through to the menu
                // container
            }
        }
        // schema-less keys: only a container value is a paste target
        if under_cursor && r.kind == RowKind::Unknown && !r.path.is_empty() {
            if let Some(n) = self.doc().get(&r.path) {
                match self.doc().value(n) {
                    Value::Object(_) => return (r.path.clone(), false, String::new()),
                    Value::Array(_) => return (r.path.clone(), true, String::new()),
                    _ => {}
                }
            }
        }
        match st.kind {
            MenuKind::Object => {
                if self.paste_container_ok(&st.path, false) {
                    return (st.path.clone(), false, String::new());
                }
                (
                    Vec::new(),
                    false,
                    "the current menu is not a map".to_string(),
                )
            }
            MenuKind::Items | MenuKind::EnumArray => {
                if self.paste_container_ok(&st.path, true) {
                    return (st.path.clone(), true, String::new());
                }
                (
                    Vec::new(),
                    false,
                    "the current menu is not an array".to_string(),
                )
            }
            MenuKind::OneOf => {
                if let Some(n) = self.doc().get(&st.path) {
                    match self.doc().value(n) {
                        Value::Object(_) => return (st.path.clone(), false, String::new()),
                        Value::Array(_) => return (st.path.clone(), true, String::new()),
                        _ => {}
                    }
                }
                (
                    Vec::new(),
                    false,
                    "a oneOf value must be set before pasting into it".to_string(),
                )
            }
        }
    }

    /// pasteContainerOK reports whether the value at path can receive
    /// container content of the given shape.
    fn paste_container_ok(&self, path: &[PathSeg], array: bool) -> bool {
        match self.doc().get(path) {
            None => true,
            Some(n) => {
                if paste_empty_node(self.doc(), n) {
                    return true;
                }
                if array {
                    matches!(self.doc().value(n), Value::Array(_))
                } else {
                    matches!(self.doc().value(n), Value::Object(_))
                }
            }
        }
    }

    /// pasteInsertIndex is where the contiguous block of pasted content
    /// starts (Go pasteInsertIndex).
    fn paste_insert_index(
        &self,
        r: &Row,
        path: &[PathSeg],
        array: bool,
        under_cursor: bool,
    ) -> usize {
        let Some(st) = self.top() else {
            return 0;
        };
        if st.path != path {
            // pasting into a container under the cursor: append
            if array {
                return doc_array_len(self.doc(), path);
            }
            if let Some(cur) = self.doc().get(path) {
                if let Value::Object(mems) = self.doc().value(cur) {
                    return mems.len();
                }
            }
            return 0;
        }
        if array {
            if matches!(r.kind, RowKind::Field | RowKind::Drill) && r.idx >= 0 {
                return if under_cursor {
                    r.idx as usize
                } else {
                    r.idx as usize + 1
                };
            }
            return doc_array_len(self.doc(), path);
        }
        if under_cursor {
            self.member_insert_index(r, false)
        } else {
            self.member_insert_index(r, true)
        }
    }

    /// pasteIntoObject merges the members (Go pasteIntoObject).
    fn paste_into_object(
        &mut self,
        src: &Doc,
        path: &[PathSeg],
        members: &[PasteMember],
        mut idx: usize,
    ) {
        for mem in members {
            let exists = self
                .doc()
                .get(path)
                .and_then(|n| match self.doc().value(n) {
                    Value::Object(mems) => Some(paste_has_key(mems, &mem.key)),
                    _ => None,
                })
                .unwrap_or(false);
            let val = copy_detached_from_doc(&mut self.fs_mut().doc, src, mem.val);
            let _ = self.fs_mut().doc.insert_member(path, &mem.key, idx, val);
            if !exists {
                idx += 1;
            }
        }
    }

    /// pasteIntoArray splices the items as one contiguous block.
    fn paste_into_array(&mut self, path: &[PathSeg], src: &Doc, items: &[NodeId], mut idx: usize) {
        for &it in items {
            let val = copy_detached_from_doc(&mut self.fs_mut().doc, src, it);
            let _ = self.fs_mut().doc.insert(path, idx, val);
            idx += 1;
        }
    }

    /// membersAsItem wraps pasted members into one detached object node.
    fn members_as_item(&mut self, src: &Doc, members: &[PasteMember]) -> NodeId {
        let doc = &mut self.fs_mut().doc;
        let obj = doc.new_object();
        let mut vals: Vec<(String, NodeId)> = Vec::new();
        for mem in members {
            let val = copy_detached_from_doc(doc, src, mem.val);
            vals.push((mem.key.clone(), val));
        }
        if let Value::Object(m) = &mut doc.node_mut(obj).value {
            for (k, val) in vals {
                m.push(Member { key: k, val });
            }
        }
        obj
    }

    // --- jumps (search / issues / anchors) ----------------------------------

    /// jumpToRow rebuilds the navigation stack down to the menu that
    /// contains path and puts the cursor on the row itself.
    pub fn jump_to_row(&mut self, path: &[PathSeg]) {
        if path.is_empty() {
            self.jump_to_path(path);
            return;
        }
        self.reveal_hidden_empty(path);
        let parent = path[..path.len() - 1].to_vec();
        let stack = {
            let f = &mut self.files[self.cur];
            rebuild_stack(&mut f.schema, &f.doc, &parent)
        };
        self.fs_mut().stack = stack;
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
        if let Some(st) = self.top_mut() {
            st.cursor = cursor_by_path(&rows, path);
            st.offset = 0;
        }
    }

    /// jumpToPath rebuilds the navigation stack down to path (search-jump
    /// semantics). A oneOf key with a known value lands on its row in the
    /// parent menu instead of rebuilding the old variant picker beneath it.
    pub fn jump_to_path(&mut self, path: &[PathSeg]) {
        self.reveal_hidden_empty(path);
        if !path.is_empty() {
            let parent = path[..path.len() - 1].to_vec();
            let stack = {
                let f = &mut self.files[self.cur];
                rebuild_stack(&mut f.schema, &f.doc, &parent)
            };
            self.fs_mut().stack = stack;
            let rows = self.menu_rows();
            let variant_key = {
                let f = &self.fs();
                rows.iter().any(|r| {
                    r.path == path
                        && r.fn_.map_or(false, |fn_| {
                            f.schema.nodes[fn_ as usize].kind == Kind::OneOf
                        })
                })
            };
            if variant_key {
                self.clamp_cursor(&rows);
                if let Some(st) = self.top_mut() {
                    st.cursor = cursor_by_path(&rows, path);
                    st.offset = 0;
                }
                return;
            }
        }
        let stack = {
            let f = &mut self.files[self.cur];
            rebuild_stack(&mut f.schema, &f.doc, path)
        };
        self.fs_mut().stack = stack;
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
        if let Some(st) = self.top_mut() {
            st.cursor = cursor_by_path(&rows, path);
            st.offset = 0;
        }
    }

    /// revealHiddenEmpty turns the z filter off when path names an object
    /// key with no document value.
    fn reveal_hidden_empty(&mut self, path: &[PathSeg]) {
        if !self.hide_empty || path.is_empty() {
            return;
        }
        if !matches!(path.last(), Some(PathSeg::Key(_))) {
            return;
        }
        if is_null_opt(self.doc(), self.doc().get(path)) {
            self.hide_empty = false;
            self.notice = "empty fields shown".to_string();
        }
    }

    /// openIssues shows the load-time issues overlay, recomputed against
    /// the current document when save-time validation is available.
    fn open_issues(&mut self) {
        let mut items = self.fs().issues_now.clone();
        if let Some(validate) = self.opts().validate.clone() {
            items = self.live_issues(validate(&self.doc()));
            self.set_issues(items.clone());
        }
        self.fs_mut().issues_parked = None; // reopening resets the parked list
        self.overlay = Some(Overlay::Issues(IssuesState {
            items,
            cursor: 0,
            search_ti: TextInput::default(),
            searching: false,
            search_cursor: 0,
        }));
    }

    /// issuesReturnTo reopens the parked issue list at the jump target.
    fn issues_return_to(&mut self) {
        let leaving = self.nav_snapshot();
        let mut items = Vec::new();
        let mut cur = 0usize;
        if let Some(parked) = self.fs().issues_parked.clone() {
            items = parked.items;
            cur = parked.cursor;
        }
        if let Some(validate) = self.opts().validate.clone() {
            items = self.live_issues(validate(&self.doc()));
            self.set_issues(items.clone());
        }
        self.fs_mut().issues_parked = None;
        if let Some(origin) = self.fs_mut().issues_origin.take() {
            self.fs_mut().stack = origin;
        }
        self.nav_back_step(leaving);
        if !items.is_empty() && cur > items.len() - 1 {
            cur = items.len() - 1;
        }
        self.overlay = Some(Overlay::Issues(IssuesState {
            items,
            cursor: cur,
            search_ti: TextInput::default(),
            searching: false,
            search_cursor: cur,
        }));
    }

    // --- search ------------------------------------------------------------

    /// openSearch arms the scoped search overlay (Go openSearch).
    pub fn open_search(&mut self, replace: bool) {
        let st = self.top().cloned();
        let all = match &st {
            Some(st) => {
                let f = &mut self.files[self.cur];
                search_index(&mut f.schema, &f.doc, st)
            }
            None => Vec::new(),
        };
        let mut ti = TextInput::new();
        ti.set_value("");
        let rti = TextInput::new();
        let mut s = SearchState {
            replace,
            fold_case: true,
            ti,
            rti,
            replace_focused: false,
            all,
            results: Vec::new(),
            cursor: 0,
            regex: true, // regex queries by default; alt+x falls back to literal
            re: None,
            re_err: String::new(),
            parked: false,
            return_depth: 0,
            origin: None,
        };
        if replace {
            self.refilter(&mut s);
        } else {
            s.results = s.all.clone();
        }
        self.fs_mut().search_parked = None; // a fresh search replaces a parked one
        self.overlay = Some(Overlay::Search(s));
    }

    /// closeSearch tears the overlay down, returning to the menu the search
    /// was opened from.
    fn close_search(&mut self, s: SearchState) {
        if let Some(origin) = s.origin {
            self.fs_mut().stack = origin;
        }
        self.fs_mut().search_parked = None;
    }

    /// searchReturnTo reopens the parked search overlay from its jump
    /// target.
    fn search_return_to(&mut self) {
        let leaving = self.nav_snapshot();
        let Some(mut s) = self.fs_mut().search_parked.take() else {
            return;
        };
        let cur = s.cursor;
        s.parked = false;
        s.return_depth = 0;
        if let Some(origin) = s.origin.take() {
            self.fs_mut().stack = origin;
        }
        self.nav_back_step(leaving);
        {
            let st = self.top().cloned();
            if let Some(st) = st {
                let f = &mut self.files[self.cur];
                s.all = search_index(&mut f.schema, &f.doc, &st);
            }
        }
        self.refilter(&mut s);
        let mut cur = cur;
        if cur > s.results.len().saturating_sub(1) {
            cur = s.results.len().saturating_sub(1);
        }
        s.cursor = cur;
        self.fs_mut().search_parked = None;
        self.overlay = Some(Overlay::Search(s));
    }

    /// refilter recomputes the result list from the current inputs (Go
    /// refilter).
    fn refilter(&mut self, s: &mut SearchState) {
        let q = s.ti.value().to_string();
        s.re = None;
        s.re_err = String::new();
        if s.regex && !q.is_empty() {
            let mut pattern = q.clone();
            if !s.replace && (is_path_query(&q) || is_dot_query(&q)) {
                pattern = path_pattern(&q);
            }
            if s.fold_case {
                pattern = format!("(?i){pattern}");
            }
            match Regex::new(&pattern) {
                Err(err) => {
                    // report the error against the typed query, not our wrappers
                    if let Err(raw_err) = Regex::new(&q) {
                        s.re_err = raw_err.to_string();
                    } else {
                        s.re_err = err.to_string();
                    }
                }
                Ok(re) => s.re = Some(re),
            }
        }
        if !s.re_err.is_empty() {
            s.results = Vec::new();
            s.cursor = 0;
            return;
        }
        let ctx = self.ctx();
        let rep = s.rti.value().to_string();
        if s.replace {
            s.results = replace_hits(&ctx, &s.all, &q, &rep, s.fold_case, s.re.as_ref());
        } else {
            s.results = filter_search(&ctx, &s.all, &q, s.fold_case, s.re.as_ref());
        }
        s.cursor = 0;
    }

    /// replaceSearchCurrent rewrites the highlighted hit's string value.
    fn replace_search_current(&mut self, s: &mut SearchState) {
        let q = s.ti.value().to_string();
        if q.is_empty() || s.results.is_empty() {
            return;
        }
        let res = s.results[s.cursor].clone();
        let Some(n) = self.doc().get(&res.path) else {
            return;
        };
        let Value::Str(old) = self.doc().value(n).clone() else {
            return;
        };
        let rep = s.rti.value().to_string();
        let (new, c) = replace_all(&old, &q, &rep, s.fold_case, s.re.as_ref());
        if c == 0 {
            return;
        }
        let v = self.fs_mut().doc.new_str(new);
        let _ = self.fs_mut().doc.set(&res.path, v);
        let cur = s.cursor;
        self.refilter(s);
        let mut cur = cur;
        if cur >= s.results.len() {
            cur = s.results.len().saturating_sub(1);
        }
        s.cursor = cur;
    }

    /// replaceSearchAll rewrites every occurrence in every string value of
    /// the scoped index, closes the overlay and leaves a count notice.
    fn replace_search_all(&mut self, s: SearchState) {
        let q = s.ti.value().to_string();
        if q.is_empty() || s.results.is_empty() {
            self.overlay = Some(Overlay::Search(s));
            return;
        }
        let rep = s.rti.value().to_string();
        let mut occ = 0usize;
        let mut vals = 0usize;
        let paths: Vec<Vec<PathSeg>> = s.results.iter().map(|r| r.path.clone()).collect();
        for p in paths {
            let Some(n) = self.doc().get(&p) else {
                continue;
            };
            let Value::Str(old) = self.doc().value(n).clone() else {
                continue;
            };
            let (new, c) = replace_all(&old, &q, &rep, s.fold_case, s.re.as_ref());
            if c == 0 {
                continue;
            }
            let v = self.fs_mut().doc.new_str(new);
            let _ = self.fs_mut().doc.set(&p, v);
            occ += c;
            vals += 1;
        }
        self.close_search(s);
        self.overlay = None;
        self.notice = format!("replaced {} occurrence(s) in {} value(s)", occ, vals);
    }

    // --- anchors -----------------------------------------------------------

    /// gotoAnchor performs ga: from a reference row, jump to its anchor
    /// carrier (Go gotoAnchor).
    fn goto_anchor(&mut self, r: &Row) {
        if !r.merged.is_empty() {
            // a merged key has no anchor identity of its own: ga lands on
            // the field that defines the value inside the anchor's mapping
            self.anchor_jump_row(r.path.clone());
            return;
        }
        let Some((name, is_ref)) = anchor_role(self.doc(), &r.path) else {
            self.notice = "not on an anchor or reference".to_string();
            return;
        };
        if !is_ref {
            self.notice = format!("already at the anchor &{}", name);
            return;
        }
        let Some(n) = self.doc().get(&r.path) else {
            return;
        };
        let target = anchor_holder_path(self.doc(), n).unwrap_or_default();
        self.anchor_jump(target);
    }

    /// gotoRefs performs gr (Go gotoRefs).
    fn goto_refs(&mut self, r: &Row) {
        let name;
        let n;
        if !r.merged.is_empty() {
            name = r.merged.clone();
            match anchor_holder(self.doc(), &name) {
                Some(node) => n = node,
                None => {
                    self.notice = format!("the anchor &{} is no longer in the document", name);
                    return;
                }
            }
        } else {
            match anchor_role(self.doc(), &r.path) {
                Some((nm, _)) => {
                    name = nm;
                    n = self
                        .doc()
                        .get(&r.path)
                        .expect("anchor role checked the value");
                }
                None => {
                    self.notice = "not on an anchor or reference".to_string();
                    return;
                }
            }
        }
        let holder = anchor_holder_path(self.doc(), n).unwrap_or_default();
        let here = self.top().map(|st| st.path.clone()).unwrap_or_default();
        let mut targets: Vec<Vec<PathSeg>> = Vec::new();
        for p in anchor_merge_use_paths(self.doc(), n) {
            if p == holder || p == r.path || p == here {
                continue;
            }
            targets.push(p);
        }
        match targets.len() {
            0 => self.notice = format!("no other references to &{}", name),
            1 => self.anchor_jump(targets.remove(0)),
            _ => {
                self.overlay = Some(Overlay::RefPick(RefPickState {
                    name,
                    paths: targets,
                    cursor: 0,
                    search_ti: TextInput::default(),
                    searching: false,
                    search_cursor: 0,
                }));
            }
        }
    }

    /// gAnchorKeys consumes the armed-g combos ga and gr.
    fn g_anchor_keys(&mut self, msg: &Msg, r: &Row) -> bool {
        if self.cmd_buf.is_empty() {
            return false;
        }
        match msg.key_str().as_str() {
            "a" => {
                self.cmd_buf.clear();
                self.goto_anchor(r);
                true
            }
            "r" => {
                self.cmd_buf.clear();
                self.goto_refs(r);
                true
            }
            _ => false,
        }
    }

    /// expandAlias materializes the reference at path (Go expandAlias).
    fn expand_alias(&mut self, path: &[PathSeg]) {
        let Some(n) = self.doc().get(path) else {
            return;
        };
        let cloned = self.fs_mut().doc.clone_detached(n);
        let _ = self.fs_mut().doc.detach(path, cloned);
        self.notice = "alias expanded: this value is now an independent copy".to_string();
    }

    /// expandTarget resolves what `e` materializes (Go expandTarget).
    fn expand_target(&self, r: &Row) -> Option<Vec<PathSeg>> {
        if matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown)
            && !r.path.is_empty()
        {
            if let Some((_, true)) = anchor_role(self.doc(), &r.path) {
                return Some(r.path.clone());
            }
        }
        let top_path = self.top().map(|st| st.path.clone()).unwrap_or_default();
        if let Some((_, true)) = anchor_role(self.doc(), &top_path) {
            return Some(top_path);
        }
        None
    }

    /// anchorJump parks the current position and jumps to target.
    fn anchor_jump(&mut self, target: Vec<PathSeg>) {
        self.anchor_jump_to(target, false);
    }

    /// anchorJumpRow lands on the target row itself.
    fn anchor_jump_row(&mut self, target: Vec<PathSeg>) {
        self.anchor_jump_to(target, true);
    }

    fn anchor_jump_to(&mut self, target: Vec<PathSeg>, as_row: bool) {
        self.nav_forward();
        self.fs_mut().jump_origin = Some(self.fs().stack.clone());
        self.fs_mut().search_parked = None; // a fresh jump replaces parked overlays
        self.fs_mut().issues_parked = None;
        if as_row {
            self.jump_to_row(&target);
        } else {
            self.jump_to_path(&target);
        }
        self.fs_mut().jump_return = self.fs().stack.len();
        self.notice = "esc returns".to_string();
    }

    /// openAnchor opens the name input behind & and * (Go openAnchor).
    fn open_anchor(&mut self, r: &Row, mode: AnchorMode) {
        if !matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            self.notice = "& and * work on value rows".to_string();
            return;
        }
        if r.path.is_empty() {
            self.notice = "no value here to anchor".to_string();
            return;
        }
        let mut ti = TextInput::new();
        let role = anchor_role(self.doc(), &r.path);
        let (name, _is_ref) = role.clone().map(|(n, ir)| (n, ir)).unwrap_or_default();
        match (&mode, role) {
            (AnchorMode::RefSet, Some((_, true))) | (AnchorMode::AnchorSet, Some(_)) => {
                // prefill the current name: rename for anchors, retarget for refs
                ti.set_value(name);
                ti.cursor_end();
            }
            _ => {}
        }
        let mut cands: Vec<String> = Vec::new();
        if mode == AnchorMode::RefSet {
            // completion offers only anchors the field can legally hold
            for d in anchor_defs(self.doc()) {
                if !schema_accepts_value_(&self.fs().schema, r.fn_, self.doc(), Some(d.node)) {
                    continue;
                }
                if ref_would_cycle(self.doc(), &r.path, d.node) {
                    continue;
                }
                cands.push(d.name);
            }
        }
        self.overlay = Some(Overlay::Anc(AnchorState {
            path: r.path.clone(),
            mode,
            fn_: r.fn_,
            ti,
            cands,
            sug: -1,
            err: String::new(),
        }));
    }

    /// anchorFiltered lists the reference candidates matching the input.
    fn anchor_filtered(s: &AnchorState) -> Vec<String> {
        if s.mode != AnchorMode::RefSet {
            return Vec::new();
        }
        let prefix = s.ti.value().trim().to_lowercase();
        if prefix.is_empty() {
            return s.cands.clone();
        }
        s.cands
            .iter()
            .filter(|c| c.to_lowercase().starts_with(&prefix))
            .cloned()
            .collect()
    }

    /// cycleAnchorSug moves the completion cursor over the candidates.
    fn cycle_anchor_sug(&mut self, s: &mut AnchorState, delta: i64) {
        if s.mode != AnchorMode::RefSet {
            return;
        }
        let cands = Self::anchor_filtered(s);
        if cands.is_empty() {
            return;
        }
        s.sug += delta;
        if s.sug < 0 {
            s.sug = cands.len() as i64 - 1;
        }
        if s.sug >= cands.len() as i64 {
            s.sug = 0;
        }
        s.ti.set_value(cands[s.sug as usize].clone());
        s.ti.cursor_end();
    }

    /// anchorCommit applies the & / * input (Go anchorCommit).
    fn anchor_commit(&mut self, s: AnchorState) -> Option<Overlay> {
        let name = s.ti.value().trim().to_string();
        if s.mode == AnchorMode::AnchorSet {
            self.commit_anchor_set(s, name)
        } else {
            self.commit_ref_set(s, name)
        }
    }

    fn commit_anchor_set(&mut self, mut s: AnchorState, name: String) -> Option<Overlay> {
        let Some(n) = self.doc().get(&s.path) else {
            s.err = "no value here".to_string();
            return Some(Overlay::Anc(s));
        };
        let anchor = self.doc().node(n).anchor.clone();
        if name.is_empty() {
            if anchor.is_empty() {
                s.err = "this value has no anchor".to_string();
                return Some(Overlay::Anc(s));
            }
            let uses = count_anchor_uses(self.doc(), n);
            if uses > 1 {
                s.err = format!(
                    "&{} is referenced by {} alias(es) — remove them first",
                    anchor,
                    uses - 1
                );
                return Some(Overlay::Anc(s));
            }
            if merge_ref_uses(self.doc(), n) > 0 {
                s.err = format!(
                    "&{} is referenced by a merge key — remove the merge first",
                    anchor
                );
                return Some(Overlay::Anc(s));
            }
            self.fs_mut().doc.node_mut(n).anchor = String::new();
            self.notice = format!("anchor &{} removed", anchor);
            return None;
        }
        if let Some(holder) = anchor_holder(self.doc(), &name) {
            if holder != n {
                s.err = format!("anchor name &{} is already in use", name);
                return Some(Overlay::Anc(s));
            }
        }
        let renamed = !anchor.is_empty() && anchor != name;
        self.fs_mut().doc.node_mut(n).anchor = name.clone();
        if renamed {
            self.notice = format!("anchor renamed to &{name} — references updated");
        } else {
            self.notice = format!("anchor set: &{name}");
        }
        None
    }

    fn commit_ref_set(&mut self, mut s: AnchorState, name: String) -> Option<Overlay> {
        if name.is_empty() {
            match anchor_role(self.doc(), &s.path) {
                Some((_, true)) => {}
                _ => {
                    s.err = "this value is not a reference".to_string();
                    return Some(Overlay::Anc(s));
                }
            }
            if let Some(f) = s.fn_ {
                if self.fs().schema.nodes[f as usize].required {
                    s.err = format!("{} is required", self.fs().schema.nodes[f as usize].title);
                    return Some(Overlay::Anc(s));
                }
            }
            self.fs_mut().doc.remove(&s.path);
            self.notice = "reference removed".to_string();
            return None;
        }
        let Some(target) = anchor_holder(self.doc(), &name) else {
            s.err = format!("unknown anchor: &{name}");
            return Some(Overlay::Anc(s));
        };
        if ref_would_cycle(self.doc(), &s.path, target) {
            s.err = "circular reference: the anchor contains this field".to_string();
            return Some(Overlay::Anc(s));
        }
        if !self.fs_mut().doc.detach(&s.path, target) {
            // the field holds no value yet: Set creates the slot
            let _ = self.fs_mut().doc.set(&s.path, target);
        }
        self.notice = format!("value references *{}", name);
        None
    }

    // --- merge keys --------------------------------------------------------

    /// openMerge opens the merge-keys input for the current mapping menu
    /// (Go openMerge).
    fn open_merge(&mut self) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        let node_kind = self.fs().schema.nodes[st.node as usize].kind;
        if st.kind != MenuKind::Object || !matches!(node_kind, Kind::Object | Kind::Map) {
            self.notice = "< edits the merge keys of a mapping menu".to_string();
            return;
        }
        // a value of another kind refuses (converting it would lose data)
        if let Some(cur) = self.doc().get(&st.path) {
            if !is_null_val(self.doc(), cur) && !matches!(self.doc().value(cur), Value::Object(_)) {
                self.notice = "no mapping here to merge into".to_string();
                return;
            }
        }
        let mut pre: Vec<String> = Vec::new();
        if let Some(cur) = self.doc().get(&st.path) {
            for r in self.doc().merge_refs(cur) {
                let anchor = self.doc().node(r).anchor.clone();
                if !anchor.is_empty() {
                    pre.push(anchor);
                }
            }
        }
        let mut ti = TextInput::new();
        if !pre.is_empty() {
            ti.set_value(pre.join(" "));
            ti.cursor_end();
        }
        let mut cands: Vec<String> = Vec::new();
        for d in anchor_defs(self.doc()) {
            if !matches!(self.doc().value(d.node), Value::Object(_)) {
                continue;
            }
            if self.merge_would_cycle(&st.path, d.node) {
                continue;
            }
            cands.push(d.name);
        }
        self.overlay = Some(Overlay::Mrg(MergeState {
            path: st.path.clone(),
            ti,
            cands,
            sug: -1,
            err: String::new(),
        }));
    }

    /// cycleMergeSug moves the completion cursor over the filtered
    /// candidates and fills the token being typed.
    fn cycle_merge_sug(&mut self, s: &mut MergeState, delta: i64) {
        let cands = merge_filtered(s);
        if cands.is_empty() {
            return;
        }
        s.sug += delta;
        if s.sug < 0 {
            s.sug = cands.len() as i64 - 1;
        }
        if s.sug >= cands.len() as i64 {
            s.sug = 0;
        }
        let (rest, _) = split_last_token(s.ti.value());
        s.ti.set_value(format!("{}{} ", rest, cands[s.sug as usize]));
        s.ti.cursor_end();
    }

    /// mergeCommit applies the `<` input (Go mergeCommit).
    fn merge_commit(&mut self, mut s: MergeState) -> Option<Overlay> {
        let names = merge_token_list(s.ti.value());
        if names.is_empty() {
            let has_merges = self
                .doc()
                .get(&s.path)
                .map(|n| !self.doc().merge_refs(n).is_empty())
                .unwrap_or(false);
            if !has_merges {
                s.err = "this mapping merges nothing".to_string();
                return Some(Overlay::Mrg(s));
            }
            self.apply_merges(&s.path, Vec::new());
            self.notice = "merge keys removed".to_string();
            return None;
        }
        let mut dedup: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for n in names {
            if seen.insert(n.clone()) {
                dedup.push(n);
            }
        }
        let self_node = self.doc().get(&s.path);
        let mut refs: Vec<NodeId> = Vec::new();
        for name in &dedup {
            let Some(target) = anchor_holder(self.doc(), name) else {
                s.err = format!("unknown anchor: &{name}");
                return Some(Overlay::Mrg(s));
            };
            if !matches!(self.doc().value(target), Value::Object(_)) {
                s.err = format!("&{name} is not a mapping anchor");
                return Some(Overlay::Mrg(s));
            }
            if Some(target) == self_node {
                s.err = "cannot merge the mapping into itself".to_string();
                return Some(Overlay::Mrg(s));
            }
            if ref_would_cycle(self.doc(), &s.path, target)
                || self.merge_would_cycle(&s.path, target)
            {
                s.err =
                    "circular reference: the anchor would merge back into this mapping".to_string();
                return Some(Overlay::Mrg(s));
            }
            refs.push(target);
        }
        // a not-yet-created mapping becomes an empty object first
        let cur = self.doc().get(&s.path);
        if is_null_opt(self.doc(), cur) {
            let n = self.fs_mut().doc.new_object();
            let _ = self.fs_mut().doc.set(&s.path, n);
        }
        self.apply_merges(&s.path, refs);
        self.notice = format!("merged keys from {}", dedup.join(" "));
        None
    }

    /// applyMerges replaces the mapping's merge-key members with one member
    /// merging refs (empty clears them).
    fn apply_merges(&mut self, path: &[PathSeg], refs: Vec<NodeId>) {
        let Some(cur) = self.doc().get(path) else {
            return;
        };
        if !matches!(self.doc().value(cur), Value::Object(_)) {
            return;
        }
        let mut idx: Option<usize> = None;
        if let Value::Object(mems) = self.doc().value(cur) {
            for (i, m) in mems.iter().enumerate() {
                if !self.doc().node(m.val).merge.is_empty() && idx.is_none() {
                    idx = Some(i);
                }
            }
        }
        let member = path_for(path, "<<");
        while self.fs_mut().doc.remove(&member) {}
        if refs.is_empty() {
            return;
        }
        let idx = idx.unwrap_or(0);
        let n = self.fs_mut().doc.push(NodeData {
            value: Value::Null,
            anchor: String::new(),
            merge: refs,
            raw: String::new(),
        });
        let _ = self.fs_mut().doc.insert_member(path, "<<", idx, n);
    }

    /// mergeWouldCycle reports whether merging target into the mapping at
    /// path would recurse (Go mergeWouldCycle).
    fn merge_would_cycle(&self, path: &[PathSeg], target: NodeId) -> bool {
        let Some(self_node) = self.doc().get(path) else {
            return false;
        };
        let mut seen: HashSet<NodeId> = HashSet::new();
        fn dfs(doc: &Doc, self_node: NodeId, n: NodeId, seen: &mut HashSet<NodeId>) -> bool {
            if !seen.insert(n) {
                return false;
            }
            if n == self_node {
                return true;
            }
            for r in doc.merge_refs(n) {
                if dfs(doc, self_node, r, seen) {
                    return true;
                }
            }
            false
        }
        dfs(self.doc(), self_node, target, &mut seen)
    }

    /// materializeMerged copies one merged key's value into the current
    /// mapping (Go materializeMerged).
    fn materialize_merged(&mut self, r: &Row) -> Row {
        let Some(st) = self.top() else {
            return r.clone();
        };
        if r.path.is_empty() {
            return r.clone();
        }
        let Some(PathSeg::Key(key)) = r.path.last() else {
            return r.clone();
        };
        let Some(val) = self.doc().get(&r.path) else {
            return r.clone();
        };
        let p = path_for(&st.path, key);
        let cloned = self.fs_mut().doc.clone_detached(val);
        let _ = self.fs_mut().doc.set(&p, cloned);
        self.notice = "merged key expanded: this value is now an independent copy".to_string();
        let mut out = r.clone();
        out.path = p;
        out.merged = String::new();
        out
    }

    /// expandMerged materializes the merged key under the cursor (E).
    fn expand_merged(&mut self, r: &Row) {
        self.materialize_merged(r);
    }

    // --- macros ------------------------------------------------------------

    /// macroControl consumes the macro keys (@ records, . plays) before
    /// normal dispatch (Go macroControl).
    pub fn macro_control(&mut self, msg: &Msg) -> bool {
        if self.typing() {
            return false; // typing: @/. are ordinary characters
        }
        match msg.key_str().as_str() {
            "@" => {
                if self.recording {
                    if self.rec_buf.is_empty() {
                        // cancel: the previous macro survives
                        self.rec_buf.clear();
                        self.recording = false;
                        self.notice = "recording cancelled (no keys captured)".to_string();
                        return true;
                    }
                    self.macro_buf = std::mem::take(&mut self.rec_buf);
                    self.recording = false;
                    self.notice =
                        format!("macro recorded: {} keys — . plays it", self.macro_buf.len());
                    return true;
                }
                self.rec_buf = Vec::with_capacity(16);
                self.recording = true;
                self.notice = "recording — @ stops, . is ignored while recording".to_string();
                true
            }
            "." => {
                if self.recording {
                    return true; // swallowed, not recorded
                }
                if self.macro_buf.is_empty() {
                    self.notice = "no macro recorded (@ records one)".to_string();
                    return true;
                }
                self.play_macro();
                true
            }
            _ => false,
        }
    }

    /// playMacro replays the recorded keys through dispatch_key (Go
    /// playMacro).
    fn play_macro(&mut self) {
        let n = self.macro_buf.len();
        for (i, k) in self.macro_buf.clone().into_iter().enumerate() {
            self.dispatch_key(k);
            if self.aborted || self.exiting {
                self.notice = format!("macro stopped at key {}/{} (session ended)", i + 1, n);
                return;
            }
        }
        self.notice = format!("played {} keys", n);
    }

    // --- overlay key handlers ----------------------------------------------

    /// keyConfirm drives the y/n overlay (Go keyConfirm).
    pub fn key_confirm(&mut self, msg: Msg, c: ConfirmState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "y" | "enter" | " " => {
                if let Some(on_yes) = c.on_yes {
                    on_yes(self);
                }
                // the callback may have opened another surface (the enum
                // picker of a chosen variant, the save-errors overlay):
                // keep whatever it installed, like Go's model fields.
                self.overlay.take()
            }
            "n" => {
                if let Some(on_no) = c.on_no {
                    on_no(self);
                }
                self.overlay.take()
            }
            "esc" => None,
            _ => Some(Overlay::Confirm(c)),
        }
    }

    /// keyHelp drives the "?" keyboard-help overlay.
    pub fn key_help(&mut self, msg: &Msg) {
        // title + blank line + bottom hint leave height-3 entry rows.
        let page = self.height.saturating_sub(3).max(1);
        let max_scroll = crate::ui::menu::HELP_ENTRIES.len().saturating_sub(page);
        match msg.key_str().as_str() {
            "esc" | "q" | "?" => {
                self.help = false;
                self.help_scroll = 0;
            }
            "up" | "k" => self.help_scroll = self.help_scroll.saturating_sub(1),
            "down" | "j" => self.help_scroll = (self.help_scroll + 1).min(max_scroll),
            "pgup" | "ctrl+b" => self.help_scroll = self.help_scroll.saturating_sub(page),
            "pgdown" | "ctrl+f" => self.help_scroll = (self.help_scroll + page).min(max_scroll),
            _ => {}
        }
    }

    /// keyErrs drives the save-time errors overlay (Go keyErrs).
    pub fn key_errs(&mut self, msg: Msg, mut s: ErrsState) -> Option<Overlay> {
        if s.searching {
            return self.key_errs_search(msg, s);
        }
        if self.jump_keys(&msg, &mut s.cursor, s.lines.len()) {
            return Some(Overlay::Errs(s));
        }
        match msg.key_str().as_str() {
            "/" => {
                s.search_cursor = s.cursor;
                s.search_ti = TextInput::default();
                s.searching = true;
            }
            "up" | "k" => s.cursor = wrap_cursor(s.cursor, -1, s.lines.len()),
            "down" | "j" => s.cursor = wrap_cursor(s.cursor, 1, s.lines.len()),
            "pgdown" | "ctrl+f" => {
                let page = self.errs_page_size(false);
                s.cursor = page_cursor(s.cursor, 1, page, s.lines.len());
            }
            "pgup" | "ctrl+b" => {
                let page = self.errs_page_size(false);
                s.cursor = page_cursor(s.cursor, -1, page, s.lines.len());
            }
            "enter" => {
                if let Some(Some(path)) = s.paths.get(s.cursor) {
                    let path = path.clone();
                    self.fs_mut().issues_parked = None; // a fresh jump replaces a parked list
                    self.nav_forward();
                    self.jump_to_path(&path);
                    return None;
                }
            }
            "y" => {
                if self.opts().force {
                    let exit = s.exit;
                    let target = s.target.clone();
                    self.write_to(exit, true, &target);
                    return None;
                }
            }
            "n" | "esc" => return None,
            _ => {}
        }
        Some(Overlay::Errs(s))
    }

    /// keyErrsSearch consumes keys while the save-error filter is active.
    fn key_errs_search(&mut self, msg: Msg, mut s: ErrsState) -> Option<Overlay> {
        match msg {
            Msg::Esc => {
                s.searching = false;
                s.search_ti = TextInput::default();
                s.cursor = s.search_cursor.min(s.lines.len().saturating_sub(1));
            }
            Msg::Enter => {
                let path = err_indices(&s)
                    .nth(s.cursor)
                    .and_then(|i| s.paths.get(i))
                    .and_then(|p| p.clone());
                if let Some(path) = path {
                    self.fs_mut().issues_parked = None; // a fresh jump replaces a parked list
                    self.nav_forward();
                    self.jump_to_path(&path);
                    return None;
                }
            }
            Msg::Up | Msg::BackTab | Msg::Ctrl('k') => {
                let n = err_indices(&s).count();
                s.cursor = wrap_cursor(s.cursor, -1, n);
            }
            Msg::Down | Msg::Tab | Msg::Ctrl('j') => {
                let n = err_indices(&s).count();
                s.cursor = wrap_cursor(s.cursor, 1, n);
            }
            Msg::PageDown | Msg::Ctrl('f') => {
                let n = err_indices(&s).count();
                let page = self.errs_page_size(true);
                s.cursor = page_cursor(s.cursor, 1, page, n);
            }
            Msg::PageUp | Msg::Ctrl('b') => {
                let n = err_indices(&s).count();
                let page = self.errs_page_size(true);
                s.cursor = page_cursor(s.cursor, -1, page, n);
            }
            _ => {
                let Some(key) = msg_key(&msg) else {
                    return Some(Overlay::Errs(s));
                };
                let before = s.search_ti.value().to_string();
                s.search_ti.handle_key(key);
                if s.search_ti.value() != before {
                    s.cursor = 0;
                }
            }
        }
        Some(Overlay::Errs(s))
    }

    fn errs_page_size(&self, searching: bool) -> usize {
        self.height
            .saturating_sub(2 + usize::from(searching))
            .max(1)
    }

    /// keyIssues drives the load-time issues overlay (Go keyIssues).
    pub fn key_issues(&mut self, msg: Msg, mut s: IssuesState) -> Option<Overlay> {
        if s.searching {
            return self.key_issues_search(msg, s);
        }
        if self.jump_keys(&msg, &mut s.cursor, s.items.len()) {
            return Some(Overlay::Issues(s));
        }
        match msg.key_str().as_str() {
            "/" if !s.items.is_empty() => {
                s.search_cursor = s.cursor;
                s.search_ti = TextInput::default();
                s.searching = true;
            }
            "up" | "k" | "ctrl+k" => s.cursor = wrap_cursor(s.cursor, -1, s.items.len()),
            "down" | "j" | "ctrl+j" => s.cursor = wrap_cursor(s.cursor, 1, s.items.len()),
            "pgdown" | "ctrl+f" => {
                let page = self.issues_page_size(false);
                s.cursor = page_cursor(s.cursor, 1, page, s.items.len());
            }
            "pgup" | "ctrl+b" => {
                let page = self.issues_page_size(false);
                s.cursor = page_cursor(s.cursor, -1, page, s.items.len());
            }
            "enter" => {
                let items = s.items.clone();
                if self.issues_jump(&items, s.cursor) {
                    return None;
                }
            }
            "esc" => return None,
            _ => {}
        }
        Some(Overlay::Issues(s))
    }

    /// keyIssuesSearch consumes keys while the issue-list filter is active.
    fn key_issues_search(&mut self, msg: Msg, mut s: IssuesState) -> Option<Overlay> {
        match msg {
            Msg::Esc => {
                s.searching = false;
                s.search_ti = TextInput::default();
                s.cursor = s.search_cursor.min(s.items.len().saturating_sub(1));
            }
            Msg::Enter => {
                let idx = issue_indices(&s).nth(s.cursor);
                if let Some(idx) = idx {
                    let items = s.items.clone();
                    if self.issues_jump(&items, idx) {
                        return None;
                    }
                }
            }
            Msg::Up | Msg::BackTab | Msg::Ctrl('k') => {
                let n = issue_indices(&s).count();
                s.cursor = wrap_cursor(s.cursor, -1, n);
            }
            Msg::Down | Msg::Tab | Msg::Ctrl('j') => {
                let n = issue_indices(&s).count();
                s.cursor = wrap_cursor(s.cursor, 1, n);
            }
            Msg::PageDown | Msg::Ctrl('f') => {
                let n = issue_indices(&s).count();
                let page = self.issues_page_size(true);
                s.cursor = page_cursor(s.cursor, 1, page, n);
            }
            Msg::PageUp | Msg::Ctrl('b') => {
                let n = issue_indices(&s).count();
                let page = self.issues_page_size(true);
                s.cursor = page_cursor(s.cursor, -1, page, n);
            }
            _ => {
                let Some(key) = msg_key(&msg) else {
                    return Some(Overlay::Issues(s));
                };
                let before = s.search_ti.value().to_string();
                s.search_ti.handle_key(key);
                if s.search_ti.value() != before {
                    s.cursor = 0;
                }
            }
        }
        Some(Overlay::Issues(s))
    }

    /// issuesJump parks the list and jumps to one issue's document path.
    fn issues_jump(&mut self, items: &[Issue], idx: usize) -> bool {
        let Some(path) = items.get(idx).and_then(|it| it.path.clone()) else {
            return false;
        };
        // closing the list (this park cycle or a later one) returns here
        self.fs_mut().issues_parked = Some(IssuesState {
            items: items.to_vec(),
            cursor: idx,
            search_ti: TextInput::default(),
            searching: false,
            search_cursor: idx,
        });
        self.fs_mut().issues_origin = Some(self.fs().stack.clone());
        self.fs_mut().search_parked = None; // a fresh jump replaces a parked search
        self.fs_mut().jump_origin = None;
        self.fs_mut().jump_return = 0;
        self.nav_forward();
        self.jump_to_path(&path);
        self.fs_mut().issues_return = self.fs().stack.len();
        true
    }

    /// issues_page_size mirrors issues_view's item area.
    fn issues_page_size(&self, searching: bool) -> usize {
        let mut height = self.height.saturating_sub(2 + usize::from(searching));
        if !self.notice.is_empty() {
            height = height.saturating_sub(1);
        }
        height.max(1)
    }

    /// keyRefPick drives the gr picker (Go keyRefPick).
    pub fn key_ref_pick(&mut self, msg: Msg, mut s: RefPickState) -> Option<Overlay> {
        if s.searching {
            return self.key_ref_search(msg, s);
        }
        if self.jump_keys(&msg, &mut s.cursor, s.paths.len()) {
            return Some(Overlay::RefPick(s));
        }
        match msg.key_str().as_str() {
            "/" => {
                s.search_cursor = s.cursor;
                s.search_ti = TextInput::default();
                s.searching = true;
            }
            "up" | "k" | "ctrl+k" => s.cursor = wrap_cursor(s.cursor, -1, s.paths.len()),
            "down" | "j" | "ctrl+j" => s.cursor = wrap_cursor(s.cursor, 1, s.paths.len()),
            "pgdown" | "ctrl+f" => {
                let page = self.refs_page_size(false);
                s.cursor = page_cursor(s.cursor, 1, page, s.paths.len());
            }
            "pgup" | "ctrl+b" => {
                let page = self.refs_page_size(false);
                s.cursor = page_cursor(s.cursor, -1, page, s.paths.len());
            }
            "enter" => {
                if let Some(path) = s.paths.get(s.cursor).cloned() {
                    self.anchor_jump(path);
                    return None;
                }
            }
            "esc" => return None,
            _ => {}
        }
        Some(Overlay::RefPick(s))
    }

    /// keyRefSearch consumes keys while the reference-picker filter is
    /// active.
    fn key_ref_search(&mut self, msg: Msg, mut s: RefPickState) -> Option<Overlay> {
        match msg {
            Msg::Esc => {
                s.searching = false;
                s.search_ti = TextInput::default();
                s.cursor = s.search_cursor.min(s.paths.len().saturating_sub(1));
            }
            Msg::Enter => {
                let path = self
                    .ref_indices(&s)
                    .nth(s.cursor)
                    .map(|i| s.paths[i].clone());
                if let Some(path) = path {
                    self.anchor_jump(path);
                    return None;
                }
            }
            Msg::Up | Msg::BackTab | Msg::Ctrl('k') => {
                let n = self.ref_indices(&s).count();
                s.cursor = wrap_cursor(s.cursor, -1, n);
            }
            Msg::Down | Msg::Tab | Msg::Ctrl('j') => {
                let n = self.ref_indices(&s).count();
                s.cursor = wrap_cursor(s.cursor, 1, n);
            }
            Msg::PageDown | Msg::Ctrl('f') => {
                let n = self.ref_indices(&s).count();
                let page = self.refs_page_size(true);
                s.cursor = page_cursor(s.cursor, 1, page, n);
            }
            Msg::PageUp | Msg::Ctrl('b') => {
                let n = self.ref_indices(&s).count();
                let page = self.refs_page_size(true);
                s.cursor = page_cursor(s.cursor, -1, page, n);
            }
            _ => {
                let Some(key) = msg_key(&msg) else {
                    return Some(Overlay::RefPick(s));
                };
                let before = s.search_ti.value().to_string();
                s.search_ti.handle_key(key);
                if s.search_ti.value() != before {
                    s.cursor = 0;
                }
            }
        }
        Some(Overlay::RefPick(s))
    }

    fn refs_page_size(&self, searching: bool) -> usize {
        self.height
            .saturating_sub(2 + usize::from(searching))
            .max(1)
    }

    /// ref_label renders one reference row: path plus value summary.
    fn ref_label(&self, p: &[PathSeg]) -> String {
        format!(
            "{}  {}",
            crate::doc::jsondoc::path_string(p),
            raw_value_summary(&self.fs().schema, self.doc(), p)
        )
    }

    /// ref_indices lists the reference paths matching the picker query.
    fn ref_indices<'a>(&'a self, s: &'a RefPickState) -> impl Iterator<Item = usize> + 'a {
        let q = s.search_ti.value().to_lowercase();
        s.paths
            .iter()
            .enumerate()
            .filter(move |(_, p)| q.is_empty() || self.ref_label(p).to_lowercase().contains(&q))
            .map(|(i, _)| i)
    }

    /// keyText drives the raw textarea overlay (Go keyText).
    pub fn key_text(&mut self, msg: Msg, mut s: TextState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "ctrl+s" => match self.apply_text(&s) {
                Ok(()) => return None,
                Err(e) => {
                    s.err = e;
                    return Some(Overlay::Text(s));
                }
            },
            "esc" => return None,
            _ => {}
        }
        if let Some(k) = msg_key(&msg) {
            s.ta.handle_key(k);
        }
        Some(Overlay::Text(s))
    }

    /// applyText parses the textarea and applies it (Go applyText). An error
    /// leaves the overlay open with the message.
    fn apply_text(&mut self, s: &TextState) -> Result<(), String> {
        let text = s.ta.value();
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            match &s.path {
                None => return Err("cannot be empty".to_string()),
                Some(p) => {
                    self.fs_mut().doc.remove(p);
                    return Ok(());
                }
            }
        }
        if s.verbatim_str {
            if let Some(p) = &s.path {
                let n = self.fs_mut().doc.new_str(text);
                let _ = self.fs_mut().doc.set(p, n);
            }
            return Ok(());
        }
        let label = self.opts().raw.raw_label();
        let decoded = match &s.path {
            // The whole-document editor must use the document codec: it
            // adopts format metadata (comments/directives) and accepts the
            // file's full grammar (a TOML document is not a TOML value).
            None => self.opts().raw.decode_raw_doc(&trimmed),
            Some(_) => self.opts().raw.decode_raw_value(&trimmed),
        };
        match decoded {
            Err(e) => Err(format!("invalid {}: {}", label, e)),
            Ok(nd) => {
                let root = nd.root;
                match &s.path {
                    None => {
                        self.fs_mut().doc = nd;
                        if self.fs().schema.schema_less {
                            self.rebuild_schema_less_root();
                        }
                    }
                    Some(p) => {
                        let n = copy_from_doc(&mut self.fs_mut().doc, &nd, root);
                        let _ = self.fs_mut().doc.set(p, n);
                    }
                }
                Ok(())
            }
        }
    }

    /// keySel drives the enum option picker (Go keySel).
    pub fn key_sel(&mut self, msg: Msg, mut s: SelState) -> Option<Overlay> {
        if s.searching {
            return self.key_sel_search(msg, s);
        }
        let matches = sel_match_count(&s);
        if self.jump_keys(&msg, &mut s.cursor, matches) {
            return Some(Overlay::Sel(s));
        }
        match msg.key_str().as_str() {
            "/" => {
                // empty query; the cursor keeps its current row
                s.search_cursor = s.cursor;
                s.search_ti = TextInput::default();
                s.searching = true;
            }
            "t" => {
                if let Some(one_of) = s.one_of {
                    let path = s.path.clone();
                    self.push(one_of, path);
                    return None;
                }
                return Some(Overlay::Sel(s));
            }
            "up" | "k" => s.cursor = wrap_cursor(s.cursor, -1, sel_match_count(&s)),
            "down" | "j" => s.cursor = wrap_cursor(s.cursor, 1, sel_match_count(&s)),
            "enter" => {
                if self.apply_sel(&s) {
                    return None;
                }
            }
            "esc" => return None,
            _ => {}
        }
        Some(Overlay::Sel(s))
    }

    /// keySelSearch consumes keys while the enum picker's search input is
    /// active. It follows the schema/oneOf search: plain characters edit the
    /// query, up/down or tab move the filtered list, enter selects, and esc
    /// leaves the search without closing the picker.
    fn key_sel_search(&mut self, msg: Msg, mut s: SelState) -> Option<Overlay> {
        match msg {
            Msg::Esc => {
                s.searching = false;
                s.search_ti = TextInput::default();
                s.cursor = s.search_cursor.min(s.opts.len().saturating_sub(1));
            }
            Msg::Enter => {
                // no match: stay in search mode so backspace can undo it
                if self.apply_sel(&s) {
                    return None;
                }
            }
            Msg::Up | Msg::BackTab | Msg::Ctrl('k') => {
                s.cursor = wrap_cursor(s.cursor, -1, sel_match_count(&s));
            }
            Msg::Down | Msg::Tab | Msg::Ctrl('j') => {
                s.cursor = wrap_cursor(s.cursor, 1, sel_match_count(&s));
            }
            _ => {
                let Some(key) = msg_key(&msg) else {
                    return Some(Overlay::Sel(s));
                };
                let before = s.search_ti.value().to_string();
                s.search_ti.handle_key(key);
                if s.search_ti.value() != before {
                    s.cursor = 0;
                }
            }
        }
        Some(Overlay::Sel(s))
    }

    /// applySel writes the option under the picker cursor. It reports
    /// whether a (matching) option was selected.
    fn apply_sel(&mut self, s: &SelState) -> bool {
        let Some(opt) = sel_option_at(s, s.cursor) else {
            return false;
        };
        let val = opt.val;
        let path = s.path.clone();
        let fi = self.cur;
        let f = &mut self.files[fi];
        let n = copy_from_doc(&mut f.doc, &f.schema.doc, val);
        let _ = f.doc.set(&path, n);
        true
    }

    /// keyEdit drives the in-place scalar input (Go keyEdit).
    pub fn key_edit(&mut self, msg: Msg, mut e: EditState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => {
                let node: FormNode = match (&e.fn_, &e.synth) {
                    (Some(_), _) | (_, Some(_)) => {
                        e.synth.clone().unwrap_or_else(|| FormNode::default())
                    }
                    _ => FormNode::default(),
                };
                // resolve the schema node when the edit targets one
                let node = match e.fn_ {
                    Some(id) => self.fs().schema.nodes[id as usize].clone(),
                    None => node,
                };
                let val = e.ti.value().to_string();
                let path = e.path.clone();
                let res = {
                    let f = &mut self.files[self.cur];
                    apply_scalar(&mut f.doc, &node, &f.schema.doc, &path, &val)
                };
                match res {
                    Err(err) => {
                        e.err = err;
                        return Some(Overlay::Edit(e));
                    }
                    Ok(()) => return None,
                }
            }
            "esc" => return None,
            "tab" | "ctrl+n" | "shift+tab" | "ctrl+p" => {
                if e.sugs.is_empty() {
                    return Some(Overlay::Edit(e));
                }
                match msg.key_str().as_str() {
                    "shift+tab" | "ctrl+p" => {
                        if e.sug_idx < 0 {
                            e.sug_idx = e.sugs.len() as i64 - 1;
                        } else {
                            e.sug_idx = wrap_cursor(e.sug_idx as usize, -1, e.sugs.len()) as i64;
                        }
                    }
                    _ => {
                        // Go's wrapCursor(-1, 1, n) starts at the first
                        // candidate (menu_keys.go:1549): -1 is "nothing
                        // selected", not "before 0".
                        e.sug_idx = if e.sug_idx < 0 {
                            0
                        } else {
                            wrap_cursor(e.sug_idx as usize, 1, e.sugs.len()) as i64
                        };
                    }
                }
                e.ti.set_value(e.sugs[e.sug_idx as usize].clone());
                e.ti.cursor_end();
                return Some(Overlay::Edit(e));
            }
            _ => {}
        }
        if let Some(k) = msg_key(&msg) {
            e.ti.handle_key(k);
        }
        self.overlay = Some(Overlay::Edit(e));
        self.refresh_edit_suggestions();
        // refresh_edit_suggestions mutates the overlay we just set in place;
        // hand it back to route_key (which assigns the return value).
        self.overlay.take()
    }

    /// keyAdd drives the key-name input (Go keyAdd).
    pub fn key_add(&mut self, msg: Msg, mut s: AddState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => {
                let k = s.ti.value().trim().to_string();
                if k.is_empty() {
                    s.err = "key required".to_string();
                    return Some(Overlay::Add(s));
                }
                let node = &self.fs().schema.nodes[s.node as usize].clone();
                for c in &node.children {
                    if self.fs().schema.nodes[*c as usize].key == k {
                        s.err = "key defined by the schema — edit its row instead".to_string();
                        return Some(Overlay::Add(s));
                    }
                }
                if let Some(cur) = self.doc().get(&s.path) {
                    if let Value::Object(mems) = self.doc().value(cur) {
                        for m in mems {
                            if m.key == k {
                                s.err = "key already exists".to_string();
                                return Some(Overlay::Add(s));
                            }
                        }
                    }
                }
                if node.kind == Kind::Map && !node.key_pattern.is_empty() {
                    match Regex::new(&node.key_pattern) {
                        Ok(re) if re.is_match(&k) => {}
                        _ => {
                            s.err = format!("key must match {}", node.key_pattern);
                            return Some(Overlay::Add(s));
                        }
                    }
                }
                let path = path_for(&s.path, &k);
                let idx = s.idx;
                // the key exists from here on: insert a null placeholder so
                // it keeps the requested position while the type is chosen
                let n = self.fs_mut().doc.new_null();
                let _ = self.fs_mut().doc.insert_member(&s.path, &k, idx, n);
                if node.kind == Kind::Map {
                    // the new value edits against the map's value schema
                    let items = node.items;
                    match items.map(|i| self.fs().schema.nodes[i as usize].kind) {
                        None => self.open_any_type(path),
                        Some(Kind::Raw) => {
                            self.open_field(items.expect("checked above"), path, true)
                        }
                        Some(Kind::OneOf) => {
                            let items = items.expect("checked above");
                            if dynamic_variants(&self.fs().schema, items).is_some() {
                                self.nav_forward();
                                let st = self.dynamic_menu(items, path);
                                self.fs_mut().stack.push(st);
                            } else {
                                self.push(items, path);
                            }
                        }
                        Some(Kind::Object) => {
                            let n = self.fs_mut().doc.new_object();
                            let _ = self.fs_mut().doc.set(&path, n);
                            self.push(items.expect("checked above"), path);
                        }
                        Some(Kind::Array) => {
                            let n = self.fs_mut().doc.new_array();
                            let _ = self.fs_mut().doc.set(&path, n);
                            self.push(items.expect("checked above"), path);
                        }
                        Some(Kind::Boolean) => {
                            let mut b = false;
                            if let Some(d) = node
                                .items
                                .and_then(|i| self.fs().schema.nodes[i as usize].default_)
                            {
                                if let Value::Bool(v) = self.fs().schema.doc.value(d) {
                                    b = *v;
                                }
                            }
                            let n = self.fs_mut().doc.new_bool(b);
                            let _ = self.fs_mut().doc.set(&path, n);
                        }
                        Some(_) => {
                            self.open_field(items.expect("checked above"), path, true);
                        }
                    }
                    return None;
                }
                // schema-less object member: choose the value type first
                self.open_any_type(path);
                None
            }
            "esc" => None,
            _ => {
                if let Some(k) = msg_key(&msg) {
                    s.ti.handle_key(k);
                }
                Some(Overlay::Add(s))
            }
        }
    }

    /// keyWrite drives the w/W write-to-path input (Go keyWrite).
    pub fn key_write(&mut self, msg: Msg, mut w: WriteState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => {
                let path = w.ti.value().trim().to_string();
                if path.is_empty() {
                    w.err = "path required".to_string();
                    return Some(Overlay::Write(w));
                }
                if path == self.opts().path {
                    // unedited: a plain save of the session file
                    self.validate_and_write(false, "");
                } else {
                    self.validate_and_write(false, &path);
                }
                // validate_and_write may have opened the errors overlay;
                // keep whatever it installed (Go's m.errs survives).
                self.overlay.take()
            }
            "tab" | "ctrl+n" | "shift+tab" | "ctrl+p" => {
                if w.sugs.is_empty() {
                    return Some(Overlay::Write(w));
                }
                match msg.key_str().as_str() {
                    "shift+tab" | "ctrl+p" => {
                        if w.sug_idx < 0 {
                            w.sug_idx = w.sugs.len() as i64 - 1;
                        } else {
                            w.sug_idx = wrap_cursor(w.sug_idx as usize, -1, w.sugs.len()) as i64;
                        }
                    }
                    _ => {
                        // Go's wrapCursor(-1, 1, n) starts at the first
                        // candidate (menu_keys.go:1198).
                        w.sug_idx = if w.sug_idx < 0 {
                            0
                        } else {
                            wrap_cursor(w.sug_idx as usize, 1, w.sugs.len()) as i64
                        };
                    }
                }
                w.ti.set_value(w.sugs[w.sug_idx as usize].clone());
                w.ti.cursor_end();
                Some(Overlay::Write(w))
            }
            "esc" => None,
            _ => {
                w.err = String::new();
                if let Some(k) = msg_key(&msg) {
                    w.ti.handle_key(k);
                }
                self.overlay = Some(Overlay::Write(w));
                self.refresh_write_suggestions();
                // refresh_write_suggestions edits the overlay in place;
                // hand it back so route_key's assignment keeps the input
                // open (Go returns the model with m.write still set).
                self.overlay.take()
            }
        }
    }

    /// refreshWriteSuggestions rebuilds the path candidates for the
    /// write-to value (Go refreshWriteSuggestions).
    pub fn refresh_write_suggestions(&mut self) {
        let Some(Overlay::Write(w)) = &mut self.overlay else {
            return;
        };
        w.sug_idx = -1;
        w.sugs.clear();
        let value = w.ti.value().to_string();
        let (dir, prefix) = match value.rfind('/') {
            Some(i) => (value[..=i].to_string(), value[i + 1..].to_string()),
            None => (String::new(), value.clone()),
        };
        let read = if dir.is_empty() {
            ".".to_string()
        } else {
            dir.clone()
        };
        let Ok(entries) = std::fs::read_dir(&read) else {
            return;
        };
        let lower = prefix.to_lowercase();
        for e in entries.flatten() {
            let mut name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && !prefix.starts_with('.') {
                continue;
            }
            if !name.to_lowercase().starts_with(&lower) {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                || std::fs::metadata(e.path())
                    .map(|m| m.is_dir())
                    .unwrap_or(false);
            if is_dir {
                name.push('/');
            }
            w.sugs.push(format!("{}{}", dir, name));
        }
    }

    /// keyAnchor drives the & / * name input (Go keyAnchor).
    pub fn key_anchor(&mut self, msg: Msg, mut s: AnchorState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => return self.anchor_commit(s),
            "esc" => return None,
            "tab" | "ctrl+n" => {
                self.cycle_anchor_sug(&mut s, 1);
                return Some(Overlay::Anc(s));
            }
            "shift+tab" | "ctrl+p" => {
                self.cycle_anchor_sug(&mut s, -1);
                return Some(Overlay::Anc(s));
            }
            _ => {}
        }
        s.sug = -1;
        if let Some(k) = msg_key(&msg) {
            s.ti.handle_key(k);
        }
        Some(Overlay::Anc(s))
    }

    /// keyMerge drives the merge-keys input (Go keyMerge).
    pub fn key_merge(&mut self, msg: Msg, mut s: MergeState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "enter" => return self.merge_commit(s),
            "esc" => return None,
            "tab" | "ctrl+n" => {
                self.cycle_merge_sug(&mut s, 1);
                return Some(Overlay::Mrg(s));
            }
            "shift+tab" | "ctrl+p" => {
                self.cycle_merge_sug(&mut s, -1);
                return Some(Overlay::Mrg(s));
            }
            _ => {}
        }
        s.sug = -1;
        if let Some(k) = msg_key(&msg) {
            s.ti.handle_key(k);
        }
        Some(Overlay::Mrg(s))
    }

    /// keySearch drives the scoped search overlay (Go keySearch).
    pub fn key_search(&mut self, msg: Msg, mut s: SearchState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "up" | "ctrl+k" => {
                s.cursor = wrap_cursor(s.cursor, -1, s.results.len());
                return Some(Overlay::Search(s));
            }
            "down" | "ctrl+j" => {
                s.cursor = wrap_cursor(s.cursor, 1, s.results.len());
                return Some(Overlay::Search(s));
            }
            "pgdown" | "ctrl+f" => {
                let page = self.search_page_size(&s);
                s.cursor = page_cursor(s.cursor, 1, page, s.results.len());
                return Some(Overlay::Search(s));
            }
            "pgup" | "ctrl+b" => {
                let page = self.search_page_size(&s);
                s.cursor = page_cursor(s.cursor, -1, page, s.results.len());
                return Some(Overlay::Search(s));
            }
            "tab" | "shift+tab" => {
                s.replace = true;
                s.replace_focused = !s.replace_focused;
                self.refilter(&mut s);
                return Some(Overlay::Search(s));
            }
            "alt+i" => {
                s.fold_case = !s.fold_case;
                self.refilter(&mut s);
                return Some(Overlay::Search(s));
            }
            "alt+x" => {
                s.regex = !s.regex;
                self.refilter(&mut s);
                return Some(Overlay::Search(s));
            }
            "ctrl+a" => {
                if s.replace {
                    self.replace_search_all(s);
                    // replace_search_all clears the overlay on a real
                    // replace and reinstalls it when there is nothing to
                    // do; keep that state (Go returns m unchanged).
                    return self.overlay.take();
                }
            }
            "enter" => {
                if s.replace && s.replace_focused {
                    self.replace_search_current(&mut s);
                    return Some(Overlay::Search(s));
                }
                if s.results.is_empty() {
                    return Some(Overlay::Search(s));
                }
                let res = s.results[s.cursor].clone();
                // park the overlay: esc at the landing reopens it
                s.parked = true;
                s.origin = Some(self.fs().stack.clone());
                self.fs_mut().issues_parked = None; // a fresh jump replaces a parked list
                self.fs_mut().jump_origin = None;
                self.fs_mut().jump_return = 0;
                self.nav_forward();
                // A variant key is a row in its parent menu: jumping to it
                // must stop there, not rebuild the oneOf picker underneath.
                let jump_path = {
                    let f = &self.files[self.cur];
                    match res.fn_ {
                        Some(fn_)
                            if f.schema.nodes[fn_ as usize].kind == Kind::OneOf
                                && matches!(res.kind, RowKind::Field | RowKind::Drill) =>
                        {
                            res.path[..res.path.len().saturating_sub(1)].to_vec()
                        }
                        _ => res.path.clone(),
                    }
                };
                let stack = {
                    let f = &mut self.files[self.cur];
                    rebuild_stack(&mut f.schema, &f.doc, &jump_path)
                };
                self.fs_mut().stack = stack;
                let rows = self.menu_rows();
                self.clamp_cursor(&rows);
                let mut target =
                    cursor_for_row(&rows, &res).or_else(|| self.enum_array_cursor(&rows, &res));
                // A variant picker is seeded on the value's variant; its rows
                // all sit at the picker path, so the ancestor fallback would
                // reset that cursor to row 0.
                if target.is_none() && self.top().map(|st| st.kind) != Some(MenuKind::OneOf) {
                    target = cursor_ancestor(&rows, &res.path);
                }
                if let Some(idx) = target {
                    if let Some(st) = self.top_mut() {
                        st.cursor = idx;
                        st.offset = 0;
                    }
                }
                s.return_depth = self.fs().stack.len();
                self.fs_mut().search_parked = Some(s);
                return None;
            }
            "esc" => {
                // replace mode is one level deeper: esc drops back to the
                // plain search list; the next esc closes the overlay.
                if s.replace {
                    s.replace = false;
                    s.replace_focused = false;
                    let cur = s.cursor;
                    self.refilter(&mut s);
                    s.cursor = cur.min(s.results.len().saturating_sub(1));
                    return Some(Overlay::Search(s));
                }
                self.close_search(s);
                return None;
            }
            _ => {}
        }
        if let Some(k) = msg_key(&msg) {
            if s.replace_focused {
                s.rti.handle_key(k);
            } else {
                s.ti.handle_key(k);
            }
        }
        self.refilter(&mut s);
        Some(Overlay::Search(s))
    }

    /// enum_array_cursor finds the toggle row a search hit points at: an
    /// enum-array menu has one toggle per schema option, so the document
    /// element's value picks the row (not the element's index).
    fn enum_array_cursor(&self, rows: &[Row], res: &Row) -> Option<usize> {
        let st = self.top()?;
        if st.kind != MenuKind::EnumArray {
            return None;
        }
        let schema = &self.fs().schema;
        let items = schema.nodes[st.node as usize].items?;
        let doc = self.doc();
        let val = doc.get(&res.path)?;
        let oi = schema.nodes[items as usize]
            .enum_
            .iter()
            .position(|o| nodes_equal(&schema.doc, Some(o.val), doc, Some(val)))?;
        rows.iter()
            .position(|r| r.kind == RowKind::Toggle && r.idx == oi as i64)
    }

    /// search_page_size mirrors render_search_body's result area.
    fn search_page_size(&self, s: &SearchState) -> usize {
        let inputs = if s.replace { 2 } else { 1 };
        self.height
            .saturating_sub(2 + inputs + self.status_lines().len() + 1)
            .max(1)
    }

    // --- external $EDITOR ---------------------------------------------------

    /// editRawExternal writes the whole document to a temp file and arms
    /// $EDITOR (Go editRawExternal). Used by E at the root.
    fn edit_raw_external(&mut self) {
        let raw = self.opts().raw.clone();
        match raw.encode_raw_doc(self.doc()) {
            Err(e) => self.notice = format!("editor: {e}"),
            Ok(data) => {
                self.launch_external_edit(data, None, true, -1);
            }
        }
    }

    /// editValueExternal edits one value in $EDITOR (Go editValueExternal).
    fn edit_value_external(&mut self, path: Vec<PathSeg>, insert_idx: i64) {
        let mut buf = String::new();
        if let Some(cur) = self.doc().get(&path) {
            if !is_null_val(self.doc(), cur) {
                let raw = self.opts().raw.clone();
                match raw.encode_raw_value(self.doc(), cur) {
                    Err(e) => {
                        self.notice = format!("editor: {e}");
                        return;
                    }
                    Ok(s) => buf = s,
                }
            }
        }
        self.launch_external_edit(buf, Some(path), false, insert_idx);
    }

    /// run_pending_editor runs the armed $EDITOR and applies its output (Go
    /// applyExternalEdit + the exec path). In tests (exec_editor=false) the
    /// pending edit is stashed so `finish_external_value` can deliver the
    /// content instead.
    pub fn run_pending_editor(&mut self) {
        if !self.exec_editor {
            return;
        }
        let Some(edit) = self.pending_edit.clone() else {
            return;
        };
        self.pending_edit = None;
        let editor = std::env::var("EDITOR")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("VISUAL").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "vi".to_string());
        // sh -c keeps EDITOR values with arguments ("code --wait") working
        // while the temp path is passed as a quoted positional argument.
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!("{} \"$1\"", editor))
            .arg("sh")
            .arg(&edit.path);
        let edit_path = edit.path.clone();
        // suspend the terminal for the editor
        {
            let mut stdout = std::io::stdout();
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(
                stdout,
                crossterm::cursor::Show,
                crossterm::event::DisableBracketedPaste,
                crossterm::terminal::LeaveAlternateScreen
            );
        }
        let status = cmd.status();
        {
            let mut stdout = std::io::stdout();
            let _ = crossterm::terminal::enable_raw_mode();
            let _ = crossterm::execute!(
                stdout,
                crossterm::terminal::EnterAlternateScreen,
                crossterm::event::EnableBracketedPaste,
                crossterm::cursor::Hide
            );
        }
        match status {
            Err(e) => self.notice = format!("editor: {e}"),
            Ok(st) if !st.success() => self.notice = format!("editor: exited with {st}"),
            Ok(_) => {
                if let Ok(data) = std::fs::read(&edit.path) {
                    self.apply_external(edit, &data);
                } else {
                    self.notice = "editor: temp file unreadable".to_string();
                }
            }
        }
        let _ = std::fs::remove_file(&edit_path);
    }

    /// editExternalJSON writes data to a temp file and arms the editor.
    fn launch_external_edit(
        &mut self,
        data: String,
        target: Option<Vec<PathSeg>>,
        whole_doc: bool,
        insert_idx: i64,
    ) {
        let ext = self.opts().raw.raw_ext();
        // uniquify: pid + a counter would race across processes; use nanos
        let path = std::env::temp_dir().join(format!(
            "jse-{}-{}{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            ext
        ));
        if let Err(e) = std::fs::write(&path, data) {
            self.notice = format!("editor: {e}");
            return;
        }
        let edit = ExternalEdit {
            target: target.unwrap_or_default(),
            insert_idx,
            path: path.to_string_lossy().to_string(),
            whole_doc,
        };
        self.pending_edit = Some(edit);
    }

    /// rebuildSchemaLessRoot re-derives the runtime root after a whole-document
    /// raw edit swapped the root type (object ↔ array ↔ scalar): the
    /// schema-less model is a snapshot of the loaded shape, so a stale root
    /// would offer the wrong menu. Synth nodes and the stack are rebuilt
    /// against the new model.
    fn rebuild_schema_less_root(&mut self) {
        let f = self.fs_mut();
        f.schema = crate::smodel::untyped(&f.doc);
        let root = f.schema.root;
        let kind = crate::ui::rows::menu_kind_for(&f.schema, root);
        f.menu_cache.clear();
        f.stack.truncate(1);
        if let Some(st) = f.stack.first_mut() {
            st.kind = kind;
            st.node = root;
            st.path.clear();
            st.cursor = 0;
            st.offset = 0;
            st.variant_of = None;
            st.one_of = None;
            st.sel.clear();
            st.visual = false;
            st.vanchor = 0;
            st.search_ti = Default::default();
            st.searching = false;
            st.search_cursor = 0;
        }
    }

    /// applyExternalEdit parses the editor's temp file and applies it (Go
    /// applyExternalEdit / applyExternalValue).
    fn apply_external(&mut self, edit: ExternalEdit, data: &[u8]) {
        if edit.whole_doc {
            let pre = self.doc().clone();
            let raw = self.opts().raw.clone();
            let label = raw.raw_label();
            match raw.decode_raw_doc(&String::from_utf8_lossy(data)) {
                Err(e) => {
                    self.notice = format!("editor: invalid {}: {}", label, e);
                }
                Ok(nd) => {
                    if nodes_equal_doc(&pre, &nd) {
                        self.notice = format!("Raw {} unchanged", label);
                    } else {
                        self.fs_mut().doc = nd;
                        self.fs_mut().history.push_undo(pre);
                        if self.fs().schema.schema_less {
                            self.rebuild_schema_less_root();
                        }
                        self.refresh_variant_menu();
                        self.refresh_issues();
                        let rows = self.menu_rows();
                        self.clamp_cursor(&rows);
                        self.notice = format!("Raw {} edited", label);
                    }
                }
            }
            return;
        }
        // one value
        let s = String::from_utf8_lossy(data).trim().to_string();
        let pre = self.doc().clone();
        if s.is_empty() {
            if edit.insert_idx < 0 {
                self.fs_mut().doc.remove(&edit.target);
            }
            if nodes_equal_doc(&pre, &self.fs().doc) {
                self.notice = "Value unchanged".to_string();
                return;
            }
            self.fs_mut().history.push_undo(pre);
            self.after_external_value_edit();
            self.notice = "Value removed".to_string();
            return;
        }
        let raw = self.opts().raw.clone();
        let label = raw.raw_label();
        match raw.decode_raw_value(&s) {
            Err(e) => {
                self.notice = format!("editor: invalid {}: {}", label, e);
            }
            Ok(nd) => {
                let n = nd.root;
                if edit.insert_idx >= 0 {
                    // positioned add (a/A): splice the new member at its index
                    let parent = edit.target[..edit.target.len() - 1].to_vec();
                    let Some(PathSeg::Key(key)) = edit.target.last() else {
                        return;
                    };
                    let val = copy_from_doc(&mut self.fs_mut().doc, &nd, n);
                    let _ = self.fs_mut().doc.insert_member(
                        &parent,
                        key,
                        edit.insert_idx as usize,
                        val,
                    );
                } else {
                    let val = copy_from_doc(&mut self.fs_mut().doc, &nd, n);
                    let _ = self.fs_mut().doc.set(&edit.target, val);
                }
                if nodes_equal_doc(&pre, &self.fs().doc) {
                    self.notice = "Value unchanged".to_string();
                    return;
                }
                self.fs_mut().history.push_undo(pre);
                self.after_external_value_edit();
                self.notice = "Value edited".to_string();
            }
        }
    }

    fn after_external_value_edit(&mut self) {
        self.refresh_variant_menu();
        self.refresh_issues();
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
    }

    /// The test path of the external editor: write content to the pending
    /// temp file and apply it like the editor would (Go tests'
    /// finishExternalValue + Update(rawEditorDoneMsg)).
    #[cfg(test)]
    #[allow(dead_code)] // the test seam for the $EDITOR path
    pub fn finish_external_value(&mut self, content: &str) {
        let Some(edit) = self.pending_edit.clone() else {
            panic!("no pending external edit");
        };
        self.pending_edit = None;
        self.apply_external(edit, content.as_bytes());
    }

    /// The editor run failed (test path: the rawEditorDoneMsg error case).
    #[cfg(test)]
    #[allow(dead_code)] // the test seam for the $EDITOR path
    pub fn external_edit_failed(&mut self, err: &str) {
        self.pending_edit = None;
        self.notice = format!("editor: {err}");
    }

    // --- rendering (Go menu.go View layer) ----------------------------------

    /// View renders the active surface (Go View).
    pub fn view(&mut self) -> String {
        if self.help {
            return self.help_view();
        }
        // The search overlay renders inline and needs &mut self (its
        // breadcrumb may synthesize menu rows), so it is taken out before the
        // read-only dispatch below.
        if matches!(&self.overlay, Some(Overlay::Search(_))) {
            if let Some(Overlay::Search(s)) = self.overlay.take() {
                let out = self.list_view_search(&s);
                self.overlay = Some(Overlay::Search(s));
                return out;
            }
        }
        match &self.overlay {
            Some(Overlay::Confirm(c)) => return self.confirm_view(c),
            Some(Overlay::Issues(s)) => return self.issues_view(s),
            Some(Overlay::Errs(s)) => return self.errs_view(s),
            Some(Overlay::RefPick(s)) => return self.ref_pick_view(s),
            Some(Overlay::Text(s)) => return self.text_view(s),
            Some(Overlay::CommentEdit(s)) => return self.comment_edit_view(s),
            Some(Overlay::Sel(s)) => return self.sel_view(s),
            Some(Overlay::Comment(s)) => return self.comment_view(s),
            _ => {}
        }
        if self.file_pick.is_some() {
            return self.file_pick_view();
        }
        self.list_view()
    }

    /// breadcrumb renders the navigation context as a document path.
    fn breadcrumb(&mut self) -> String {
        let st = self.top().cloned();
        let rows = self.menu_rows_snapshot();
        self.breadcrumb_with_rows(st.as_ref(), &rows)
    }

    /// breadcrumb_with_rows is breadcrumb for a caller that already built the
    /// row list: the merge-key arrow needs the cursor row, and building all
    /// rows again doubled listView's per-frame cost on a wide menu.
    fn breadcrumb_with_rows(&mut self, st: Option<&MenuState>, rows: &[Row]) -> String {
        let Some(st) = st else {
            return "$".to_string();
        };
        let mut crumb = crate::doc::jsondoc::path_string(&st.path);
        if let Some((_, true)) = anchor_role(self.doc(), &st.path) {
            if let Some(n) = self.doc().get(&st.path) {
                if let Some(holder) = anchor_holder_path(self.doc(), n) {
                    crumb += " → ";
                    crumb += &crate::doc::jsondoc::path_string(&holder);
                    return clip_left(&crumb, self.width);
                }
            }
        }
        if let Some(anchor) = Self::merge_crumb_row(st, rows) {
            crumb += " → ";
            crumb += &crate::doc::jsondoc::path_string(&anchor);
        }
        clip_left(&crumb, self.width)
    }

    /// mergeCrumbRow returns the anchor path the breadcrumb appends while the
    /// cursor rests on a merged key.
    fn merge_crumb_row(st: &MenuState, rows: &[Row]) -> Option<Vec<PathSeg>> {
        if st.cursor >= rows.len() {
            return None;
        }
        let r = &rows[st.cursor];
        if r.merged.is_empty() || r.path.is_empty() {
            return None;
        }
        Some(r.path[..r.path.len() - 1].to_vec())
    }

    /// menuTitle is the title-line label of the current menu (Go menuTitle).
    fn menu_title(&self, st: &MenuState) -> (String, String) {
        // The root screen always names the file, never the schema's title
        // ("Schema for rust-toolchain.toml", "Server Configuration", …).
        if st.path.is_empty() {
            let base = crate::file_base(&self.opts().path);
            if !base.is_empty() && base != "." {
                return (base.to_string(), self.menu_summary(st));
            }
        }
        let node = &self.fs().schema.nodes[st.node as usize];
        if !node.title.is_empty() {
            return (node.title.clone(), self.menu_summary(st));
        }
        let field = st.one_of.or(st.variant_of);
        let mut title = String::new();
        if let Some(f) = field {
            let fnode = &self.fs().schema.nodes[f as usize];
            if !fnode.title.is_empty() {
                title = fnode.title.clone();
            } else if !fnode.key.is_empty() {
                title = fnode.key.clone();
            }
        }
        if title.is_empty() {
            title = doc_key_at(&st.path);
        }
        if title.is_empty() {
            // nothing to name it after
            let summary = if st.one_of.is_some() {
                variant_label(&self.fs().schema, st.node)
            } else {
                String::new()
            };
            return (display_title(&self.fs().schema, st.node), summary);
        }
        let summary = if st.kind == MenuKind::OneOf {
            self.variant_picker_summary(st)
        } else if st.one_of.is_some() {
            variant_label(&self.fs().schema, st.node)
        } else {
            variant_signature(&self.fs().schema, Some(st.node))
        };
        (title, summary)
    }

    /// menu_summary is the bottom line for a titled menu (schema title or the
    /// root filename).
    fn menu_summary(&self, st: &MenuState) -> String {
        if st.kind == MenuKind::OneOf {
            self.variant_picker_summary(st)
        } else if st.one_of.is_some() {
            variant_label(&self.fs().schema, st.node)
        } else {
            String::new()
        }
    }

    /// variantPickerSummary is the bottom `variant:` line of a oneOf type
    /// picker: the matched variant's type name followed by its signature.
    /// Other menus keep the node's own signature.
    fn variant_picker_summary(&self, st: &MenuState) -> String {
        let node = &self.fs().schema.nodes[st.node as usize];
        if st.kind != MenuKind::OneOf || node.kind != Kind::OneOf {
            return variant_signature(&self.fs().schema, Some(st.node));
        }
        let val = self.doc().get(&st.path);
        let idx = current_variant(&self.fs().schema, st.node, self.doc(), val);
        if idx < 0 {
            return variant_signature(&self.fs().schema, Some(st.node));
        }
        variant_label(&self.fs().schema, node.variants[idx as usize])
    }

    /// statusLines renders the session status notices.
    fn status_lines(&self) -> Vec<String> {
        if self.fs().stack.len() != 1 {
            return Vec::new();
        }
        let mut combined = self.opts().status.clone();
        let startup = self.fs().startup.clone();
        if !startup.is_empty() {
            if !combined.is_empty() {
                combined = format!("{}\n{}", startup, combined);
            } else {
                combined = startup;
            }
        }
        if combined.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for line in combined.split('\n') {
            for w in wrap_text(line, self.width.saturating_sub(1)) {
                out.push(style::fg3(&w));
            }
        }
        out
    }

    /// helpLine renders the bottom hint row.
    fn help_line(&self) -> String {
        if let Some(f) = &self.fast_move {
            if f.backward {
                return self.rec_hint("Fast move: type a character to move up · esc cancels");
            }
            return self.rec_hint("Fast move: type a character to move down · esc cancels");
        }
        if let Some(st) = self.top() {
            if st.searching {
                let hints = if st.kind == MenuKind::EnumArray {
                    "type to filter · ↑/↓ or tab move · enter toggle · esc cancel search"
                } else {
                    "type to filter · ↑/↓ or tab move · enter select · esc cancel search"
                };
                return self.rec_hint(hints);
            }
            if st.visual {
                return self.rec_hint(
                    "Visual select: j/k, gg/G or arrows mark rows · y copy · d delete · v/esc stop",
                );
            }
            let n = st.sel.len();
            if n > 0 {
                let rows = if n == 1 { "row" } else { "rows" };
                return self.rec_hint(&format!(
                    "Marked {} {}: y copy · d delete · m unmark",
                    n, rows
                ));
            }
        }
        self.rec_hint(crate::ui::menu::MENU_HELP)
    }

    /// finish_with_hint pads the view with blank rows so the help row lands
    /// on the terminal's last line, then appends it without a trailing
    /// newline. Views must size their content to leave that row free (the
    /// windowed overlays do).
    fn finish_with_hint(&self, mut b: String, hint: String) -> String {
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1) {
            b.push('\n');
            lines += 1;
        }
        b.push_str(&hint);
        b
    }

    /// recHint renders a hint row with a right-aligned tail (cmd buffer /
    /// recording marker).
    fn rec_hint(&self, hints: &str) -> String {
        let limit = self.width.saturating_sub(1);
        let mut parts: Vec<String> = Vec::new();
        if !self.cmd_buf.is_empty() {
            parts.push(style::fg3(&self.cmd_buf.join("")));
        }
        if self.recording {
            parts.push(style::fg1(&format!("● REC {}", self.rec_buf.len())));
        }
        if parts.is_empty() {
            return style::help(&clip_display(hints, limit));
        }
        let right = parts.join(" ");
        let w = display_width(&right);
        if self.width < w + 8 {
            return style::help(&clip_display(hints, limit));
        }
        let left = clip_display(hints, limit.saturating_sub(w + 1));
        let mut pad = limit.saturating_sub(display_width(&left)).saturating_sub(w);
        if pad < 1 {
            pad = 1;
        }
        format!("{}{}{}", style::help(&left), " ".repeat(pad), right)
    }

    /// helpView renders the "?" overlay.
    fn help_view(&self) -> String {
        let mut b = String::new();
        b.push_str(&style::bold("Keyboard help"));
        b.push_str("\n\n");
        let entries = crate::ui::menu::HELP_ENTRIES;
        let page = self.height.saturating_sub(3).max(1);
        let max_scroll = entries.len().saturating_sub(page);
        let off = self.help_scroll.min(max_scroll);
        for e in entries.iter().skip(off).take(page) {
            let line = if self.width - 1 < crate::ui::menu::HELP_COL_WIDTH + 12 {
                // narrow terminal: do not waste the row on padding
                if e.keys.is_empty() {
                    e.desc.to_string()
                } else {
                    format!("{} {}", e.keys, e.desc)
                }
            } else {
                format!("{}{}", self.help_col(e.keys), e.desc)
            };
            b.push_str(&clip_display(
                &style::help(&line),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1) {
            b.push('\n');
            lines += 1;
        }
        b.push_str(&self.rec_hint(if max_scroll > 0 {
            "j/k or pgup/pgdn scroll · esc/? close"
        } else {
            "esc/? close"
        }));
        b
    }

    fn help_col(&self, left: &str) -> String {
        let w = display_width(left);
        if w >= crate::ui::menu::HELP_COL_WIDTH {
            format!("{left} ")
        } else {
            format!("{left}{}", " ".repeat(crate::ui::menu::HELP_COL_WIDTH - w))
        }
    }

    /// listView renders the plain menu screen (Go listView).
    fn list_view(&mut self) -> String {
        let mut b = String::new();
        let Some(st) = self.top().cloned() else {
            return b;
        };
        let (title_text, summary) = self.menu_title(&st);
        let mut title = style::bold(&title_text);
        // the menu of an anchored value names the anchor on the title line
        if let Some((name, is_ref)) = anchor_role(self.doc(), &st.path) {
            if is_ref {
                title += &format!(": {}", style::anchor(&format!("*{name}")));
            } else {
                title += &format!(": {}", style::anchor(&format!("&{name}")));
            }
        }
        // a mapping with merge keys names them on the title
        let merges = self.merge_names(&st.path);
        if !merges.is_empty() {
            title += &format!(" {}", style::anchor(&merges));
        }
        if self.dirty() {
            title += &format!(" {}", style::diff("●"));
        }
        if self.files.len() > 1 {
            let base = crate::file_base(&self.opts().path);
            // The root title is already the file name; repeat only the
            // position, not the name.
            let label = if base == title_text {
                format!("[{}/{}]", self.cur + 1, self.files.len())
            } else {
                format!("[{}/{} {}]", self.cur + 1, self.files.len(), base)
            };
            title += &format!(" {}", style::faint(&label));
        }
        b.push_str(&clip_display(&title, self.width.saturating_sub(1)));
        b.push('\n');
        let rows = self.menu_rows_snapshot();
        b.push_str(&clip_display(
            &style::faint(&clip_runes(
                &self.breadcrumb_with_rows(Some(&st), &rows),
                self.width,
            )),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        let mut header = 2usize;
        // the "<" merge-keys input edits the mapping itself: header row
        if let Some(Overlay::Mrg(s)) = &self.overlay {
            for line in self.merge_input_lines(s).split('\n') {
                b.push_str(&clip_display(line, self.width.saturating_sub(1)));
                b.push('\n');
                header += 1;
            }
        }
        if let Some(Overlay::Search(s)) = &self.overlay {
            return self.render_search_body(b, header, s);
        }
        // Inline picker searches sit above their list, like every other
        // search prompt.
        if matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray) && st.searching {
            // search rows use the terminal's default foreground (white);
            // only the help row below the views is dim gray
            b.push_str(&clip_display(
                &search_input_line("search: ", &st.search_ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
            header += 1;
        }
        let desc_lines = self.desc_lines_for(&st, &rows, !self.desc_expanded);
        let mut summary_lines: Vec<String> = Vec::new();
        if !summary.is_empty() {
            summary_lines = wrap_text(
                &format!("variant: {}", summary),
                self.width.saturating_sub(1),
            );
        }
        let status = self.status_lines();
        let title_width = self.menu_title_col_width(&rows);
        let write_lines = self.write_input_lines(title_width);
        let mut h = self.height.saturating_sub(
            header + 1 + desc_lines.len() + summary_lines.len() + status.len() + write_lines.len(),
        );
        if !self.notice.is_empty() {
            h = h.saturating_sub(1);
        }
        if self.hide_empty {
            h = h.saturating_sub(1);
        }
        self.write_rows(&mut b, &st, &rows, h, title_width);
        if self.hide_empty {
            b.push_str(&clip_display(
                &format!("  {}", style::faint("[empty hidden]")),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        // pin the bottom block above the hint bar
        let bottom = desc_lines.len()
            + summary_lines.len()
            + status.len()
            + usize::from(!self.notice.is_empty())
            + write_lines.len();
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1).saturating_sub(bottom) {
            b.push('\n');
            lines += 1;
        }
        for line in &desc_lines {
            b.push_str(&clip_display(
                &desc_line_view(line),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        for line in &summary_lines {
            b.push_str(&clip_display(
                &style::faint(line),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        for line in &status {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if !self.notice.is_empty() {
            b.push_str(&clip_display(
                &style::fg1(&clip_runes(&self.notice, self.width)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        for line in &write_lines {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        b.push_str(&self.help_line());
        b
    }

    /// The search branch of listView (search overlay is open and renders
    /// inline in the list frame).
    fn list_view_search(&mut self, s: &SearchState) -> String {
        let mut b = String::new();
        let Some(st) = self.top().cloned() else {
            return b;
        };
        let (title_text, _summary) = self.menu_title(&st);
        let mut title = style::bold(&title_text);
        if let Some((name, is_ref)) = anchor_role(self.doc(), &st.path) {
            if is_ref {
                title += &format!(": {}", style::anchor(&format!("*{name}")));
            } else {
                title += &format!(": {}", style::anchor(&format!("&{name}")));
            }
        }
        let merges = self.merge_names(&st.path);
        if !merges.is_empty() {
            title += &format!(" {}", style::anchor(&merges));
        }
        if self.dirty() {
            title += &format!(" {}", style::diff("●"));
        }
        b.push_str(&clip_display(&title, self.width.saturating_sub(1)));
        b.push('\n');
        b.push_str(&clip_display(
            &style::faint(&clip_runes(&self.breadcrumb(), self.width)),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        self.render_search_body(b, 2, s)
    }

    fn render_search_body(&self, mut b: String, header: usize, s: &SearchState) -> String {
        let mut inputs = 1usize;
        if s.replace {
            inputs += 1;
        }
        let status = self.status_lines();
        let search_focused = !s.replace || !s.replace_focused;
        b.push_str(&clip_display(
            &search_field_line("search", &s.ti, search_focused),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        if s.replace {
            b.push_str(&clip_display(
                &search_field_line("replace", &s.rti, s.replace_focused),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let mut h = self
            .height
            .saturating_sub(header + inputs + status.len() + 1);
        if h < 1 {
            h = 1;
        }
        self.write_search_rows(&mut b, s, h);
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1).saturating_sub(status.len()) {
            b.push('\n');
            lines += 1;
        }
        let case_txt = if s.fold_case {
            "case-insensitive"
        } else {
            "case-sensitive"
        };
        if !s.re_err.is_empty() {
            b.push_str(&clip_display(
                &style::fg1(&clip_runes(
                    &format!("invalid regex: {} · esc close", s.re_err),
                    self.width,
                )),
                self.width.saturating_sub(1),
            ));
            return b;
        }
        let mode_txt = if s.regex { "regex" } else { "literal" };
        for line in &status {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        let help = if s.replace {
            format!(
                "↑/↓ ctrl+j/k · pgup/dn · enter jump/replace · ctrl+a all · tab focus · alt+i {} · alt+x {} · esc",
                case_txt, mode_txt
            )
        } else {
            format!(
                "↑/↓ ctrl+j/k · pgup/pgdn page · enter jump · tab replace · alt+i {} · alt+x {} · esc close",
                case_txt, mode_txt
            )
        };
        b.push_str(&clip_display(
            &style::help(&help),
            self.width.saturating_sub(1),
        ));
        b
    }

    /// writeSearchRows renders the visible slice of search results.
    fn write_search_rows(&self, b: &mut String, s: &SearchState, height: usize) {
        let height = height.max(1);
        let mut cursor = s.cursor;
        if cursor >= s.results.len() {
            cursor = s.results.len().saturating_sub(1);
        }
        let mut off = cursor as i64 - height as i64 + 1;
        if off < 0 {
            off = 0;
        }
        let off = off as usize;
        for i in off..s.results.len().min(off + height) {
            let mut line = format!("{}{}", cursor_prefix(i == cursor), s.results[i].label);
            if i == cursor {
                line = style::sel(&line);
            }
            self.write_cut(b, &line);
        }
    }

    /// writeCut clips s to the terminal width line by line.
    fn write_cut(&self, b: &mut String, s: &str) {
        for line in s.split('\n') {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
    }

    /// writeRows renders the visible slice of rows (scroll computed
    /// read-only around the persisted offset).
    fn write_rows(
        &self,
        b: &mut String,
        st: &MenuState,
        rows: &[Row],
        height: usize,
        title_width: usize,
    ) {
        let height = height.max(1);
        let cursor = st.cursor.min(rows.len().saturating_sub(1));
        let mut offset = st.offset;
        if offset > cursor {
            offset = cursor;
        }
        if cursor >= offset + height {
            offset = cursor + 1 - height;
        }
        let max_off = rows.len().saturating_sub(height);
        if offset > max_off {
            offset = max_off;
        }
        for i in offset..rows.len().min(offset + height) {
            let line = self.row_line(&rows[i], i == cursor, title_width);
            self.write_cut(b, &line);
            // the & / * name input sits on the line under the cursor
            if let Some(Overlay::Anc(s)) = &self.overlay {
                if i == cursor {
                    let lines = self.anchor_input_lines(s);
                    self.write_cut(b, &lines);
                }
            }
        }
        // in-place input for a brand-new array item on its own row
        if let Some(Overlay::Edit(e)) = &self.overlay {
            if e.is_new && !self.has_inline_edit_row(rows, e) {
                let line =
                    self.edit_lines(e, &self.title_col(&self.new_item_title(st), title_width));
                self.write_cut(b, &line);
            }
        }
        // key-name input for a schema-less member (a)
        if let Some(Overlay::Add(a)) = &self.overlay {
            let line = format!(
                "{}{}{}",
                cursor_prefix(true),
                self.title_col("New key", title_width),
                ti_view(&a.ti)
            );
            self.write_cut(b, &line);
            if !a.err.is_empty() {
                self.write_cut(b, &style::fg1(&format!("  {}", a.err)));
            }
        }
    }

    /// write_input_lines renders the W write-to-path input in the bottom
    /// block (above the help bar, so it never scrolls with the menu rows).
    /// The line has no "> " cursor marker: it is the only focused input
    /// there.
    fn write_input_lines(&self, title_width: usize) -> Vec<String> {
        let Some(Overlay::Write(w)) = &self.overlay else {
            return Vec::new();
        };
        let mut lines = vec![format!(
            "{}{}",
            self.title_col("Write to", title_width),
            ti_view(&w.ti)
        )];
        if !w.err.is_empty() {
            lines.push(style::fg1(&format!("  {}", w.err)));
        } else {
            let hint = self.suggestion_hint(&w.sugs, w.sug_idx);
            if !hint.is_empty() {
                lines.push(hint);
            }
        }
        lines
    }

    /// toggle_desc implements c: expand the focused description inline, or
    /// open the dedicated scrolling page when it cannot fit one screen.
    fn toggle_desc(&mut self, r: &Row, rows: &[Row]) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        let lines = self.desc_lines_for(&st, rows, false);
        if lines.is_empty() {
            return;
        }
        if lines.len() > self.desc_page_capacity() {
            self.desc_expanded = false;
            let title = if !r.label.is_empty() {
                r.label.clone()
            } else if let Some(f) = r.fn_ {
                self.fs().schema.nodes[f as usize].title.clone()
            } else {
                String::new()
            };
            self.overlay = Some(Overlay::Comment(CommentState {
                title,
                lines,
                scroll: 0,
            }));
        } else {
            self.desc_expanded = !self.desc_expanded;
        }
    }

    /// desc_page_capacity is how many wrapped description lines fit while
    /// keeping the title, breadcrumb, one menu row and the help bar.
    fn desc_page_capacity(&self) -> usize {
        let status = self.status_lines().len();
        let notice = usize::from(!self.notice.is_empty());
        let hidden = usize::from(self.hide_empty);
        self.height
            .saturating_sub(4 + status + notice + hidden)
            .max(1)
    }

    /// keyComment drives the dedicated long-comment page.
    pub fn key_comment(&mut self, msg: Msg, mut s: CommentState) -> Option<Overlay> {
        let page = self.height.saturating_sub(2).max(1);
        let max_scroll = s.lines.len().saturating_sub(page);
        match msg.key_str().as_str() {
            "up" | "k" => s.scroll = s.scroll.saturating_sub(1),
            "down" | "j" => s.scroll = (s.scroll + 1).min(max_scroll),
            "pgdown" | "ctrl+f" => s.scroll = (s.scroll + page).min(max_scroll),
            "pgup" | "ctrl+b" => s.scroll = s.scroll.saturating_sub(page),
            "c" | "esc" => return None,
            _ => {}
        }
        Some(Overlay::Comment(s))
    }

    /// key_comment_edit drives the multiline document-comment editor.
    /// ctrl+s writes the comment into the session metadata (a later save emits it);
    /// the document body is unchanged.
    pub fn key_comment_edit(&mut self, msg: Msg, mut s: CommentEditState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "ctrl+s" => {
                let text = s.ta.value();
                if self.doc_comment_at(&s.path).unwrap_or_default() == text {
                    return None; // no-op: keep the stored formatting/markers
                }
                let setter = self.opts().set_comment.clone();
                if let Some(set) = setter {
                    set(&s.path, &text);
                    self.fs_mut().comments_dirty = true;
                }
                return None;
            }
            "esc" => return None,
            _ => {}
        }
        if let Some(k) = msg_key(&msg) {
            s.ta.handle_key(k);
        }
        Some(Overlay::CommentEdit(s))
    }

    /// edit_comment_row implements C: open the multiline editor for the
    /// focused key's document comment. A key without a comment starts with an
    /// empty buffer, so the same key adds one; applying empty text removes it.
    fn edit_comment_row(&mut self, r: &Row) {
        if !matches!(
            r.kind,
            RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Toggle | RowKind::Variant
        ) {
            return;
        }
        let path = r.path.clone();
        if self.doc().get(&path).is_none() {
            self.notice = "comment: key has no value in the document".to_string();
            return;
        }
        let text = self.doc_comment_at(&path).unwrap_or_default();
        let title = if !r.label.is_empty() {
            r.label.clone()
        } else if let Some(f) = r.fn_ {
            self.fs().schema.nodes[f as usize].title.clone()
        } else {
            String::new()
        };
        self.open_comment_edit(&path, &title, &text);
    }

    /// open_comment_edit arms the multiline comment editor.
    fn open_comment_edit(&mut self, path: &[PathSeg], title: &str, text: &str) {
        self.overlay = Some(self.comment_edit_overlay(path, title, text));
    }

    /// comment_edit_overlay builds the comment editor state (the overlay
    /// router assigns the returned surface).
    fn comment_edit_overlay(&self, path: &[PathSeg], title: &str, text: &str) -> Overlay {
        let mut ta = TextArea::new();
        let w = self.width.saturating_sub(4).clamp(20, 100);
        let h = self.height.saturating_sub(6).clamp(3, 24);
        ta.set_width(w as u16);
        ta.set_height(h as u16);
        ta.set_value(text);
        Overlay::CommentEdit(CommentEditState {
            path: path.to_vec(),
            title: title.to_string(),
            ta,
        })
    }

    /// doc_comment_at reads the session's document comment for one path.
    fn doc_comment_at(&self, path: &[PathSeg]) -> Option<String> {
        let f = self.opts().comment_at.clone()?;
        f(path)
    }

    /// row_comment_text returns the document comment of a member row (None
    /// for synthetic rows such as add/save/raw and for paths without one).
    fn row_comment_text(&self, r: &Row) -> Option<String> {
        if !matches!(
            r.kind,
            RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Toggle | RowKind::Variant
        ) || self.doc().get(&r.path).is_none()
        {
            return None;
        }
        self.doc_comment_at(&r.path)
    }

    /// focused_sources returns the schema help and document comment the
    /// description panel should show, after applying `C`'s source mode.
    fn focused_sources(&self, st: &MenuState, rows: &[Row]) -> (String, String) {
        if rows.is_empty() || st.cursor >= rows.len() {
            return (String::new(), String::new());
        }
        let r = &rows[st.cursor];
        let mut schema = String::new();
        if let Some(f) = r.fn_ {
            schema = self.fs().schema.nodes[f as usize].desc.clone();
        }
        if schema.is_empty() {
            if let Some(v) = r.variant {
                schema = self.fs().schema.nodes[v as usize].desc.clone();
            }
        }
        let comment = self.row_comment_text(r).unwrap_or_default();
        (schema, comment)
    }

    /// desc_lines_for wraps the focused sources into colored lines. When
    /// `folded`, each source independently keeps at most
    /// [`DESC_BLOCK_PREVIEW_LINES`] lines and ends with an emphasized `…`.
    fn desc_lines_for(&self, st: &MenuState, rows: &[Row], folded: bool) -> Vec<DescLine> {
        let (schema, comment) = self.focused_sources(st, rows);
        let width = self.width.saturating_sub(1);
        let mut out = Vec::new();
        for (source, raw) in [(DescSource::Schema, schema), (DescSource::Comment, comment)] {
            if raw.trim().is_empty() {
                continue;
            }
            let mut lines = wrap_text(&raw, width);
            let cut = folded && lines.len() > DESC_BLOCK_PREVIEW_LINES;
            if cut {
                lines = fold_desc_block(lines, width);
            }
            let last = lines.len().saturating_sub(1);
            out.extend(lines.into_iter().enumerate().map(|(i, text)| DescLine {
                source,
                text,
                folded: cut && i == last,
            }));
        }
        out
    }

    /// suggestionHint renders the completion hint line (Go suggestionHint).
    fn suggestion_hint(&self, sugs: &[String], idx: i64) -> String {
        if sugs.is_empty() {
            return String::new();
        }
        const WINDOW: usize = 3;
        let mut start = 0usize;
        if idx >= 0 {
            let idx = idx as usize;
            start = idx.saturating_sub(WINDOW - 1);
            let max = sugs.len().saturating_sub(WINDOW);
            if start > max {
                start = max;
            }
        }
        let mut parts: Vec<String> = Vec::new();
        if start > 0 {
            parts.push("…".to_string());
        }
        for i in start..sugs.len().min(start + WINDOW) {
            if i as i64 == idx {
                parts.push(style::accent(&sugs[i]));
            } else {
                parts.push(sugs[i].clone());
            }
        }
        if start + WINDOW < sugs.len() {
            parts.push("…".to_string());
        }
        format!(
            "{}{}",
            style::help("  tab: "),
            parts.join(&style::help(" · "))
        )
    }

    /// editLines renders the in-place input line plus hint and error.
    fn edit_lines(&self, e: &EditState, left: &str) -> String {
        let mut line = format!("{}{}{}", cursor_prefix(true), left, ti_view(&e.ti));
        let hint = self.suggestion_hint(&e.sugs, e.sug_idx);
        if !hint.is_empty() {
            line += &format!("\n{hint}");
        }
        if !e.err.is_empty() {
            line += &format!("\n{}", style::fg1(&format!("  {}", e.err)));
        }
        line
    }

    /// anchorInputLines renders the & / * name input under the cursor row.
    fn anchor_input_lines(&self, s: &AnchorState) -> String {
        let prompt = if s.mode == AnchorMode::RefSet {
            "  reference: "
        } else {
            "  anchor: "
        };
        let mut lines = vec![format!("{}{}", style::help(prompt), ti_view(&s.ti))];
        if s.mode == AnchorMode::RefSet && !Self::anchor_filtered(s).is_empty() {
            lines.push(format!(
                "{}{}",
                style::help("  tab: "),
                self.anchor_sug_view(s)
            ));
        }
        if !s.err.is_empty() {
            lines.push(style::fg1(&format!("  {}", s.err)));
        }
        lines.join("\n")
    }

    fn anchor_sug_view(&self, s: &AnchorState) -> String {
        let cands = Self::anchor_filtered(s);
        let parts: Vec<String> = cands
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i as i64 == s.sug {
                    style::accent(c)
                } else {
                    c.clone()
                }
            })
            .collect();
        parts.join(" · ")
    }

    /// mergeInputLines renders the merge-keys input under the title.
    fn merge_input_lines(&self, s: &MergeState) -> String {
        let mut lines = vec![format!("{}{}", style::help("merge keys: "), ti_view(&s.ti))];
        if !merge_filtered(s).is_empty() {
            let cands = merge_filtered(s);
            let parts: Vec<String> = cands
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    if i as i64 == s.sug {
                        style::accent(c)
                    } else {
                        c.clone()
                    }
                })
                .collect();
            lines.push(format!("{}{}", style::help("tab: "), parts.join(" · ")));
        }
        if !s.err.is_empty() {
            lines.push(style::fg1(&s.err));
        }
        lines.join("\n")
    }

    /// menuTitleColWidth returns the adaptive title column width.
    fn menu_title_col_width(&self, rows: &[Row]) -> usize {
        let mut width = crate::ui::menu::TITLE_COL_MIN;
        for r in rows {
            let n = match r.kind {
                RowKind::Field | RowKind::Drill => {
                    if r.fn_.is_none() {
                        continue;
                    }
                    let mut w = display_width(&self.row_title_text(r));
                    if self.field_required(r) {
                        w += 1; // required marker
                    }
                    w
                }
                RowKind::Unknown => display_width(&r.label),
                RowKind::Variant => {
                    let title = r
                        .variant
                        .map(|v| display_title(&self.fs().schema, v))
                        .unwrap_or_default();
                    display_width("○ ") + display_width(&title)
                }
                _ => continue,
            };
            if n > width {
                width = n;
            }
        }
        width.min(crate::ui::menu::TITLE_COL_MAX)
    }

    /// titleCol pads the left column to width cells.
    fn title_col(&self, left: &str, width: usize) -> String {
        let w = display_width(left);
        if w < width {
            format!("{left}{}", " ".repeat(width - w + 1))
        } else {
            format!("{left} ")
        }
    }

    /// rowTitleText is the left-column label of a field or drill row.
    fn row_title_text(&self, r: &Row) -> String {
        if !r.label.is_empty() {
            return r.label.clone();
        }
        match r.fn_ {
            Some(f) => self.fs().schema.nodes[f as usize].title.clone(),
            None => String::new(),
        }
    }

    /// newItemTitle labels the standalone input of the append-only flow.
    fn new_item_title(&self, st: &MenuState) -> String {
        let mut key = self.fs().schema.nodes[st.node as usize].key.clone();
        if let Some(PathSeg::Key(s)) = st.path.last() {
            key = s.clone();
        }
        format!("{}[{}]", key, doc_array_len(self.doc(), &st.path))
    }

    /// inlineEditRow reports whether the active editor renders on r.
    fn inline_edit_row(&self, r: &Row, e: &EditState) -> bool {
        if r.path != e.path {
            return false;
        }
        if r.kind == RowKind::Unknown {
            return true;
        }
        match (r.kind, r.fn_, e.fn_) {
            (RowKind::Field, Some(_), Some(_)) if r.fn_ == e.fn_ => true,
            (RowKind::Field, Some(f), _) | (RowKind::Drill, Some(f), _) => {
                self.fs().schema.nodes[f as usize].kind == Kind::OneOf
                    && e.synth.is_none()
                    && self.variant_contains_edit(f, e)
            }
            _ => false,
        }
    }

    fn variant_contains_edit(&self, one_of: FormNodeId, e: &EditState) -> bool {
        let Some(target) = e.fn_ else { return false };
        variant_contains(&self.fs().schema, one_of, target)
    }

    fn has_inline_edit_row(&self, rows: &[Row], e: &EditState) -> bool {
        rows.iter().any(|r| self.inline_edit_row(r, e))
    }

    /// rowLine renders one menu line (Go rowLine).
    /// drill_marker reports whether the row shows the "--->" drill marker.
    /// A oneOf field whose current variant resolved to a filled-in leaf
    /// value has nothing to drill into; the marker is kept only for mapping
    /// variants (objects/maps/arrays) and undetermined variants, where
    /// entering opens a real submenu.
    fn drill_marker(&self, r: &Row) -> bool {
        if r.kind == RowKind::Issues {
            return true;
        }
        if r.kind != RowKind::Drill {
            return false;
        }
        let Some(f) = r.fn_ else { return true };
        let schema = &self.fs().schema;
        if schema.nodes[f as usize].kind != Kind::OneOf {
            return true;
        }
        let doc = self.doc();
        let Some(v) = doc.get(&r.path) else {
            return true;
        };
        if is_null_val(doc, v) {
            return true;
        }
        let Some(leaf) = matched_leaf_variant(schema, f, doc, Some(v)) else {
            return true;
        };
        matches!(
            schema.nodes[leaf as usize].kind,
            Kind::Object | Kind::Map | Kind::Array
        )
    }

    fn row_line(&self, r: &Row, cursor: bool, title_width: usize) -> String {
        let st = self.top();
        // in-place input replaces the edited row
        if let Some(Overlay::Edit(e)) = &self.overlay {
            if self.inline_edit_row(r, e) {
                return self.edit_lines(e, &self.title_col(&self.row_title_text(r), title_width));
            }
        }
        let sel = self.row_selected(r);
        let visual = st.map(|s| s.visual).unwrap_or(false);
        let left;
        let mut right = String::new();
        match r.kind {
            RowKind::Variant => {
                let v = self.doc().get(&r.path);
                let cur = st.and_then(|s| {
                    let cur = current_variant(&self.fs().schema, s.node, self.doc(), v);
                    if cur < 0 {
                        None
                    } else {
                        Some(self.fs().schema.nodes[s.node as usize].variants[cur as usize])
                    }
                });
                let mut mark = "○";
                let mut mark_styled = style::faint(mark);
                if cur == r.variant {
                    mark = "●";
                    mark_styled = style::accent(mark);
                }
                let mut l = mark_styled;
                if let Some(vn) = r.variant {
                    // untitled variants stand in their structural signature,
                    // matching the any-type picker's name column instead of
                    // leaving the title cell blank
                    let title = display_title(&self.fs().schema, vn);
                    if !title.is_empty() {
                        l += &format!(" {title}");
                    }
                }
                left = l;
                // a titled variant keeps the signature in the summary
                // column; the untitled case already shows it as the title
                right = match r.variant {
                    Some(vn) if !self.fs().schema.nodes[vn as usize].title.is_empty() => {
                        style::faint(&variant_signature(&self.fs().schema, r.variant))
                    }
                    _ => String::new(),
                };
                if mark == "●" {
                    if let Some(v) = self.doc().get(&r.path) {
                        if !is_null_val(self.doc(), v) {
                            if let Some(vn) = r.variant {
                                let s = variant_value_summary(
                                    &self.fs().schema,
                                    vn,
                                    self.doc(),
                                    Some(v),
                                );
                                if !s.is_empty() {
                                    right += &format!(" {}", style::val_enum(&s));
                                }
                            }
                        }
                    }
                }
            }
            RowKind::Toggle => {
                let on = self.enum_member_on(r);
                let (box_, box_style) = if on {
                    ("[x]", style::accent("[x]"))
                } else {
                    ("[ ]", style::faint("[ ]"))
                };
                let label = r
                    .fn_
                    .and_then(|f| self.fs().schema.nodes[f as usize].items)
                    .and_then(|it| {
                        self.fs().schema.nodes[it as usize]
                            .enum_
                            .get(r.idx.max(0) as usize)
                    })
                    .map(|o| o.display.clone())
                    .unwrap_or_default();
                left = format!("{} {}", box_style, label);
                let _ = box_;
            }
            RowKind::Add | RowKind::Expand => {
                left = style::accent(&r.label);
            }
            RowKind::NoItems => {
                // An empty object menu's "< Add Key >" is an action row and
                // carries the accent color; the oneOf/enum-array placeholders
                // are informational and stay faint.
                let placeholder = matches!(
                    st.map(|s| s.kind),
                    Some(MenuKind::OneOf) | Some(MenuKind::EnumArray)
                );
                left = if placeholder {
                    style::faint(&r.label)
                } else {
                    style::accent(&r.label)
                };
            }
            RowKind::Unknown => {
                let title = r.label.clone();
                left = if sel && cursor {
                    style::bold(&style::fg9(&title))
                } else if sel {
                    style::fg9(&title)
                } else {
                    style::fg3(&title)
                };
                let v = self.doc().get(&r.path);
                right = format!(
                    "{}{}{}",
                    self.merge_badge(r),
                    self.anchor_badge(&r.path),
                    self.raw_value_styled(
                        v,
                        &raw_value_summary(&self.fs().schema, self.doc(), &r.path)
                    )
                );
            }
            RowKind::Issues => {
                left = self.issues_severity().render_str(&r.label);
            }
            _ => {
                // rowField / rowDrill
                let mut title = self.row_title_text(r);
                if self.field_required(r) {
                    title += &style::fg1("*");
                }
                left = if sel && cursor {
                    style::bold(&style::fg9(&title))
                } else if sel {
                    style::fg9(&title)
                } else {
                    title
                };
                let summ = match r.fn_ {
                    Some(f) => summary(&self.ctx(), f, &r.path),
                    None => String::new(),
                };
                right = format!(
                    "{}{}{}",
                    self.merge_badge(r),
                    self.anchor_badge(&r.path),
                    (self.value_style(r)).render_str(&summ)
                );
            }
        }
        let content = match r.kind {
            RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Variant => {
                format!("{}{}", self.title_col(&left, title_width), right)
            }
            _ => format!("{left}{right}"),
        };
        let mut prefix = cursor_prefix(cursor).to_string();
        if sel && (!cursor || visual) {
            prefix = style::fg9("> "); // selected rows carry a red > marker
        }
        let mut line = format!("{prefix}{content}");
        if r.kind == RowKind::Unknown && !r.in_schema && !self.fs().schema.schema_less {
            line += &format!(" {}", style::faint("(not in schema)"));
        }
        if self.drill_marker(r) {
            line += &format!(" {}", style::accent("--->"));
        }
        if matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            if self.has_err(&r.path) {
                line += &format!(" {}", style::fg1(ERR));
            } else if (r.kind == RowKind::Unknown && !r.in_schema && !self.fs().schema.schema_less)
                || self.has_warn(&r.path)
            {
                line += &format!(" {}", style::fg3(WARN));
            }
            if self.changed(&r.path) {
                line += &format!(" {}", style::diff("●"));
            }
        }
        if cursor {
            line = style::sel(&line);
        }
        line
    }

    /// rawValueStyle colors a schema-less value by its JSON type.
    fn raw_value_styled(&self, v: Option<NodeId>, s: &str) -> String {
        let styled = match v.map(|n| self.doc().value(n)) {
            None => style::faint(s),
            Some(Value::Null) => style::faint(s),
            Some(Value::Str(_)) => style::val_str(s),
            Some(Value::Num(_)) => style::val_num(s),
            Some(Value::Bool(_)) => style::val_bool(s),
            Some(_) => style::bold(s),
        };
        styled
    }

    fn has_err(&self, path: &[PathSeg]) -> bool {
        self.fs().err_paths.iter().any(|p| is_path_prefix(path, p))
    }

    fn has_warn(&self, path: &[PathSeg]) -> bool {
        self.fs().warn_paths.iter().any(|p| is_path_prefix(path, p))
    }

    fn issues_severity(&self) -> Sev {
        if self.fs().issues_now.iter().any(|it| !it.warn) {
            Sev::Err
        } else {
            Sev::Warn
        }
    }

    fn severity(&self, it: &Issue) -> String {
        if it.warn {
            style::fg3(&it.msg)
        } else {
            style::fg1(&it.msg)
        }
    }

    /// mergeNames renders the anchor list a mapping merges for its title.
    fn merge_names(&self, path: &[PathSeg]) -> String {
        let Some(n) = self.doc().get(path) else {
            return String::new();
        };
        let mut parts: Vec<String> = Vec::new();
        for r in self.doc().merge_refs(n) {
            let anchor = self.doc().node(r).anchor.clone();
            if !anchor.is_empty() {
                parts.push(format!("*{anchor}"));
            }
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("<<: {}", parts.join(" "))
    }

    /// mergeBadge renders the "*anchor" provenance marker.
    fn merge_badge(&self, r: &Row) -> String {
        if r.merged.is_empty() {
            return String::new();
        }
        format!("{} ", style::anchor(&format!("*{}", r.merged)))
    }

    /// anchorBadge renders the "&name" / "*name" marker.
    fn anchor_badge(&self, path: &[PathSeg]) -> String {
        match anchor_role(self.doc(), path) {
            Some((name, true)) => format!("{} ", style::anchor(&format!("*{name}"))),
            Some((name, false)) => format!("{} ", style::anchor(&format!("&{name}"))),
            None => String::new(),
        }
    }

    /// fieldRequired reports whether the row's field is required in the
    /// current context (Go fieldRequired).
    fn field_required(&self, r: &Row) -> bool {
        let Some(f) = r.fn_ else { return false };
        let mut required = self.fs().schema.nodes[f as usize].required;
        let Some(st) = self.top() else {
            return required;
        };
        let Some(variant_of) = st.variant_of else {
            return required;
        };
        let variants = dynamic_variants(&self.fs().schema, variant_of).unwrap_or_default();
        if variants.is_empty() {
            return required;
        }
        let key = self.fs().schema.nodes[f as usize].key.clone();
        let (_, defs) = variant_field_defs(&self.fs().schema, &variants);
        let Some(ds) = defs.get(&key) else {
            return required;
        };
        if ds.is_empty() {
            return required;
        }
        let val = self.doc().get(&st.path);
        if match_variant_list(&self.fs().schema, &variants, self.doc(), val).len() != 1 {
            // merged view: required only when every defining variant requires it
            required = true;
            for d in ds {
                if !self.fs().schema.nodes[*d as usize].required {
                    required = false;
                    break;
                }
            }
        }
        required
    }

    /// showsSchemaDefault reports whether the row previews fn.Default.
    fn shows_schema_default(&self, f: Option<FormNodeId>, path: &[PathSeg]) -> bool {
        let Some(f) = f else { return false };
        let node = &self.fs().schema.nodes[f as usize];
        let Some(d) = node.default_ else { return false };
        if is_null_val(&self.fs().schema.doc, d) {
            return false;
        }
        is_null_opt(self.doc(), self.doc().get(path))
    }

    /// valueStyle picks the value-summary color.
    fn value_style(&self, r: &Row) -> ValStyle {
        if self.shows_schema_default(r.fn_, &r.path) {
            return ValStyle::Default;
        }
        let Some(f) = r.fn_ else { return ValStyle::Dim };
        if is_null_opt(self.doc(), self.doc().get(&r.path)) {
            return ValStyle::Dim;
        }
        let mut eff = f;
        if self.fs().schema.nodes[f as usize].kind == Kind::OneOf {
            let v = self.doc().get(&r.path);
            if let Some(leaf) = matched_leaf_variant_pub(&self.fs().schema, f, self.doc(), v) {
                eff = leaf;
            }
        }
        match self.fs().schema.nodes[eff as usize].kind {
            Kind::Enum => ValStyle::Enum,
            Kind::Boolean => ValStyle::Bool,
            Kind::Object | Kind::Array | Kind::Map | Kind::OneOf => ValStyle::Obj,
            _ => match self.doc().get(&r.path).map(|n| self.doc().value(n)) {
                Some(Value::Num(_)) => ValStyle::Num,
                Some(Value::Bool(_)) => ValStyle::Bool,
                _ => ValStyle::Str,
            },
        }
    }

    /// enumMemberOn reports whether toggle row r's option is in the doc.
    fn enum_member_on(&self, r: &Row) -> bool {
        let Some(cur) = self.doc().get(&r.path) else {
            return false;
        };
        let Value::Array(items) = self.doc().value(cur) else {
            return false;
        };
        let Some(f) = r.fn_ else { return false };
        let Some(it) = self.fs().schema.nodes[f as usize].items else {
            return false;
        };
        let Some(opt) = self.fs().schema.nodes[it as usize]
            .enum_
            .get(r.idx.max(0) as usize)
        else {
            return false;
        };
        items
            .iter()
            .any(|&i| encode_value(self.doc(), i) == opt.encoded)
    }

    // --- overlay views -----------------------------------------------------

    fn confirm_view(&self, c: &ConfirmState) -> String {
        let hint = if c.hint.is_empty() {
            "y/enter/space yes · n/esc no"
        } else {
            &c.hint
        };
        let b = format!(
            "{}\n",
            clip_display(&style::bold(&c.title), self.width.saturating_sub(1))
        );
        self.finish_with_hint(
            b,
            clip_display(&style::help(hint), self.width.saturating_sub(1)),
        )
    }

    fn issues_view(&self, s: &IssuesState) -> String {
        let mut b = String::new();
        b.push_str(&clip_display(
            &style::bold(&format!(
                "{} Validation issues ({})",
                worst_issue_icon(&s.items),
                s.items.len()
            )),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        if s.items.is_empty() {
            b.push_str(&style::faint("( no issues )"));
            b.push('\n');
            return self.finish_with_hint(b, self.rec_hint("esc close"));
        }
        if s.searching {
            b.push_str(&clip_display(
                &search_input_line("search: ", &s.search_ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let notice = usize::from(!self.notice.is_empty());
        let height = self
            .height
            .saturating_sub(2 + usize::from(s.searching) + notice)
            .max(1);
        let total = issue_indices(s).count();
        let cursor = s.cursor.min(total.saturating_sub(1));
        let off = (cursor as i64 - height as i64 + 1).max(0) as usize;
        for (i, idx) in issue_indices(s).skip(off).take(height).enumerate() {
            let it = &s.items[idx];
            let mut line = format!("  {}", self.severity(it));
            if off + i == cursor {
                line = style::sel(&format!("> {}", it.msg));
            }
            b.push_str(&clip_display(&line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if total == 0 {
            b.push_str(&style::faint("(no matches)"));
            b.push('\n');
        }
        if !self.notice.is_empty() {
            b.push_str(&clip_display(
                &style::fg1(&clip_runes(&self.notice, self.width)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let hint = if s.searching {
            "enter jump · ↑/↓ or tab move · esc cancel search"
        } else {
            "↑/↓ or ctrl+j/k move · pgup/pgdn page · enter jump · / search · esc close"
        };
        self.finish_with_hint(b, self.rec_hint(hint))
    }

    fn ref_pick_view(&self, s: &RefPickState) -> String {
        let mut b = String::new();
        b.push_str(&clip_display(
            &style::bold(&format!("References to &{} ({})", s.name, s.paths.len())),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        if s.searching {
            b.push_str(&clip_display(
                &search_input_line("search: ", &s.search_ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let height = self
            .height
            .saturating_sub(2 + usize::from(s.searching))
            .max(1);
        let total = self.ref_indices(s).count();
        let cursor = s.cursor.min(total.saturating_sub(1));
        let off = (cursor as i64 - height as i64 + 1).max(0) as usize;
        for (i, idx) in self.ref_indices(s).skip(off).take(height).enumerate() {
            let label = self.ref_label(&s.paths[idx]);
            let line = if off + i == cursor {
                style::sel(&format!("> {label}"))
            } else {
                format!("  {label}")
            };
            b.push_str(&clip_display(&line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if total == 0 {
            b.push_str(&style::faint("(no matches)"));
            b.push('\n');
        }
        let hint = if s.searching {
            "enter jump · ↑/↓ or tab move · esc cancel search"
        } else {
            "↑/↓ or ctrl+j/k move · pgup/pgdn page · enter jump · / search · esc close"
        };
        self.finish_with_hint(b, self.rec_hint(hint))
    }

    fn errs_view(&self, s: &ErrsState) -> String {
        let mut b = String::new();
        b.push_str(&clip_display(
            &style::bold(&format!("{} Cannot save: validation errors", ERR)),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        if s.searching {
            b.push_str(&clip_display(
                &search_input_line("search: ", &s.search_ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let height = self
            .height
            .saturating_sub(2 + usize::from(s.searching))
            .max(1);
        let total = err_indices(s).count();
        let cursor = s.cursor.min(total.saturating_sub(1));
        let off = (cursor as i64 - height as i64 + 1).max(0) as usize;
        for (i, idx) in err_indices(s).skip(off).take(height).enumerate() {
            let line = if off + i == cursor {
                style::sel(&format!("> {}", s.lines[idx]))
            } else {
                format!("  {}", s.lines[idx])
            };
            b.push_str(&clip_display(&line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if total == 0 {
            b.push_str(&style::faint("(no matches)"));
            b.push('\n');
        }
        let hint = if s.searching {
            "enter jump · ↑/↓ or tab move · esc cancel search"
        } else if self.opts().force {
            "pgup/pgdn page · enter jump · / search · y save anyway · n back"
        } else {
            "pgup/pgdn page · enter jump · / search · fix fields and save again, or rerun with --force"
        };
        self.finish_with_hint(
            b,
            clip_display(&style::help(hint), self.width.saturating_sub(1)),
        )
    }

    fn comment_view(&self, s: &CommentState) -> String {
        let mut b = String::new();
        let title = if s.title.is_empty() {
            "Comment".to_string()
        } else {
            format!("Comment: {}", s.title)
        };
        b.push_str(&clip_display(
            &style::bold(&title),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        let page = self.height.saturating_sub(2).max(1);
        let max_scroll = s.lines.len().saturating_sub(page);
        let off = s.scroll.min(max_scroll);
        for line in s.lines.iter().skip(off).take(page) {
            b.push_str(&clip_display(
                &desc_line_view(line),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1) {
            b.push('\n');
            lines += 1;
        }
        b.push_str(&self.rec_hint("j/k · PgUp/PgDn page · c/q/esc close"));
        b
    }

    /// comment_edit_view renders the multiline document-comment editor.
    fn comment_edit_view(&self, s: &CommentEditState) -> String {
        let title = if s.title.is_empty() {
            "Edit comment".to_string()
        } else {
            format!("Edit comment: {}", s.title)
        };
        let mut b = format!("{}\n", style::bold(&title));
        b.push_str(&ta_view(&s.ta));
        b.push('\n');
        self.finish_with_hint(b, style::help("ctrl+s apply · esc cancel"))
    }

    fn text_view(&self, s: &TextState) -> String {
        let mut title = format!("Edit raw {}", self.opts().raw.raw_label());
        if let Some(f) = s.fn_ {
            let t = self.fs().schema.nodes[f as usize].title.clone();
            if !t.is_empty() {
                title = t;
            }
        }
        let mut b = format!("{}\n", style::bold(&title));
        b.push_str(&ta_view(&s.ta));
        b.push('\n');
        self.finish_with_hint(b, style::help("ctrl+s apply · esc cancel"))
    }

    fn sel_view(&self, s: &SelState) -> String {
        let mut cur_enc = String::new();
        if let Some(v) = self.doc().get(&s.path) {
            if !is_null_val(self.doc(), v) {
                cur_enc = encode_value(self.doc(), v);
            }
        }
        let mut b = clip_display(
            &style::bold(&display_title(&self.fs().schema, s.fn_)),
            self.width.saturating_sub(1),
        );
        b.push('\n');
        if s.searching {
            b.push_str(&clip_display(
                &search_input_line("search: ", &s.search_ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        // Reserve the title, optional search row and the bottom help row;
        // the option window scrolls with the cursor.
        let list_height = self
            .height
            .saturating_sub(1 + usize::from(s.searching) + 1)
            .max(1);
        let total = sel_match_count(s);
        let cursor = s.cursor.min(total.saturating_sub(1));
        let mut off = cursor.saturating_sub(list_height - 1);
        let max_off = total.saturating_sub(list_height);
        if off > max_off {
            off = max_off;
        }
        for (i, o) in sel_matches(s).skip(off).take(list_height).enumerate() {
            let pos = off + i;
            let (mark, mark_style) = if o.encoded == cur_enc {
                ("●", style::accent("●"))
            } else {
                ("○", style::faint("○"))
            };
            let mut line = format!(
                "{}{} {}",
                cursor_prefix(pos == cursor),
                mark_style,
                o.display
            );
            if pos == cursor {
                line = style::sel(&line);
            }
            let _ = mark;
            b.push_str(&clip_display(&line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if total == 0 {
            b.push_str(&clip_display(
                &style::faint("(no matches)"),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let hint = if s.searching {
            "enter select · ↑/↓ or tab move · esc cancel search"
        } else {
            crate::ui::menu::OVERLAY_HELP
        };
        self.finish_with_hint(
            b,
            clip_display(&style::help(hint), self.width.saturating_sub(1)),
        )
    }

    fn file_pick_view(&self) -> String {
        let Some(pick) = &self.file_pick else {
            return String::new();
        };
        let mut b = String::new();
        b.push_str(&clip_display(
            &style::bold(&format!("Files ({})", self.files.len())),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        if pick.searching {
            b.push_str(&clip_display(
                &search_input_line("search: ", &pick.search_ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let height = self
            .height
            .saturating_sub(2 + usize::from(pick.searching))
            .max(1);
        let total = self.file_pick_indices(pick).count();
        let cursor = pick.cursor.min(total.saturating_sub(1));
        let off = (cursor as i64 - height as i64 + 1).max(0) as usize;
        for (i, idx) in self
            .file_pick_indices(pick)
            .skip(off)
            .take(height)
            .enumerate()
        {
            let mut label = self.files[idx].opts.path.clone();
            if self.tab_dirty(idx) {
                label += &format!(" {}", style::diff("●"));
            }
            let line = if off + i == cursor {
                style::sel(&format!("> {label}"))
            } else {
                format!("  {label}")
            };
            b.push_str(&clip_display(&line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if total == 0 {
            b.push_str(&style::faint("(no matches)"));
            b.push('\n');
        }
        let hint = if pick.searching {
            "↑/↓ or tab move · enter switch · esc cancel search"
        } else {
            "↑/↓ or ctrl+j/k move · pgup/pgdn page · enter switch · / search · esc close"
        };
        self.finish_with_hint(b, self.rec_hint(hint))
    }
}

// matchedLeafVariant re-export (rows.rs) used by value_style
fn matched_leaf_variant_pub(
    schema: &Schema,
    node: FormNodeId,
    doc: &Doc,
    val: Option<NodeId>,
) -> Option<FormNodeId> {
    crate::ui::rows::matched_leaf_variant(schema, node, doc, val)
}

/// schemaAcceptsValue wrapper (rows.rs).
fn schema_accepts_value_(
    schema: &Schema,
    def: Option<FormNodeId>,
    doc: &Doc,
    v: Option<NodeId>,
) -> bool {
    crate::ui::rows::schema_accepts_value(schema, def, doc, v)
}

/// same_state by value (menu.rs same_state needs two MenuStates).
fn same_state_eq(a: &MenuState, b: &MenuState) -> bool {
    a.node == b.node
        && a.kind == b.kind
        && a.variant_of == b.variant_of
        && a.one_of == b.one_of
        && a.path == b.path
}

/// Severity marker for the issues status row.
enum Sev {
    Err,
    Warn,
}

impl Sev {
    fn render_str(&self, s: &str) -> String {
        match self {
            Sev::Err => style::fg1(s),
            Sev::Warn => style::fg3(s),
        }
    }
}

/// Value-summary style marker (Go valueStyle → style choice).
enum ValStyle {
    Default,
    Dim,
    Enum,
    Bool,
    Num,
    Str,
    Obj,
}

impl ValStyle {
    fn render_str(&self, s: &str) -> String {
        match self {
            ValStyle::Default => style::val_default(s),
            ValStyle::Dim => style::faint(s),
            ValStyle::Enum => style::val_enum(s),
            ValStyle::Bool => style::val_bool(s),
            ValStyle::Num => style::val_num(s),
            ValStyle::Str => style::val_str(s),
            ValStyle::Obj => style::bold(s),
        }
    }
}

#[cfg(test)]
mod tests;
