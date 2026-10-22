//! Ports of internal/yamldoc/yamldoc_test.go and the YAML half of
//! internal/docfmt/docfmt_test.go.

use super::*;
use crate::doc::codec_for;
use crate::doc::jsondoc::{self, PathSeg};

fn must_json(s: &str) -> Doc {
    jsondoc::parse(s).expect("reference JSON must parse")
}

fn write_string(doc: &Doc, meta: Option<&Meta>) -> String {
    write(doc, meta).expect("write")
}

#[test]
fn parse_basic() {
    let src = "name: demo\nport: 8080\nratio: 1.0\nitems:\n  - a\n  - b\nnested:\n  k: v\n";
    let (doc, meta) = parse(src).unwrap();
    let want = must_json(
        r#"{"name":"demo","port":8080,"ratio":1.0,"items":["a","b"],"nested":{"k":"v"}}"#,
    );
    assert_eq!(doc.write(), want.write());
    assert_eq!(meta.aliases, 0);
}

#[test]
fn parse_number_normalization() {
    let src = "hex: 0x1F\noct: 0o17\nunder: 1_000\nf: 1.0\nexp: 1e3\n";
    let (doc, _) = parse(src).unwrap();
    let want = must_json(r#"{"hex":31,"oct":15,"under":1000,"f":1.0,"exp":1e3}"#);
    assert_eq!(doc.write(), want.write());
}

#[test]
fn parse_errors() {
    let cases = [
        (
            "multiple documents",
            "a: 1\n---\nb: 2\n",
            "multiple YAML documents",
        ),
        ("non-string key", "1: x\n", "only string keys"),
        (
            "merge into scalar anchor",
            "s: &b 1\nc:\n  <<: *b\n",
            "requires a mapping anchor",
        ),
        (
            "merge plain value",
            "c:\n  <<: 1\n",
            "alias or a sequence of aliases",
        ),
        ("merge self", "c: &x\n  <<: *x\n", "itself"),
        ("infinity", "v: .inf\n", "unsupported YAML float"),
        ("nan", "v: .nan\n", "unsupported YAML float"),
        ("empty", "", "empty YAML document"),
    ];
    for (name, src, want) in cases {
        match parse(src) {
            Ok(_) => panic!("{name}: expected error"),
            Err(e) => assert!(
                e.to_string().contains(want),
                "{name}: err = {e}, want {want:?}"
            ),
        }
    }
}

#[test]
fn parse_alias_keeps_reference() {
    let (doc, meta) = parse("a: &x {p: 1}\nb: *x\n").unwrap();
    let want = must_json(r#"{"a":{"p":1},"b":{"p":1}}"#);
    assert_eq!(doc.write(), want.write());
    assert_eq!(meta.aliases, 1);
    let a = doc.get(&[PathSeg::Key("a".into())]).unwrap();
    let b = doc.get(&[PathSeg::Key("b".into())]).unwrap();
    assert_eq!(a, b, "alias occurrence should share the anchored node");
    assert_eq!(doc.node(a).anchor, "x");
    // Editing through the reference writes the anchored original.
    let mut doc = doc;
    let two = doc.new_num("2");
    doc.set(&[PathSeg::Key("b".into()), PathSeg::Key("p".into())], two)
        .unwrap();
    let got = doc
        .get(&[PathSeg::Key("a".into()), PathSeg::Key("p".into())])
        .unwrap();
    assert_eq!(doc.value(got), &jsondoc::Value::Num("2".into()));
}

#[test]
fn parse_scalar_alias_shares() {
    let (doc, meta) = parse("a: &s 42\nb: *s\n").unwrap();
    let a = doc.get(&[PathSeg::Key("a".into())]).unwrap();
    let b = doc.get(&[PathSeg::Key("b".into())]).unwrap();
    assert_eq!(a, b, "scalar alias should share the anchored node");
    assert_eq!(meta.aliases, 1);
    // A write through the reference keeps both views in sync.
    let mut doc = doc;
    let replaced = doc.new_str("replaced");
    doc.set(&[PathSeg::Key("b".into())], replaced).unwrap();
    assert_eq!(doc.value(a), &jsondoc::Value::Str("replaced".into()));
}

#[test]
fn write_restores_anchors_and_aliases() {
    let src = "base: &x\n  p: 1\ncopy: *x\n";
    let (doc, meta) = parse(src).unwrap();
    assert_eq!(write_string(&doc, Some(&meta)), src);
}

#[test]
fn write_drops_alias_after_expand() {
    let (mut doc, meta) = parse("base: &x {p: 1}\ncopy: *x\n").unwrap();
    let mem = doc.get(&[PathSeg::Key("copy".into())]).unwrap();
    let detached = doc.clone_detached(mem);
    assert!(doc.detach(&[PathSeg::Key("copy".into())], detached));
    let want = "base: &x\n  p: 1\ncopy:\n  p: 1\n";
    let out = write_string(&doc, Some(&meta));
    assert_eq!(out, want);
    // Re-parsing yields two independent objects.
    let (doc2, meta2) = parse(&out).unwrap();
    assert_eq!(meta2.aliases, 0, "aliases after expand");
    let a2 = doc2.get(&[PathSeg::Key("base".into())]);
    assert!(
        a2.is_some() && doc2.node(a2.unwrap()).anchor == "x",
        "base anchor lost"
    );
    let b2 = doc2.get(&[PathSeg::Key("copy".into())]).unwrap();
    assert!(
        doc2.node(b2).anchor.is_empty(),
        "expanded copy should be plain data"
    );
}

#[test]
fn write_nil_meta_ignores_anchors() {
    let mut doc = must_json(r#"{"a":{"p":1}}"#);
    let a = doc.get(&[PathSeg::Key("a".into())]).unwrap();
    doc.node_mut(a).anchor = "x".to_string();
    assert_eq!(write_string(&doc, None), "a:\n  p: 1\n");
}

#[test]
fn parse_timestamp_as_string() {
    let (doc, _) = parse("t: 2026-10-18\n").unwrap();
    let n = doc.get(&[PathSeg::Key("t".into())]).unwrap();
    assert_eq!(doc.value(n), &jsondoc::Value::Str("2026-10-18".into()));
}

#[test]
fn write_round_trip() {
    let src =
        "# config header\nname: demo # keep me\nport: 8080\nitems:\n  - a # item note\n  - b\n";
    let (doc, meta) = parse(src).unwrap();
    let out = write_string(&doc, Some(&meta));
    let (doc2, _) = parse(&out).unwrap_or_else(|e| panic!("reparse: {e}\n{out}"));
    assert_eq!(doc2.write(), doc.write(), "round-trip doc\nout:\n{out}");
}

#[test]
fn comments_preserved_verbatim() {
    let src = "# config header\nname: demo # keep me\n# port note\nport: 8080\nitems:\n  - a # item note\n  - b\n";
    let (doc, meta) = parse(src).unwrap();
    assert_eq!(write_string(&doc, Some(&meta)), src);
}

#[test]
fn blank_line_separated_comment_preserved() {
    // yaml.v3 does not record the blank line itself, so the comment is
    // preserved but the separation collapses. The comment must stay with
    // the member it precedes.
    let src = "name: demo\n\n# separated note\nport: 8080\n";
    let (doc, meta) = parse(src).unwrap();
    let out = write_string(&doc, Some(&meta));
    assert!(
        out.contains("# separated note\nport: 8080"),
        "comment lost or moved:\n{out}"
    );
    assert!(out.contains("name: demo"), "data lost:\n{out}");
}

#[test]
fn comment_dropped_with_deleted_node() {
    let src = "name: demo\n# port note\nport: 8080\n";
    let (mut doc, meta) = parse(src).unwrap();
    assert!(doc.remove(&[PathSeg::Key("port".into())]));
    let out = write_string(&doc, Some(&meta));
    assert!(
        !out.contains("# port note") && !out.contains("port:"),
        "deleted member leaked:\n{out}"
    );
}

#[test]
fn write_quotes_ambiguous_strings() {
    let cases = [
        ("v: \"123\"\n", "v: \"123\"\n"),
        ("v: \"true\"\n", "v: \"true\"\n"),
        ("v: \"\"\n", "v: \"\"\n"),
    ];
    for (src, want) in cases {
        let (doc, meta) = parse(src).unwrap();
        assert_eq!(write_string(&doc, Some(&meta)), want, "in {src:?}");
    }
}

#[test]
fn write_indent() {
    let doc = must_json(r#"{"a":{"b":1},"c":[2,3]}"#);
    let want = "a:\n  b: 1\nc:\n  - 2\n  - 3\n";
    assert_eq!(write_string(&doc, None), want);
}

#[test]
fn write_nil_meta() {
    let doc = must_json(r#"{"a":1}"#);
    assert_eq!(write_string(&doc, None), "a: 1\n");
}

#[test]
fn write_edited_value_keeps_comments() {
    let src = "# header\nname: demo # keep me\n";
    let (mut doc, meta) = parse(src).unwrap();
    let newname = doc.new_str("newname");
    doc.set(&[PathSeg::Key("name".into())], newname).unwrap();
    let want = "# header\nname: newname # keep me\n";
    assert_eq!(write_string(&doc, Some(&meta)), want);
}

#[test]
fn write_dropped_trailing_root_comment() {
    let src = "a: 1\n# tail\n";
    let (doc, meta) = parse(src).unwrap();
    assert_eq!(write_string(&doc, Some(&meta)), src);
}

#[test]
fn parse_merge_key() {
    let src = "base1: &b1\n  env: e1\nbase2: &b2\n  env: e2\n  out: o2\nchild:\n  <<: [*b1, *b2]\n  own: yes\n";
    let (doc, meta) = parse(src).unwrap();
    assert_eq!(meta.aliases, 2, "the 2 merge references");
    let child = doc.get(&[PathSeg::Key("child".into())]).unwrap();
    let refs = doc.merge_refs(child);
    assert_eq!(refs.len(), 2);
    assert_eq!(doc.node(refs[0]).anchor, "b1");
    assert_eq!(doc.node(refs[1]).anchor, "b2");
    // resolved with YAML precedence: own wins, earlier anchor wins
    let want = must_json(
        r#"{"base1":{"env":"e1"},"base2":{"env":"e2","out":"o2"},"child":{"own":"yes","env":"e1","out":"o2"}}"#,
    );
    assert_eq!(doc.write(), want.write());
    // a write through a merged key reaches the anchor's object
    let mut doc = doc;
    let no = doc.new_str("no");
    doc.set(
        &[PathSeg::Key("child".into()), PathSeg::Key("own".into())],
        no,
    )
    .unwrap();
    assert!(
        doc.get(&[PathSeg::Key("child".into()), PathSeg::Key("env".into())])
            .is_none(),
        "a merged key must not materialize in the mapping"
    );
}

#[test]
fn write_merge_key_roundtrip() {
    let src = "base: &b\n  p: 1\nchild:\n  <<: *b\n  q: 2\n";
    let (doc, meta) = parse(src).unwrap();
    let out = write_string(&doc, Some(&meta));
    assert_eq!(out, src, "single-alias merge roundtrip");
    // a sequence of aliases stays a sequence
    let seq = "a: &x\n  p: 1\nb: &y\n  q: 2\nc:\n  <<:\n    - *x\n    - *y\n";
    let (doc2, meta2) = parse(seq).unwrap();
    let out2 = write_string(&doc2, Some(&meta2));
    assert_eq!(out2, seq, "sequence merge roundtrip");
}

#[test]
fn write_merge_key_after_edit() {
    let (mut doc, meta) = parse("base: &b\n  p: 1\nchild:\n  <<: *b\n").unwrap();
    // retarget the merge at a second anchor: b2 is appended after b1
    use crate::doc::jsondoc::{Member, Value};
    let b2 = doc.new_object();
    doc.node_mut(b2).anchor = "b2".to_string();
    let r3 = doc.new_num("3");
    if let Value::Object(m) = &mut doc.nodes[b2 as usize].value {
        m.push(Member {
            key: "r".into(),
            val: r3,
        });
    }
    doc.insert_member(&[], "base2", 1, b2).unwrap();
    let base = doc.get(&[PathSeg::Key("base".into())]).unwrap();
    let child = doc.get(&[PathSeg::Key("child".into())]).unwrap();
    let merge_val = doc.push(jsondoc::NodeData {
        value: Value::Null,
        anchor: String::new(),
        merge: vec![base, b2],
    });
    let own = doc.new_str("x");
    if let Value::Object(m) = &mut doc.nodes[child as usize].value {
        *m = vec![
            Member {
                key: "<<".into(),
                val: merge_val,
            },
            Member {
                key: "own".into(),
                val: own,
            },
        ];
    }
    let want =
        "base: &b\n  p: 1\nbase2: &b2\n  r: 3\nchild:\n  <<:\n    - *b\n    - *b2\n  own: x\n";
    let out = write_string(&doc, Some(&meta));
    assert_eq!(out, want, "edited merge output");
}

#[test]
fn write_merge_refuses_dangling_anchor() {
    let (mut doc, meta) = parse("base: &b\n  p: 1\nchild:\n  <<: *b\n").unwrap();
    // deleting the anchor carrier leaves the merge pointing nowhere
    assert!(doc.remove(&[PathSeg::Key("base".into())]));
    let out = write(&doc, Some(&meta));
    match out {
        Ok(s) => panic!("write should refuse dangling anchor, got {s:?}"),
        Err(e) => assert!(
            e.to_string().contains("*b"),
            "err = {e}, want a dangling *b refusal"
        ),
    }
}

#[test]
fn merge_comment_preserved() {
    let src = "base: &b\n  p: 1 # keep\nchild:\n  <<: *b # merge note\n  q: 2\n";
    let (doc, meta) = parse(src).unwrap();
    let out = write_string(&doc, Some(&meta));
    assert!(
        out.contains("# keep") && out.contains("# merge note"),
        "merge comments lost:\n{out}"
    );
}

#[test]
fn merge_chain_own_fields_win() {
    // b2 merges b1 and then defines name itself: b2's own field must win
    // over the inherited one, wherever its "<<" sits (yaml.v3 agrees)
    let src =
        "b1: &b1\n  name: from-b1\n  a: 1\nb2: &b2\n  <<: *b1\n  name: from-b2\njob:\n  <<: *b2\n";
    let (mut doc, _) = parse(src).unwrap();
    let want = must_json(
        r#"{"b1":{"name":"from-b1","a":1},"b2":{"name":"from-b2","a":1},"job":{"name":"from-b2","a":1}}"#,
    );
    assert_eq!(doc.write(), want.write());
    // the merge chain resolves in detached copies too
    let job = doc.get(&[PathSeg::Key("job".into())]).unwrap();
    let det = doc.clone_detached(job);
    let w = must_json(r#"{"name":"from-b2","a":1}"#);
    assert_eq!(doc.write_subtree(det), w.write(), "detached");
}

#[test]
fn merge_chain_deep_precedence() {
    // job's own x wins; y comes from b2's override of b1's y; z from b3
    let src = "b1: &b1\n  x: 1\n  y: 1\nb2: &b2\n  <<: *b1\n  y: 2\nb3: &b3\n  <<: *b2\n  z: 3\njob:\n  <<: *b3\n  x: 9\n";
    let (doc, meta) = parse(src).unwrap();
    let job = doc.get(&[PathSeg::Key("job".into())]).unwrap();
    let want = must_json(r#"{"x":9,"z":3,"y":2}"#); // own members first, then the chain
    assert_eq!(doc.write_subtree(job), want.write());
    // writing keeps the chain intact and reloadable
    let out = write_string(&doc, Some(&meta));
    let (doc2, _) = parse(&out).unwrap();
    let j2 = doc2.get(&[PathSeg::Key("job".into())]).unwrap();
    let v = doc2.to_value(j2);
    assert_eq!(v["y"], serde_json::json!(2), "reloaded job: {v}");
}

// --- docfmt YAML tests ---------------------------------------------------------

#[test]
fn yaml_format_notices() {
    let c = codec_for("a.yaml");
    let p = c.parse("a: &x {p: 1}\nb: *x\n").unwrap();
    assert_eq!(
        c.notices(&p.doc, &p.meta),
        vec![
            "yaml: 1 alias kept: edits write through to the anchored object (< Expand > detaches a copy)"
        ]
    );
    let plain = c.parse("a: 1\n").unwrap();
    assert!(c.notices(&plain.doc, &plain.meta).is_empty());
    let empty = c.empty();
    assert!(c.notices(&empty.doc, &empty.meta).is_empty());
    let jsonc = codec_for("a.json").parse(r#"{"a":1}"#).unwrap();
    assert!(
        codec_for("a.json")
            .notices(&jsonc.doc, &jsonc.meta)
            .is_empty()
    );
}

#[test]
fn yaml_schema_directive() {
    let src = "# config header\nname: demo # keep me\n";
    let c = codec_for("a.yaml");
    let p = c.parse(src).unwrap();
    assert!(c.schema_location(&p.doc, &p.meta).is_none());
    let mut p = p;
    c.set_schema_location(&mut p.doc, &mut p.meta, "sch.json");
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("sch.json")
    );
    let want = "# yaml-language-server: $schema=sch.json\n# config header\nname: demo # keep me\n";
    assert_eq!(c.write(&p.doc, &p.meta).unwrap(), want);
    c.set_schema_location(&mut p.doc, &mut p.meta, "other.json"); // keeps existing
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("sch.json")
    );
    // output is valid YAML without a $schema member
    let out = c.write(&p.doc, &p.meta).unwrap();
    let (doc2, _) = super::parse(&out).unwrap();
    assert!(matches!(doc2.value(doc2.root), crate::doc::Value::Object(m) if m.len() == 1));
    c.clear_schema_location(&mut p.doc, &mut p.meta);
    assert!(c.schema_location(&p.doc, &p.meta).is_none());
    assert_eq!(c.write(&p.doc, &p.meta).unwrap(), src);
}

#[test]
fn yaml_directive_parsed() {
    let src = "# yaml-language-server: $schema=https://example.com/s.json\nname: demo\n";
    let c = codec_for("a.yaml");
    let p = c.parse(src).unwrap();
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("https://example.com/s.json")
    );
    assert_eq!(c.write(&p.doc, &p.meta).unwrap(), src);
    let mut p = p;
    c.set_schema_location(&mut p.doc, &mut p.meta, "other.json"); // keeps the directive
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("https://example.com/s.json")
    );
    // --pick clears first, then records the chosen schema
    c.clear_schema_location(&mut p.doc, &mut p.meta);
    c.set_schema_location(&mut p.doc, &mut p.meta, "picked.json");
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("picked.json")
    );
    let out = c.write(&p.doc, &p.meta).unwrap();
    assert!(
        !out.contains("example.com") && out.contains("yaml-language-server: $schema=picked.json"),
        "old directive not replaced:\n{out}"
    );
}

#[test]
fn yaml_explicit_schema_member() {
    let src = "$schema: old.json\nname: demo\n";
    let c = codec_for("a.yaml");
    let p = c.parse(src).unwrap();
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("old.json")
    );
    let mut p = p;
    c.set_schema_location(&mut p.doc, &mut p.meta, "new.json");
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("old.json")
    );
    assert!(
        !c.write(&p.doc, &p.meta)
            .unwrap()
            .contains("yaml-language-server")
    );
    c.clear_schema_location(&mut p.doc, &mut p.meta);
    assert!(c.schema_location(&p.doc, &p.meta).is_none());
    assert_eq!(c.write(&p.doc, &p.meta).unwrap(), "name: demo\n");
}

#[test]
fn yaml_schema_member_used_when_schema_declares_it() {
    let c = codec_for("a.yaml");
    let mut p = c.parse("name: demo\n").unwrap();
    c.set_schema_location_with(&mut p.doc, &mut p.meta, "sch.json", true);
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("sch.json")
    );
    let out = c.write(&p.doc, &p.meta).unwrap();
    assert_eq!(out, "$schema: sch.json\nname: demo\n");
    assert!(!out.contains("yaml-language-server"), "{out}");
}

