//! TOML write: renders the document tree as a TOML document.
//!
//! The emitter is canonical rather than byte-preserving: tables become
//! `[table]` headers, arrays of tables become `[[table]]`, and strings use
//! basic quoted form. Key order, comments, and the exact source spelling of
//! non-JSON numbers and date/time literals (stored on `NodeData::raw`) are
//! retained.
//!
//! TOML puts scalar members before the first sub-table header of a table, so
//! members are emitted in two passes: direct values first, then tables in
//! their original relative order.

use crate::doc::jsondoc::{Doc, DocError, Member, NodeId, PathSeg, Value};
use crate::doc::{CommentSet, Meta};

/// Write emits a complete TOML document.
pub fn write(doc: &Doc, meta: &Meta) -> Result<String, DocError> {
    if !matches!(doc.value(doc.root), Value::Object(_)) {
        return Err(DocError {
            msg: "toml: document root must be a table".to_string(),
        });
    }
    let mut e = Emitter {
        doc,
        meta,
        out: String::new(),
    };
    if let Some(cs) = meta.comments.get(&[] as &[PathSeg]) {
        let head = combine(&cs.head, &cs.key_head);
        comment_lines(&mut e.out, &head, 0);
    }
    e.table_body(doc.root, &[]).map_err(|msg| DocError {
        msg: format!("toml: {msg}"),
    })?;
    if let Some(cs) = meta.comments.get(&[] as &[PathSeg]) {
        let foot = combine(&cs.foot, &cs.key_foot);
        if !foot.is_empty() {
            comment_lines(&mut e.out, &foot, 0);
        }
    }
    if !meta.tail.is_empty() {
        comment_lines(&mut e.out, &meta.tail, 0);
    }
    if !e.out.is_empty() && !e.out.ends_with('\n') {
        e.out.push('\n');
    }
    Ok(e.out)
}

/// WriteValue renders one subtree as a TOML value expression (the raw value
/// editor's format).
pub fn write_value(doc: &Doc, id: NodeId) -> Result<String, DocError> {
    let meta = Meta::default();
    let mut e = Emitter {
        doc,
        meta: &meta,
        out: String::new(),
    };
    e.inline_value(id, &[], 0).map_err(|msg| DocError {
        msg: format!("toml: {msg}"),
    })
}

struct Emitter<'a> {
    doc: &'a Doc,
    meta: &'a Meta,
    out: String,
}

