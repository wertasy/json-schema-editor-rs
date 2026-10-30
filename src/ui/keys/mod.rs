//! Key handling for the menu TUI, plus its rendering (the Go menu_keys.go /
//! menu_copy.go / menu_find.go / menu_merge.go / menu_alias.go /
//! menu_paste.go / menu_select.go port, and menu.go's View layer).
//!
//! Every edit writes through to the document immediately; overlays are
//! handled by the [`App`] methods `dispatch_key` routes to. Each overlay
//! handler returns the overlay to keep (None closes it), matching menu.rs's
//! `self.overlay = match ov { … }` dispatch.

use std::collections::HashSet;

use regex::Regex;

use crate::doc::jsondoc;
use crate::doc::{CopyMode, Doc, Member, NodeData, NodeId, PathSeg, Value, append_idx, append_key};
use crate::icons::{ERR, WARN};
use crate::smodel::{EnumOpt, FormNode, FormNodeId, Kind, Schema};
use crate::ui::command;
use crate::ui::menu::{
    AddState, AnchorMode, AnchorState, App, CommandState, CommentEditState, CommentState,
    ConfirmState, DESC_BLOCK_PREVIEW_LINES, DescLine, DescSource, EditState, ErrsState,
    ExternalEdit, FastMoveState, FilePickState, FilterAction, HelpState, IssuesState, ListFilter,
    MergeState, Msg, Overlay, RefPickState, SearchState, ShellOutState, TextState, WriteState,
    anchor_defs, anchor_holder, anchor_holder_path, anchor_merge_use_paths, anchor_role,
    count_anchor_uses, cursor_ancestor, cursor_by_path, cursor_for_row, cursor_prefix,
    is_path_prefix, merge_ref_uses, nodes_equal, nodes_equal_doc, page_delta, read_clipboard,
    ref_would_cycle, remap_state_cursor, vertical_delta, write_clipboard,
};
use crate::ui::rows::{
    SynthKey, any_type_menu, clip_runes, current_variant, display_title, dynamic_variants,
    filter_search, is_dot_query, is_path_query, match_variant_list, matched_leaf_variant,
    path_pattern, raw_value_summary, rebuild_stack, replace_all, replace_hits, search_index,
    seed_variant_cursor, summary, variant_field_defs, variant_signature, variant_value_summary,
};
use crate::ui::shell::{self, ShellTarget};
use crate::ui::util::{page_cursor, wrap_cursor};
use crate::ui::widgets::{Key, TextArea, TextInput, apply_scalar};
use crate::ui::{Issue, MenuKind, MenuState, Row, RowKind};

/// MAX_CMD_HISTORY caps the `:` command history ring; the oldest entry
/// drops first.
pub const MAX_CMD_HISTORY: usize = 100;

/// CMD_SUG_WINDOW caps the visible tab-completion candidates above the
/// command prompt.
pub const CMD_SUG_WINDOW: usize = 8;

/// Candidate overlays size to their content with a readable floor; the
/// terminal's available width is the only ceiling.
pub const SUG_MIN_WIDTH: usize = 24;
/// Candidates sharing at least this many leading characters hide the shared
/// prefix so their differing tails stay visible.
pub const SUG_STRIP_PREFIX_MIN: usize = 8;

/// cmd_sug_refresh rebuilds the auto-completion list for the word before
/// the cursor without choosing a candidate; tab/arrows select from it.
/// The scan is cheap except at a word start: a fresh shell argument word
/// runs bash's programmable completion once (BashCache), later keystrokes
/// filter the fetched words locally.
fn cmd_sug_refresh(s: &mut CommandState, file_path: &str) {
    let line = s.ti.value().to_string();
    let cursor = s.ti.cursor_byte();
    let done = s.bash_cache.auto_candidates(&line, cursor, file_path);
    cmd_sug_apply(s, done, cursor);
}

/// cmd_tab_refresh is the fresh scan of an explicit tab press: it always
/// re-asks bash's programmable completion for the word's full context
/// (shell argument words only) and stores the fetch for later filtering.
fn cmd_tab_refresh(s: &mut CommandState, file_path: &str) {
    let line = s.ti.value().to_string();
    let cursor = s.ti.cursor_byte();
    let done = s.bash_cache.tab_candidates(&line, cursor, file_path);
    cmd_sug_apply(s, done, cursor);
}

