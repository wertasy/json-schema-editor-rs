//! Document layer: format-agnostic tree model (jsondoc), JSONC comments
//! (jsoncdoc), YAML (yamldoc), and the codec registry (docfmt).
//!
//! The codec layer is the trait-ified version of Go's docfmt.Register /
//! docfmt.ForFormat: adding a format is one `impl Codec` plus one entry in
//! [`FORMATS`].

use std::collections::HashMap;

pub mod jsoncdoc;
pub mod jsondoc;
pub mod tomldoc;
pub mod yamldoc;

pub use jsondoc::{Doc, DocError, Member, NodeData, NodeId, PathSeg, Value};

/// commentSet holds the comments attached to one key/value pair. Key* belong
/// to the object key, Head/Line/Foot to the value (for an array item or the
/// root value there is no key, so Key* stay empty). Foot of a container
/// value holds the comments just inside its closing brace/bracket.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommentSet {
    pub key_head: String,
    pub key_line: String,
    pub key_foot: String,
    pub head: String,
    pub line: String,
    pub foot: String,
}

/// Meta carries the format state a session needs to write the document back
/// faithfully: the comment table (keyed by document path), comments after
/// the root JSONC value or at the end of a TOML document, and the YAML alias
/// count.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub comments: HashMap<Vec<PathSeg>, CommentSet>,
    /// JSONC: comments after the root value, outside a root container.
    /// TOML: comments after the last member.
    pub tail: String,
    /// YAML: number of alias references found while parsing.
    pub aliases: usize,
}

impl Meta {
    /// Whether any comment state is recorded (drives the JSONC plain-JSON
    /// delegation).
    pub fn has_comments(&self) -> bool {
        !self.comments.is_empty() || !self.tail.is_empty()
    }

