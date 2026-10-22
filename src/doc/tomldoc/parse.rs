//! TOML parse: turns a TOML document into the arena document tree.
//!
//! TOML is table-oriented, so a document is a root table whose members are
//! scalars, arrays, nested tables ([table]) and arrays of tables ([[table]]).
//! Dotted keys build nested tables just like headers. Comments are captured
//! by line and attached to the following key/header (or to the trailing line
//! of the key/value they follow) by [`attach`].
//!
//! Scalars that JSON cannot spell exactly keep their source text on
//! [`NodeData::raw`]: hexadecimal/octal/binary integers, underlined numbers,
//! and TOML date/time literals. Everything the editor writes is canonical,
//! but an untouched file round-trips those literals.

use crate::doc::Meta;
use crate::doc::jsondoc::{Doc, DocError, Member, PathSeg, Value};

/// Parse reads one TOML document.
pub fn parse(text: &str) -> Result<(Doc, Meta), DocError> {
    parse_inner(text).map_err(|msg| DocError {
        msg: format!("toml: {msg}"),
    })
}

/// ParseValue reads one TOML value (scalar, array or inline table) as a
/// standalone document. Raw value editors use it for subtrees.
pub fn parse_value(text: &str) -> Result<(Doc, Meta), DocError> {
    parse_value_inner(text)
        .map(|node| {
            let mut doc = Doc::new();
            doc.root = node_to_doc(&node, &mut doc);
            (doc, Meta::default())
        })
        .map_err(|msg| DocError {
            msg: format!("toml: {msg}"),
        })
}

fn parse_inner(text: &str) -> Result<(Doc, Meta), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser::new(text);
    let root = p.parse_document()?;
    Ok(root.into_doc(p.sites, p.comments))
}

fn parse_value_inner(text: &str) -> Result<TNode, String> {
    let mut p = Parser::new(text);
    p.skip_ws_comments()?;
    if p.peek().is_none() {
        return p.err("expected a value");
    }
    let node = p.parse_value(&[])?;
    p.skip_ws_comments()?;
    if p.peek().is_some() {
        return p.err("unexpected trailing text after value");
    }
    Ok(node)
}

// --- builder tree ------------------------------------------------------------

/// A table under construction. The flags drive TOML's redefinition rules:
/// a `[header]` cannot reuse an explicitly defined or dotted-key table, and
/// inline tables are frozen.
#[derive(Default)]
struct TTable {
    entries: Vec<(String, TNode)>,
    /// defined by a [header] (or a [[header]] element)
    explicit: bool,
    /// created as an intermediate of a dotted key
    dotted: bool,
    /// created as an intermediate of a header path
    implicit: bool,
    /// an inline table value: no later extension is allowed
    inline: bool,
}

enum TNode {
    /// The Doc value plus the exact source spelling when it differs (numbers,
    /// date/time literals).
    Scalar(Value, String),
    Array(Vec<TNode>),
    Tables(Vec<TTable>),
    Table(TTable),
}

impl TTable {
    fn into_doc(self, sites: Vec<Site>, comments: Vec<RawComment>) -> (Doc, Meta) {
        let mut doc = Doc::new();
        doc.root = table_to_doc(&self, &mut doc);
        let meta = attach(sites, comments);
        (doc, meta)
    }
}

fn table_to_doc(t: &TTable, doc: &mut Doc) -> u32 {
    let id = doc.new_object();
    let mut members = Vec::with_capacity(t.entries.len());
    for (key, node) in &t.entries {
        members.push(Member {
            key: key.clone(),
            val: node_to_doc(node, doc),
        });
    }
    if let Value::Object(m) = &mut doc.node_mut(id).value {
        *m = members;
    }
    id
}

fn node_to_doc(n: &TNode, doc: &mut Doc) -> u32 {
    match n {
        TNode::Scalar(value, raw) => {
            let id = match value {
                Value::Str(s) => doc.new_str(s.clone()),
                Value::Num(lit) => doc.new_num(lit.clone()),
                Value::Bool(b) => doc.new_bool(*b),
                Value::Null => doc.new_null(),
                _ => unreachable!("scalar node holds a scalar"),
            };
            doc.node_mut(id).raw = raw.clone();
            id
        }
        TNode::Array(items) => {
            let id = doc.new_array();
            let vals: Vec<u32> = items.iter().map(|it| node_to_doc(it, doc)).collect();
            if let Value::Array(a) = &mut doc.node_mut(id).value {
                *a = vals;
            }
            id
        }
        TNode::Tables(tables) => {
            let id = doc.new_array();
            let vals: Vec<u32> = tables.iter().map(|t| table_to_doc(t, doc)).collect();
            if let Value::Array(a) = &mut doc.node_mut(id).value {
                *a = vals;
            }
            id
        }
        TNode::Table(t) => table_to_doc(t, doc),
    }
}

// --- comments ----------------------------------------------------------------

struct RawComment {
    line: usize,
    text: String,
}

struct Site {
    line: usize,
    end_line: usize,
    path: Vec<PathSeg>,
    kind: SiteKind,
}