/// cmd_sug_apply stores one scan result as the active completion list.
fn cmd_sug_apply(s: &mut CommandState, done: command::Completion, cursor: usize) {
    s.sug_prefix = done.prefix;
    s.sug = done.candidates;
    s.sug_idx = None;
    s.sug_line.clear();
    s.sug_start = done.start;
    s.sug_end = cursor;
    // The candidate window aligns with the word being completed: deriving
    // the anchor from the token start keeps the window still while the user
    // types inside one word; a separator that starts a new word re-anchors.
    s.sug_col = display_width(&s.ti.value()[..s.sug_start]);
}

/// cmd_complete applies the next/previous completion. A fresh cycle scans the
/// word before the cursor (builtin command, PATH command, bash completion or
/// filesystem path) and inserts the candidate there, leaving the text after
/// the cursor alone; repeated tab presses walk the same candidate list.
fn cmd_complete(s: &mut CommandState, forward: bool, file_path: &str) {
    let value = s.ti.value().to_string();
    let inserted = &value[s.sug_start.min(value.len())..s.sug_end.min(value.len())];
    // Entering a directory or the shell marker needs a fresh scan, not a
    // cycle over the single applied candidate.
    if s.sug_idx.is_none()
        || value != s.sug_line
        || inserted.ends_with(['/', '\\'])
        || inserted == "!"
    {
        cmd_tab_refresh(s, file_path);
        if s.sug.is_empty() {
            return;
        }
        let idx = if forward { 0 } else { s.sug.len() - 1 };
        cmd_apply_sug(s, idx);
        return;
    }
    cmd_complete_step(s, forward);
}

/// cycle_text_sugs moves a TextInput completion cursor over a stored candidate
/// list and writes the selected candidate. Used by arrows once completion is
/// active, so the list is not rebuilt from the completed value (which would
/// shrink it to that one candidate).
fn cycle_text_sugs(ti: &mut TextInput, sugs: &[String], idx: &mut i64, delta: i64) -> bool {
    if sugs.is_empty() {
        return false;
    }
    let n = sugs.len() as i64;
    *idx = if *idx < 0 {
        if delta > 0 { 0 } else { n - 1 }
    } else {
        (*idx + delta).rem_euclid(n)
    };
    ti.set_value(sugs[*idx as usize].clone());
    ti.cursor_end();
    true
}

/// cmd_completion_active reports whether the input still holds a candidate
/// from the current completion list (so arrows should move within it).
fn cmd_completion_active(s: &CommandState) -> bool {
    s.sug_idx.is_some() && s.ti.value() == s.sug_line && s.ti.cursor_byte() == s.sug_end
}

/// cmd_complete_step moves the candidate cursor without rescanning the
/// token; tab and, while active, the arrow keys use this.
fn cmd_complete_step(s: &mut CommandState, forward: bool) {
    let Some(i) = s.sug_idx else { return };
    let n = s.sug.len();
    if n == 0 {
        return;
    }
    let idx = if forward {
        (i + 1) % n
    } else {
        (i + n - 1) % n
    };
    cmd_apply_sug(s, idx);
}

fn cmd_apply_sug(s: &mut CommandState, idx: usize) {
    let candidate = s.sug[idx].clone();
    s.ti.replace_range(s.sug_start, s.sug_end, &candidate);
    s.sug_end = s.sug_start + candidate.len();
    s.sug_line = s.ti.value().to_string();
    s.sug_idx = Some(idx);
    s.hist_idx = None;
}

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