    /// comment_text returns the document comment attached to `path` as plain
    /// text (markers stripped, schema directives dropped), or None when the
    /// member has no comment. Only the member's own slots are read: the
    /// comments inside a container value (`foot`) belong to its children.
    pub fn comment_text(&self, path: &[PathSeg]) -> Option<String> {
        let cs = self.comments.get(path)?;
        let mut parts: Vec<String> = Vec::new();
        for raw in [&cs.key_head, &cs.head, &cs.key_line, &cs.line, &cs.key_foot] {
            let text = clean_comment(raw);
            if !text.is_empty() && !parts.contains(&text) {
                parts.push(text);
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }

    /// set_comment_text replaces the member comment at `path` with `text`
    /// (plain text; `prefix` is the format's line-comment marker). An empty
    /// text removes the comment. Existing schema-directive lines are kept,
    /// and the comments inside a container value are untouched.
    pub fn set_comment_text(&mut self, path: &[PathSeg], text: &str, prefix: &str) {
        let mut cs = self.comments.get(path).cloned().unwrap_or_default();
        let slots = [&cs.key_head, &cs.head, &cs.key_line, &cs.line, &cs.key_foot];
        let target = slots.iter().position(|s| !s.is_empty()).unwrap_or(
            if matches!(path.last(), Some(PathSeg::Key(_))) {
                0
            } else {
                1
            },
        );
        let mut dirs: [String; 5] = std::array::from_fn(|_| String::new());
        for (i, slot) in [
            &mut cs.key_head,
            &mut cs.head,
            &mut cs.key_line,
            &mut cs.line,
            &mut cs.key_foot,
        ]
        .into_iter()
        .enumerate()
        {
            dirs[i] = take_directives(slot);
        }
        let body = format_comment(text, prefix);
        let mut merged: [String; 5] = std::array::from_fn(|i| {
            let dir = std::mem::take(&mut dirs[i]);
            if i != target {
                return dir;
            }
            match (dir.is_empty(), body.is_empty()) {
                (true, true) => String::new(),
                (false, true) => dir,
                (true, false) => body.clone(),
                (false, false) => format!("{dir}\n{body}"),
            }
        });
        cs.key_head = std::mem::take(&mut merged[0]);
        cs.head = std::mem::take(&mut merged[1]);
        cs.key_line = std::mem::take(&mut merged[2]);
        cs.line = std::mem::take(&mut merged[3]);
        cs.key_foot = std::mem::take(&mut merged[4]);
        if cs == CommentSet::default() {
            self.comments.remove(path);
        } else {
            self.comments.insert(path.to_vec(), cs);
        }
    }
}

/// is_directive_line reports the schema-location comments the loader writes
/// (`# yaml-language-server: $schema=…`, `#:schema …`); they are metadata,
/// not user comments, so the description panel hides them.
fn is_directive_line(line: &str) -> bool {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("//") {
        // JSONC has no directive form; only the YAML header can appear here
        // when a comment was merged into a JSONC slot.
        return is_yaml_directive(rest.trim_start());
    }
    if let Some(rest) = t.strip_prefix("#:schema") {
        // TOML's Taplo convention: exactly `#:schema` (no space after #).
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('=').map(str::trim_start).unwrap_or(rest);
        return !rest.is_empty();
    }
    if let Some(rest) = t.strip_prefix('#') {
        return is_yaml_directive(rest.trim_start());
    }
    false
}

/// is_yaml_directive recognizes `yaml-language-server: $schema=<loc>`.
fn is_yaml_directive(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("yaml-language-server:") else {
        return false;
    };
    let Some(loc) = rest.trim_start().strip_prefix("$schema=") else {
        return false;
    };
    !loc.trim().is_empty()
}

/// take_directives removes schema-directive lines from a slot, returning
/// them joined for re-insertion at the same place.
fn take_directives(slot: &mut String) -> String {
    if slot.is_empty() {
        return String::new();
    }
    let kept: Vec<String> = slot
        .split('\n')
        .filter(|l| is_directive_line(l))
        .map(str::to_string)
        .collect();
    if kept.is_empty() {
        return String::new();
    }
    slot.clear();
    kept.join("\n")
}

/// clean_comment strips comment markers and surrounding whitespace from a
/// recorded comment slot. Blank lines inside the comment are kept.
fn clean_comment(raw: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in raw.split('\n') {
        let mut t = line.trim();
        if is_directive_line(t) {
            continue;
        }
        if let Some(rest) = t.strip_prefix("//") {
            t = rest.trim_start();
        } else if let Some(rest) = t.strip_prefix("/**") {
            t = rest.trim_start();
            if let Some(r) = t.strip_suffix("*/") {
                t = r.trim_end();
            }
        } else if let Some(rest) = t.strip_prefix("/*") {
            t = rest.trim_start();
            if let Some(r) = t.strip_suffix("*/") {
                t = r.trim_end();
            }
        } else if let Some(rest) = t.strip_prefix('*') {
            let rest = rest.trim_start();
            if rest == "/" {
                t = "";
            } else {
                t = rest;
                if let Some(r) = t.strip_suffix("*/") {
                    t = r.trim_end();
                }
            }
        } else if let Some(rest) = t.strip_prefix('#') {
            t = rest.trim_start();
        }
        lines.push(t.to_string());
    }
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// format_comment renders plain text as a comment block using `prefix`.
fn format_comment(text: &str, prefix: &str) -> String {
    let text = text.trim_matches(['\n', '\r']);
    if text.trim().is_empty() {
        return String::new();
    }
    text.split('\n')
        .map(|line| {
            let line = line.trim_end();
            if line.trim().is_empty() {
                prefix.to_string()
            } else {
                let line = line.trim_start();
                format!("{prefix} {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// pathKey encodes a document path as a unique map key, mirroring Go's
/// json.Marshal of the []any path: nil → "null", ["a"] → `["a"]`, [0] →
/// `[0]`. Keys are JSON-escaped, so keys with dots/quotes stay apart.
#[allow(dead_code)] // the Go-equivalent path-key helper; exercised by tests
pub fn path_key(path: &[PathSeg]) -> String {
    if path.is_empty() {
        // Go marshals a nil []any as "null"
        return "null".to_string();
    }
    let mut out = String::from("[");
    for (i, seg) in path.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        match seg {
            PathSeg::Key(k) => jsondoc::write_json_string(&mut out, k),
            PathSeg::Idx(i) => out.push_str(&i.to_string()),
        }
    }
    out.push(']');
    out
}

// --- codec registry ---------------------------------------------------------

/// Parsed is one loaded document: the tree plus its format state.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub doc: Doc,
    pub meta: Meta,
}

/// Codec edits one configuration file format.
pub trait Codec: Sync {
    /// Extensions lists the lower-case, dot-prefixed file extensions this
    /// format handles (".yaml", ".yml", …).
    fn extensions(&self) -> &'static [&'static str];
    /// Parse one document.
    fn parse(&self, text: &str) -> Result<Parsed, DocError>;
    /// Write the document back.
    fn write(&self, doc: &Doc, meta: &Meta) -> Result<String, DocError>;
    /// The session for a file that does not exist yet: an empty object
    /// document.
    fn empty(&self) -> Parsed;
    /// The schema location recorded in the file's own convention.
    fn schema_location(&self, doc: &Doc, meta: &Meta) -> Option<String>;
    /// Records loc when no non-empty location exists yet; an existing value
    /// is kept.
    fn set_schema_location(&self, doc: &mut Doc, meta: &mut Meta, loc: &str);
    /// set_schema_location_with records loc for a schema that declares a
    /// top-level "$schema" property (`schema_member`). Formats whose idiomatic
    /// location is a comment (YAML) write the member instead, so the field
    /// the schema defines is the single source of truth.
    fn set_schema_location_with(
        &self,
        doc: &mut Doc,
        meta: &mut Meta,
        loc: &str,
        schema_member: bool,
    ) {
        let _ = schema_member;
        self.set_schema_location(doc, meta, loc);
    }
    /// Removes any recorded location; a fresh --pick must overwrite the old
    /// value.
    fn clear_schema_location(&self, doc: &mut Doc, meta: &mut Meta);
    /// Session status lines; empty when there is nothing to report.
    fn notices(&self, doc: &Doc, meta: &Meta) -> Vec<String>;
}

/// JSONC codec: .json/.jsonc. JSONC is a superset of JSON, so it is also the
/// default format for unknown or missing extensions (the historical
/// "everything is JSON" behavior).
pub struct JsoncCodec;

impl Codec for JsoncCodec {
    fn extensions(&self) -> &'static [&'static str] {
        &[".json", ".jsonc"]
    }

    fn parse(&self, text: &str) -> Result<Parsed, DocError> {
        let (doc, meta) = jsoncdoc::parse(text)?;
        Ok(Parsed { doc, meta })
    }

    fn write(&self, doc: &Doc, meta: &Meta) -> Result<String, DocError> {
        if !meta.has_comments() {
            // No comments: byte-for-byte the historical JSON output.
            return Ok(doc.write());
        }
        Ok(jsoncdoc::write(doc, meta))
    }

    fn empty(&self) -> Parsed {
        let mut doc = Doc::new();
        let obj = doc.new_object();
        doc.set(&[], obj).expect("root set");
        Parsed {
            doc,
            meta: Meta::default(),
        }
    }

    fn schema_location(&self, doc: &Doc, _meta: &Meta) -> Option<String> {
        schema_from_doc(doc)
    }

    fn set_schema_location(&self, doc: &mut Doc, _meta: &mut Meta, loc: &str) {
        set_schema_member(doc, loc);
    }

    fn clear_schema_location(&self, doc: &mut Doc, _meta: &mut Meta) {
        clear_schema_member(doc);
    }

    fn notices(&self, _doc: &Doc, _meta: &Meta) -> Vec<String> {
        Vec::new()
    }
}

/// YAML codec: .yaml/.yml (behavior lands with the YAML phase; the registry
/// entry and its extension dispatch exist from the start).
pub struct YamlCodec;

impl Codec for YamlCodec {
    fn extensions(&self) -> &'static [&'static str] {
        &[".yaml", ".yml"]
    }

    fn parse(&self, text: &str) -> Result<Parsed, DocError> {
        yamldoc::parse(text).map(|(doc, meta)| Parsed { doc, meta })
    }

    fn write(&self, doc: &Doc, meta: &Meta) -> Result<String, DocError> {
        yamldoc::write(doc, Some(meta))
    }

    fn empty(&self) -> Parsed {
        let mut doc = Doc::new();
        let obj = doc.new_object();
        doc.set(&[], obj).expect("root set");
        Parsed {
            doc,
            meta: Meta::default(),
        }
    }

    fn schema_location(&self, doc: &Doc, meta: &Meta) -> Option<String> {
        // prefers an explicit "$schema" member over the directive comment
        schema_from_doc(doc).or_else(|| yamldoc::schema_directive(doc, meta))
    }

    fn set_schema_location(&self, doc: &mut Doc, meta: &mut Meta, loc: &str) {
        // keeps YAML idiomatic: writes a yaml-language-server comment and
        // never creates or fills a "$schema" member
        if schema_from_doc(doc).is_some() {
            return;
        }
        yamldoc::set_schema_directive(doc, meta, loc);
    }

    fn set_schema_location_with(
        &self,
        doc: &mut Doc,
        meta: &mut Meta,
        loc: &str,
        schema_member: bool,
    ) {
        if !schema_member {
            return self.set_schema_location(doc, meta, loc);
        }
        // The schema declares "$schema" as a real property: record the
        // location there instead of adding the directive comment. An
        // existing location (member or directive) keeps precedence, matching
        // set_schema_location's keep-existing rule; a directive is migrated
        // into the member and then removed so only one source remains.
        let existing = self.schema_location(doc, meta);
        match existing {
            Some(existing) => crate::doc::set_schema_member(doc, &existing),
            None => crate::doc::set_schema_member(doc, loc),
        }
        yamldoc::clear_schema_directive(doc, meta);
    }

    fn clear_schema_location(&self, doc: &mut Doc, meta: &mut Meta) {
        clear_schema_member(doc);
        yamldoc::clear_schema_directive(doc, meta);
    }

    fn notices(&self, _doc: &Doc, meta: &Meta) -> Vec<String> {
        if meta.aliases == 0 {
            return Vec::new();
        }
        let noun = if meta.aliases > 1 { "aliases" } else { "alias" };
        vec![format!(
            "yaml: {n} {noun} kept: edits write through to the anchored object (< Expand > detaches a copy)",
            n = meta.aliases
        )]
    }
}

/// TOML codec: .toml. Scalars JSON cannot spell exactly (non-decimal
/// integers, date/time values) keep their source literal on the node for
/// faithful saves; edits write canonical TOML.
pub struct TomlCodec;

impl Codec for TomlCodec {
    fn extensions(&self) -> &'static [&'static str] {
        &[".toml"]
    }

    fn parse(&self, text: &str) -> Result<Parsed, DocError> {
        tomldoc::parse(text).map(|(doc, meta)| Parsed { doc, meta })
    }

    fn write(&self, doc: &Doc, meta: &Meta) -> Result<String, DocError> {
        tomldoc::write(doc, meta)
    }

    fn empty(&self) -> Parsed {
        let mut doc = Doc::new();
        let obj = doc.new_object();
        doc.set(&[], obj).expect("root set");
        Parsed {
            doc,
            meta: Meta::default(),
        }
    }

    fn schema_location(&self, doc: &Doc, meta: &Meta) -> Option<String> {
        // an explicit "$schema" member wins over the header directive
        schema_from_doc(doc).or_else(|| tomldoc::schema_directive(doc, meta))
    }

    fn set_schema_location(&self, doc: &mut Doc, meta: &mut Meta, loc: &str) {
        // keeps TOML idiomatic: `#:schema <loc>` in the header, never a
        // "$schema" member unless the schema itself declares one
        if schema_from_doc(doc).is_some() {
            return;
        }
        tomldoc::set_schema_directive(doc, meta, loc);
    }

    fn set_schema_location_with(
        &self,
        doc: &mut Doc,
        meta: &mut Meta,
        loc: &str,
        schema_member: bool,
    ) {
        if !schema_member {
            return self.set_schema_location(doc, meta, loc);
        }
        // The schema declares "$schema" as a property: record the location
        // there and drop any header directive so only one source remains.
        let existing = self.schema_location(doc, meta);
        match existing {
            Some(existing) => crate::doc::set_schema_member(doc, &existing),
            None => crate::doc::set_schema_member(doc, loc),
        }
        tomldoc::clear_schema_directive(doc, meta);
    }

    fn clear_schema_location(&self, doc: &mut Doc, meta: &mut Meta) {
        clear_schema_member(doc);
        tomldoc::clear_schema_directive(doc, meta);
    }

    fn notices(&self, _doc: &Doc, _meta: &Meta) -> Vec<String> {
        Vec::new()
    }
}

/// The registered formats, in dispatch order.
static FORMATS: [&dyn Codec; 3] = [&JsoncCodec, &YamlCodec, &TomlCodec];

/// go_ext mirrors filepath.Ext: the suffix beginning at the final dot of the
/// final element (".cshrc" → ".cshrc", "a.b.json" → ".json", "a" → "").
fn go_ext(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) => &name[i..],
        None => "",
    }
}