enum SiteKind {
    Header,
    Key,
    Item,
}

/// Attach comment runs to sites. A run on a site's closing line is that
/// member's trailing comment; an own-line run belongs to the next site's
/// leading comments. Leading file comments go to the document root and
/// comments after the last site become the document tail.
fn attach(sites: Vec<Site>, comments: Vec<RawComment>) -> Meta {
    let mut meta = Meta::default();
    let first_line = sites.iter().map(|s| s.line).min();
    // Own-line comments form a leading run for the next site. A trailing
    // comment flushes the run: it belongs to the site it shares a line with.
    let mut pending: Vec<RawComment> = Vec::new();
    let flush = |meta: &mut Meta, pending: &mut Vec<RawComment>| {
        let Some(first) = pending.first() else {
            return;
        };
        let line = first.line;
        let text = pending
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        pending.clear();
        if first_line.is_none_or(|f| line < f) {
            push_slot(meta, &[], "head", &text);
            return;
        }
        if let Some(next) = sites.iter().find(|s| s.line > line) {
            let slot = match next.kind {
                SiteKind::Item => "head",
                SiteKind::Header | SiteKind::Key => "key_head",
            };
            push_slot(meta, &next.path, slot, &text);
        } else {
            meta.tail = join(&meta.tail, &text);
        }
    };
    for c in comments {
        if let Some(site) = sites
            .iter()
            .rev()
            .find(|s| s.end_line == c.line || s.line == c.line)
        {
            flush(&mut meta, &mut pending);
            let slot = match site.kind {
                SiteKind::Item => "line",
                SiteKind::Header | SiteKind::Key => "key_line",
            };
            push_slot(&mut meta, &site.path, slot, &c.text);
            continue;
        }
        pending.push(c);
    }
    flush(&mut meta, &mut pending);
    meta
}

fn join(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        format!("{a}\n{b}")
    }
}

fn push_slot(meta: &mut Meta, path: &[PathSeg], slot: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    let cs = meta.comments.entry(path.to_vec()).or_default();
    let cur = match slot {
        "head" => &mut cs.head,
        "key_head" => &mut cs.key_head,
        "line" => &mut cs.line,
        "key_line" => &mut cs.key_line,
        "foot" => &mut cs.foot,
        _ => &mut cs.key_foot,
    };
    *cur = join(cur, text);
}

// --- parser ------------------------------------------------------------------

