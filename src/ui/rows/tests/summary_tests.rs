//! Row value summaries.

use super::*;
use super::{DB, Fx, TB, child_of, dbuild, fnode, scalar, tenum};

// --- summary ------------------------------------------------------------

#[test]
fn test_summary_rules() {
    let fx = Fx::new();
    let root = fx.schema.root;
    let name = child_of(&fx.schema, root, "name");
    let mode = child_of(&fx.schema, root, "mode");
    let server = child_of(&fx.schema, root, "server");
    let port = child_of(&fx.schema, server, "port");
    let host = child_of(&fx.schema, server, "host");
    let debug = child_of(&fx.schema, server, "debug");
    let ratio = child_of(&fx.schema, server, "ratio");
    let mounts = child_of(&fx.schema, root, "mounts");
    let auth = child_of(&fx.schema, root, "auth");
    let ctx = fx.ctx();
    let cases: Vec<(FormNodeId, Vec<PathSeg>, &str)> = vec![
        (name, vec![PathSeg::key("name")], "webapp"),
        (mode, vec![PathSeg::key("mode")], "http"), // default displayed
        (
            port,
            vec![PathSeg::key("server"), PathSeg::key("port")],
            "8080",
        ),
        (
            host,
            vec![PathSeg::key("server"), PathSeg::key("host")],
            "0.0.0.0",
        ),
        (
            debug,
            vec![PathSeg::key("server"), PathSeg::key("debug")],
            "[y]",
        ),
        (
            ratio,
            vec![PathSeg::key("server"), PathSeg::key("ratio")],
            "—",
        ),
        (
            server,
            vec![PathSeg::key("server")],
            "port=8080 host=0.0.0.0 +1",
        ),
        (mounts, vec![PathSeg::key("mounts")], "1 item"),
        (auth, vec![PathSeg::key("auth")], "—"), // unset oneOf
    ];
    for (f, path, want) in cases {
        assert_eq!(
            summary(&ctx, f, &path),
            want,
            "summary({})",
            fx.schema.nodes[f as usize].key
        );
    }

    // oneOf value summaries: enum/leaf variants show the value itself,
    // object variants keep the title for context
    let d2 = dbuild(|db| db.obj(vec![("auth", db.s("none"))]));
    let ctx2 = MenuCtx {
        schema: &fx.schema,
        doc: &d2,
    };
    assert_eq!(summary(&ctx2, auth, &[PathSeg::key("auth")]), "none");

    let d3 = dbuild(|db| db.obj(vec![("auth", db.obj(vec![("token", db.s("secret123"))]))]));
    let ctx3 = MenuCtx {
        schema: &fx.schema,
        doc: &d3,
    };
    assert_eq!(
        summary(&ctx3, auth, &[PathSeg::key("auth")]),
        "Token token=secret123"
    );

    // untitled object variant: members only, no synthesized fallback label
    let mut b = TB::new();
    let action = b.id(fnode("action", "", Kind::Scalar));
    let v = b.id(FormNode {
        key: "".into(),
        kind: Kind::Object,
        children: vec![action],
        ..FormNode::default()
    });
    let untitled = b.id(FormNode {
        key: "rules".into(),
        kind: Kind::OneOf,
        variants: vec![v],
        ..FormNode::default()
    });
    let untitled_schema = b.finish();
    let d3b = dbuild(|db| db.obj(vec![("rules", db.obj(vec![("action", db.s("sniff"))]))]));
    let ctx3b = MenuCtx {
        schema: &untitled_schema,
        doc: &d3b,
    };
    assert_eq!(
        summary(&ctx3b, untitled, &[PathSeg::key("rules")]),
        "action=sniff"
    );

    // empty and long arrays
    let d4 = dbuild(|db| db.obj(vec![("mounts", db.arr(vec![]))]));
    let ctx4 = MenuCtx {
        schema: &fx.schema,
        doc: &d4,
    };
    assert_eq!(summary(&ctx4, mounts, &[PathSeg::key("mounts")]), "empty");

    let d5 = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![db.obj(vec![]), db.obj(vec![])]),
        )])
    });
    let ctx5 = MenuCtx {
        schema: &fx.schema,
        doc: &d5,
    };
    assert_eq!(summary(&ctx5, mounts, &[PathSeg::key("mounts")]), "2 items");

    // long string clipped to 32 runes including ellipsis
    let long: String = "a".repeat(40);
    let d6 = dbuild(|db| db.obj(vec![("name", db.s(&long))]));
    let ctx6 = MenuCtx {
        schema: &fx.schema,
        doc: &d6,
    };
    assert_eq!(
        summary(&ctx6, name, &[PathSeg::key("name")]),
        format!("{}…", &long[..31])
    );

    // string lists preview their members: [N] v0 v1 v2 +rest
    let mut b = TB::new();
    let tags = b.id(FormNode {
        key: "tags".into(),
        kind: Kind::Array,
        ..FormNode::default()
    });
    let tags_schema = b.finish();
    let p = [PathSeg::key("tags")];
    macro_rules! tags_summary {
        ($build:expr) => {{
            let doc = dbuild($build);
            let ctx = MenuCtx {
                schema: &tags_schema,
                doc: &doc,
            };
            summary(&ctx, tags, &p)
        }};
    }
    assert_eq!(
        tags_summary!(|db: &DB| db.obj(vec![(
            "tags",
            db.arr(vec![db.s("a"), db.s("b"), db.s("c"), db.s("d")])
        )])),
        "[4] a b c +1"
    );
    assert_eq!(
        tags_summary!(|db: &DB| db.obj(vec![("tags", db.arr(vec![db.s("a"), db.s("b")]))])),
        "[2] a b"
    );
    assert_eq!(
        tags_summary!(|db: &DB| db.obj(vec![("tags", db.arr(vec![db.s("legacy")]))])),
        "[1] legacy"
    );
    assert_eq!(
        tags_summary!(|db: &DB| db.obj(vec![("tags", db.arr(vec![db.s("a"), db.n("1")]))])),
        "[2] a 1"
    );
    // numbers preview like strings; objects in a mixed list render as {…}
    assert_eq!(
        tags_summary!(|db: &DB| db.obj(vec![(
            "tags",
            db.arr(vec![db.n("1"), db.n("22"), db.n("3.5")])
        )])),
        "[3] 1 22 3.5"
    );
    assert_eq!(
        tags_summary!(|db: &DB| db.obj(vec![(
            "tags",
            db.arr(vec![
                db.s("a"),
                db.n("1"),
                db.obj(vec![("k", db.s("v"))]),
                db.b(true)
            ])
        )])),
        "[4] a 1 {…} +1"
    );
    // pure object arrays keep the plain count
    assert_eq!(
        tags_summary!(
            |db: &DB| db.obj(vec![("tags", db.arr(vec![db.obj(vec![]), db.obj(vec![])]))])
        ),
        "2 items"
    );
    assert_eq!(
        tags_summary!(|db: &DB| db.obj(vec![("tags", db.arr(vec![db.s(&long)]))])),
        format!("[1] {}…", &long[..15])
    );

    // raw nodes
    let mut b = TB::new();
    let raw = b.id(fnode("extra", "Extra", Kind::Raw));
    let raw_schema = b.finish();
    let d7 = dbuild(|db| db.obj(vec![("extra", db.obj(vec![("a", db.n("1"))]))]));
    let ctx7 = MenuCtx {
        schema: &raw_schema,
        doc: &d7,
    };
    assert_eq!(
        summary(&ctx7, raw, &[PathSeg::key("extra")]),
        r#"json {"a":1}"#
    );
}