impl Emitter<'_> {
    fn table_body(&mut self, id: NodeId, path: &[PathSeg]) -> Result<(), String> {
        let Value::Object(members) = self.doc.value(id) else {
            return Err("table value is not an object".to_string());
        };
        let mut direct: Vec<&Member> = Vec::new();
        let mut subs: Vec<&Member> = Vec::new();
        for m in members {
            match self.doc.value(m.val) {
                Value::Null => {} // TOML has no null; a placeholder adds no key
                Value::Object(_) => subs.push(m),
                Value::Array(items) if is_array_of_tables(items, self.doc) => subs.push(m),
                _ => direct.push(m),
            }
        }
        for m in direct {
            self.keyval(m, path)?;
        }
        for m in subs {
            match self.doc.value(m.val) {
                Value::Object(_) => self.table(m, path)?,
                Value::Array(_) => self.table_array(m, path)?,
                _ => unreachable!("subs holds tables and table arrays"),
            }
        }
        Ok(())
    }

    fn keyval(&mut self, m: &Member, path: &[PathSeg]) -> Result<(), String> {
        let child = child_path(path, &m.key);
        let head = self.head_text(&child);
        comment_lines(&mut self.out, &head, 0);
        self.out.push_str(&key_text(&m.key));
        self.out.push_str(" = ");
        let value = self.value_text(m.val, &child, true, 0)?;
        self.out.push_str(&value);
        let line = self.line_text(&child);
        trailing_comment(&mut self.out, &line);
        self.out.push('\n');
        Ok(())
    }

    fn table(&mut self, m: &Member, path: &[PathSeg]) -> Result<(), String> {
        let child = child_path(path, &m.key);
        let head = self.head_text(&child);
        comment_lines(&mut self.out, &head, 0);
        self.out.push('[');
        self.out.push_str(&header_path(&child));
        self.out.push(']');
        let line = self.line_text(&child);
        trailing_comment(&mut self.out, &line);
        self.out.push('\n');
        self.table_body(m.val, &child)
    }

    fn table_array(&mut self, m: &Member, path: &[PathSeg]) -> Result<(), String> {
        let Value::Array(items) = self.doc.value(m.val) else {
            return Err("table array is not an array".to_string());
        };
        let items = items.clone();
        for (i, item) in items.iter().enumerate() {
            let mut child = child_path(path, &m.key);
            child.push(PathSeg::Idx(i));
            let head = self.head_text(&child);
            comment_lines(&mut self.out, &head, 0);
            self.out.push_str("[[");
            self.out.push_str(&header_path(&child));
            self.out.push_str("]]");
            let line = self.line_text(&child);
            trailing_comment(&mut self.out, &line);
            self.out.push('\n');
            self.table_body(*item, &child)?;
        }
        Ok(())
    }

    fn value_text(
        &mut self,
        id: NodeId,
        path: &[PathSeg],
        table_ctx: bool,
        indent: usize,
    ) -> Result<String, String> {
        match self.doc.value(id) {
            Value::Object(_) => self.inline_table(id, path, indent),
            Value::Array(items) if table_ctx && self.has_item_comments(path, items.len()) => {
                self.multiline_array(items, path, indent)
            }
            Value::Array(_) => self.inline_array(id, path, indent),
            _ => self.scalar_text(id),
        }
    }

    fn inline_value(
        &mut self,
        id: NodeId,
        path: &[PathSeg],
        indent: usize,
    ) -> Result<String, String> {
        match self.doc.value(id) {
            Value::Object(_) => self.inline_table(id, path, indent),
            Value::Array(_) => self.inline_array(id, path, indent),
            _ => self.scalar_text(id),
        }
    }

    fn scalar_text(&self, id: NodeId) -> Result<String, String> {
        let raw = &self.doc.node(id).raw;
        match self.doc.value(id) {
            Value::Str(s) => {
                if !raw.is_empty() && raw == s {
                    Ok(raw.clone())
                } else {
                    Ok(quote(s))
                }
            }
            Value::Num(lit) => {
                if raw.is_empty() {
                    Ok(lit.clone())
                } else {
                    Ok(raw.clone())
                }
            }
            Value::Bool(true) => Ok("true".to_string()),
            Value::Bool(false) => Ok("false".to_string()),
            Value::Null => Err("null is not representable in TOML".to_string()),
            Value::Object(_) | Value::Array(_) => unreachable!("scalar_text on a container"),
        }
    }

    fn inline_table(
        &mut self,
        id: NodeId,
        path: &[PathSeg],
        indent: usize,
    ) -> Result<String, String> {
        let Value::Object(members) = self.doc.value(id) else {
            unreachable!("inline_table on a non-object");
        };
        let members = members.clone();
        let mut out = String::from("{");
        let mut first = true;
        for m in &members {
            if matches!(self.doc.value(m.val), Value::Null) {
                continue;
            }
            if !first {
                out.push(',');
            }
            first = false;
            let child = child_path(path, &m.key);
            out.push(' ');
            out.push_str(&key_text(&m.key));
            out.push_str(" = ");
            out.push_str(&self.inline_value(m.val, &child, indent)?);
        }
        if !first {
            out.push(' ');
        }
        out.push('}');
        Ok(out)
    }

    fn inline_array(
        &mut self,
        id: NodeId,
        path: &[PathSeg],
        indent: usize,
    ) -> Result<String, String> {
        let Value::Array(items) = self.doc.value(id) else {
            unreachable!("inline_array on a non-array");
        };
        let items = items.clone();
        let mut out = String::from("[");
        for (i, &item) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let mut ipath = path.to_vec();
            ipath.push(PathSeg::Idx(i));
            out.push_str(&self.inline_value(item, &ipath, indent)?);
        }
        out.push(']');
        Ok(out)
    }

    fn multiline_array(
        &mut self,
        items: &[NodeId],
        path: &[PathSeg],
        indent: usize,
    ) -> Result<String, String> {
        let mut out = String::from("[\n");
        for (i, &item) in items.iter().enumerate() {
            let mut ipath = path.to_vec();
            ipath.push(PathSeg::Idx(i));
            let head = self.head_text(&ipath);
            comment_lines(&mut out, &head, indent + 1);
            for _ in 0..indent + 1 {
                out.push_str("  ");
            }
            out.push_str(&self.inline_value(item, &ipath, indent + 1)?);
            out.push(',');
            let line = self.line_text(&ipath);
            trailing_comment(&mut out, &line);
            out.push('\n');
        }
        for _ in 0..indent {
            out.push_str("  ");
        }
        out.push(']');
        Ok(out)
    }

    fn has_item_comments(&self, path: &[PathSeg], len: usize) -> bool {
        (0..len).any(|i| {
            let mut ipath = path.to_vec();
            ipath.push(PathSeg::Idx(i));
            self.meta
                .comments
                .get(&ipath)
                .is_some_and(|cs| !item_comments_empty(cs))
        })
    }

    fn head_text(&self, path: &[PathSeg]) -> String {
        match self.meta.comments.get(path) {
            Some(cs) => combine(&cs.key_head, &cs.head),
            None => String::new(),
        }
    }

    fn line_text(&self, path: &[PathSeg]) -> String {
        match self.meta.comments.get(path) {
            Some(cs) => combine(&cs.key_line, &cs.line),
            None => String::new(),
        }
    }
}