#[test]
fn yaml_empty_member_filled_when_schema_declares_it() {
    let c = codec_for("a.yaml");
    let mut p = c.parse("$schema: \"\"\nname: demo\n").unwrap();
    c.set_schema_location_with(&mut p.doc, &mut p.meta, "sch.json", true);
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("sch.json")
    );
    let out = c.write(&p.doc, &p.meta).unwrap();
    assert_eq!(out, "$schema: sch.json\nname: demo\n");
    assert!(!out.contains("yaml-language-server"), "{out}");
}

#[test]
fn yaml_directive_migrates_to_declared_member() {
    let c = codec_for("a.yaml");
    let src = "# config header\n# yaml-language-server: $schema=old.json\nname: demo\n";
    let mut p = c.parse(src).unwrap();
    c.set_schema_location_with(&mut p.doc, &mut p.meta, "new.json", true);
    // An existing location keeps precedence, now recorded in the field.
    assert_eq!(
        c.schema_location(&p.doc, &p.meta).as_deref(),
        Some("old.json")
    );
    let out = c.write(&p.doc, &p.meta).unwrap();
    assert!(out.contains("$schema: old.json"), "{out}");
    assert!(out.contains("# config header"), "{out}");
    assert!(!out.contains("yaml-language-server"), "{out}");
}