struct Parser<'a> {
    src: &'a str,
    i: usize,
    line: usize,
    col: usize,
    sites: Vec<Site>,
    comments: Vec<RawComment>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Parser<'a> {
        Parser {
            src,
            i: 0,
            line: 1,
            col: 0,
            sites: Vec::new(),
            comments: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.i..].chars().next()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.i..].starts_with(s)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.col = 0;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, String> {
        Err(format!("line {}: {}", self.line, msg.into()))
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        match self.peek() {
            Some(got) if got == c => {
                self.bump();
                Ok(())
            }
            Some(got) => self.err(format!("expected '{c}', found '{got}'")),
            None => self.err(format!("expected '{c}', found end of file")),
        }
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t')) {
            self.bump();
        }
    }

    fn skip_comment(&mut self) -> Result<(), String> {
        if self.peek() != Some('#') {
            return Ok(());
        }
        let line = self.line;
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
        let text = self.src[start..self.i]
            .trim_end_matches('\r')
            .trim_end()
            .to_string();
        self.comments.push(RawComment { line, text });
        Ok(())
    }

    /// Whitespace, newlines and comments, as allowed inside arrays and around
    /// a standalone value in the raw value editor.
    fn skip_ws_comments(&mut self) -> Result<(), String> {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\r') | Some('\n') => {
                    self.bump();
                }
                Some('#') => self.skip_comment()?,
                _ => return Ok(()),
            }
        }
    }

    /// Consume the rest of a key/value or header line.
    fn finish_line(&mut self) -> Result<(), String> {
        self.skip_spaces();
        if self.peek() == Some('#') {
            self.skip_comment()?;
        }
        match self.peek() {
            None => Ok(()),
            Some('\r') => {
                self.bump();
                match self.peek() {
                    Some('\n') => {
                        self.bump();
                        Ok(())
                    }
                    None => Ok(()),
                    _ => self.err("expected a newline"),
                }
            }
            Some('\n') => {
                self.bump();
                Ok(())
            }
            Some(c) => self.err(format!("unexpected '{c}' after value")),
        }
    }

    fn parse_document(&mut self) -> Result<TTable, String> {
        let mut root = TTable::default();
        let mut cur_keys: Vec<String> = Vec::new();
        let mut cur_doc: Vec<PathSeg> = Vec::new();
        loop {
            self.skip_spaces();
            match self.peek() {
                None => break,
                Some('\n') => {
                    self.bump();
                    continue;
                }
                Some('\r') => {
                    self.bump();
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    continue;
                }
                Some('#') => {
                    self.skip_comment()?;
                    continue;
                }
                Some('[') => {
                    let (keys, path) = self.parse_header(&mut root)?;
                    cur_keys = keys;
                    cur_doc = path;
                }
                Some(_) => self.parse_keyval(&mut root, &cur_keys, &cur_doc)?,
            }
            self.finish_line()?;
        }
        Ok(root)
    }

    fn parse_header(&mut self, root: &mut TTable) -> Result<(Vec<String>, Vec<PathSeg>), String> {
        let line = self.line;
        self.expect('[')?;
        let array = if self.peek() == Some('[') {
            self.bump();
            true
        } else {
            false
        };
        self.skip_spaces();
        let keys = self.parse_key_path()?;
        self.skip_spaces();
        self.expect(']')?;
        if array {
            self.expect(']')?;
        }
        let path = define_table(root, &keys, array)?;
        self.sites.push(Site {
            line,
            end_line: self.line,
            path: path.clone(),
            kind: SiteKind::Header,
        });
        Ok((keys, path))
    }

    fn parse_keyval(
        &mut self,
        root: &mut TTable,
        cur_keys: &[String],
        cur_doc: &[PathSeg],
    ) -> Result<(), String> {
        let line = self.line;
        let keys = self.parse_key_path()?;
        self.skip_spaces();
        self.expect('=')?;
        self.skip_spaces();

        let mut path = cur_doc.to_vec();
        for k in &keys {
            path.push(PathSeg::Key(k.clone()));
        }
        let value = self.parse_value(&path)?;
        let end_line = self.line;

        let table = table_at_mut(root, cur_keys)?;
        insert_key(table, &keys, value)?;
        self.sites.push(Site {
            line,
            end_line,
            path,
            kind: SiteKind::Key,
        });
        Ok(())
    }

    fn parse_key_path(&mut self) -> Result<Vec<String>, String> {
        let mut keys = Vec::new();
        loop {
            keys.push(self.parse_simple_key()?);
            self.skip_spaces();
            if self.peek() == Some('.') {
                self.bump();
                self.skip_spaces();
                continue;
            }
            break;
        }
        Ok(keys)
    }

    fn parse_simple_key(&mut self) -> Result<String, String> {
        match self.peek() {
            Some('"') => self.parse_basic_string(false),
            Some('\'') => self.parse_literal_string(false),
            Some(c) if is_bare_key_char(c) => {
                let start = self.i;
                while self.peek().is_some_and(is_bare_key_char) {
                    self.bump();
                }
                Ok(self.src[start..self.i].to_string())
            }
            Some(c) => self.err(format!("invalid key start '{c}'")),
            None => self.err("expected a key"),
        }
    }

    fn parse_value(&mut self, path: &[PathSeg]) -> Result<TNode, String> {
        match self.peek() {
            Some('"') => {
                let s = self.parse_basic_string(true)?;
                Ok(TNode::Scalar(Value::Str(s), String::new()))
            }
            Some('\'') => {
                let s = self.parse_literal_string(true)?;
                Ok(TNode::Scalar(Value::Str(s), String::new()))
            }
            Some('[') => self.parse_array(path),
            Some('{') => self.parse_inline_table(path),
            Some(_) => self.parse_bare(),
            None => self.err("expected a value"),
        }
    }

    fn parse_bare(&mut self) -> Result<TNode, String> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, ',' | ']' | '}' | '#') {
                break;
            }
            self.bump();
        }
        let mut tok = self.src[start..self.i].to_string();
        if tok.is_empty() {
            return self.err("expected a value");
        }
        // A local date-time may use a space between the date and the time.
        if is_date(&tok) && self.peek() == Some(' ') {
            let rest = self.src[self.i..].trim_start_matches([' ', '\t']);
            let time_tok: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && !matches!(c, ',' | ']' | '}' | '#'))
                .collect();
            if looks_like_time(&time_tok) {
                while matches!(self.peek(), Some(' ') | Some('\t')) {
                    self.bump();
                }
                let time_start = self.i;
                while self
                    .peek()
                    .is_some_and(|c| !c.is_whitespace() && !matches!(c, ',' | ']' | '}' | '#'))
                {
                    self.bump();
                }
                let combined = format!("{tok} {}", &self.src[time_start..self.i]);
                if !is_datetime(&combined) {
                    return self.err(format!("invalid date-time {combined:?}"));
                }
                tok = combined;
            }
        }
        if tok == "true" {
            return Ok(TNode::Scalar(Value::Bool(true), String::new()));
        }
        if tok == "false" {
            return Ok(TNode::Scalar(Value::Bool(false), String::new()));
        }
        if is_datetime(&tok) {
            return Ok(TNode::Scalar(Value::Str(tok.clone()), tok));
        }
        if let Some(canonical) = int_canonical(&tok)? {
            let raw = if canonical == tok { String::new() } else { tok };
            return Ok(TNode::Scalar(Value::Num(canonical), raw));
        }
        if let Some(canonical) = float_canonical(&tok)? {
            let raw = if canonical == tok { String::new() } else { tok };
            return Ok(TNode::Scalar(Value::Num(canonical), raw));
        }
        self.err(format!("invalid value {tok:?}"))
    }

    fn parse_array(&mut self, path: &[PathSeg]) -> Result<TNode, String> {
        self.expect('[')?;
        let mut items = Vec::new();
        let mut idx = 0usize;
        loop {
            self.skip_ws_comments()?;
            if self.peek() == Some(']') {
                self.bump();
                break;
            }
            let line = self.line;
            let mut ipath = path.to_vec();
            ipath.push(PathSeg::Idx(idx));
            let item = self.parse_value(&ipath)?;
            let end_line = self.line;
            self.sites.push(Site {
                line,
                end_line,
                path: ipath,
                kind: SiteKind::Item,
            });
            items.push(item);
            idx += 1;
            self.skip_ws_comments()?;
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {
                    self.bump();
                    break;
                }
                _ => return self.err("expected ',' or ']' in array"),
            }
        }
        Ok(TNode::Array(items))
    }

    fn parse_inline_table(&mut self, path: &[PathSeg]) -> Result<TNode, String> {
        self.expect('{')?;
        let mut table = TTable {
            inline: true,
            ..TTable::default()
        };
        loop {
            self.skip_spaces();
            if self.peek() == Some('}') {
                self.bump();
                break;
            }
            let keys = self.parse_key_path()?;
            self.skip_spaces();
            self.expect('=')?;
            self.skip_spaces();
            let mut vpath = path.to_vec();
            for k in &keys {
                vpath.push(PathSeg::Key(k.clone()));
            }
            let value = self.parse_value(&vpath)?;
            insert_key(&mut table, &keys, value)?;
            self.skip_spaces();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some('}') => {
                    self.bump();
                    break;
                }
                Some('\n') | Some('\r') => {
                    return self.err("inline tables must stay on one line");
                }
                _ => return self.err("expected ',' or '}' in inline table"),
            }
        }
        Ok(TNode::Table(table))
    }

    fn parse_basic_string(&mut self, allow_multiline: bool) -> Result<String, String> {
        let multiline = self.starts_with("\"\"\"");
        if multiline && !allow_multiline {
            return self.err("multi-line strings cannot be keys");
        }
        if multiline {
            self.bump();
            self.bump();
            self.bump();
            self.trim_leading_newline();
        } else {
            self.bump();
        }
        let mut out = String::new();
        loop {
            if self.i >= self.src.len() {
                return self.err("unterminated string");
            }
            if multiline && self.starts_with("\"\"\"") {
                let mut run = 0;
                while self.peek() == Some('"') {
                    self.bump();
                    run += 1;
                }
                for _ in 3..run {
                    out.push('"');
                }
                return Ok(out);
            }
            match self.peek() {
                Some('"') => {
                    self.bump();
                    return Ok(out);
                }
                Some('\\') => self.parse_escape(&mut out, multiline)?,
                Some('\r') if multiline => {
                    self.bump();
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    out.push('\n');
                }
                Some('\n') if multiline => {
                    self.bump();
                    out.push('\n');
                }
                Some('\n') | Some('\r') => return self.err("unterminated string"),
                Some(c) => {
                    self.bump();
                    out.push(c);
                }
                None => return self.err("unterminated string"),
            }
        }
    }

    fn parse_literal_string(&mut self, allow_multiline: bool) -> Result<String, String> {
        let multiline = self.starts_with("'''");
        if multiline && !allow_multiline {
            return self.err("multi-line strings cannot be keys");
        }
        if multiline {
            self.bump();
            self.bump();
            self.bump();
            self.trim_leading_newline();
        } else {
            self.bump();
        }
        let mut out = String::new();
        loop {
            if self.i >= self.src.len() {
                return self.err("unterminated string");
            }
            if multiline && self.starts_with("'''") {
                let mut run = 0;
                while self.peek() == Some('\'') {
                    self.bump();
                    run += 1;
                }
                for _ in 3..run {
                    out.push('\'');
                }
                return Ok(out);
            }
            match self.peek() {
                Some('\'') => {
                    self.bump();
                    return Ok(out);
                }
                Some('\r') if multiline => {
                    self.bump();
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    out.push('\n');
                }
                Some('\n') if multiline => {
                    self.bump();
                    out.push('\n');
                }
                Some('\n') | Some('\r') => return self.err("unterminated string"),
                Some(c) => {
                    self.bump();
                    out.push(c);
                }
                None => return self.err("unterminated string"),
            }
        }
    }

    fn trim_leading_newline(&mut self) {
        if self.peek() == Some('\r') {
            self.bump();
            if self.peek() == Some('\n') {
                self.bump();
            }
        } else if self.peek() == Some('\n') {
            self.bump();
        }
    }

    fn parse_escape(&mut self, out: &mut String, multiline: bool) -> Result<(), String> {
        self.bump(); // backslash
        match self.peek() {
            Some('b') => {
                self.bump();
                out.push('\u{8}');
            }
            Some('t') => {
                self.bump();
                out.push('\t');
            }
            Some('n') => {
                self.bump();
                out.push('\n');
            }
            Some('f') => {
                self.bump();
                out.push('\u{c}');
            }
            Some('r') => {
                self.bump();
                out.push('\r');
            }
            Some('"') => {
                self.bump();
                out.push('"');
            }
            Some('\\') => {
                self.bump();
                out.push('\\');
            }
            Some('u') => {
                self.bump();
                let c = self.parse_hex_escape(4)?;
                out.push(c);
            }
            Some('U') => {
                self.bump();
                let c = self.parse_hex_escape(8)?;
                out.push(c);
            }
            Some(c) if multiline && matches!(c, ' ' | '\t' | '\r' | '\n') => {
                // Line-ending backslash: drop all following whitespace.
                while matches!(self.peek(), Some(' ' | '\t' | '\r' | '\n')) {
                    self.bump();
                }
            }
            Some(c) => return self.err(format!("invalid escape '\\{c}'")),
            None => return self.err("unterminated escape"),
        }
        Ok(())
    }

    fn parse_hex_escape(&mut self, digits: usize) -> Result<char, String> {
        let mut value: u32 = 0;
        for _ in 0..digits {
            let Some(c) = self.peek() else {
                return self.err("unterminated unicode escape");
            };
            let Some(d) = c.to_digit(16) else {
                return self.err("invalid unicode escape digit");
            };
            self.bump();
            value = value * 16 + d;
        }
        char::from_u32(value).ok_or_else(|| format!("line {}: invalid unicode scalar", self.line))
    }
}