fn item_comments_empty(cs: &CommentSet) -> bool {
    cs.head.is_empty() && cs.key_head.is_empty() && cs.line.is_empty() && cs.key_line.is_empty()
}

fn is_array_of_tables(items: &[NodeId], doc: &Doc) -> bool {
    !items.is_empty()
        && items
            .iter()
            .all(|&id| matches!(doc.value(id), Value::Object(_)))
}

fn child_path(path: &[PathSeg], key: &str) -> Vec<PathSeg> {
    let mut p = path.to_vec();
    p.push(PathSeg::Key(key.to_string()));
    p
}

fn combine(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else if b.is_empty() {
        a.to_string()
    } else {
        format!("{a}\n{b}")
    }
}

fn comment_lines(out: &mut String, text: &str, indent: usize) {
    if text.is_empty() {
        return;
    }
    for line in text.split('\n') {
        if line.is_empty() {
            out.push('\n');
            continue;
        }
        for _ in 0..indent {
            out.push_str("  ");
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
}

fn trailing_comment(out: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    let mut lines = text.split('\n').filter(|l| !l.trim().is_empty());
    if let Some(first) = lines.next() {
        out.push(' ');
        out.push_str(first.trim());
    }
    for line in lines {
        out.push('\n');
        out.push_str(line.trim_end());
    }
}

fn key_text(key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        key.to_string()
    } else {
        quote(key)
    }
}

fn header_path(path: &[PathSeg]) -> String {
    let mut out = String::new();
    for seg in path {
        if let PathSeg::Key(k) = seg {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(&key_text(k));
        }
    }
    out
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c <= '\u{1f}' || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(src: &str) -> String {
        let (doc, meta) = crate::doc::tomldoc::parse(src).unwrap();
        write(&doc, &meta).unwrap()
    }

    #[test]
    fn writes_tables_arrays_and_inline_tables() {
        let out = round(
            "title = \"x\"\n\
             [server]\n\
             host = \"h\"\n\
             tls.enabled = true\n\
             [[mounts]]\n\
             src = \"/a\"\n\
             [[mounts]]\n\
             src = \"/b\"\n",
        );
        assert!(out.contains("title = \"x\"\n"), "{out}");
        assert!(out.contains("[server]\nhost = \"h\"\n"), "{out}");
        assert!(out.contains("[server.tls]\nenabled = true\n"), "{out}");
        assert!(out.contains("[[mounts]]\nsrc = \"/a\"\n"), "{out}");
        assert!(out.contains("[[mounts]]\nsrc = \"/b\"\n"), "{out}");
    }

    #[test]
    fn keeps_exact_number_and_datetime_literals() {
        let out = round("hex = 0x1F\nunder = 1_000\nwhen = 1979-05-27T07:32:00Z\n");
        assert!(out.contains("hex = 0x1F\n"), "{out}");
        assert!(out.contains("under = 1_000\n"), "{out}");
        assert!(out.contains("when = 1979-05-27T07:32:00Z\n"), "{out}");
    }

    #[test]
    fn emits_comments_and_quotes_awkward_keys() {
        let out =
            round("# top\na = 1 # inline\n# before\n\"a.b\" = 2\n[t] # note\nx = 1\n# tail\n");
        assert!(
            out.starts_with("# top\na = 1 # inline\n# before\n\"a.b\" = 2\n"),
            "{out}"
        );
        assert!(out.contains("[t] # note\nx = 1\n# tail\n"), "{out}");
    }

    #[test]
    fn omits_null_placeholders_and_rejects_null_items() {
        let mut doc = Doc::new();
        let root = doc.new_object();
        doc.set(&[], root).unwrap();
        let null = doc.new_null();
        doc.set(&[PathSeg::Key("skip".into())], null).unwrap();
        assert_eq!(write(&doc, &Meta::default()).unwrap(), "");

        let arr = doc.new_array();
        let n = doc.new_null();
        if let Value::Array(items) = &mut doc.node_mut(arr).value {
            items.push(n);
        }
        doc.set(&[PathSeg::Key("a".into())], arr).unwrap();
        let err = write(&doc, &Meta::default()).unwrap_err().msg;
        assert!(err.contains("null is not representable"), "{err}");
    }

    #[test]
    fn keeps_array_item_comments() {
        let src = "a = [\n  # first\n  1, # one\n  2,\n]\n";
        let out = round(src);
        assert!(out.contains("# first\n  1, # one\n  2,\n"), "{out}");
        // The emitted document parses back to the same value and comments.
        let (doc2, meta2) = crate::doc::tomldoc::parse(&out).unwrap();
        assert!(matches!(doc2.value(doc2.root), Value::Object(m) if m.len() == 1));
        let mut p = vec![PathSeg::Key("a".into()), PathSeg::Idx(0)];
        assert_eq!(meta2.comments[&p].head, "# first");
        p.pop();
        p.push(PathSeg::Idx(0));
        assert_eq!(meta2.comments[&p].line, "# one");
    }

    #[test]
    fn round_trips_nested_tables_and_table_arrays() {
        let src = "\
[[a]]
x = 1
[a.b]
y = 2
[[a]]
x = 3
[[a.c]]
z = 4
[c.d]
w = 5
[c]
v = 6
inline = { x = 1, y.z = \"q\" }
";
        let out = round(src);
        let (before, _) = crate::doc::tomldoc::parse(src).unwrap();
        let (after, _) = crate::doc::tomldoc::parse(&out).unwrap();
        assert_eq!(
            before.to_value(before.root),
            after.to_value(after.root),
            "{out}"
        );
        assert!(out.contains("[[a.c]]"), "{out}");
        assert!(out.contains("[c]\nv = 6\n[c.d]\nw = 5\n"), "{out}");
    }

    #[test]
    fn value_neutral_writes_keep_the_source_spelling() {
        let (mut doc, meta) = crate::doc::tomldoc::parse("hex = 0x1F\n").unwrap();
        let path = [PathSeg::Key("hex".into())];
        let same = doc.new_num("31");
        doc.set(&path, same).unwrap();
        assert!(write(&doc, &meta).unwrap().contains("hex = 0x1F\n"));

        let changed = doc.new_num("32");
        doc.set(&path, changed).unwrap();
        assert!(write(&doc, &meta).unwrap().contains("hex = 32\n"));
    }

    #[test]
    fn writes_table_array_comments() {
        let out = round(
            "# header\n[[mounts]] # first\nsrc = \"/a\" # inline\n[[mounts]]\nsrc = \"/b\"\n",
        );
        assert!(
            out.contains("# header\n[[mounts]] # first\nsrc = \"/a\" # inline\n"),
            "{out}"
        );
        assert!(out.contains("[[mounts]]\nsrc = \"/b\"\n"), "{out}");
    }

    #[test]
    fn write_value_renders_inline_forms() {
        let mut doc = Doc::new();
        let obj = doc.new_object();
        let one = doc.new_num("1");
        if let Value::Object(m) = &mut doc.node_mut(obj).value {
            m.push(Member {
                key: "x".into(),
                val: one,
            });
        }
        doc.root = obj;
        let out = write_value(&doc, obj).unwrap();
        assert_eq!(out, "{ x = 1 }");
    }
}