#[test]
fn yaml_directive_stays_when_schema_has_no_member() {
    let c = codec_for("a.yaml");
    let mut p = c.parse("name: demo\n").unwrap();
    c.set_schema_location_with(&mut p.doc, &mut p.meta, "sch.json", false);
    let out = c.write(&p.doc, &p.meta).unwrap();
    assert!(
        out.contains("# yaml-language-server: $schema=sch.json"),
        "{out}"
    );
    assert!(!out.contains("$schema:"), "{out}");
}

#[test]
fn yaml_empty_schema_member_not_filled() {
    let cases = [
        (
            "empty string",
            "$schema: \"\"\nname: demo\n",
            "$schema: \"\"",
        ),
        (
            "explicit null",
            "$schema: null\nname: demo\n",
            "$schema: null",
        ),
    ];
    for (name, src, keep) in cases {
        let c = codec_for("a.yaml");
        let mut p = c.parse(src).unwrap_or_else(|e| panic!("{name}: {e}"));
        c.set_schema_location(&mut p.doc, &mut p.meta, "sch.json");
        let out = c.write(&p.doc, &p.meta).unwrap();
        assert!(
            out.contains(keep),
            "{name}: empty $schema member was filled or removed:\n{out}"
        );
        assert!(
            !out.contains("$schema: sch.json"),
            "{name}: schema written into a $schema field:\n{out}"
        );
        assert!(
            out.contains("# yaml-language-server: $schema=sch.json"),
            "{name}: directive missing:\n{out}"
        );
        assert_eq!(
            c.schema_location(&p.doc, &p.meta).as_deref(),
            Some("sch.json"),
            "{name}"
        );
    }
}

#[test]
fn yaml_new_empty_schema_directive() {
    let c = codec_for("a.yaml");
    let mut p = c.empty();
    c.set_schema_location(&mut p.doc, &mut p.meta, "sch.json");
    let want = "# yaml-language-server: $schema=sch.json\n{}\n";
    assert_eq!(c.write(&p.doc, &p.meta).unwrap(), want);
}

#[test]
fn new_empty_writes_both_formats() {
    // restored from Phase 1 now that the YAML codec writes
    let jsonc: &dyn crate::doc::Codec = codec_for("a.json");
    let yaml: &dyn crate::doc::Codec = codec_for("a.yaml");
    for c in [jsonc, yaml] {
        let p = c.empty();
        assert_eq!(c.write(&p.doc, &p.meta).unwrap(), "{}\n");
    }
}

#[test]
fn yaml_session_round_trip() {
    let src = "# config header\nname: demo # keep me\nitems:\n  - a # item note\n";
    let c = codec_for("a.yaml");
    let p = c.parse(src).unwrap();
    assert_eq!(c.write(&p.doc, &p.meta).unwrap(), src);
}