// --- TOML table rules --------------------------------------------------------

fn table_at_mut<'a>(root: &'a mut TTable, keys: &[String]) -> Result<&'a mut TTable, String> {
    let mut t = root;
    for key in keys {
        let Some(pos) = t.entries.iter().position(|(k, _)| k == key) else {
            return Err(format!("table {key:?} disappeared"));
        };
        t = match &mut t.entries[pos].1 {
            TNode::Table(sub) => sub,
            TNode::Tables(v) => v
                .last_mut()
                .ok_or_else(|| format!("empty table array {key:?}"))?,
            _ => return Err(format!("{key:?} is not a table")),
        };
    }
    Ok(t)
}

fn define_table(root: &mut TTable, keys: &[String], array: bool) -> Result<Vec<PathSeg>, String> {
    let (last, parents) = keys
        .split_last()
        .ok_or_else(|| "empty table name".to_string())?;
    let mut path = Vec::new();
    let mut t = root;

    for key in parents {
        path.push(PathSeg::Key(key.clone()));
        let pos = match t.entries.iter().position(|(k, _)| k == key) {
            Some(p) => p,
            None => {
                t.entries.push((
                    key.clone(),
                    TNode::Table(TTable {
                        implicit: true,
                        ..TTable::default()
                    }),
                ));
                t.entries.len() - 1
            }
        };
        t = match &mut t.entries[pos].1 {
            TNode::Table(sub) => {
                if sub.inline {
                    return Err(format!("table {key:?} is an inline table"));
                }
                sub
            }
            TNode::Tables(v) => {
                let n = v
                    .len()
                    .checked_sub(1)
                    .ok_or_else(|| format!("empty table array {key:?}"))?;
                path.push(PathSeg::Idx(n));
                &mut v[n]
            }
            _ => return Err(format!("{key:?} is not a table")),
        };
    }

    path.push(PathSeg::Key(last.clone()));
    let pos = t.entries.iter().position(|(k, _)| k == last);
    if array {
        match pos {
            Some(p) => match &mut t.entries[p].1 {
                TNode::Tables(v) => {
                    v.push(TTable {
                        explicit: true,
                        ..TTable::default()
                    });
                    path.push(PathSeg::Idx(v.len() - 1));
                }
                _ => {
                    return Err(format!("cannot redefine {last:?} as an array of tables"));
                }
            },
            None => {
                t.entries.push((
                    last.clone(),
                    TNode::Tables(vec![TTable {
                        explicit: true,
                        ..TTable::default()
                    }]),
                ));
                path.push(PathSeg::Idx(0));
            }
        }
        return Ok(path);
    }

    match pos {
        Some(p) => match &mut t.entries[p].1 {
            TNode::Table(sub) if !sub.inline => {
                if sub.explicit {
                    return Err(format!("table {last:?} is already defined"));
                }
                if sub.dotted {
                    return Err(format!(
                        "table {last:?} was already defined by a dotted key"
                    ));
                }
                sub.explicit = true;
                sub.implicit = false;
            }
            TNode::Table(_) => return Err(format!("table {last:?} is an inline table")),
            _ => return Err(format!("cannot redefine {last:?} as a table")),
        },
        None => {
            t.entries.push((
                last.clone(),
                TNode::Table(TTable {
                    explicit: true,
                    ..TTable::default()
                }),
            ));
        }
    }
    Ok(path)
}