/// enumIndex returns the cursor position of the current (or default) value
/// in fn's option list.
fn enum_index(schema: &Schema, fn_: FormNodeId, vdoc: &Doc, v: Option<NodeId>) -> usize {
    let f = &schema.nodes[fn_ as usize];
    let mut enc = String::new();
    if let Some(v) = v {
        if !matches!(vdoc.value(v), Value::Null) {
            enc = vdoc.encode_value(v);
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
        let de = schema.doc.encode_value(d);
        for (i, o) in f.enum_.iter().enumerate() {
            if o.encoded == de {
                return i;
            }
        }
    }
    0
}

/// listSearchMatch reports whether text contains the query
/// (case-insensitive); an empty query matches everything.
fn list_search_match(q: &str, text: &str) -> bool {
    q.is_empty() || text.to_lowercase().contains(&q.to_lowercase())
}

/// filter_enum_keywords keeps options whose display contains the query as
/// a case-insensitive substring (keyword search in the enum dropdown); an
/// empty query keeps every option. Unlike filter_completions an exact
/// match stays listed: the input text itself names a selectable option.
fn filter_enum_keywords(opts: &[EnumOpt], query: &str) -> Vec<String> {
    let lq = query.to_lowercase();
    opts.iter()
        .filter(|o| lq.is_empty() || o.display.to_lowercase().contains(&lq))
        .map(|o| o.display.clone())
        .collect()
}

/// enumChoiceError names the options a failed enum input must choose from,
/// spelled as JSON literals so empty-string members stay visible.
fn enum_choice_error(opts: &[EnumOpt]) -> String {
    let names: Vec<&str> = opts.iter().map(|o| o.encoded.as_str()).collect();
    format!("must be one of: {}", names.join(", "))
}

/// issueIndices lists the issues matching the overlay's search query.
fn issue_indices(s: &IssuesState) -> impl Iterator<Item = usize> + '_ {
    let q = s.filter.ti.value();
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
    let q = s.filter.ti.value();
    s.lines
        .iter()
        .enumerate()
        .filter(move |(_, line)| list_search_match(q, line))
        .map(|(i, _)| i)
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
    Some(jsondoc::number_text(sum))
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

/// clip_keep_tail clips `body` so a protected tail of `keep` cells (change
/// dot, severity glyph) still fits into `avail` cells, marking the cut with
/// an ellipsis. A body that already fits comes back unchanged; the caller
/// appends the tail itself.
pub(crate) fn clip_keep_tail(body: &str, avail: usize, keep: usize) -> String {
    let keep = keep.min(avail);
    let budget = avail.saturating_sub(keep);
    if display_width(body) <= budget {
        return body.to_string();
    }
    if budget < 2 {
        return String::new();
    }
    let mut out = clip_display(body, budget - 1);
    out.push('…');
    out
}

/// strip_query removes the already-typed prefix from a candidate. Matching is
/// case-insensitive, mirroring suggestion filtering; the returned slice is
/// the differing tail.
fn strip_query<'a>(candidate: &'a str, query: &str) -> Option<&'a str> {
    if query.is_empty() {
        return None;
    }
    let mut end = 0usize;
    let mut cand = candidate.char_indices();
    for q in query.chars() {
        let Some((idx, c)) = cand.next() else {
            return None;
        };
        if c != q && !c.to_lowercase().eq(q.to_lowercase()) {
            return None;
        }
        end = idx + c.len_utf8();
    }
    Some(&candidate[end..])
}

/// clip_left_ellipsis keeps the end of plain text within max cells, marking
/// the cut with a leading `…`. The tail is never dropped: it is the part that
/// distinguishes candidates sharing a long prefix.
fn clip_left_ellipsis(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut w = 0usize;
    for c in s.chars().rev() {
        let cw = char_width(c);
        if w + cw > max - 1 {
            break;
        }
        tail.push(c);
        w += cw;
    }
    let mut out = String::from("…");
    out.extend(tail.into_iter().rev());
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
        Msg::Ctrl('y') => Key::CtrlY,
        _ => return None,
    })
}

/// input_avail is the viewport width of a TextInput rendered after `prefix`
/// cells on a line clipped to `width`: the caret cell and one margin column
/// stay inside the clip.
pub(crate) fn input_avail(width: usize, prefix: usize) -> usize {
    width.saturating_sub(prefix + 2)
}

/// ti_window returns the char range [start, end) of a TextInput value that a
/// viewport of `avail` cells shows so the cursor cell stays visible, plus
/// the display width skipped before `start`. Long values scroll horizontally
/// around the caret instead of running past the terminal edge (the schema
/// picker's inputLine behavior, bubbles textinput with a Width).
pub(crate) fn ti_window(ti: &TextInput, avail: usize) -> (usize, usize, usize) {
    let chars: Vec<char> = ti.value().chars().collect();
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
    (start, end, cell_at(start))
}

/// ti_view renders a TextInput viewport of `avail` cells like bubbles
/// textinput.View(): the value with the cursor cell reverse-video, scrolled
/// horizontally so the caret stays visible. Returns the skipped leading
/// cells so callers can map value columns (sug_col) to screen columns.
pub(crate) fn ti_view(ti: &TextInput, avail: usize) -> (usize, String) {
    let (start, end, shift) = ti_window(ti, avail);
    let chars: Vec<char> = ti.value().chars().collect();
    let cursor = ti.cursor().min(chars.len());
    let mut out = String::new();
    if cursor < end {
        out.push_str(&chars[start..cursor].iter().collect::<String>());
        out.push_str("\x1b[7m");
        out.push(chars[cursor]);
        out.push_str("\x1b[27m");
        out.push_str(&chars[cursor + 1..end].iter().collect::<String>());
    } else {
        out.push_str(&chars[start..end].iter().collect::<String>());
        out.push_str("\x1b[7m \x1b[27m");
    }
    (shift, out)
}

