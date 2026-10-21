//! JSONC: strict JSON plus `//` and `/* */` comments and trailing commas.
//!
//! Rust port of `internal/jsoncdoc/jsoncdoc.go`. The same scanner serves
//! both parsers: `parse` (lenient — comments and trailing commas accepted,
//! exactly the Go jsoncdoc behavior) and `jsondoc::parse` (strict — comments
//! rejected, trailing commas rejected, matching Go's encoding/json).
//!
//! Comments follow the key or value they belong to: an edited member keeps
//! its comments, a deleted member takes its comments with it, and a newly
//! added member has none. A document without comments is written by
//! delegating to [`super::jsondoc::Doc::write`], so plain JSON output stays
//! byte-for-byte identical to the historical format.

use super::jsondoc::{self, Doc, DocError, Member, NodeId, PathSeg, Value};
use super::{CommentSet, Meta};

fn err<T>(msg: impl Into<String>) -> Result<T, DocError> {
    Err(DocError { msg: msg.into() })
}

/// Parse reads one JSONC document into a document tree and captures comments.
pub fn parse(text: &str) -> Result<(Doc, Meta), DocError> {
    parse_impl(text, true)
}

/// Shared entry: lenient mode accepts comments and trailing commas (JSONC);
/// strict mode rejects both (plain JSON, Go encoding/json behavior).
pub(crate) fn parse_impl(text: &str, lenient: bool) -> Result<(Doc, Meta), DocError> {
    let data = text.as_bytes();
    let newlines: Vec<usize> = data
        .iter()
        .enumerate()
        .filter_map(|(i, &b)| (b == b'\n').then_some(i))
        .collect();
    let mut p = Parser {
        data,
        pos: 0,
        lenient,
        meta: Meta::default(),
        newlines,
    };
    let head = p.skip()?;
    let mut doc = Doc::new();
    let root = p.parse_value_into(&mut doc, &[])?;
    doc.root = root;
    let value_line = p.line_at(p.pos);
    let tail_toks = p.skip_tokens()?;
    if p.pos < p.data.len() {
        return err("trailing data after JSON document");
    }
    let (same, rest) = split_line(&tail_toks, value_line);
    if !same.is_empty() {
        let cs = p.meta.comments.entry(Vec::new()).or_default();
        cs.line = join_tokens(&same);
    }
    p.meta.tail = join_tokens(&rest);
    if !head.is_empty() {
        let cs = p.meta.comments.entry(Vec::new()).or_default();
        cs.head = head;
    }
    Ok((doc, p.meta))
}

struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
    lenient: bool,
    meta: Meta,
    /// byte offsets of every '\n', for O(log n) line lookups: rescanning the
    /// whole prefix per call made parsing a 479 KB schema quadratic (~2 s)
    newlines: Vec<usize>,
}

/// commentTok is one comment with the line it starts on, used to keep a
/// comment written after a member's comma on that member's line.
struct CommentTok {
    text: String,
    line: usize,
}

impl<'a> Parser<'a> {
    fn line_at(&self, pos: usize) -> usize {
        self.newlines.partition_point(|&off| off < pos)
    }