/// TestSummaryNestedOneOfGroups covers the sing-box DNSRule shape: the
/// item schema is a oneOf over nested oneOf groups, so the summary must
/// descend to the matched leaf instead of falling through to "—".
/// (The Go test's second half asserts rendered View output; rendering
/// lands in a later phase and is covered by the pty tests.)
#[test]
fn test_summary_nested_one_of_groups() {
    let mut b = TB::new();
    let mut type_default = fnode("type", "", Kind::Enum);
    type_default.enum_ = tenum(&mut b, &["default"]);
    type_default.required = true;
    let type_default = b.id(type_default);
    let ds_items = b.id(scalar("domain_suffix", "", "string"));
    let domain_suffix = b.id(FormNode {
        key: "domain_suffix".into(),
        kind: Kind::Array,
        items: Some(ds_items),
        ..FormNode::default()
    });
    let server = b.id(scalar("server", "", "string"));
    let matcher = b.id(FormNode {
        key: "rules".into(),
        kind: Kind::Object,
        children: vec![type_default, domain_suffix, server],
        ..FormNode::default()
    });
    let g1 = b.id(FormNode {
        key: "rules".into(),
        kind: Kind::OneOf,
        variants: vec![matcher],
        ..FormNode::default()
    });

    let mut type_logical = fnode("type", "", Kind::Enum);
    type_logical.enum_ = tenum(&mut b, &["logical"]);
    type_logical.required = true;
    let type_logical = b.id(type_logical);
    let mut mode = fnode("mode", "", Kind::Enum);
    mode.enum_ = tenum(&mut b, &["and", "or"]);
    mode.required = true;
    let mode = b.id(mode);
    let rules_arr = b.id(FormNode {
        key: "rules".into(),
        kind: Kind::Array,
        required: true,
        ..FormNode::default()
    });
    let logical = b.id(FormNode {
        key: "rules".into(),
        kind: Kind::Object,
        children: vec![type_logical, mode, rules_arr],
        ..FormNode::default()
    });
    let g2 = b.id(FormNode {
        key: "rules".into(),
        kind: Kind::OneOf,
        variants: vec![logical],
        ..FormNode::default()
    });
    let rules = b.id(FormNode {
        key: "rules".into(),
        kind: Kind::OneOf,
        variants: vec![g1, g2],
        ..FormNode::default()
    });
    let schema = b.finish();

    let doc = dbuild(|db| {
        db.obj(vec![(
            "rules",
            db.arr(vec![
                db.obj(vec![
                    ("type", db.s("default")),
                    ("domain_suffix", db.arr(vec![db.s("a.com"), db.s("b.com")])),
                    ("server", db.s("dns-local")),
                ]),
                db.obj(vec![
                    ("type", db.s("logical")),
                    ("mode", db.s("or")),
                    ("rules", db.arr(vec![])),
                ]),
            ]),
        )])
    });
    let cx = MenuCtx {
        schema: &schema,
        doc: &doc,
    };
    assert_eq!(
        summary(&cx, rules, &[PathSeg::key("rules"), PathSeg::Idx(0)]),
        "type=default domain_suffix=[2] +1"
    );
    assert_eq!(
        summary(&cx, rules, &[PathSeg::key("rules"), PathSeg::Idx(1)]),
        "type=logical mode=or +1"
    );
    // classic picker rows: a nested group variant shows its matched
    // leaf's value
    let logical_val = doc.get(&[PathSeg::key("rules"), PathSeg::Idx(1)]).unwrap();
    assert_eq!(
        variant_value_summary(
            &schema,
            schema.nodes[rules as usize].variants[1],
            &doc,
            Some(logical_val)
        ),
        "type=logical mode=or +1"
    );
}

