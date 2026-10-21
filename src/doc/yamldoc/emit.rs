//! YAML writer: a faithful port of the block-context subset of yaml.v3's
//! emitter (emitterc.go + the node encoding of encode.go), which is what
//! yamldoc.Write drives: block mappings/sequences with indent 2, flow style
//! only for empty collections, anchors at the first occurrence of an anchored
//! node with alias references after it, merge keys as an alias or a block
//! sequence of aliases, head/line/foot/tail comment slots, literal blocks
//! with chomping hints for multiline strings, and resolve-based quoting.
//! Width folding never fires: yaml.v3 leaves the emitter width unset, which
//! resolves to an unlimited width.
//!
//! Structure: the document is compiled into the same event stream yaml.v3's
//! encoder would produce (`build_events`), then consumed by a direct port of
//! the emitter state machine (`Emitter::run`).

use std::collections::HashSet;

use super::resolve_plain;
use crate::doc::jsondoc::{Doc, DocError, NodeId, PathSeg, Value};
use crate::doc::{CommentSet, Meta};

fn err<T>(msg: impl Into<String>) -> Result<T, DocError> {
    Err(DocError { msg: msg.into() })
}

// --- event stream ------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Plain,
    Single,
    Double,
    Literal,
}

#[derive(Clone, Debug)]
enum Ev {
    StreamStart,
    StreamEnd,
    DocStart,
    DocEnd,
    SeqStart {
        anchor: Option<String>,
        head: String,
    },
    SeqEnd {
        line: String,
        foot: String,
    },
    MapStart {
        anchor: Option<String>,
        head: String,
    },
    MapEnd {
        tail: String,
        line: String,
        foot: String,
    },
    Scalar {
        value: String,
        anchor: Option<String>,
        style: Style,
        head: String,
        line: String,
        foot: String,
        tail: String,
    },
    Alias {
        name: String,
        head: String,
        line: String,
        foot: String,
    },
}

impl Ev {
    /// The event's head comment (analyze_event).
    fn head(&self) -> &str {
        match self {
            Ev::Scalar { head, .. } | Ev::Alias { head, .. } => head,
            Ev::SeqStart { head, .. } | Ev::MapStart { head, .. } => head,
            _ => "",
        }
    }
    fn line(&self) -> &str {
        match self {
            Ev::Scalar { line, .. } | Ev::Alias { line, .. } => line,
            Ev::SeqEnd { line, .. } | Ev::MapEnd { line, .. } => line,
            _ => "",
        }
    }
    fn foot(&self) -> &str {
        match self {
            Ev::Scalar { foot, .. } | Ev::Alias { foot, .. } => foot,
            Ev::SeqEnd { foot, .. } | Ev::MapEnd { foot, .. } => foot,
            _ => "",
        }
    }
    fn tail(&self) -> &str {
        match self {
            Ev::Scalar { tail, .. } => tail,
            Ev::MapEnd { tail, .. } => tail,
            _ => "",
        }
    }
    fn is_end(&self) -> bool {
        matches!(self, Ev::SeqEnd { .. } | Ev::MapEnd { .. })
    }
}

/// The comment slots of one member: key slots and value slots share the
/// member's path (yamldoc stores them at the member path).
fn cs_of(meta: Option<&Meta>, path: &[PathSeg]) -> CommentSet {
    match meta {
        Some(m) => m.comments.get(path).cloned().unwrap_or_default(),
        None => CommentSet::default(),
    }
}

/// Scalar presentation (encode.go e.node): `!!str`-tagged nodes quote when
/// their plain form would resolve to a non-string tag (forceQuoting);
/// multiline strings use literal blocks. Typed scalars (numbers, bools,
/// null) and the tagless merge key render plain. Tags always resolve back
/// identically and are dropped.
fn scalar_event(
    value: &str,
    anchor: Option<String>,
    force_quote: bool,
    head: String,
    line: String,
    foot: String,
    tail: String,
) -> Ev {
    let style = if value.contains('\n') {
        Style::Literal
    } else if force_quote && resolve_plain(value) != super::PlainTag::Str {
        Style::Double
    } else {
        Style::Plain
    };
    Ev::Scalar {
        value: value.to_string(),
        anchor,
        style,
        head,
        line,
        foot,
        tail,
    }
}

fn typed_scalar_event(id: NodeId, doc: &Doc, anchor: Option<String>, cs: &CommentSet) -> Ev {
    let text = scalar_text(doc, id);
    scalar_event(
        &text,
        anchor,
        false,
        cs.head.clone(),
        cs.line.clone(),
        cs.foot.clone(),
        String::new(),
    )
}