    /// skipTokens consumes whitespace (and comments in lenient mode),
    /// returning the comments in order with their start lines.
    fn skip_tokens(&mut self) -> Result<Vec<CommentTok>, DocError> {
        let mut comments = Vec::new();
        while self.pos < self.data.len() {
            match self.data[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                b'/' if self.lenient => {
                    if self.pos + 1 >= self.data.len() {
                        return err(format!(
                            "invalid character {} looking for beginning of value",
                            quote_byte(b'/')
                        ));
                    }
                    let line = self.line_at(self.pos);
                    match self.data[self.pos + 1] {
                        b'/' => {
                            let start = self.pos;
                            self.pos += 2;
                            while self.pos < self.data.len() && self.data[self.pos] != b'\n' {
                                self.pos += 1;
                            }
                            comments.push(CommentTok {
                                text: String::from_utf8_lossy(&self.data[start..self.pos])
                                    .trim_end_matches([' ', '\t', '\r'])
                                    .to_string(),
                                line,
                            });
                        }
                        b'*' => {
                            let start = self.pos;
                            self.pos += 2;
                            while self.pos + 1 < self.data.len()
                                && !(self.data[self.pos] == b'*' && self.data[self.pos + 1] == b'/')
                            {
                                self.pos += 1;
                            }
                            if self.pos + 1 >= self.data.len() {
                                return err("unexpected end of JSON input");
                            }
                            self.pos += 2;
                            comments.push(CommentTok {
                                text: String::from_utf8_lossy(&self.data[start..self.pos])
                                    .to_string(),
                                line,
                            });
                        }
                        _ => {
                            return err(format!(
                                "invalid character {} looking for beginning of value",
                                quote_byte(b'/')
                            ));
                        }
                    }
                }
                _ => return Ok(comments),
            }
        }
        Ok(comments)
    }

    /// skip consumes whitespace and comments, returning the comments in
    /// order, one per line.
    fn skip(&mut self) -> Result<String, DocError> {
        let toks = self.skip_tokens()?;
        Ok(join_tokens(&toks))
    }

    fn unexpected(&self) -> DocError {
        if self.pos >= self.data.len() {
            return DocError {
                msg: "unexpected end of JSON input".to_string(),
            };
        }
        DocError {
            msg: format!(
                "invalid character {} looking for beginning of value",
                quote_byte(self.data[self.pos])
            ),
        }
    }

    fn peek_byte(&self) -> u8 {
        if self.pos >= self.data.len() {
            0
        } else {
            self.data[self.pos]
        }
    }

    fn parse_value_into(&mut self, doc: &mut Doc, path: &[PathSeg]) -> Result<NodeId, DocError> {
        if self.pos >= self.data.len() {
            return Err(self.unexpected());
        }
        match self.data[self.pos] {
            b'{' => self.parse_object(doc, path),
            b'[' => self.parse_array(doc, path),
            b'"' => {
                let s = self.parse_string()?;
                Ok(doc.new_str(s))
            }
            b'-' | b'0'..=b'9' => {
                let num = self.parse_number()?;
                Ok(doc.new_num(num))
            }
            b't' if self.data[self.pos..].starts_with(b"true") => {
                self.pos += 4;
                Ok(doc.new_bool(true))
            }
            b'f' if self.data[self.pos..].starts_with(b"false") => {
                self.pos += 5;
                Ok(doc.new_bool(false))
            }
            b'n' if self.data[self.pos..].starts_with(b"null") => {
                self.pos += 4;
                Ok(doc.new_null())
            }
            _ => Err(self.unexpected()),
        }
    }