fn insert_key(t: &mut TTable, keys: &[String], node: TNode) -> Result<(), String> {
    let (last, parents) = keys.split_last().ok_or_else(|| "empty key".to_string())?;
    if parents.is_empty() {
        if t.entries.iter().any(|(k, _)| k == last) {
            return Err(format!("duplicate key {last:?}"));
        }
        t.entries.push((last.clone(), node));
        return Ok(());
    }
    let pos = match t.entries.iter().position(|(k, _)| *k == parents[0]) {
        Some(p) => p,
        None => {
            t.entries.push((
                parents[0].clone(),
                TNode::Table(TTable {
                    dotted: true,
                    ..TTable::default()
                }),
            ));
            t.entries.len() - 1
        }
    };
    let sub = match &mut t.entries[pos].1 {
        TNode::Table(sub) if !sub.inline && !sub.explicit => {
            sub.dotted = true;
            sub
        }
        TNode::Table(sub) if sub.inline => {
            return Err(format!("inline table {:?} cannot be extended", parents[0]));
        }
        TNode::Table(_) => {
            return Err(format!(
                "table {:?} is already defined; dotted keys cannot extend it",
                parents[0]
            ));
        }
        _ => return Err(format!("{:?} is not a table", parents[0])),
    };
    insert_key(sub, &keys[1..], node)
}