/// inputLine renders `prompt + value` with a reverse-video caret, scrolling
/// horizontally so the caret stays visible (the schema picker's search row).
fn search_input_line(prompt: &str, ti: &TextInput, width: usize) -> String {
    let (_, input) = ti_view(ti, input_avail(width, display_width(prompt)));
    format!("{prompt}{input}")
}

/// search_field_line renders one overlay input row. Only the focused input
/// shows the reverse-video caret; the inactive row is dimmed and caret-less
/// so tabbing between search and replace is visible.
fn search_field_line(label: &str, ti: &TextInput, focused: bool, width: usize) -> String {
    if focused {
        let (_, input) = ti_view(ti, input_avail(width, display_width(label) + 2));
        format!("{label}: {input}")
    } else {
        style::faint(&format!("{label}: {}", ti.value()))
    }
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
    /// Rows available to a scrolling overlay list: the screen minus the
    /// title/footer chrome, an optional extra line and the filter input.
    pub(crate) fn overlay_page_size(&self, extra: usize, filtering: bool) -> usize {
        self.height
            .saturating_sub(2 + extra + usize::from(filtering))
            .max(1)
    }

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
    pub(super) fn move_page(&mut self, delta: i64) {
        let rows = self.menu_rows();
        self.move_page_rows(delta, &rows);
    }

    pub(super) fn move_page_rows(&mut self, delta: i64, rows: &[Row]) {
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
    pub(super) fn move_plain(&mut self, delta: i64) {
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
    pub(super) fn list_page_size(&self, st: &MenuState, rows: &[Row]) -> usize {
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
        if st.kind == MenuKind::OneOf && st.filter.active {
            summary_lines += 1;
        }
        self.height
            .saturating_sub(
                2 + 1
                    + desc_lines
                    + summary_lines
                    + self.status_lines().len()
                    + self.notice_lines().len()
                    + usize::from(self.hide_empty),
            )
            .max(1)
    }

    // --- schema switching (S) ----------------------------------------------

    /// openSchemaPicker searches the SchemaStore catalog for the active file
    /// and switches the form model to the chosen schema. The document body is
    /// untouched apart from the recorded schema location.
    pub(super) fn open_schema_picker(&mut self) {
        if self.opts().accessible {
            return; // no key events in the line protocol
        }
        if self.opts().load_schema.is_none() {
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
        let base = crate::paths::file_base(&self.opts().path);
        let chosen = crate::ui::schema_picker::pick(
            &entries,
            &crate::ui::schema_picker::PickOptions {
                accessible: false,
                filename: base.to_string(),
                query: crate::paths::search_stem(base),
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
    pub(super) fn apply_schema(&mut self, chosen: &crate::catalog::Entry) {
        let loc = chosen.url.clone();
        let Some(load) = self.opts().load_schema.clone() else {
            self.notice = "schema switching unavailable".to_string();
            return;
        };
        let crate::ui::LoadedSchema {
            schema: model,
            validate,
            status,
            schema_member,
            ..
        } = match load(&loc) {
            Ok(loaded) => loaded,
            Err(e) => {
                self.notice = format!("load schema: {e}");
                return;
            }
        };
        self.fs_mut().replace_schema_location(&loc, schema_member);

        let f = self.fs_mut();
        f.schema = model;
        f.menu_cache.clear();
        f.opts.validate = validate;
        f.opts.status = status;
        f.issues_parked = None;
        f.issues_return = 0;
        f.issues_origin = None;
        f.search_parked = None;
        f.change_jump = None;
        f.jump_origin = None;
        f.jump_return = 0;
        f.stack = crate::ui::files::root_stack(&f.schema, &f.doc);

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
            "ctrl+j" => self.adaptive_jump(&rows, 1),
            "ctrl+k" => self.adaptive_jump(&rows, -1),
            "n" => self.move_to_value(&rows, 1),
            "N" => self.move_to_value(&rows, -1),
            // y/p/P/x/X/d act on the selection: a live visual select ends
            // first, then the operation runs on the marks
            "p" => {
                self.end_visual_for_op();
                self.paste_clipboard(&r, false);
            }
            "P" => {
                self.end_visual_for_op();
                self.paste_clipboard(&r, true);
            }
            "y" => {
                self.end_visual_for_op();
                self.copy_row_or_selection(&r);
            }
            "Y" => self.copy_row(&r, false),
            "x" => {
                self.end_visual_for_op();
                if st_kind == Some(MenuKind::Items) {
                    self.cut_row_or_selection(&r, false);
                } else {
                    self.cut_row_or_selection(&r, true);
                }
            }
            "X" => {
                self.end_visual_for_op();
                self.cut_row_or_selection(&r, false);
            }
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
            "d" | "delete" => {
                self.end_visual_for_op();
                self.delete_row(&r);
            }
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
                    self.insert_item(&r, true);
                } else {
                    self.add_key(&r, false);
                }
            }
            "A" => {
                if st_kind == Some(MenuKind::Items) {
                    self.insert_item(&r, false);
                } else {
                    self.add_key(&r, true);
                }
            }
            "c" => self.toggle_desc(&r, &rows),
            "C" => self.edit_comment_row(&r),
            "?" => self.overlay = Some(Overlay::Help(HelpState::default())),
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
                    self.overlay = Some(Overlay::FilePick(FilePickState {
                        cursor: self.cur,
                        filter: ListFilter {
                            restore: self.cur,
                            ..Default::default()
                        },
                    }));
                } else {
                    self.notice = "only one file is open".to_string();
                }
            }
            "S" => self.open_schema_picker(),
            "ctrl+s" | "w" => self.save(),
            "W" => self.open_write_to(),
            ":" => self.open_command(),
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
    pub(super) fn move_to_value(&mut self, rows: &[Row], delta: i64) {
        let Some(mut st) = self.top().cloned() else {
            return;
        };
        crate::ui::rows::move_to_value(&self.ctx(), &mut st, rows, delta);
        if let Some(t) = self.top_mut() {
            t.cursor = st.cursor;
        }
    }

    /// activate performs the enter action for a row (Go activate).
    pub(super) fn activate(&mut self, r: Row) {
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
    pub(super) fn space_key(&mut self, r: Row) {
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
            filter: Default::default(),
        };
        {
            let f = self.fs_mut();
            seed_variant_cursor(&f.schema, &mut st, &f.doc);
        }
        self.fs_mut().stack.push(st);
    }

    /// pushChildMenu drills into a child node (Go pushChildMenu).
    pub(super) fn push_child_menu(&mut self, fn_: FormNodeId, path: Vec<PathSeg>) {
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
    pub(super) fn count_armed(&self) -> bool {
        self.list_mode() && self.top().map(|st| st.kind) == Some(MenuKind::Items)
    }

    /// jumpItem moves the cursor to the item row with the given array index,
    /// clamped to the last item (Go jumpItem).
    pub(super) fn jump_item(&mut self, cursor: &mut usize, idx: usize) -> bool {
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
    pub(super) fn fast_move_to(&mut self, ch: char, backward: bool) {
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
    pub(super) fn toggle_select(&mut self, r: &Row) {
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

    /// endVisualForOp ends a live visual select before a selection-consuming
    /// operation runs: the marks stay until the operation itself consumes or
    /// clears them, and j/k afterwards navigate plainly again.
    pub(super) fn end_visual_for_op(&mut self) {
        if let Some(st) = self.top_mut() {
            st.visual = false;
        }
    }

    /// clearStaleMarks drops a live selection after a paste moved rows: the
    /// marks are index paths, so an insert would silently re-target them.
    pub(super) fn clear_stale_marks(&mut self) {
        if let Some(st) = self.top_mut() {
            st.sel.clear();
        }
    }

    /// visualSync rebuilds the selection while visual select is on.
    pub(super) fn visual_sync(&mut self, rows: &[Row]) {
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
    pub(super) fn visual_jump(&mut self, rows: &[Row], from: usize) {
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
    pub(super) fn row_selected(&self, r: &Row) -> bool {
        match self.top() {
            Some(st) => !r.path.is_empty() && st.sel.contains(&sel_key(&r.path)),
            None => false,
        }
    }
}

mod clipboard;
mod edit;
mod overlays;
mod render;
mod search;

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