    /// parseObject parses an object. A comment written after a member's
    /// comma on the same line stays with that member (Line); comments on
    /// later lines become the next key's Head. Comments after the last
    /// member become the container's Foot unless they sit on the value's own
    /// line.
    fn parse_object(&mut self, doc: &mut Doc, path: &[PathSeg]) -> Result<NodeId, DocError> {
        self.pos += 1; // '{'
        let obj = doc.new_object();
        let mut members: Vec<Member> = Vec::new();
        let mut carry = String::new();
        loop {
            let mut pending = self.skip()?;
            pending = join_comments(&carry, &pending);
            carry.clear();
            if self.pos >= self.data.len() {
                return Err(self.unexpected());
            }
            if self.data[self.pos] == b'}' {
                self.pos += 1;
                self.record_container_foot(path, &pending);
                if let Value::Object(m) = &mut doc.nodes[obj as usize].value {
                    *m = members;
                }
                return Ok(obj);
            }
            if self.data[self.pos] != b'"' {
                return err(format!(
                    "invalid character {} looking for beginning of object key string",
                    quote_byte(self.peek_byte())
                ));
            }
            let key = self.parse_string()?;
            let after_key = self.skip()?;
            pending = join_comments(&pending, &after_key);
            if self.pos >= self.data.len() || self.data[self.pos] != b':' {
                return err(format!(
                    "invalid character {} after object key",
                    quote_byte(self.peek_byte())
                ));
            }
            self.pos += 1;
            let vhead = self.skip()?;
            let mut mpath = path.to_vec();
            mpath.push(PathSeg::Key(key.clone()));
            let val = self.parse_value_into(doc, &mpath)?;
            let value_line = self.line_at(self.pos);
            let post_toks = self.skip_tokens()?;
            let mut cs = CommentSet {
                key_head: pending,
                head: vhead,
                ..Default::default()
            };
            if self.pos < self.data.len() && self.data[self.pos] == b',' {
                cs.line = join_tokens(&post_toks);
                self.pos += 1;
                let after_toks = self.skip_tokens()?;
                if !self.lenient {
                    // strict JSON: no trailing comma before the closer
                    let mut j = self.pos;
                    while j < self.data.len()
                        && matches!(self.data[j], b' ' | b'\t' | b'\r' | b'\n')
                    {
                        j += 1;
                    }
                    if j < self.data.len() && self.data[j] == b'}' {
                        return err(format!(
                            "invalid character {} looking for beginning of object key string",
                            quote_byte(b'}')
                        ));
                    }
                }
                let (same, rest) = split_line(&after_toks, value_line);
                cs.line = join_comments(&cs.line, &join_tokens(&same));
                carry = join_tokens(&rest);
                self.record(&mpath, cs);
                members.push(Member { key, val });
            } else if self.pos < self.data.len() && self.data[self.pos] == b'}' {
                let (same, rest) = split_line(&post_toks, value_line);
                cs.line = join_tokens(&same);
                self.record(&mpath, cs);
                self.record_container_foot(path, &join_tokens(&rest));
                self.pos += 1;
                members.push(Member { key, val });
                if let Value::Object(m) = &mut doc.nodes[obj as usize].value {
                    *m = members;
                }
                return Ok(obj);
            } else {
                return err(format!(
                    "invalid character {} after object key:value pair",
                    quote_byte(self.peek_byte())
                ));
            }
        }
    }

    /// parseArray parses an array with the same comment rules as
    /// parseObject.
    fn parse_array(&mut self, doc: &mut Doc, path: &[PathSeg]) -> Result<NodeId, DocError> {
        self.pos += 1; // '['
        let arr = doc.new_array();
        let mut items: Vec<NodeId> = Vec::new();
        let mut carry = String::new();
        loop {
            let mut head = self.skip()?;
            head = join_comments(&carry, &head);
            carry.clear();
            if self.pos >= self.data.len() {
                return Err(self.unexpected());
            }
            if self.data[self.pos] == b']' {
                self.pos += 1;
                self.record_container_foot(path, &head);
                if let Value::Array(a) = &mut doc.nodes[arr as usize].value {
                    *a = items;
                }
                return Ok(arr);
            }
            let mut ipath = path.to_vec();
            ipath.push(PathSeg::Idx(items.len()));
            let val = self.parse_value_into(doc, &ipath)?;
            let value_line = self.line_at(self.pos);
            let post_toks = self.skip_tokens()?;
            if self.pos < self.data.len() && self.data[self.pos] == b',' {
                let mut cs = CommentSet {
                    head,
                    line: join_tokens(&post_toks),
                    ..Default::default()
                };
                self.pos += 1;
                let after_toks = self.skip_tokens()?;
                if !self.lenient {
                    // strict JSON: no trailing comma before the closer
                    let mut j = self.pos;
                    while j < self.data.len()
                        && matches!(self.data[j], b' ' | b'\t' | b'\r' | b'\n')
                    {
                        j += 1;
                    }
                    if j < self.data.len() && self.data[j] == b']' {
                        return err(format!(
                            "invalid character {} looking for beginning of value",
                            quote_byte(b']')
                        ));
                    }
                }
                let (same, rest) = split_line(&after_toks, value_line);
                cs.line = join_comments(&cs.line, &join_tokens(&same));
                carry = join_tokens(&rest);
                self.record(&ipath, cs);
                items.push(val);
            } else if self.pos < self.data.len() && self.data[self.pos] == b']' {
                let (same, rest) = split_line(&post_toks, value_line);
                self.record(
                    &ipath,
                    CommentSet {
                        head,
                        line: join_tokens(&same),
                        ..Default::default()
                    },
                );
                self.record_container_foot(path, &join_tokens(&rest));
                self.pos += 1;
                items.push(val);
                if let Value::Array(a) = &mut doc.nodes[arr as usize].value {
                    *a = items;
                }
                return Ok(arr);
            } else {
                return err(format!(
                    "invalid character {} after array element",
                    quote_byte(self.peek_byte())
                ));
            }
        }
    }