/// build_events compiles the document into the encoder's event stream.
/// Anchored nodes emit their anchor at the first occurrence; later
/// occurrences (shared NodeIds) become alias references. Merge members are
/// re-emitted as the "<<" key plus an alias or a block sequence of aliases.
/// With meta == None merges resolve inline and anchors are ignored (plain
/// value export).
struct Builder<'a> {
    doc: &'a Doc,
    meta: Option<&'a Meta>,
    emitted: HashSet<String>,
    evs: Vec<Ev>,
}

impl<'a> Builder<'a> {
    fn build(mut self) -> Vec<Ev> {
        self.evs.push(Ev::StreamStart);
        self.evs.push(Ev::DocStart);
        let cs = cs_of(self.meta, &[]);
        self.node(self.doc.root, &[], &cs);
        self.evs.push(Ev::DocEnd);
        self.evs.push(Ev::StreamEnd);
        self.evs
    }

    fn anchor_of(&self, id: NodeId) -> Option<String> {
        let a = self.doc.node(id).anchor.clone();
        (!a.is_empty() && self.meta.is_some()).then_some(a)
    }

    fn node(&mut self, id: NodeId, path: &[PathSeg], cs: &CommentSet) {
        // alias occurrence: an anchored node reached again emits a reference
        let node = self.doc.node(id);
        if self.meta.is_some() && !node.anchor.is_empty() {
            if !self.emitted.insert(node.anchor.clone()) {
                self.evs.push(Ev::Alias {
                    name: node.anchor.clone(),
                    head: cs.head.clone(),
                    line: cs.line.clone(),
                    foot: cs.foot.clone(),
                });
                return;
            }
        }
        match self.doc.value(id) {
            Value::Object(_) => {
                let anchor = self.anchor_of(id);
                self.evs.push(Ev::MapStart {
                    anchor,
                    head: cs.head.clone(),
                });
                let resolved = if self.meta.is_none() {
                    self.doc.resolved_members(id)
                } else {
                    match self.doc.value(id) {
                        Value::Object(m) => m.clone(),
                        _ => Vec::new(),
                    }
                };
                let mut tail = String::new();
                for m in &resolved {
                    let mut mpath = path.to_vec();
                    mpath.push(PathSeg::Key(m.key.clone()));
                    let kcs = cs_of(self.meta, &mpath);
                    let merge_refs: Vec<NodeId> = if self.meta.is_some() {
                        self.doc.nodes[m.val as usize].merge.clone()
                    } else {
                        Vec::new()
                    };
                    // the key: strings quote like any string value, except the
                    // tagless merge key which stays plain; the key's foot
                    // moves to the next key's tail (or the map end)
                    self.evs.push(scalar_event(
                        &m.key,
                        None,
                        merge_refs.is_empty(),
                        kcs.key_head.clone(),
                        kcs.key_line.clone(),
                        String::new(),
                        tail.clone(),
                    ));
                    tail = kcs.key_foot.clone();
                    let vcs = if self.meta.is_some() {
                        kcs
                    } else {
                        CommentSet::default()
                    };
                    if !merge_refs.is_empty() {
                        // merge member: "<<" plus alias or block sequence
                        if merge_refs.len() == 1 {
                            self.evs.push(Ev::Alias {
                                name: self.doc.node(merge_refs[0]).anchor.clone(),
                                head: vcs.head.clone(),
                                line: vcs.line.clone(),
                                foot: vcs.foot.clone(),
                            });
                        } else {
                            self.evs.push(Ev::SeqStart {
                                anchor: None,
                                head: vcs.head.clone(),
                            });
                            for &r in &merge_refs {
                                self.evs.push(Ev::Alias {
                                    name: self.doc.node(r).anchor.clone(),
                                    head: String::new(),
                                    line: String::new(),
                                    foot: String::new(),
                                });
                            }
                            self.evs.push(Ev::SeqEnd {
                                line: String::new(),
                                foot: vcs.foot.clone(),
                            });
                        }
                        continue;
                    }
                    self.node(m.val, &mpath, &vcs);
                }
                self.evs.push(Ev::MapEnd {
                    tail,
                    line: cs.line.clone(),
                    foot: cs.foot.clone(),
                });
            }
            Value::Array(_) => {
                let anchor = self.anchor_of(id);
                self.evs.push(Ev::SeqStart {
                    anchor,
                    head: cs.head.clone(),
                });
                let items = match self.doc.value(id) {
                    Value::Array(a) => a.clone(),
                    _ => Vec::new(),
                };
                for (i, &it) in items.iter().enumerate() {
                    let mut ipath = path.to_vec();
                    ipath.push(PathSeg::Idx(i));
                    let ics = cs_of(self.meta, &ipath);
                    self.node(it, &ipath, &ics);
                }
                self.evs.push(Ev::SeqEnd {
                    line: cs.line.clone(),
                    foot: cs.foot.clone(),
                });
            }
            _ => {
                let anchor = self.anchor_of(id);
                match self.doc.value(id) {
                    Value::Str(s) => {
                        self.evs.push(scalar_event(
                            s,
                            anchor,
                            true,
                            cs.head.clone(),
                            cs.line.clone(),
                            cs.foot.clone(),
                            String::new(),
                        ));
                    }
                    _ => {
                        self.evs.push(typed_scalar_event(id, self.doc, anchor, cs));
                    }
                }
            }
        }
    }
}

