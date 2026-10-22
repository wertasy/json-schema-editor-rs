//! YAML layer: converts between YAML and the document tree (arena).
//!
//! Rust port of `internal/yamldoc/yamldoc.go` plus the parts of yaml.v3 it
//! leans on. Anchors are kept on the parsed nodes and alias occurrences
//! share the anchored `NodeId` (no auto-expansion): viewing and editing
//! through a reference reaches the anchored original, and `write` restores
//! `&anchor` / `*alias` — unless the reference was materialized (its anchor
//! identity dropped), which writes plain content.
//!
//! Parsing: two passes over yaml-rust2 (libyaml lineage — the same scanner
//! family as yaml.v3, so scalar resolution, anchors and merge keys agree):
//! a token pass (anchor names in registration order, item dash markers) and
//! an event pass (tree build). Comments are extracted by a quote- and
//! block-scalar-aware line scanner and attached to the six comment slots
//! with yaml.v3's observable rules (see `attach`). Writing: a faithful port
//! of yaml.v3's block-context emitter ([`emit`]) — indent 2, no width
//! folding (yaml.v3 leaves the width unset), flow style for empty
//! collections, literal blocks with chomping hints, and double-quote
//! escaping including astral characters.

mod emit;
mod resolve;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use yaml_rust2::parser::{Event, Parser, Tag};
use yaml_rust2::scanner::{Marker, Scanner, TScalarStyle as ScalarStyle, TokenType};

type P<'a> = Parser<std::str::Chars<'a>>;

use crate::doc::jsondoc::{self, Doc, DocError, Member, NodeId, PathSeg, Value};
use crate::doc::{CommentSet, Meta};

pub use emit::write;

pub(crate) use resolve::{PlainTag, normalize_num, resolve_plain};

/// Short form of a yaml-rust2 tag: `("!!", "str")` → `!!str`; a verbatim
/// `tag:yaml.org,2002:X` → `!!X`; anything else is None (unknown → string).
fn short_tag(tag: &Option<Tag>) -> Option<String> {
    let t = tag.as_ref()?;
    if t.handle == "!!" {
        return Some(format!("!!{}", t.suffix));
    }
    if let Some(s) = t.suffix.strip_prefix("tag:yaml.org,2002:") {
        return Some(format!("!!{s}"));
    }
    None
}

// --- comment extraction -------------------------------------------------------

/// One comment run: maximal consecutive comment lines (content lines break a
/// run). Interior blank lines are preserved as gaps so the joined text keeps
/// them, exactly like yaml.v3's comment text.
struct Run {
    start_line: usize,           // 1-based line of the first comment line
    indent: usize,               // column of the first '#'
    items: Vec<(usize, String)>, // (1-based line, text) in order
}

impl Run {
    /// The comment text: comment lines joined by one newline per source line
    /// crossed (blank lines become empty lines in the text).
    fn text(&self) -> String {
        let mut out = String::new();
        let mut prev: Option<usize> = None;
        for (line, text) in &self.items {
            if let Some(p) = prev {
                for _ in p..*line {
                    out.push('\n');
                }
            }
            out.push_str(text);
            prev = Some(*line);
        }
        out
    }
}