// --- scalar spellings --------------------------------------------------------

fn is_bare_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Strip underscores only when they sit between two valid digits.
fn digits_no_underscore(s: &str, valid: fn(char) -> bool) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_digit = false;
    let mut prev_underscore = false;
    for c in s.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_underscore = true;
        } else if valid(c) {
            out.push(c);
            prev_digit = true;
            prev_underscore = false;
        } else {
            return None;
        }
    }
    if prev_underscore {
        return None;
    }
    Some(out)
}

fn int_canonical(tok: &str) -> Result<Option<String>, String> {
    let (sign, body) = match tok.as_bytes().first() {
        Some(b'+') | Some(b'-') => (&tok[..1], &tok[1..]),
        _ => ("", tok),
    };
    if let Some(rest) = body.strip_prefix("0x") {
        if !sign.is_empty() {
            return Err(format!("signed hexadecimal integer {tok:?}"));
        }
        let ds = digits_no_underscore(rest, |c| c.is_ascii_hexdigit())
            .ok_or_else(|| format!("invalid hexadecimal integer {tok:?}"))?;
        let v =
            u64::from_str_radix(&ds, 16).map_err(|_| format!("integer out of range {tok:?}"))?;
        if v > i64::MAX as u64 {
            return Err(format!("integer out of range {tok:?}"));
        }
        return Ok(Some(v.to_string()));
    }
    if let Some(rest) = body.strip_prefix("0o") {
        if !sign.is_empty() {
            return Err(format!("signed octal integer {tok:?}"));
        }
        let ds = digits_no_underscore(rest, |c| ('0'..='7').contains(&c))
            .ok_or_else(|| format!("invalid octal integer {tok:?}"))?;
        let v = u64::from_str_radix(&ds, 8).map_err(|_| format!("integer out of range {tok:?}"))?;
        if v > i64::MAX as u64 {
            return Err(format!("integer out of range {tok:?}"));
        }
        return Ok(Some(v.to_string()));
    }
    if let Some(rest) = body.strip_prefix("0b") {
        if !sign.is_empty() {
            return Err(format!("signed binary integer {tok:?}"));
        }
        let ds = digits_no_underscore(rest, |c| c == '0' || c == '1')
            .ok_or_else(|| format!("invalid binary integer {tok:?}"))?;
        let v = u64::from_str_radix(&ds, 2).map_err(|_| format!("integer out of range {tok:?}"))?;
        if v > i64::MAX as u64 {
            return Err(format!("integer out of range {tok:?}"));
        }
        return Ok(Some(v.to_string()));
    }
    if !body.starts_with(|c: char| c.is_ascii_digit()) {
        return Ok(None);
    }
    // Fraction/exponent markers mean this is a float, not a decimal integer.
    if body.contains(['.', 'e', 'E']) {
        return Ok(None);
    }
    if body.starts_with('0') && body.len() > 1 && body.as_bytes()[1] != b'_' {
        return Err(format!("leading zero in integer {tok:?}"));
    }
    let Some(ds) = digits_no_underscore(body, |c| c.is_ascii_digit()) else {
        return Ok(None);
    };
    if ds.len() > 1 && ds.starts_with('0') {
        return Err(format!("leading zero in integer {tok:?}"));
    }
    let lit = format!("{sign}{ds}");
    let parsed = lit
        .parse::<i64>()
        .map_err(|_| format!("integer out of range {tok:?}"))?;
    Ok(Some(parsed.to_string()))
}