#[test]
fn test_object_summary() {
    let fx = Fx::new();
    let plain = |build: &dyn Fn(&DB) -> NodeId| -> String {
        let doc = dbuild(|db| build(db));
        let rootv = doc.root;
        object_summary(&fx.schema, None, &doc, Some(rootv))
    };
    assert_eq!(object_summary(&fx.schema, None, &fx.doc, None), "{}");
    assert_eq!(plain(&|db: &DB| db.obj(vec![])), "{}");
    assert_eq!(plain(&|db: &DB| db.obj(vec![("a", db.n("1"))])), "a=1");
    assert_eq!(
        plain(&|db: &DB| db.obj(vec![("a", db.n("1")), ("b", db.b(true))])),
        "a=1 b=true"
    );
    assert_eq!(
        plain(&|db: &DB| db.obj(vec![("a", db.n("1")), ("b", db.b(true)), ("c", db.s("x"))])),
        "a=1 b=true +1"
    );
    assert_eq!(
        plain(&|db: &DB| db.obj(vec![("a", db.obj(vec![])), ("b", db.arr(vec![db.n("1")]))])),
        "a={…} b=[1]"
    );
    // null value needs a Null node; dbuild wraps a root of any shape
    let doc = dbuild(|db| db.obj(vec![("a", db.push(Value::Null))]));
    assert_eq!(
        object_summary(&fx.schema, None, &doc, Some(doc.root)),
        "a=null"
    );

    // members render in schema order even when the document lists them
    // differently: two list rows summarize alike (type before tag)
    let mut b = TB::new();
    let t = b.id(fnode("type", "", Kind::Scalar));
    let tg = b.id(fnode("tag", "", Kind::Scalar));
    let li = b.id(fnode("listen", "", Kind::Scalar));
    let po = b.id(fnode("port", "", Kind::Scalar));
    let schema = b.id(FormNode {
        key: "inbound".into(),
        kind: Kind::Object,
        children: vec![t, tg, li, po],
        ..FormNode::default()
    });
    let inbound_schema = b.finish();
    let doc = dbuild(|db| {
        db.obj(vec![
            ("tag", db.s("in-proxy")),
            ("type", db.s("mixed")),
            ("listen", db.s("::")),
            ("port", db.n("1080")),
        ])
    });
    assert_eq!(
        object_summary(&inbound_schema, Some(schema), &doc, Some(doc.root)),
        "type=mixed tag=in-proxy +2"
    );
    let doc_reordered = dbuild(|db| {
        db.obj(vec![
            ("port", db.n("1080")),
            ("listen", db.s("::")),
            ("type", db.s("mixed")),
            ("tag", db.s("in-proxy")),
        ])
    });
    assert_eq!(
        object_summary(
            &inbound_schema,
            Some(schema),
            &doc_reordered,
            Some(doc_reordered.root)
        ),
        "type=mixed tag=in-proxy +2"
    );
    // keys the schema does not know keep document order after the known ones
    let doc_unknown = dbuild(|db| {
        db.obj(vec![
            ("zz", db.n("1")),
            ("tag", db.s("t")),
            ("type", db.s("x")),
        ])
    });
    assert_eq!(
        object_summary(
            &inbound_schema,
            Some(schema),
            &doc_unknown,
            Some(doc_unknown.root)
        ),
        "type=x tag=t +1"
    );
}