/// Extract comment runs: `#` at a scalar boundary starts a comment to EOL.
/// Quote- and block-scalar-aware, so `#` inside scalars or block content is
/// not a comment.
fn extract_comment_runs(src: &str) -> Vec<Run> {
    let b = src.as_bytes();
    let mut runs: Vec<Run> = Vec::new();
    let mut cur: Option<Run> = None;

    let mut i = 0usize;
    let mut line = 1usize;
    let mut line_start = 0usize; // index of the current line's first byte
    let mut in_single = false;
    let mut in_double = false;
    // Block scalar: the header line's indent; content is every following
    // line that is blank or indented deeper. None when not inside one.
    let mut block_header_indent: Option<usize> = None;

    let end_run = |cur: &mut Option<Run>, runs: &mut Vec<Run>| {
        if let Some(r) = cur.take() {
            if !r.items.is_empty() {
                runs.push(r);
            }
        }
    };

    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            end_run(&mut cur, &mut runs);
            i += 1;
            line += 1;
            line_start = i;
            in_single = false;
            in_double = false;
            // blank lines keep the block alive; a content line decides
            continue;
        }
        if let Some(header_indent) = block_header_indent {
            let col = i - line_start;
            // measure indentation of the first content line after blanks
            let mut j = i;
            let mut ind = col;
            while j < b.len() && b[j] == b' ' {
                ind += 1;
                j += 1;
            }
            let is_blank_line = j >= b.len() || b[j] == b'\n';
            if is_blank_line {
                // consume the blank line; block stays open
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if ind > header_indent {
                // block content line: consume verbatim
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            // dedented: block ends here, reprocess this line normally
            block_header_indent = None;
            continue;
        }
        if in_single {
            if c == b'\'' {
                if b.get(i + 1) == Some(&b'\'') {
                    i += 1;
                } else {
                    in_single = false;
                }
            }
            i += 1;
            continue;
        }
        if in_double {
            if c == b'\\' {
                i += 1;
                if i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if c == b'"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' {
            i += 1;
            continue;
        }
        if c == b'#' {
            let col = i - line_start;
            // scalar boundary: comment starts only after line start or a blank
            let boundary = i == line_start || b[i - 1] == b' ' || b[i - 1] == b'\t';
            if !boundary {
                // '#' mid-token (e.g. "a#b"): plain scalar content
                let mut j = i;
                while j < b.len() && b[j] != b'\n' {
                    j += 1;
                }
                i = j;
                continue;
            }
            let start = i;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            let text = String::from_utf8_lossy(&b[start..i])
                .trim_end_matches([' ', '\t', '\r'])
                .to_string();
            match cur.as_mut() {
                Some(r) => r.items.push((line, text)),
                None => {
                    cur = Some(Run {
                        start_line: line,
                        indent: col,
                        items: vec![(line, text)],
                    });
                }
            }
            continue;
        }
        // other content: closes the current run
        end_run(&mut cur, &mut runs);
        if c == b'\'' {
            in_single = true;
        } else if c == b'"' {
            in_double = true;
        } else if c == b'|' || c == b'>' {
            // block scalar header only in a value position: after ':', '-',
            // '?' or at line start (the preceding non-space check)
            let boundary = i == line_start
                || b[i - 1] == b' '
                || b[i - 1] == b'\t'
                || b[i - 1] == b':'
                || b[i - 1] == b'-'
                || b[i - 1] == b'?';
            if boundary {
                let mut j = i + 1;
                while j < b.len() && (b[j] == b'+' || b[j] == b'-' || b[j].is_ascii_digit()) {
                    j += 1;
                }
                let header_ok =
                    j >= b.len() || b[j] == b'\n' || b[j] == b' ' || b[j] == b'\t' || b[j] == b'#';
                if header_ok {
                    let header_indent = i - line_start;
                    // consume the rest of the header line
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                    block_header_indent = Some(header_indent);
                    continue;
                }
            }
        }
        i += 1;
    }
    end_run(&mut cur, &mut runs);
    runs
}

// --- parse ------------------------------------------------------------------

/// Structural site for comment attachment, in document order.
struct Site {
    line: usize, // 1-based: the key's line or the item's '-' line
    col: usize,
    path: Vec<PathSeg>, // path of the member/item value node
    kind: SiteKind,
    value_start_line: usize, // first line of the value (== line for inline)
}

enum SiteKind {
    Key,
    Item,
}

struct Ctx {
    doc: Doc,
    built: HashMap<usize, NodeId>, // anchor id → node (ids are 1-based)
    aliases: usize,
    names: Vec<String>, // anchor names in registration order (id-1)
    sites: Vec<Site>,
    dashes: Vec<(usize, usize)>, // BlockEntry (line, col), in order
    dash_idx: usize,
    path: Vec<PathSeg>,
}

/// parse reads one YAML document into a document tree, keeping anchors and
/// alias references (no expansion into copies) and capturing comments.
pub fn parse(text: &str) -> Result<(Doc, Meta), DocError> {
    parse_inner(text).map_err(|msg| DocError { msg })
}

fn parse_inner(text: &str) -> Result<(Doc, Meta), String> {
    // token pass: anchor names in registration order + item dash markers
    let mut names: Vec<String> = Vec::new();
    let mut dashes: Vec<(usize, usize)> = Vec::new();
    {
        let mut sc = Scanner::new(text.chars());
        while let Some(tok) = sc.next_token().map_err(|e| format!("yaml: {e}"))? {
            match tok.1 {
                TokenType::Anchor(name) => names.push(name),
                TokenType::BlockEntry => dashes.push((tok.0.line(), tok.0.col())),
                _ => {}
            }
        }
    }

    let mut ctx = Ctx {
        doc: Doc::new(),
        built: HashMap::new(),
        aliases: 0,
        names,
        sites: Vec::new(),
        dashes,
        dash_idx: 0,
        path: Vec::new(),
    };

    // event pass
    let mut p = Parser::new_from_str(text);
    let root: NodeId = loop {
        let (ev, mark) = p.next_token().map_err(|e| format!("yaml: {e}"))?;
        match ev {
            Event::StreamStart | Event::DocumentStart | Event::DocumentEnd | Event::Nothing => {
                continue;
            }
            Event::StreamEnd => return Err("empty YAML document".to_string()),
            ev => {
                let r = ctx.walk(ev, &mark, &mut p)?;
                // drain to stream end; any second document is an error
                loop {
                    let (ev2, _) = p.next_token().map_err(|e| format!("yaml: {e}"))?;
                    match ev2 {
                        Event::StreamEnd => break,
                        Event::DocumentEnd => continue,
                        _ => {
                            return Err("multiple YAML documents are not supported".to_string());
                        }
                    }
                }
                break r;
            }
        }
    };
    ctx.doc.root = root;

    let Ctx {
        doc,
        aliases,
        sites,
        ..
    } = ctx;
    let runs = extract_comment_runs(text);
    let mut meta = attach(&doc, sites, runs);
    meta.aliases = aliases;
    Ok((doc, meta))
}

impl Ctx {
    fn anchor_name(&self, id: usize) -> String {
        self.names
            .get(id.wrapping_sub(1))
            .cloned()
            .unwrap_or_default()
    }

    /// walk builds the subtree for one node, consuming nested events.
    fn walk(&mut self, ev: Event, _mark: &Marker, p: &mut P<'_>) -> Result<NodeId, String> {
        match ev {
            Event::Alias(id) => {
                self.aliases += 1;
                match self.built.get(&id) {
                    Some(&n) => Ok(n),
                    None => Err(format!("yaml: unknown anchor reference (id {id})")),
                }
            }
            Event::Scalar(value, style, anchor_id, tag) => {
                let node = self.scalar_node(&value, style, &tag)?;
                if anchor_id != 0 {
                    self.built.insert(anchor_id, node);
                    let name = self.anchor_name(anchor_id);
                    self.doc.node_mut(node).anchor = name;
                }
                Ok(node)
            }
            Event::SequenceStart(anchor_id, _) => {
                let node = self.doc.new_array();
                if anchor_id != 0 {
                    self.built.insert(anchor_id, node);
                    let name = self.anchor_name(anchor_id);
                    self.doc.node_mut(node).anchor = name;
                }
                self.walk_sequence(node, p)?;
                Ok(node)
            }
            Event::MappingStart(anchor_id, _) => {
                let node = self.doc.new_object();
                if anchor_id != 0 {
                    self.built.insert(anchor_id, node);
                    let name = self.anchor_name(anchor_id);
                    self.doc.node_mut(node).anchor = name;
                }
                self.walk_mapping(node, p)?;
                Ok(node)
            }
            _ => Err("unexpected event".to_string()),
        }
    }

    fn walk_sequence(&mut self, node: NodeId, p: &mut P<'_>) -> Result<(), String> {
        let base_path = std::mem::take(&mut self.path);
        let mut items: Vec<NodeId> = Vec::new();
        let mut idx = 0usize;
        loop {
            let (ev, mark) = p.next_token().map_err(|e| format!("yaml: {e}"))?;
            match ev {
                Event::SequenceEnd => break,
                ev => {
                    // the item's '-' marker comes from the token pass
                    let (item_line, item_col) = self
                        .dashes
                        .get(self.dash_idx)
                        .copied()
                        .unwrap_or((mark.line(), mark.col()));
                    self.dash_idx += 1;
                    let mut ipath = base_path.clone();
                    ipath.push(PathSeg::Idx(idx));
                    self.sites.push(Site {
                        line: item_line,
                        col: item_col,
                        path: ipath.clone(),
                        kind: SiteKind::Item,
                        value_start_line: item_line,
                    });
                    self.path = ipath;
                    let val = self.walk(ev, &mark, p)?;
                    items.push(val);
                    idx += 1;
                }
            }
        }
        self.path = base_path;
        if let Value::Array(a) = &mut self.doc.nodes[node as usize].value {
            *a = items;
        }
        Ok(())
    }

    fn walk_mapping(&mut self, node: NodeId, p: &mut P<'_>) -> Result<(), String> {
        let base_path = std::mem::take(&mut self.path);
        let mut members: Vec<Member> = Vec::new();
        loop {
            let (kev, kmark) = p.next_token().map_err(|e| format!("yaml: {e}"))?;
            let Event::Scalar(key_value, key_style, _key_anchor, key_tag) = kev else {
                if matches!(kev, Event::MappingEnd) {
                    break;
                }
                return Err(format!(
                    "non-string mapping key at {}: only string keys are supported",
                    jsondoc::path_string(&base_path)
                ));
            };
            let key_line = kmark.line();
            let key_col = kmark.col();

            let is_merge = match short_tag(&key_tag).as_deref() {
                Some("!!merge") => true,
                None => {
                    key_style == ScalarStyle::Plain && resolve_plain(&key_value) == PlainTag::Merge
                }
                _ => false,
            };
            if !is_merge {
                // only string keys are supported (yaml.v3 requires !!str)
                let string_key = match short_tag(&key_tag).as_deref() {
                    Some("!!str") => true,
                    Some(_) => false,
                    None => {
                        key_style != ScalarStyle::Plain
                            || resolve_plain(&key_value) == PlainTag::Str
                    }
                };
                if !string_key {
                    return Err(format!(
                        "non-string mapping key at {}: only string keys are supported",
                        jsondoc::path_string(&base_path)
                    ));
                }
            }

            let (vev, vmark) = p.next_token().map_err(|e| format!("yaml: {e}"))?;

            if is_merge {
                let refs = self.merge_refs(vev, node, p)?;
                let merge_val = self.doc.push(jsondoc::NodeData {
                    value: Value::Null,
                    anchor: String::new(),
                    merge: refs,
                    raw: String::new(),
                });
                let mut mpath = base_path.clone();
                mpath.push(PathSeg::Key("<<".to_string()));
                self.sites.push(Site {
                    line: key_line,
                    col: key_col,
                    path: mpath,
                    kind: SiteKind::Key,
                    value_start_line: key_line,
                });
                members.push(Member {
                    key: "<<".to_string(),
                    val: merge_val,
                });
                continue;
            }

            let mut mpath = base_path.clone();
            mpath.push(PathSeg::Key(key_value.clone()));
            // block collections start at their first content line; the
            // collection event's own marker sits on the key's line (libyaml
            // rolls the indent at the key)
            let value_start_line = match &vev {
                Event::MappingStart(..) | Event::SequenceStart(..) => {
                    p.peek().map(|(_, m)| m.line()).unwrap_or(vmark.line())
                }
                _ => vmark.line(),
            };
            self.sites.push(Site {
                line: key_line,
                col: key_col,
                path: mpath.clone(),
                kind: SiteKind::Key,
                value_start_line,
            });
            self.path = mpath.clone();
            let val = self.walk(vev, &vmark, p)?;
            members.push(Member {
                key: key_value,
                val,
            });
        }
        self.path = base_path;
        if let Value::Object(m) = &mut self.doc.nodes[node as usize].value {
            *m = members;
        }
        Ok(())
    }

    /// mergeRefs resolves the value of a "<<" merge key into the anchored
    /// mapping nodes it merges: one alias or a sequence of aliases.
    fn merge_refs(
        &mut self,
        ev: Event,
        self_node: NodeId,
        p: &mut P<'_>,
    ) -> Result<Vec<NodeId>, String> {
        let mut alias_ids: Vec<usize> = Vec::new();
        match ev {
            Event::Alias(id) => alias_ids.push(id),
            Event::SequenceStart(_, _) => loop {
                let (ev2, _) = p.next_token().map_err(|e| format!("yaml: {e}"))?;
                match ev2 {
                    Event::SequenceEnd => break,
                    Event::Alias(id) => alias_ids.push(id),
                    _ => {
                        return Err(
                            "YAML merge keys: the value must be an alias or a sequence of aliases"
                                .to_string(),
                        );
                    }
                }
            },
            _ => {
                return Err(
                    "YAML merge keys: the value must be an alias or a sequence of aliases"
                        .to_string(),
                );
            }
        }
        let mut refs = Vec::new();
        for id in alias_ids {
            self.aliases += 1;
            let Some(&j) = self.built.get(&id) else {
                // Unreachable for well-formed YAML: the scanner rejects
                // aliases before their anchors.
                return Err("YAML merge key references unknown anchor".to_string());
            };
            if j == self_node {
                return Err(format!(
                    "YAML merge key &{} would merge the mapping into itself",
                    self.anchor_name(id)
                ));
            }
            if !matches!(self.doc.value(j), Value::Object(_)) {
                return Err(format!(
                    "YAML merge key requires a mapping anchor: &{}",
                    self.anchor_name(id)
                ));
            }
            refs.push(j);
        }
        Ok(refs)
    }

    /// scalarNode converts one scalar event to a node (yaml.v3 resolution).
    fn scalar_node(
        &mut self,
        value: &str,
        style: ScalarStyle,
        tag: &Option<Tag>,
    ) -> Result<NodeId, String> {
        let stag = short_tag(tag);
        Ok(match stag.as_deref() {
            Some("!!null") => self.doc.new_null(),
            Some("!!bool") => match value {
                "true" | "True" | "TRUE" => self.doc.new_bool(true),
                "false" | "False" | "FALSE" => self.doc.new_bool(false),
                _ => return Err(format!("cannot decode {value:?} as a !!bool")),
            },
            Some("!!int") => {
                let lit = normalize_num("!!int", value).map_err(|e| e.msg)?;
                self.doc.new_num(lit)
            }
            Some("!!float") => {
                let lit = normalize_num("!!float", value).map_err(|e| e.msg)?;
                self.doc.new_num(lit)
            }
            _ => {
                if style == ScalarStyle::Plain && stag.is_none() {
                    match resolve_plain(value) {
                        PlainTag::Null => self.doc.new_null(),
                        PlainTag::Bool => {
                            self.doc.new_bool(matches!(value, "true" | "True" | "TRUE"))
                        }
                        PlainTag::Int => {
                            let lit = normalize_num("!!int", value).map_err(|e| e.msg)?;
                            self.doc.new_num(lit)
                        }
                        PlainTag::Float => {
                            let lit = normalize_num("!!float", value).map_err(|e| e.msg)?;
                            self.doc.new_num(lit)
                        }
                        // Timestamps survive as text; "<<" as a value is a
                        // string like any other.
                        PlainTag::Timestamp | PlainTag::Merge | PlainTag::Str => {
                            self.doc.new_str(value)
                        }
                    }
                } else {
                    self.doc.new_str(value)
                }
            }
        })
    }
}

// --- comment attachment -------------------------------------------------------

/// attach binds comment runs to the six comment slots, reproducing yaml.v3's
/// observable attachment:
///
/// - a run on its own line attaches as the HEAD of the next key/item when its
///   indentation is at or left of that site, else as the FOOT of the previous
///   member (head → KeyHead for keys, Head for items; foot → KeyFoot for keys
///   after decode's value→key redistribution, Foot for items);
/// - a run starting on a key's line with the value below is the key's line
///   comment (KeyLine), on an inline value's line the value's (Line);
/// - runs after the last member are the document tail (the last member's
///   KeyFoot, or the root value's Foot for non-mapping roots);
/// - header runs split at the last blank line: the part before is the
///   document root head, the rest the first key's KeyHead;
/// - run text keeps interior blank lines.
fn attach(doc: &Doc, sites: Vec<Site>, runs: Vec<Run>) -> Meta {
    let mut meta = Meta::default();
    let slot =
        |meta: &mut Meta, path: &[PathSeg], f: fn(&mut CommentSet) -> &mut String, text: &str| {
            let cs = meta.comments.entry(path.to_vec()).or_default();
            let cur = f(cs);
            if cur.is_empty() {
                *cur = text.to_string();
            } else {
                cur.push('\n');
                cur.push_str(text);
            }
        };

    let first_site = sites
        .iter()
        .min_by_key(|s| (s.line, s.col))
        .map(|s| (s.line, s.col));
    // split sites vs header runs
    let mut header_texts: Vec<(usize, String)> = Vec::new(); // (line, text)
    for r in &runs {
        let in_header = match first_site {
            Some((fl, _)) => r.start_line < fl,
            None => true,
        };
        if in_header {
            header_texts.push((r.start_line, r.text()));
        }
    }
    // header: split at the last blank-line boundary (only within one run can
    // a blank exist; runs are separated by content lines)
    if let Some(first) = sites.first().map(|s| s.path.clone()) {
        let is_mapping_root = matches!(doc.value(doc.root), Value::Object(_));
        // concatenate all header runs (they are each before the first site)
        let mut combined = Run {
            start_line: 0,
            indent: 0,
            items: Vec::new(),
        };
        for r in &runs {
            if header_texts.iter().any(|(l, _)| *l == r.start_line) {
                combined.items.extend(r.items.iter().cloned());
            }
        }
        if !combined.items.is_empty() {
            let split_idx = combined
                .items
                .windows(2)
                .rposition(|w| w[1].0 > w[0].0 + 1) // a blank line gap
                .map(|p| p + 1);
            match (split_idx, is_mapping_root) {
                (Some(idx), true) => {
                    let mut head_run = Run {
                        start_line: combined.start_line,
                        indent: combined.indent,
                        items: combined.items[..idx].to_vec(),
                    };
                    let mut key_run = Run {
                        start_line: combined.start_line,
                        indent: combined.indent,
                        items: combined.items[idx..].to_vec(),
                    };
                    let head_text = head_run.text();
                    let key_text = {
                        // the split lands mid-run: drop the leading blank gap
                        if let Some((_, first)) = key_run.items.first_mut() {
                            let _ = first;
                        }
                        key_run.start_line = key_run.items[0].0;
                        key_run.text()
                    };
                    let _ = &mut head_run;
                    let head_text = head_text.trim_start_matches('\n').to_string();
                    if !head_text.is_empty() {
                        slot(&mut meta, &[], |c| &mut c.head, &head_text);
                    }
                    if !key_text.is_empty() {
                        slot(&mut meta, &first, |c| &mut c.key_head, &key_text);
                    }
                }
                _ => {
                    let whole = combined.text();
                    if !whole.is_empty() {
                        slot(&mut meta, &[], |c| &mut c.head, &whole);
                    }
                }
            }
        }
    }

    // remaining runs in order
    for r in &runs {
        let in_header = match first_site {
            Some((fl, _)) => r.start_line < fl,
            None => true,
        };
        if in_header {
            continue;
        }
        let text = r.text();
        // 1. same line as a key/item site
        if let Some(site) = sites.iter().rev().find(|s| s.line == r.start_line) {
            if site.value_start_line == r.start_line {
                // inline value: the value's line comment
                slot(&mut meta, &site.path, |c| &mut c.line, &text);
                continue;
            }
            // block value below: the key's line comment
            if matches!(site.kind, SiteKind::Key) {
                slot(&mut meta, &site.path, |c| &mut c.key_line, &text);
                continue;
            }
        }
        // 2. own-line run: head of the next site or foot of the previous one
        let next = sites.iter().find(|s| s.line > r.start_line);
        match next {
            Some(n) => {
                if r.indent > n.col {
                    // foot of the previous member
                    if let Some(prev) = sites.iter().rev().find(|s| s.line < r.start_line) {
                        match prev.kind {
                            SiteKind::Key => {
                                slot(&mut meta, &prev.path, |c| &mut c.key_foot, &text);
                            }
                            SiteKind::Item => {
                                slot(&mut meta, &prev.path, |c| &mut c.foot, &text);
                            }
                        }
                    } else {
                        slot(&mut meta, &[], |c| &mut c.foot, &text);
                    }
                } else {
                    match n.kind {
                        SiteKind::Key => {
                            slot(&mut meta, &n.path, |c| &mut c.key_head, &text);
                        }
                        SiteKind::Item => {
                            slot(&mut meta, &n.path, |c| &mut c.head, &text);
                        }
                    }
                }
            }
            None => {
                // after everything: document tail
                match doc.value(doc.root) {
                    Value::Object(members) => {
                        if let Some(last) = members.last() {
                            let mut path: Vec<PathSeg> = Vec::new();
                            path.push(PathSeg::Key(last.key.clone()));
                            slot(&mut meta, &path, |c| &mut c.key_foot, &text);
                        } else {
                            slot(&mut meta, &[], |c| &mut c.foot, &text);
                        }
                    }
                    Value::Array(items) => {
                        if let Some(&last) = items.last() {
                            // find the item's path: it is the deepest Idx site
                            let idx = items.iter().position(|&x| x == last).unwrap_or(0);
                            let path = vec![PathSeg::Idx(idx)];
                            slot(&mut meta, &path, |c| &mut c.foot, &text);
                        } else {
                            slot(&mut meta, &[], |c| &mut c.foot, &text);
                        }
                    }
                    _ => {
                        slot(&mut meta, &[], |c| &mut c.foot, &text);
                    }
                }
            }
        }
    }
    meta
}

// --- schema directive (yaml-language-server comment) --------------------------

/// Matches `# yaml-language-server: $schema=<location>` (schemaDirectiveRE).
fn directive_loc(line: &str) -> Option<String> {
    let t = line.trim();
    let rest = t.strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("yaml-language-server:")?.trim_start();
    let rest = rest.strip_prefix("$schema=")?;
    let loc = rest.trim();
    if loc.is_empty() {
        None
    } else {
        Some(loc.to_string())
    }
}

fn is_directive(line: &str) -> bool {
    directive_loc(line).is_some()
}

/// headerPaths lists the comment regions a file header can live in: the
/// document root and the first mapping member.
fn header_paths(doc: &Doc) -> Vec<Vec<PathSeg>> {
    let mut paths = vec![Vec::new()];
    if let Value::Object(members) = doc.value(doc.root) {
        if let Some(first) = members.first() {
            paths.push(vec![PathSeg::Key(first.key.clone())]);
        }
    }
    paths
}

/// SchemaDirective returns the schema location recorded by a
/// yaml-language-server comment in the document header, if any.
pub fn schema_directive(doc: &Doc, meta: &Meta) -> Option<String> {
    for path in header_paths(doc) {
        let Some(cs) = meta.comments.get(&path) else {
            continue;
        };
        for head in [&cs.head, &cs.key_head] {
            for line in head.split('\n') {
                if let Some(loc) = directive_loc(line) {
                    return Some(loc);
                }
            }
        }
    }
    None
}

/// SetSchemaDirective records loc as a yaml-language-server comment when the
/// header has no directive yet. The line goes into the document-root comment
/// set, so it stays at the top of the file.
pub fn set_schema_directive(doc: &Doc, meta: &mut Meta, loc: &str) {
    if loc.is_empty() {
        return;
    }
    if schema_directive(doc, meta).is_some() {
        return;
    }
    let line = format!("# yaml-language-server: $schema={loc}");
    let cs = meta.comments.entry(Vec::new()).or_default();
    if cs.head.is_empty() {
        cs.head = line;
    } else {
        cs.head = format!("{line}\n{}", cs.head);
    }
}

/// ClearSchemaDirective removes yaml-language-server directives from the
/// document header (a --pick must be able to replace them).
pub fn clear_schema_directive(doc: &Doc, meta: &mut Meta) {
    for path in header_paths(doc) {
        let Some(cs) = meta.comments.get_mut(&path) else {
            continue;
        };
        let mut changed = false;
        for head in [&mut cs.head, &mut cs.key_head] {
            if head.is_empty() {
                continue;
            }
            let kept: Vec<&str> = head.split('\n').filter(|l| !is_directive(l)).collect();
            if kept.len() != head.split('\n').count() {
                changed = true;
            }
            *head = kept.join("\n");
        }
        if !changed {
            continue;
        }
        if *cs == CommentSet::default() {
            meta.comments.remove(&path);
        }
    }
}
#[cfg(test)]
mod difftest;