fn float_canonical(tok: &str) -> Result<Option<String>, String> {
    let (sign, body) = match tok.as_bytes().first() {
        Some(b'+') | Some(b'-') => (&tok[..1], &tok[1..]),
        _ => ("", tok),
    };
    if body == "inf" || body == "nan" {
        return Err(format!(
            "unsupported TOML float {tok:?} (JSON cannot represent it)"
        ));
    }
    if !body.starts_with(|c: char| c.is_ascii_digit()) {
        return Ok(None);
    }
    let (mantissa, exp) = match body.find(['e', 'E']) {
        Some(p) => (&body[..p], Some(&body[p..])),
        None => (body, None),
    };
    if !mantissa.contains('.') && exp.is_none() {
        return Ok(None);
    }
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    if int_part.is_empty() {
        return Ok(None);
    }
    let Some(int_digits) = digits_no_underscore(int_part, |c| c.is_ascii_digit()) else {
        return Ok(None);
    };
    if int_digits.len() > 1 && int_digits.starts_with('0') {
        return Err(format!("leading zero in float {tok:?}"));
    }
    let mut canonical = String::new();
    if sign == "-" {
        canonical.push('-');
    }
    canonical.push_str(&int_digits);
    if let Some(frac) = frac_part {
        let Some(fdigits) = digits_no_underscore(frac, |c| c.is_ascii_digit()) else {
            return Err(format!("invalid float fraction in {tok:?}"));
        };
        canonical.push('.');
        canonical.push_str(&fdigits);
    }
    if let Some(exp) = exp {
        let e = &exp[..1];
        let rest = &exp[1..];
        let (esign, edigits) = match rest.as_bytes().first() {
            Some(b'+') | Some(b'-') => (&rest[..1], &rest[1..]),
            _ => ("", rest),
        };
        let Some(ed) = digits_no_underscore(edigits, |c| c.is_ascii_digit()) else {
            return Err(format!("invalid float exponent in {tok:?}"));
        };
        canonical.push_str(e);
        canonical.push_str(esign);
        canonical.push_str(&ed);
    }
    Ok(Some(canonical))
}

// --- date/time ---------------------------------------------------------------

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        if r.clone().all(|i| b[i].is_ascii_digit()) {
            Some(s[r].parse().ok()?)
        } else {
            None
        }
    };
    let Some(year) = digits(0..4) else {
        return false;
    };
    let Some(month) = digits(5..7) else {
        return false;
    };
    let Some(day) = digits(8..10) else {
        return false;
    };
    let _ = year;
    if !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=max).contains(&day)
}

fn looks_like_time(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 8 && b[2] == b':' && b[5] == b':'
}

/// Byte length of a valid local time body (HH:MM:SS with optional fraction).
fn time_body_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 8 || b[2] != b':' || b[5] != b':' {
        return None;
    }
    let two = |i: usize| -> Option<u32> {
        if b[i].is_ascii_digit() && b[i + 1].is_ascii_digit() {
            Some((b[i] - b'0') as u32 * 10 + (b[i + 1] - b'0') as u32)
        } else {
            None
        }
    };
    let hh = two(0)?;
    let mm = two(3)?;
    let ss = two(6)?;
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let mut i = 8;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    Some(i)
}

fn is_time(s: &str) -> bool {
    time_body_len(s) == Some(s.len())
}