    /// record stores cs at path, preserving a container Foot already
    /// captured for a nested value parsed before its parent member is
    /// recorded.
    fn record(&mut self, path: &[PathSeg], mut cs: CommentSet) {
        if let Some(prev) = self.meta.comments.get(path) {
            if cs.foot.is_empty() {
                cs.foot = prev.foot.clone();
            }
        }
        if cs != CommentSet::default() {
            self.meta.comments.insert(path.to_vec(), cs);
        }
    }

    /// recordContainerFoot stores the comments just inside path's closing
    /// brace or bracket, preserving the member/value comments already
    /// recorded there.
    fn record_container_foot(&mut self, path: &[PathSeg], foot: &str) {
        if foot.is_empty() {
            return;
        }
        let cs = self.meta.comments.entry(path.to_vec()).or_default();
        cs.foot = foot.to_string();
    }

    fn parse_string(&mut self) -> Result<String, DocError> {
        let start = self.pos;
        self.pos += 1; // opening quote
        while self.pos < self.data.len() {
            match self.data[self.pos] {
                b'\\' => self.pos += 2,
                b'"' => {
                    self.pos += 1;
                    let raw = String::from_utf8_lossy(&self.data[start..self.pos]).into_owned();
                    return decode_json_string(&raw);
                }
                _ => self.pos += 1,
            }
        }
        err("unexpected end of JSON input")
    }