/// ForFormat returns the first registered format whose Extensions contains
/// the lower-cased extension of path. No match falls back to the JSONC
/// default; it never returns none.
pub fn codec_for(path: &str) -> &'static dyn Codec {
    let ext = go_ext(path).to_ascii_lowercase();
    for f in FORMATS {
        if f.extensions().contains(&ext.as_str()) {
            return f;
        }
    }
    &JsoncCodec
}

// --- schema location helpers -----------------------------------------------

/// schemaFromDoc returns the first non-empty top-level "$schema" string.
pub(crate) fn schema_from_doc(doc: &Doc) -> Option<String> {
    let Value::Object(members) = doc.value(doc.root) else {
        return None;
    };
    for m in members {
        if m.key != "$schema" {
            continue;
        }
        if let Value::Str(s) = doc.value(m.val) {
            if !s.is_empty() {
                return Some(s.clone());
            }
        }
    }
    None
}

/// setSchemaMember sets the top-level "$schema" member to loc when the
/// document has none (or an empty/null one), keeping the file's own value
/// otherwise. A new member goes first, the conventional position.
pub(crate) fn set_schema_member(doc: &mut Doc, loc: &str) {
    if !matches!(doc.value(doc.root), Value::Object(_)) {
        return;
    }
    let mut slot: Option<usize> = None;
    {
        let Value::Object(members) = doc.value(doc.root) else {
            return;
        };
        for (i, m) in members.iter().enumerate() {
            if m.key != "$schema" {
                continue;
            }
            if let Value::Str(s) = doc.value(m.val) {
                if !s.is_empty() {
                    return;
                }
            }
            slot = Some(i);
            break;
        }
    }
    let id = doc.new_str(loc);
    if let Value::Object(members) = &mut doc.nodes[doc.root as usize].value {
        match slot {
            Some(i) => members[i].val = id,
            None => members.insert(
                0,
                Member {
                    key: "$schema".to_string(),
                    val: id,
                },
            ),
        }
    }
}