fn scalar_text(doc: &Doc, id: NodeId) -> String {
    match doc.value(id) {
        Value::Str(s) => s.clone(),
        Value::Num(lit) => lit.clone(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        _ => "null".to_string(),
    }
}

// --- byte helpers (yamlprivateh.go) --------------------------------------------

fn char_width(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn is_break_byte(b: &[u8], i: usize) -> bool {
    b[i] == b'\n' || (b[i] == 0xC2 && b.get(i + 1) == Some(&0x85))
}

fn is_bom(b: &[u8], i: usize) -> bool {
    b[i] == 0xEF && b.get(i + 1) == Some(&0xBB) && b.get(i + 2) == Some(&0xBF)
}

/// is_printable: byte-level UTF-8 printability. Four-byte sequences (astral
/// characters) are NOT printable and get \U escapes (matches yaml.v3 output).
fn is_printable(b: &[u8], i: usize) -> bool {
    let at = |k: usize| b.get(k).copied().unwrap_or(0);
    b[i] == 0x0A
        || (0x20..=0x7E).contains(&b[i])
        || (b[i] == 0xC2 && at(i + 1) >= 0xA0)
        || (b[i] > 0xC2 && b[i] < 0xED)
        || (b[i] == 0xED && at(i + 1) < 0xA0)
        || b[i] == 0xEE
        || (b[i] == 0xEF
            && !(at(i + 1) == 0xBB && at(i + 2) == 0xBF)
            && !(at(i + 1) == 0xBF && (at(i + 2) == 0xBE || at(i + 2) == 0xBF)))
}

// --- emitter -----------------------------------------------------------------

struct ScalarData {
    value: String,
    multiline: bool,
    block_plain_allowed: bool,
    single_quoted_allowed: bool,
    block_allowed: bool,
    style: Style,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    DocContent,
    DocEnd,
    BlockSeqFirstItem,
    BlockSeqItem,
    BlockMapFirstKey,
    BlockMapKey,
    BlockMapSimpleValue,
    BlockMapValue,
    FlowSeqFirstItem,
    FlowMapFirstKey,
    Done,
}

/// yaml.v3 emitter state (block context only; tags/canonical never occur).
struct Emitter {
    out: String,
    column: usize,
    whitespace: bool,
    indention: bool,
    open_ended: bool,
    foot_indent: i64,
    indent: i64,
    indents: Vec<i64>,
    best_indent: i64,
    head_comment: String,
    line_comment: String,
    foot_comment: String,
    tail_comment: String,
    key_line_comment: String,
    scalar: ScalarData,
}

impl Emitter {
    fn new() -> Emitter {
        Emitter {
            out: String::new(),
            column: 0,
            whitespace: true,
            indention: true,
            open_ended: false,
            foot_indent: -1,
            indent: -1,
            indents: Vec::new(),
            best_indent: 2,
            head_comment: String::new(),
            line_comment: String::new(),
            foot_comment: String::new(),
            tail_comment: String::new(),
            key_line_comment: String::new(),
            scalar: ScalarData {
                value: String::new(),
                multiline: false,
                block_plain_allowed: true,
                single_quoted_allowed: true,
                block_allowed: true,
                style: Style::Plain,
            },
        }
    }

    fn put(&mut self, value: char) {
        self.out.push(value);
        self.column += 1;
    }

    fn put_break(&mut self) {
        self.out.push('\n');
        self.column = 0;
        self.indention = true;
    }

    fn write_all(&mut self, s: &str) {
        for ch in s.chars() {
            self.out.push(ch);
            self.column += 1;
        }
    }

    fn write_one(&mut self, s: &str, at: &mut usize) {
        let ch = s[*at..].chars().next().unwrap();
        self.out.push(ch);
        self.column += 1;
        *at += ch.len_utf8();
    }

    /// yaml_emitter_write_indent.
    fn write_indent(&mut self) {
        let indent = if self.indent < 0 { 0 } else { self.indent } as usize;
        if !self.indention || self.column > indent || (self.column == indent && !self.whitespace) {
            self.put_break();
        }
        if self.foot_indent == indent as i64 {
            self.put_break();
        }
        while self.column < indent {
            self.put(' ');
        }
        self.whitespace = true;
        self.foot_indent = -1;
    }

    /// yaml_emitter_write_indicator.
    fn write_indicator(
        &mut self,
        indicator: &str,
        need_whitespace: bool,
        is_whitespace: bool,
        is_indention: bool,
    ) {
        if need_whitespace && !self.whitespace {
            self.put(' ');
        }
        self.write_all(indicator);
        self.whitespace = is_whitespace;
        self.indention = self.indention && is_indention;
        self.open_ended = false;
    }

    fn process_anchor(&mut self, alias: bool, name: &str) {
        let c = if alias { '*' } else { '&' };
        self.write_indicator(&c.to_string(), true, false, false);
        self.write_all(name);
        self.whitespace = false;
        self.indention = false;
    }

    /// yaml_emitter_write_comment: each line at its own indent, '#' added
    /// when missing, trailing break after the last line.
    fn write_comment(&mut self, comment: &str) {
        let mut breaks = false;
        let mut pound = false;
        let mut i = 0usize;
        let b = comment.as_bytes();
        while i < b.len() {
            if b[i] == b'\n' {
                self.put_break();
                i += 1;
                breaks = true;
                pound = false;
            } else {
                if breaks {
                    self.write_indent();
                }
                if !pound {
                    if b[i] != b'#' {
                        self.put('#');
                        self.put(' ');
                    }
                    pound = true;
                }
                self.write_one(comment, &mut i);
                self.indention = false;
                breaks = false;
            }
        }
        if !breaks {
            self.put_break();
        }
        self.whitespace = true;
    }

    fn process_head_comment(&mut self) {
        if !self.tail_comment.is_empty() {
            self.write_indent();
            let tail = std::mem::take(&mut self.tail_comment);
            self.write_comment(&tail);
            self.foot_indent = self.indent.max(0);
        }
        if self.head_comment.is_empty() {
            return;
        }
        self.write_indent();
        let head = std::mem::take(&mut self.head_comment);
        self.write_comment(&head);
    }

    fn process_line_comment(&mut self) {
        if self.line_comment.is_empty() {
            return;
        }
        if !self.whitespace {
            self.put(' ');
        }
        let line = std::mem::take(&mut self.line_comment);
        self.write_comment(&line);
    }

    fn process_foot_comment(&mut self) {
        if self.foot_comment.is_empty() {
            return;
        }
        self.write_indent();
        let foot = std::mem::take(&mut self.foot_comment);
        self.write_comment(&foot);
        self.foot_indent = self.indent.max(0);
    }

    /// yaml_emitter_analyze_scalar (block-context flags).
    fn analyze_scalar(&mut self, value: &str) {
        let b = value.as_bytes();
        let mut block_indicators = false;
        let mut line_breaks = false;
        let mut special_characters = false;
        let mut tab_characters = false;

        let mut leading_space = false;
        let mut leading_break = false;
        let mut trailing_space = false;
        let mut trailing_break = false;
        let mut break_space = false;
        let mut space_break = false;

        let mut preceded_by_whitespace;
        let mut previous_space = false;
        let mut previous_break = false;

        if value.is_empty() {
            self.scalar = ScalarData {
                value: String::new(),
                multiline: false,
                block_plain_allowed: true,
                single_quoted_allowed: true,
                block_allowed: false,
                style: Style::Plain,
            };
            return;
        }

        if b.starts_with(b"---") || b.starts_with(b"...") {
            block_indicators = true;
        }

        preceded_by_whitespace = true;
        let mut i = 0usize;
        while i < b.len() {
            let w = char_width(b[i]);
            let followed_by_whitespace = i + w >= b.len() || matches!(b[i + w], b' ' | b'\t');
            if i == 0 {
                match b[i] {
                    b'#' | b',' | b'[' | b']' | b'{' | b'}' | b'&' | b'*' | b'!' | b'|' | b'>'
                    | b'\'' | b'"' | b'%' | b'@' | b'`' => {
                        block_indicators = true;
                    }
                    b'?' | b':' => {
                        if followed_by_whitespace {
                            block_indicators = true;
                        }
                    }
                    b'-' => {
                        if followed_by_whitespace {
                            block_indicators = true;
                        }
                    }
                    _ => {}
                }
            } else {
                match b[i] {
                    b':' => {
                        if followed_by_whitespace {
                            block_indicators = true;
                        }
                    }
                    b'#' => {
                        if preceded_by_whitespace {
                            block_indicators = true;
                        }
                    }
                    _ => {}
                }
            }

            if b[i] == b'\t' {
                tab_characters = true;
            } else if !is_printable(b, i) {
                special_characters = true;
            }

            let is_space = b[i] == b' ';
            let is_break = b[i] == b'\n';
            if is_space {
                if i == 0 {
                    leading_space = true;
                }
                if i + w == b.len() {
                    trailing_space = true;
                }
                if previous_break {
                    break_space = true;
                }
                previous_space = true;
                previous_break = false;
            } else if is_break {
                line_breaks = true;
                if i == 0 {
                    leading_break = true;
                }
                if i + w == b.len() {
                    trailing_break = true;
                }
                if previous_space {
                    space_break = true;
                }
                previous_space = false;
                previous_break = true;
            } else {
                previous_space = false;
                previous_break = false;
            }

            // is_blankz(value, i)
            preceded_by_whitespace = matches!(b[i], b' ' | b'\t' | b'\n' | 0);
            i += w;
        }

        let multiline = line_breaks;
        let mut block_plain_allowed = true;
        let mut single_quoted_allowed = true;
        let mut block_allowed = true;

        if leading_space || leading_break || trailing_space || trailing_break {
            block_plain_allowed = false;
        }
        if trailing_space {
            block_allowed = false;
        }
        if break_space {
            block_plain_allowed = false;
            single_quoted_allowed = false;
        }
        if space_break || tab_characters || special_characters {
            block_plain_allowed = false;
            single_quoted_allowed = false;
        }
        if space_break || special_characters {
            block_allowed = false;
        }
        if line_breaks {
            block_plain_allowed = false;
        }
        if block_indicators {
            block_plain_allowed = false;
        }

        self.scalar = ScalarData {
            value: value.to_string(),
            multiline,
            block_plain_allowed,
            single_quoted_allowed,
            block_allowed,
            style: Style::Plain,
        };
    }

    /// yaml_emitter_select_scalar_style (no tags, never canonical, never
    /// flow).
    fn select_scalar_style(&mut self, style: Style, simple_key_context: bool) {
        let mut style = style;
        if simple_key_context && self.scalar.multiline {
            style = Style::Double;
        }
        if style == Style::Plain {
            if !self.scalar.block_plain_allowed {
                style = Style::Single;
            }
            if self.scalar.value.is_empty() && simple_key_context {
                style = Style::Single;
            }
        }
        if style == Style::Single && !self.scalar.single_quoted_allowed {
            style = Style::Double;
        }
        if style == Style::Literal && (!self.scalar.block_allowed || simple_key_context) {
            style = Style::Double;
        }
        self.scalar.style = style;
    }

    fn process_scalar(&mut self, _simple_key_context: bool) {
        match self.scalar.style {
            Style::Plain => self.write_plain_scalar(),
            Style::Single => self.write_single_quoted_scalar(),
            Style::Double => self.write_double_quoted_scalar(),
            Style::Literal => self.write_literal_scalar(),
        }
    }

    fn write_plain_scalar(&mut self) {
        let value = self.scalar.value.clone();
        if !value.is_empty() && !self.whitespace {
            self.put(' ');
        }
        let mut breaks = false;
        let mut i = 0usize;
        let b = value.as_bytes();
        while i < b.len() {
            if b[i] == b' ' {
                self.out.push(' ');
                self.column += 1;
                i += 1;
            } else if b[i] == b'\n' {
                self.put_break();
                i += 1;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                self.write_one(&value, &mut i);
                self.indention = false;
                breaks = false;
            }
        }
        if !value.is_empty() {
            self.whitespace = false;
        }
        self.indention = false;
    }

    fn write_single_quoted_scalar(&mut self) {
        self.write_indicator("'", true, false, false);
        let value = self.scalar.value.clone();
        let mut breaks = false;
        let mut i = 0usize;
        let b = value.as_bytes();
        while i < b.len() {
            if b[i] == b' ' {
                self.out.push(' ');
                self.column += 1;
                i += 1;
            } else if b[i] == b'\n' {
                self.put_break();
                i += 1;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                if b[i] == b'\'' {
                    self.put('\'');
                }
                self.write_one(&value, &mut i);
                self.indention = false;
                breaks = false;
            }
        }
        self.write_indicator("'", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_double_quoted_scalar(&mut self) {
        self.write_indicator("\"", true, false, false);
        let value = self.scalar.value.clone();
        let b = value.as_bytes();
        let mut i = 0usize;
        while i < b.len() {
            if !is_printable(b, i)
                || matches!(b[i], b'"' | b'\\')
                || is_break_byte(b, i)
                || is_bom(b, i)
            {
                let first = b[i];
                let (w, v0) = match first {
                    c if c & 0x80 == 0x00 => (1usize, (c & 0x7F) as u32),
                    c if c & 0xE0 == 0xC0 => (2, (c & 0x1F) as u32),
                    c if c & 0xF0 == 0xE0 => (3, (c & 0x0F) as u32),
                    c if c & 0xF8 == 0xF0 => (4, (c & 0x07) as u32),
                    _ => (1, first as u32),
                };
                let mut v = v0;
                for k in 1..w {
                    v = (v << 6) + (b[i + k] & 0x3F) as u32;
                }
                i += w;
                self.put('\\');
                match v {
                    0x00 => self.put('0'),
                    0x07 => self.put('a'),
                    0x08 => self.put('b'),
                    0x09 => self.put('t'),
                    0x0A => self.put('n'),
                    0x0B => self.put('v'),
                    0x0C => self.put('f'),
                    0x0D => self.put('r'),
                    0x1B => self.put('e'),
                    0x22 => self.put('"'),
                    0x5C => self.put('\\'),
                    0x85 => self.put('N'),
                    0xA0 => self.put('_'),
                    0x2028 => self.put('L'),
                    0x2029 => self.put('P'),
                    _ => {
                        let (kind, width) = if v <= 0xFF {
                            ('x', 2usize)
                        } else if v <= 0xFFFF {
                            ('u', 4)
                        } else {
                            ('U', 8)
                        };
                        self.put(kind);
                        let mut k = (width - 1) * 4;
                        loop {
                            let digit = ((v >> k) & 0x0F) as u8;
                            let c = if digit < 10 {
                                b'0' + digit
                            } else {
                                b'A' + digit - 10
                            };
                            self.put(c as char);
                            if k == 0 {
                                break;
                            }
                            k -= 4;
                        }
                    }
                }
            } else {
                self.write_one(&value, &mut i);
            }
        }
        self.write_indicator("\"", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_block_scalar_hints(&mut self, value: &str) {
        let b = value.as_bytes();
        if !b.is_empty() && (b[0] == b' ' || b[0] == b'\n') {
            let hint = char::from_digit(self.best_indent as u32, 10).unwrap_or('0');
            self.write_indicator(&hint.to_string(), false, false, false);
        }
        self.open_ended = false;
        let mut chomp_hint: Option<char> = None;
        if b.is_empty() {
            chomp_hint = Some('-');
        } else {
            let mut i = b.len() - 1;
            while i > 0 && b[i] & 0xC0 == 0x80 {
                i -= 1;
            }
            if b[i] != b'\n' {
                chomp_hint = Some('-');
            } else if i == 0 {
                chomp_hint = Some('+');
                self.open_ended = true;
            } else {
                i -= 1;
                while i > 0 && b[i] & 0xC0 == 0x80 {
                    i -= 1;
                }
                if b[i] == b'\n' {
                    chomp_hint = Some('+');
                    self.open_ended = true;
                }
            }
        }
        if let Some(h) = chomp_hint {
            self.write_indicator(&h.to_string(), false, false, false);
        }
    }

    fn write_literal_scalar(&mut self) {
        self.write_indicator("|", true, false, false);
        self.write_block_scalar_hints(&self.scalar.value.clone());
        self.process_line_comment();
        self.whitespace = true;
        let value = self.scalar.value.clone();
        let mut breaks = true;
        let mut i = 0usize;
        let b = value.as_bytes();
        while i < b.len() {
            if b[i] == b'\n' {
                self.put_break();
                i += 1;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                self.write_one(&value, &mut i);
                self.indention = false;
                breaks = false;
            }
        }
    }

    fn increase_indent(&mut self, flow: bool) {
        self.indents.push(self.indent);
        if self.indent < 0 {
            self.indent = if flow { self.best_indent } else { 0 };
        } else {
            // [Go] the first indent inside a sequence skips the "- " indicator
            self.indent += self.best_indent;
        }
    }

    fn pop_indent(&mut self) {
        self.indent = self.indents.pop().unwrap_or(-1);
    }

    // --- state machine ---------------------------------------------------------

    /// The port of yaml.v3's state machine over the event stream.
    fn run(&mut self, evs: Vec<Ev>) -> Result<(), DocError> {
        let mut iter = evs.into_iter().peekable();
        // StreamStart and DocStart are structural no-ops here: the implicit
        // document start carries no comments (yamldoc's fresh DocumentNode).
        iter.next();
        iter.next();
        let mut states: Vec<St> = Vec::new();
        let mut state = St::DocContent;

        while let Some(ev) = iter.next() {
            // analyze_event: pull the event's comments and scalar analysis
            if !ev.head().is_empty() {
                self.head_comment = ev.head().to_string();
            }
            if !ev.line().is_empty() {
                self.line_comment = ev.line().to_string();
            }
            if !ev.foot().is_empty() {
                self.foot_comment = ev.foot().to_string();
            }
            if !ev.tail().is_empty() {
                self.tail_comment = ev.tail().to_string();
            }
            match &ev {
                Ev::Scalar { value, .. } => {
                    self.analyze_scalar(value);
                }
                _ => {}
            }

            match state {
                St::DocContent => {
                    self.process_head_comment();
                    states.push(St::DocEnd);
                    state = self.emit_node(ev, &mut iter, &mut states, true, false);
                    self.process_line_comment();
                    self.process_foot_comment();
                }
                St::DocEnd => match ev {
                    Ev::DocEnd => {
                        // [Go] force document foot separation
                        self.foot_indent = 0;
                        self.process_foot_comment();
                        self.foot_indent = -1;
                        self.write_indent();
                        state = St::Done;
                    }
                    other => return err(format!("emitter: expected doc end, got {other:?}")),
                },
                St::BlockSeqFirstItem | St::BlockSeqItem => {
                    if state == St::BlockSeqFirstItem {
                        self.increase_indent(false);
                    }
                    self.process_head_comment();
                    match ev {
                        Ev::SeqEnd { .. } => {
                            self.pop_indent();
                            state = states.pop().unwrap_or(St::Done);
                        }
                        ev => {
                            self.write_indent();
                            self.write_indicator("-", true, false, true);
                            states.push(St::BlockSeqItem);
                            state = self.emit_node(ev, &mut iter, &mut states, false, false);
                            self.process_line_comment();
                            self.process_foot_comment();
                        }
                    }
                }
                St::BlockMapFirstKey | St::BlockMapKey => {
                    if state == St::BlockMapFirstKey {
                        self.increase_indent(false);
                    }
                    self.process_head_comment();
                    match ev {
                        Ev::MapEnd { .. } => {
                            self.pop_indent();
                            state = states.pop().unwrap_or(St::Done);
                        }
                        ev => {
                            self.write_indent();
                            if !self.line_comment.is_empty() {
                                // a line comment on the key: render later
                                self.key_line_comment = std::mem::take(&mut self.line_comment);
                            }
                            let next_is_end = iter.peek().is_some_and(|e| e.is_end());
                            if self.check_simple_key(&ev, next_is_end) {
                                states.push(St::BlockMapSimpleValue);
                                state = self.emit_node(ev, &mut iter, &mut states, false, true);
                            } else {
                                self.write_indicator("?", true, false, true);
                                states.push(St::BlockMapValue);
                                state = self.emit_node(ev, &mut iter, &mut states, false, false);
                            }
                        }
                    }
                }
                St::BlockMapSimpleValue | St::BlockMapValue => {
                    let simple = state == St::BlockMapSimpleValue;
                    match ev {
                        Ev::Scalar { .. }
                        | Ev::Alias { .. }
                        | Ev::SeqStart { .. }
                        | Ev::MapStart { .. } => {
                            if simple {
                                self.write_indicator(":", false, false, false);
                            } else {
                                self.write_indent();
                                self.write_indicator(":", true, false, true);
                            }
                            if !self.key_line_comment.is_empty() {
                                match &ev {
                                    Ev::Scalar { .. } => {
                                        if self.line_comment.is_empty() {
                                            self.line_comment =
                                                std::mem::take(&mut self.key_line_comment);
                                        } else {
                                            self.key_line_comment.clear();
                                        }
                                    }
                                    Ev::MapStart { .. } | Ev::SeqStart { .. } => {
                                        // an indented block follows: write the
                                        // comment right now
                                        std::mem::swap(
                                            &mut self.line_comment,
                                            &mut self.key_line_comment,
                                        );
                                        self.process_line_comment();
                                        std::mem::swap(
                                            &mut self.line_comment,
                                            &mut self.key_line_comment,
                                        );
                                    }
                                    _ => {}
                                }
                            }
                            states.push(St::BlockMapKey);
                            state = self.emit_node(ev, &mut iter, &mut states, false, false);
                            self.process_line_comment();
                            self.process_foot_comment();
                        }
                        other => return err(format!("emitter: unexpected {other:?}")),
                    }
                }
                St::FlowSeqFirstItem => match ev {
                    Ev::SeqEnd { .. } => {
                        self.pop_indent();
                        if self.column == 0 {
                            self.write_indent();
                        }
                        self.write_indicator("]", false, false, false);
                        self.process_line_comment();
                        self.process_foot_comment();
                        state = states.pop().unwrap_or(St::Done);
                    }
                    other => return err(format!("emitter: non-empty flow sequence ({other:?})")),
                },
                St::FlowMapFirstKey => match ev {
                    Ev::MapEnd { .. } => {
                        self.pop_indent();
                        if self.column == 0 {
                            self.write_indent();
                        }
                        self.write_indicator("}", false, false, false);
                        self.process_line_comment();
                        self.process_foot_comment();
                        state = states.pop().unwrap_or(St::Done);
                    }
                    other => return err(format!("emitter: non-empty flow mapping ({other:?})")),
                },
                St::Done => break,
            }
        }
        Ok(())
    }

    /// yaml_emitter_check_simple_key.
    fn check_simple_key(&self, ev: &Ev, next_is_end: bool) -> bool {
        match ev {
            Ev::Alias { name, .. } => !name.is_empty() && name.len() <= 128,
            Ev::Scalar {
                value,
                anchor,
                style,
                ..
            } => {
                let multiline = value.contains('\n') || *style == Style::Literal;
                if multiline {
                    return false;
                }
                let anchor_len = anchor.as_ref().map(|a| a.len()).unwrap_or(0);
                anchor_len + value.len() <= 128
            }
            Ev::SeqStart { anchor, .. } | Ev::MapStart { anchor, .. } => {
                // only empty collections can be simple keys
                let anchor_len = anchor.as_ref().map(|a| a.len()).unwrap_or(0);
                next_is_end && anchor_len <= 128
            }
            _ => false,
        }
    }

    /// yaml_emitter_emit_node: dispatch one node event and return the state
    /// the machine continues in (Go assigns emitter.state directly).
    fn emit_node(
        &mut self,
        ev: Ev,
        iter: &mut std::iter::Peekable<std::vec::IntoIter<Ev>>,
        states: &mut Vec<St>,
        is_root: bool,
        simple_key_context: bool,
    ) -> St {
        match ev {
            Ev::Scalar {
                value,
                anchor,
                style,
                ..
            } => {
                let _ = &value;
                self.select_scalar_style(style, simple_key_context);
                if let Some(a) = &anchor {
                    self.process_anchor(false, a);
                }
                self.increase_indent(true);
                self.process_scalar(simple_key_context);
                self.pop_indent();
                if is_root {
                    self.open_ended = true;
                }
                states.pop().unwrap_or(St::Done)
            }
            Ev::Alias { name, .. } => {
                self.process_anchor(true, &name);
                states.pop().unwrap_or(St::Done)
            }
            Ev::SeqStart { anchor, .. } => {
                if let Some(a) = &anchor {
                    self.process_anchor(false, a);
                }
                let next_is_end = iter.peek().is_some_and(|e| e.is_end());
                if next_is_end {
                    self.write_indicator("[", true, true, false);
                    self.increase_indent(true);
                    St::FlowSeqFirstItem
                } else {
                    St::BlockSeqFirstItem
                }
            }
            Ev::MapStart { anchor, .. } => {
                if let Some(a) = &anchor {
                    self.process_anchor(false, a);
                }
                let next_is_end = iter.peek().is_some_and(|e| e.is_end());
                if next_is_end {
                    self.write_indicator("{", true, true, false);
                    self.increase_indent(true);
                    St::FlowMapFirstKey
                } else {
                    St::BlockMapFirstKey
                }
            }
            other => {
                // SeqEnd/MapEnd are handled by their collection states
                let _ = other;
                states.pop().unwrap_or(St::Done)
            }
        }
    }
}

// --- public API ---------------------------------------------------------------

/// checkAliases verifies every alias in the built tree follows its anchor's
/// definition. Direct alias occurrences cannot dangle — the builder tags the
/// first occurrence of an anchored node with the anchor — but a merge key's
/// alias can: its anchor may have been deleted from the document. Refusing to
/// write keeps the file loadable.
fn check_aliases(doc: &Doc) -> Result<(), DocError> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut busy: HashSet<NodeId> = HashSet::new();
    fn walk(
        doc: &Doc,
        id: NodeId,
        seen: &mut HashSet<String>,
        busy: &mut HashSet<NodeId>,
    ) -> Result<(), DocError> {
        if !busy.insert(id) {
            return Ok(()); // merge cycle guard
        }
        let node = doc.node(id);
        if !node.anchor.is_empty() {
            seen.insert(node.anchor.clone());
        }
        match doc.value(id) {
            Value::Object(members) => {
                for m in members {
                    let refs = &doc.nodes[m.val as usize].merge;
                    if !refs.is_empty() {
                        for &r in refs {
                            let name = doc.node(r).anchor.clone();
                            if !seen.contains(&name) {
                                return err(format!(
                                    "YAML alias *{name} is used before its anchor is defined (a merge key references a deleted or moved anchor)"
                                ));
                            }
                        }
                    }
                    walk(doc, m.val, seen, busy)?;
                }
            }
            Value::Array(items) => {
                for &it in items {
                    walk(doc, it, seen, busy)?;
                }
            }
            _ => {}
        }
        busy.remove(&id);
        Ok(())
    }
    walk(doc, doc.root, &mut seen, &mut busy)
}

/// write emits the document as YAML with two-space indentation, restoring the
/// comments recorded in meta. Anchors are written at the first occurrence of
/// an anchored node and later occurrences become alias references ("*name");
/// nodes without anchor identity emit plain content. Merge-key members ("<<")
/// are written back as an alias or a block sequence of aliases. meta == None
/// also disables anchor/alias handling (plain value export) and resolves
/// merges inline.
pub fn write(doc: &Doc, meta: Option<&Meta>) -> Result<String, DocError> {
    if meta.is_some() {
        check_aliases(doc)?;
    }
    let evs = Builder {
        doc,
        meta,
        emitted: HashSet::new(),
        evs: Vec::new(),
    }
    .build();
    let mut em = Emitter::new();
    em.run(evs)?;
    Ok(em.out)
}