fn is_datetime(s: &str) -> bool {
    if !s.is_ascii() {
        return false;
    }
    if is_date(s) || is_time(s) {
        return true;
    }
    let b = s.as_bytes();
    if b.len() < 11 || !is_date(&s[..10]) {
        return false;
    }
    if !matches!(b[10], b'T' | b't' | b' ') {
        return false;
    }
    let rest = &s[11..];
    let Some(n) = time_body_len(rest) else {
        return false;
    };
    if n == rest.len() {
        return true;
    }
    let off = &rest[n..];
    if off == "Z" || off == "z" {
        return true;
    }
    let ob = off.as_bytes();
    if ob.len() == 6 && matches!(ob[0], b'+' | b'-') && ob[3] == b':' {
        let hh = (ob[1] - b'0') as u32 * 10 + (ob[2] - b'0') as u32;
        let mm = (ob[4] - b'0') as u32 * 10 + (ob[5] - b'0') as u32;
        return hh <= 23 && mm <= 59;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Doc {
        parse(src).expect("parse").0
    }

    fn s(v: &Value) -> String {
        match v {
            Value::Str(s) => s.clone(),
            other => panic!("not a string: {other:?}"),
        }
    }

    fn get<'a>(doc: &'a Doc, path: &[&str]) -> &'a Value {
        let p: Vec<PathSeg> = path.iter().map(|k| PathSeg::Key(k.to_string())).collect();
        doc.value(doc.get(&p).expect("path"))
    }

    #[test]
    fn parses_tables_dotted_keys_and_arrays() {
        let d = doc("title = \"demo\"\n\
             [server]\n\
             host = \"localhost\"\n\
             port = 8080\n\
             tls.enabled = true\n\
             tls.ciphers = [\"a\", \"b\"]\n\
             [[mounts]]\n\
             src = \"/a\"\n\
             [[mounts]]\n\
             src = \"/b\"\n");
        assert_eq!(s(get(&d, &["title"])), "demo");
        assert_eq!(s(get(&d, &["server", "host"])), "localhost");
        assert_eq!(get(&d, &["server", "port"]), &Value::Num("8080".into()));
        assert_eq!(get(&d, &["server", "tls", "enabled"]), &Value::Bool(true));
        let mounts = d.get(&[PathSeg::Key("mounts".into())]).unwrap();
        assert!(matches!(d.value(mounts), Value::Array(a) if a.len() == 2));
        let first_src = d
            .get(&[
                PathSeg::Key("mounts".into()),
                PathSeg::Idx(0),
                PathSeg::Key("src".into()),
            ])
            .unwrap();
        assert_eq!(s(d.value(first_src)), "/a");
    }

    #[test]
    fn keeps_non_json_number_and_datetime_spellings() {
        let (d, _) = parse(
            "hex = 0x1F\noct = 0o17\nbin = 0b101\nunder = 1_000\nwhen = 1979-05-27T07:32:00Z\n",
        )
        .unwrap();
        assert_eq!(get(&d, &["hex"]), &Value::Num("31".into()));
        assert_eq!(
            d.node(d.get(&[PathSeg::Key("hex".into())]).unwrap()).raw,
            "0x1F"
        );
        assert_eq!(s(get(&d, &["when"])), "1979-05-27T07:32:00Z");
        assert_eq!(
            d.node(d.get(&[PathSeg::Key("when".into())]).unwrap()).raw,
            "1979-05-27T07:32:00Z"
        );
    }

    #[test]
    fn parses_floats() {
        let d = doc("a = 0.5\nb = -0.0\nc = 1e3\nd = 1_000.5\ne = +1.25e-2\nf = 6.02E23\n");
        assert_eq!(get(&d, &["a"]), &Value::Num("0.5".into()));
        assert_eq!(get(&d, &["b"]), &Value::Num("-0.0".into()));
        assert_eq!(get(&d, &["c"]), &Value::Num("1e3".into()));
        assert_eq!(get(&d, &["d"]), &Value::Num("1000.5".into()));
        assert_eq!(get(&d, &["e"]), &Value::Num("1.25e-2".into()));
        assert_eq!(get(&d, &["f"]), &Value::Num("6.02E23".into()));
        let out = crate::doc::tomldoc::write(&d, &Meta::default()).unwrap();
        assert!(out.contains("d = 1_000.5\n"), "{out}");
    }

    #[test]
    fn strips_bom() {
        let (d, _) = parse("\u{feff}a = 1\n").unwrap();
        assert_eq!(get(&d, &["a"]), &Value::Num("1".into()));
    }

    #[test]
    fn parses_string_forms_and_inline_tables() {
        let d = doc("a = \"line\\nquote\\\"\"\n\
             b = 'literal \\n'\n\
             c = \"\"\"\nmulti\nline\"\"\"\n\
             d = '''\nraw\\n'''\n\
             e = { x = 1, y.z = \"q\" }\n\
             f = [1, [2, 3], { k = true },]\n");
        assert_eq!(s(get(&d, &["a"])), "line\nquote\"");
        assert_eq!(s(get(&d, &["b"])), "literal \\n");
        assert_eq!(s(get(&d, &["c"])), "multi\nline");
        assert_eq!(s(get(&d, &["d"])), "raw\\n");
        assert_eq!(s(get(&d, &["e", "y", "z"])), "q");
        assert!(matches!(get(&d, &["f"]), Value::Array(a) if a.len() == 3));
    }

    #[test]
    fn rejects_duplicates_and_bad_numbers() {
        for (src, want) in [
            ("a = 1\na = 2\n", "duplicate key"),
            ("[a]\n[a]\n", "already defined"),
            ("a.b = 1\n[a]\n", "already defined"),
            ("a = 01\n", "leading zero"),
            ("a = 0x\n", "invalid hexadecimal"),
            ("a = 1.\n", "invalid float"),
            ("a = inf\n", "unsupported TOML float"),
            ("a = \"x\n", "unterminated"),
            ("a = 1979-13-01\n", "invalid value"),
            ("a = 名字名字\n", "invalid value"),
        ] {
            let err = parse(src).expect_err(src).msg;
            assert!(err.contains(want), "{src:?}: {err}");
        }
    }

    #[test]
    fn comments_follow_their_members() {
        let (d, meta) = parse(
            "# top\n\
             a = 1 # inline\n\
             # before b\n\
             b = 2\n\
             [t] # table note\n\
             c = 3\n\
             # eof\n",
        )
        .unwrap();
        assert_eq!(meta.comments[&vec![]].head, "# top");
        let a = vec![PathSeg::Key("a".into())];
        assert_eq!(meta.comments[&a].key_line, "# inline");
        let b = vec![PathSeg::Key("b".into())];
        assert_eq!(meta.comments[&b].key_head, "# before b");
        let t = vec![PathSeg::Key("t".into())];
        assert_eq!(meta.comments[&t].key_line, "# table note");
        assert!(meta.tail.contains("# eof"));
        assert_eq!(
            d.get(&[PathSeg::Key("t".into()), PathSeg::Key("c".into())])
                .is_some(),
            true
        );
    }

    #[test]
    fn value_parser_handles_scalars_and_containers() {
        let (d, _) = parse_value("\"x\"").unwrap();
        assert_eq!(s(d.value(d.root)), "x");
        let (d, _) = parse_value("[1, 2]").unwrap();
        assert!(matches!(d.value(d.root), Value::Array(a) if a.len() == 2));
        let (d, _) = parse_value("{ a = 1 }").unwrap();
        assert!(matches!(d.value(d.root), Value::Object(_)));
        assert!(parse_value("null").is_err());
    }
}