/// clearSchemaMember removes every top-level "$schema" member.
pub(crate) fn clear_schema_member(doc: &mut Doc) {
    if let Value::Object(members) = &mut doc.nodes[doc.root as usize].value {
        members.retain(|m| m.key != "$schema");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PathSeg::{Idx, Key};

    fn write_session(c: &dyn Codec, p: &Parsed) -> String {
        c.write(&p.doc, &p.meta).expect("write")
    }

    #[test]
    fn for_format() {
        let cases = [
            ("a.yaml", ".yaml"),
            ("a.YML", ".yaml"),
            ("x/a.yml", ".yaml"),
            ("a.toml", ".toml"),
            ("x/A.TOML", ".toml"),
            ("a.json", ".json"),
            ("a.jsonc", ".json"),
            ("a.conf", ".json"),
            ("a", ".json"),
            ("a.JSONC", ".json"),
        ];
        for (path, want_ext) in cases {
            let f = codec_for(path);
            assert_eq!(f.extensions()[0], want_ext, "{path}");
        }
        assert_eq!(codec_for("a.yaml").extensions(), &[".yaml", ".yml"]);
        assert_eq!(codec_for("a.toml").extensions(), &[".toml"]);
        assert_eq!(codec_for("a.unknown").extensions(), &[".json", ".jsonc"]);
    }

    #[test]
    fn jsonc_no_comment_delegates() {
        let c = codec_for("a.json");
        let p = c.parse(r#"{"a":1,"b":[2,3]}"#).unwrap();
        let want = p.doc.write();
        assert_eq!(write_session(c, &p), want);
    }

    #[test]
    fn jsonc_comment_smoke() {
        let src = "// hdr\n{\n  \"a\": 1 // note\n}\n";
        let c = codec_for("a.json");
        let p = c.parse(src).unwrap();
        assert_eq!(write_session(c, &p), src);
    }

    #[test]
    fn json_session_schema_location() {
        let mut doc = Doc::new();
        let name = doc.new_str("x");
        let obj = doc.new_object();
        if let Value::Object(m) = &mut doc.nodes[obj as usize].value {
            m.push(jsondoc::Member {
                key: "name".into(),
                val: name,
            });
        }
        doc.set(&[], obj).unwrap();
        let c = codec_for("a.json");

        assert!(c.schema_location(&doc, &Meta::default()).is_none());
        c.set_schema_location(&mut doc, &mut Meta::default(), "sch.json");
        assert_eq!(
            c.schema_location(&doc, &Meta::default()).as_deref(),
            Some("sch.json")
        );
        {
            let Value::Object(m) = doc.value(doc.root) else {
                panic!()
            };
            assert_eq!(m[0].key, "$schema", "new member not first");
        }
        c.set_schema_location(&mut doc, &mut Meta::default(), "other.json");
        assert_eq!(
            c.schema_location(&doc, &Meta::default()).as_deref(),
            Some("sch.json")
        );

        // Empty and null members are filled in place.
        let mut empty = Doc::new();
        let a = empty.new_str("1");
        let sch = empty.new_str("");
        let b = empty.new_str("2");
        let obj = empty.new_object();
        if let Value::Object(m) = &mut empty.nodes[obj as usize].value {
            m.push(jsondoc::Member {
                key: "a".into(),
                val: a,
            });
            m.push(jsondoc::Member {
                key: "$schema".into(),
                val: sch,
            });
            m.push(jsondoc::Member {
                key: "b".into(),
                val: b,
            });
        }
        empty.set(&[], obj).unwrap();
        c.set_schema_location(&mut empty, &mut Meta::default(), "fill.json");
        {
            let Value::Object(m) = empty.value(empty.root) else {
                panic!()
            };
            assert_eq!(m[1].key, "$schema");
            assert_eq!(empty.value(m[1].val), &Value::Str("fill.json".into()));
        }
        let mut nullv = Doc::new();
        let sch = nullv.new_null();
        let obj = nullv.new_object();
        if let Value::Object(m) = &mut nullv.nodes[obj as usize].value {
            m.push(jsondoc::Member {
                key: "$schema".into(),
                val: sch,
            });
        }
        nullv.set(&[], obj).unwrap();
        c.set_schema_location(&mut nullv, &mut Meta::default(), "n.json");
        {
            let Value::Object(m) = nullv.value(nullv.root) else {
                panic!()
            };
            assert_eq!(nullv.value(m[0].val), &Value::Str("n.json".into()));
        }

        c.clear_schema_location(&mut doc, &mut Meta::default());
        assert!(c.schema_location(&doc, &Meta::default()).is_none());
        let Value::Object(m) = doc.value(doc.root) else {
            panic!()
        };
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].key, "name");
    }

    #[test]
    fn new_empty_writes() {
        // The YAML codec's empty-session write joins when the YAML phase
        // lands (TestYAMLNewEmptySchemaDirective covers the directive form).
        let jsonc: &dyn Codec = codec_for("a.json");
        let p = jsonc.empty();
        assert_eq!(write_session(jsonc, &p), "{}\n");
    }

    #[test]
    fn path_key_shapes() {
        assert_eq!(path_key(&[]), "null");
        assert_eq!(path_key(&[Key("a".into())]), r#"["a"]"#);
        assert_eq!(path_key(&[Idx(0)]), "[0]");
        assert_eq!(
            path_key(&[Key("a".into()), Key("b".into()), Idx(3)]),
            r#"["a","b",3]"#
        );
        assert_eq!(path_key(&[Key("a\"b".into())]), r#"["a\"b"]"#);
    }

    #[test]
    fn comment_text_strips_markers_and_joins_slots() {
        let src =
            "// header\n{\n  // port note\n  \"port\": 8080, // inline\n  \"name\": \"x\"\n}\n";
        let p = codec_for("a.json").parse(src).unwrap();
        assert_eq!(
            p.meta.comment_text(&[Key("port".into())]).as_deref(),
            Some("port note\ninline")
        );
        assert!(p.meta.comment_text(&[Key("name".into())]).is_none());
        // block comments and array item comments
        let p = codec_for("a.json")
            .parse("[\n  /* item\n   * note */\n  1\n]\n")
            .unwrap();
        assert_eq!(
            p.meta.comment_text(&[Idx(0)]).as_deref(),
            Some("item\nnote")
        );
    }

    #[test]
    fn set_comment_text_rewrites_the_member_comment() {
        let src = "{\n  // old note\n  \"port\": 8080 // inline\n}\n";
        let (doc, mut meta) = {
            let p = codec_for("a.json").parse(src).unwrap();
            (p.doc, p.meta)
        };
        meta.set_comment_text(&[Key("port".into())], "new note\nline 2", "//");
        assert_eq!(
            meta.comment_text(&[Key("port".into())]).as_deref(),
            Some("new note\nline 2")
        );
        let out = codec_for("a.json").write(&doc, &meta).unwrap();
        assert!(
            out.contains("// new note") && out.contains("// line 2") && !out.contains("old note"),
            "{out}"
        );
        meta.set_comment_text(&[Key("port".into())], "", "//");
        assert!(meta.comment_text(&[Key("port".into())]).is_none());
    }

    #[test]
    fn comment_edit_keeps_schema_directives() {
        let src = "# yaml-language-server: $schema=s.json\n# header\nname: demo\n";
        let (doc, mut meta) = {
            let p = codec_for("a.yaml").parse(src).unwrap();
            (p.doc, p.meta)
        };
        assert_eq!(meta.comment_text(&[]).as_deref(), Some("header"));
        meta.set_comment_text(&[], "edited", "#");
        let head = &meta.comments[&Vec::new()].head;
        assert!(
            head.contains("# yaml-language-server: $schema=s.json") && head.contains("# edited"),
            "{head}"
        );
        assert_eq!(meta.comment_text(&[]).as_deref(), Some("edited"));
        let out = codec_for("a.yaml").write(&doc, &meta).unwrap();
        assert!(
            out.contains("# yaml-language-server: $schema=s.json") && out.contains("# edited"),
            "{out}"
        );
    }

    #[test]
    fn comment_text_round_trip_keeps_leading_markers() {
        let mut meta = Meta::default();
        let path = [Key("port".into())];
        meta.set_comment_text(&path, "//TODO", "//");
        let text = meta.comment_text(&path).unwrap();
        assert_eq!(text, "//TODO");
        meta.set_comment_text(&path, &text, "//");
        assert_eq!(meta.comment_text(&path).as_deref(), Some("//TODO"));
        assert_eq!(meta.comments[&path[..]].key_head, "// //TODO");
    }

    #[test]
    fn directive_detection_is_precise() {
        assert!(is_directive_line("#:schema ./s.json"));
        assert!(is_directive_line(
            "# yaml-language-server: $schema=./s.json"
        ));
        assert!(!is_directive_line("# :schema field is optional"));
        assert!(!is_directive_line("# yaml-language-server: note"));
        assert!(!is_directive_line("#:schema"));
        assert!(!is_directive_line("// #:schema docs"));
    }

    #[test]
    fn doc_block_comments_lose_the_decorative_star() {
        let p = codec_for("a.json")
            .parse("[\n  /** doc note */\n  1\n]\n")
            .unwrap();
        assert_eq!(p.meta.comment_text(&[Idx(0)]).as_deref(), Some("doc note"));
    }

    #[test]
    fn toml_comment_text() {
        let meta = codec_for("a.toml")
            .parse("port = 8080 # inline\n")
            .unwrap()
            .meta;
        assert_eq!(
            meta.comment_text(&[Key("port".into())]).as_deref(),
            Some("inline")
        );
    }
}