    fn parse_number(&mut self) -> Result<String, DocError> {
        let start = self.pos;
        if self.data[self.pos] == b'-' {
            self.pos += 1;
        }
        if self.pos >= self.data.len() || !self.data[self.pos].is_ascii_digit() {
            return err(format!(
                "invalid character {} in numeric literal",
                quote_byte(self.peek_byte())
            ));
        }
        if self.data[self.pos] == b'0' {
            self.pos += 1;
        } else {
            while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos < self.data.len() && self.data[self.pos] == b'.' {
            self.pos += 1;
            if self.pos >= self.data.len() || !self.data[self.pos].is_ascii_digit() {
                return err(format!(
                    "invalid character {} after decimal point in numeric literal",
                    quote_byte(self.peek_byte())
                ));
            }
            while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos < self.data.len()
            && (self.data[self.pos] == b'e' || self.data[self.pos] == b'E')
        {
            self.pos += 1;
            if self.pos < self.data.len()
                && (self.data[self.pos] == b'+' || self.data[self.pos] == b'-')
            {
                self.pos += 1;
            }
            if self.pos >= self.data.len() || !self.data[self.pos].is_ascii_digit() {
                return err(format!(
                    "invalid character {} in exponent of numeric literal",
                    quote_byte(self.peek_byte())
                ));
            }
            while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        Ok(String::from_utf8_lossy(&self.data[start..self.pos]).to_string())
    }
}

fn join_tokens(toks: &[CommentTok]) -> String {
    toks.iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// splitLine separates the tokens that start on line from the rest.
fn split_line(toks: &[CommentTok], line: usize) -> (Vec<CommentTok>, Vec<CommentTok>) {
    let mut same = Vec::new();
    let mut rest = Vec::new();
    for t in toks {
        if t.line == line {
            same.push(CommentTok {
                text: t.text.clone(),
                line: t.line,
            });
        } else {
            rest.push(CommentTok {
                text: t.text.clone(),
                line: t.line,
            });
        }
    }
    (same, rest)
}

fn join_comments(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (false, true) => a.to_string(),
        (false, false) => format!("{a}\n{b}"),
    }
}

/// Go %q of a byte, for error messages: printable ASCII verbatim, the
/// standard escapes, hex for other control bytes.
pub(crate) fn quote_byte(b: u8) -> String {
    match b {
        b'\'' | b'\\' => format!("'\\{}'", b as char),
        0x20..=0x7e => format!("'{}'", b as char),
        b'\n' => "'\\n'".to_string(),
        b'\r' => "'\\r'".to_string(),
        b'\t' => "'\\t'".to_string(),
        _ => format!("'\\x{b:02x}'"),
    }
}

/// Decode a JSON string literal (with quotes) the way encoding/json does:
/// raw control characters are errors, escapes are expanded, \uXXXX handles
/// surrogate pairs, and a lone surrogate is an error.
fn decode_json_string(raw: &str) -> Result<String, DocError> {
    let body = &raw[1..raw.len() - 1];
    let b = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => {
                i += 1;
                if i >= b.len() {
                    return err("unexpected end of JSON input");
                }
                match b[i] {
                    b'"' => {
                        out.push('"');
                        i += 1;
                    }
                    b'\\' => {
                        out.push('\\');
                        i += 1;
                    }
                    b'/' => {
                        out.push('/');
                        i += 1;
                    }
                    b'b' => {
                        out.push('\u{0008}');
                        i += 1;
                    }
                    b'f' => {
                        out.push('\u{000c}');
                        i += 1;
                    }
                    b'n' => {
                        out.push('\n');
                        i += 1;
                    }
                    b'r' => {
                        out.push('\r');
                        i += 1;
                    }
                    b't' => {
                        out.push('\t');
                        i += 1;
                    }
                    b'u' => {
                        let mut cp = decode_u4(b, i + 1)?;
                        i += 5;
                        if (0xd800..=0xdbff).contains(&cp) {
                            if b.get(i..i + 2) != Some(&b"\\u"[..]) {
                                return err("invalid surrogate pair in string literal");
                            }
                            let lo = decode_u4(b, i + 2)?;
                            if !(0xdc00..=0xdfff).contains(&lo) {
                                return err("invalid surrogate pair in string literal");
                            }
                            cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                            i += 6;
                        } else if (0xdc00..=0xdfff).contains(&cp) {
                            return err("invalid surrogate pair in string literal");
                        }
                        out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                    }
                    c => {
                        return err(format!(
                            "invalid character {} in string escape code",
                            quote_byte(c)
                        ));
                    }
                }
            }
            c if c < 0x20 => {
                return err(format!(
                    "invalid character {} in string literal",
                    quote_byte(c)
                ));
            }
            _ => {
                // one UTF-8 character (the input is a &str: bytes are valid UTF-8)
                let len = match b[i] {
                    0x00..=0x7f => 1,
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
                let end = (i + len).min(b.len());
                match std::str::from_utf8(&b[i..end]) {
                    Ok(s) => out.push_str(s),
                    Err(_) => {
                        // split surrogate reconstruction across chars is impossible
                        // here; push the replacement the decoder would
                        out.push('\u{fffd}');
                    }
                }
                i = end;
            }
        }
    }
    Ok(out)
}

/// decode_u4 decodes the 4 hex digits at b[start..start+4] (encoding/json
/// accepts upper and lower case).
fn decode_u4(b: &[u8], start: usize) -> Result<u32, DocError> {
    let hex = b.get(start..start + 4).ok_or_else(|| DocError {
        msg: "unexpected end of JSON input".to_string(),
    })?;
    let mut cp = 0u32;
    for &h in hex {
        let d = (h as char).to_digit(16).ok_or_else(|| DocError {
            msg: "invalid character in \\u hexadecimal character escape".to_string(),
        })?;
        cp = cp * 16 + d;
    }
    Ok(cp)
}

// --- write -----------------------------------------------------------------

/// Write emits the document. Without comments it delegates to jsondoc,
/// keeping the historical plain-JSON bytes; with comments it mirrors
/// jsondoc's two-space layout and re-inserts the comments.
pub fn write(doc: &Doc, meta: &Meta) -> String {
    if meta.comments.is_empty() && meta.tail.is_empty() {
        return doc.write();
    }
    let mut out = String::new();
    let root = meta.comments.get(&Vec::new()).cloned().unwrap_or_default();
    write_comments(&mut out, &root.head, 0);
    write_value(doc, doc.root, &[], 0, meta, &mut out);
    if !root.line.is_empty() {
        write_trailing_comment(&mut out, &root.line, 0);
    }
    out.push('\n');
    write_comments(&mut out, &meta.tail, 0);
    out
}

fn lookup(meta: &Meta, path: &[PathSeg]) -> CommentSet {
    meta.comments.get(path).cloned().unwrap_or_default()
}

/// writeComments emits each comment on its own line at the given indentation.
fn write_comments(out: &mut String, comment: &str, level: usize) {
    if comment.is_empty() {
        return;
    }
    for line in comment.split('\n') {
        write_indent(out, level);
        out.push_str(line.trim_end_matches([' ', '\t', '\r']));
        out.push('\n');
    }
}

/// writeTrailingComment appends a comment after a value (and after its
/// comma, when there is one) on the same line; extra lines become own-line
/// comments.
fn write_trailing_comment(out: &mut String, comment: &str, level: usize) {
    let lines: Vec<&str> = comment.split('\n').collect();
    out.push(' ');
    out.push_str(lines[0].trim_end_matches([' ', '\t', '\r']));
    for line in &lines[1..] {
        out.push('\n');
        write_indent(out, level);
        out.push_str(line.trim_end_matches([' ', '\t', '\r']));
    }
}

/// writeEmptyContainer emits an empty object/array, keeping comments that
/// were written between its delimiters.
fn write_empty_container(out: &mut String, open: char, close: char, foot: &str, indent: usize) {
    if foot.is_empty() {
        out.push(open);
        out.push(close);
        return;
    }
    out.push(open);
    out.push('\n');
    write_comments(out, foot, indent + 1);
    write_indent(out, indent);
    out.push(close);
}

fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn write_value(
    doc: &Doc,
    id: NodeId,
    path: &[PathSeg],
    indent: usize,
    meta: &Meta,
    out: &mut String,
) {
    match doc.value(id) {
        Value::Object(members) => {
            if members.is_empty() {
                let foot = lookup(meta, path).foot;
                write_empty_container(out, '{', '}', &foot, indent);
                return;
            }
            out.push_str("{\n");
            for (i, m) in members.iter().enumerate() {
                let mut mpath = path.to_vec();
                mpath.push(PathSeg::Key(m.key.clone()));
                let cs = lookup(meta, &mpath);
                write_comments(out, &cs.key_head, indent + 1);
                write_comments(out, &cs.head, indent + 1);
                write_indent(out, indent + 1);
                jsondoc::write_json_string(out, &m.key);
                out.push_str(": ");
                write_value(doc, m.val, &mpath, indent + 1, meta, out);
                if i < members.len() - 1 {
                    out.push(',');
                }
                if !cs.line.is_empty() {
                    write_trailing_comment(out, &cs.line, indent + 1);
                }
                out.push('\n');
            }
            write_comments(out, &lookup(meta, path).foot, indent);
            write_indent(out, indent);
            out.push('}');
        }
        Value::Array(items) => {
            if items.is_empty() {
                let foot = lookup(meta, path).foot;
                write_empty_container(out, '[', ']', &foot, indent);
                return;
            }
            out.push_str("[\n");
            for (i, &it) in items.iter().enumerate() {
                let mut ipath = path.to_vec();
                ipath.push(PathSeg::Idx(i));
                let cs = lookup(meta, &ipath);
                write_comments(out, &cs.head, indent + 1);
                write_indent(out, indent + 1);
                write_value(doc, it, &ipath, indent + 1, meta, out);
                if i < items.len() - 1 {
                    out.push(',');
                }
                if !cs.line.is_empty() {
                    write_trailing_comment(out, &cs.line, indent + 1);
                }
                out.push('\n');
            }
            write_comments(out, &lookup(meta, path).foot, indent);
            write_indent(out, indent);
            out.push(']');
        }
        Value::Str(s) => jsondoc::write_json_string(out, s),
        Value::Num(lit) => out.push_str(lit),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Null => out.push_str("null"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::jsondoc::parse;

    fn must_json(s: &str) -> Doc {
        parse(s).expect("test document must parse")
    }

    fn write_string(doc: &Doc, meta: &Meta) -> String {
        write(doc, meta)
    }

    #[test]
    fn parse_matches_json() {
        let cases = [
            r#"{"a":1}"#,
            r#"{"nested":{"k":"v"},"arr":[1,2,3]}"#,
            r#"{"s":"line\nquote\"tab\t"}"#,
            r#"{"f":1.0,"e":1e3,"neg":-0.5}"#,
            r#"{"b":true,"z":false,"n":null}"#,
            r#"{"dup":1,"dup":2}"#,
            r#"[1,{"a":[]},[[]]]"#,
            r#""scalar""#,
            "42",
            "null",
        ];
        for src in cases {
            let (doc, _) = super::parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            let want = must_json(src);
            assert_eq!(doc.write(), want.write(), "{src}");
        }
    }

    #[test]
    fn parse_comments() {
        let src = "// config header
{
  \"name\": \"demo\", // keep me
  // port note
  \"port\": 8080,
  \"items\": [
    // item note
    \"a\"
    // tail note
  ]
}
";
        let (doc, meta) = super::parse(src).unwrap();
        let want = must_json(r#"{"name":"demo","port":8080,"items":["a"]}"#);
        assert_eq!(doc.write(), want.write());

        let root = meta.comments.get(&Vec::new()).unwrap();
        assert_eq!(root.head, "// config header");
        let name = meta
            .comments
            .get(&vec![PathSeg::Key("name".into())])
            .unwrap();
        assert_eq!(name.line, "// keep me");
        let port = meta
            .comments
            .get(&vec![PathSeg::Key("port".into())])
            .unwrap();
        assert_eq!(port.key_head, "// port note");
        let item0 = meta
            .comments
            .get(&vec![PathSeg::Key("items".into()), PathSeg::Idx(0)])
            .unwrap();
        assert_eq!(item0.head, "// item note");
        let items = meta
            .comments
            .get(&vec![PathSeg::Key("items".into())])
            .unwrap();
        assert_eq!(items.foot, "// tail note");
    }

    #[test]
    fn parse_errors() {
        let cases = [
            ("missing value", r#"{"a": }"#, "invalid character"),
            (
                "unclosed string",
                r#"{"a": "x}"#,
                "unexpected end of JSON input",
            ),
            (
                "unclosed block comment",
                r#"{"a": 1 /* x}"#,
                "unexpected end of JSON input",
            ),
            (
                "trailing data",
                r#"{"a":1} garbage"#,
                "trailing data after JSON document",
            ),
            ("bare control char", "{\"a\":\"x\ny\"}", "invalid character"),
            ("unquoted key", "{a: 1}", "object key string"),
            ("empty", "", "unexpected end of JSON input"),
        ];
        for (name, src, want) in cases {
            match super::parse(src) {
                Ok(_) => panic!("{name}: expected error"),
                Err(e) => assert!(
                    e.to_string().contains(want),
                    "{name}: err = {e}, want {want:?}"
                ),
            }
        }
    }

    #[test]
    fn write_delegates_without_comments() {
        let doc = must_json(r#"{"a":1,"b":[2,3]}"#);
        let want = doc.write();
        assert_eq!(write_string(&doc, &Meta::default()), want, "empty meta");
    }

    #[test]
    fn write_comments_verbatim() {
        let src = "// config header
{
  \"name\": \"demo\", // keep me
  // port note
  \"port\": 8080,
  \"items\": [
    // item note
    \"a\"
  ]
}
";
        let (doc, meta) = super::parse(src).unwrap();
        assert_eq!(write_string(&doc, &meta), src);
    }

    #[test]
    fn write_normalizes_trailing_commas() {
        let (doc, meta) = super::parse(r#"{"a":1,"b":[2,3,],}"#).unwrap();
        let out = write_string(&doc, &meta);
        assert!(
            !out.contains(",]") && !out.contains(",}") && out.contains("\"b\": ["),
            "trailing commas not normalized:\n{out}"
        );
        let want = must_json(r#"{"a":1,"b":[2,3]}"#);
        assert_eq!(doc.write(), want.write());
    }

    #[test]
    fn comment_dropped_with_deleted_node() {
        let src = "{
  \"name\": \"demo\",
  // port note
  \"port\": 8080
}
";
        let (mut doc, meta) = super::parse(src).unwrap();
        assert!(doc.remove(&[PathSeg::Key("port".into())]));
        let out = write_string(&doc, &meta);
        assert!(
            !out.contains("// port note") && !out.contains("\"port\""),
            "deleted member leaked:\n{out}"
        );
    }

    #[test]
    fn write_edited_value_keeps_comments() {
        let src = "{
  \"name\": \"demo\" // keep me
}
";
        let (mut doc, meta) = super::parse(src).unwrap();
        let newname = doc.new_str("newname");
        doc.set(&[PathSeg::Key("name".into())], newname).unwrap();
        let want = "{
  \"name\": \"newname\" // keep me
}
";
        assert_eq!(write_string(&doc, &meta), want);
    }

    #[test]
    fn write_empty_container_comments() {
        let src = "{
  \"a\": {
    // empty note
  },
  \"b\": [
    // empty array
  ]
}
";
        let (doc, meta) = super::parse(src).unwrap();
        assert_eq!(write_string(&doc, &meta), src);
    }

    #[test]
    fn write_root_container_trailing_comment() {
        let src = "// hdr
{
  \"a\": 1
} // keep
";
        let (doc, meta) = super::parse(src).unwrap();
        assert_eq!(write_string(&doc, &meta), src);
    }

    #[test]
    fn write_tail_only_comment() {
        let src = "{\"a\": 1}\n// tail\n";
        let (doc, meta) = super::parse(src).unwrap();
        assert_eq!(meta.tail, "// tail");
        assert_eq!(write_string(&doc, &meta), "{\n  \"a\": 1\n}\n// tail\n");
    }

    #[test]
    fn write_root_scalar_comment() {
        let src = "// header\n\"x\"\n// tail\n";
        let (doc, meta) = super::parse(src).unwrap();
        assert_eq!(write_string(&doc, &meta), src);
    }

    #[test]
    fn strict_mode_rejects_comments_and_trailing_commas() {
        assert!(parse_impl("{\"a\": 1 // x\n}", false).is_err());
        assert!(parse_impl("{\"a\": 1,}", false).is_err());
        assert!(parse_impl("[1,]", false).is_err());
        // plain JSON still parses strictly
        assert!(parse_impl("{\"a\": 1}", false).is_ok());
    }
}
