//! Document layer: format-agnostic tree model (jsondoc), JSONC comments
//! (jsoncdoc), YAML (yamldoc), and the codec registry (docfmt).
//!
//! The codec layer is the trait-ified version of Go's docfmt.Register /
//! docfmt.ForFormat: adding a format is one `impl Codec` plus one entry in
//! [`FORMATS`].

use std::collections::HashMap;

pub mod jsoncdoc;
pub mod jsondoc;
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
/// the root JSONC value, and the YAML alias count.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub comments: HashMap<Vec<PathSeg>, CommentSet>,
    /// JSONC: comments after the root value, outside a root container.
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

/// The registered formats, in dispatch order.
static FORMATS: [&dyn Codec; 2] = [&JsoncCodec, &YamlCodec];

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
}
