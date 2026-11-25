//! oneOf variant signatures and current-variant matching.

use super::*;
use super::{TB, child_of, dbuild, fnode, scalar, tenum, test_doc, test_schema};

#[test]
fn test_variant_signature() {
    let mut b = TB::new();
    let mut cases: Vec<(FormNodeId, &str)> = Vec::new();
    cases.push((b.id(scalar("", "", "integer")), "integer"));
    cases.push((b.id(scalar("", "", "string")), "string"));
    cases.push((b.id(scalar("", "", "")), "string"));
    let mut e3 = fnode("", "", Kind::Enum);
    e3.enum_ = tenum(&mut b, &["http", "grpc", "tcp"]);
    cases.push((b.id(e3), "http |grpc |tcp"));
    let mut e4 = fnode("", "", Kind::Enum);
    e4.enum_ = tenum(&mut b, &["a", "b", "c", "d"]);
    cases.push((b.id(e4), "a |b |c …"));
    cases.push((b.id(fnode("", "", Kind::Boolean)), "yes/no"));
    cases.push((b.id(fnode("", "", Kind::Raw)), "json"));
    let s_item = b.id(scalar("", "", "string"));
    cases.push((
        b.id(FormNode {
            key: "".into(),
            kind: Kind::Array,
            items: Some(s_item),
            ..FormNode::default()
        }),
        "list of string",
    ));
    cases.push((
        b.id(FormNode {
            key: "".into(),
            kind: Kind::Array,
            ..FormNode::default()
        }),
        "list of json",
    ));
    cases.push((
        b.id(FormNode {
            key: "".into(),
            kind: Kind::Object,
            ..FormNode::default()
        }),
        "{}",
    ));
    let src = b.id(fnode("src", "", Kind::Scalar));
    b.nodes[src as usize].required = true;
    let dst = b.id(fnode("dst", "", Kind::Scalar));
    cases.push((
        b.id(FormNode {
            key: "".into(),
            kind: Kind::Object,
            children: vec![src, dst],
            ..FormNode::default()
        }),
        "{src*, dst}",
    ));
    let mut five: Vec<FormNodeId> = Vec::new();
    for k in ["a", "b", "c", "d", "e", "f"] {
        let c = b.id(fnode(k, "", Kind::Scalar));
        if k == "b" {
            b.nodes[c as usize].required = true;
        }
        five.push(c);
    }
    cases.push((
        b.id(FormNode {
            key: "".into(),
            kind: Kind::Object,
            children: five,
            ..FormNode::default()
        }),
        "{a, b*, c, d, +2}",
    ));
    // required const child shows its fixed value (sing-box style
    // discriminated variants: {type: {const: "direct"}})
    let mut ctype = fnode("type", "", Kind::Enum);
    ctype.enum_ = tenum(&mut b, &["direct"]);
    ctype.required = true;
    let ctype = b.id(ctype);
    let tag = b.id(fnode("tag", "", Kind::Scalar));
    let listen = b.id(fnode("listen", "", Kind::Scalar));
    cases.push((
        b.id(FormNode {
            key: "".into(),
            kind: Kind::Object,
            children: vec![ctype, tag, listen],
            ..FormNode::default()
        }),
        "{type=direct, tag, listen}",
    ));
    // optional const and multi-option required keep the plain/star forms
    let mut cotype = fnode("type", "", Kind::Enum);
    cotype.enum_ = tenum(&mut b, &["direct"]);
    let cotype = b.id(cotype);
    let mut cmode = fnode("mode", "", Kind::Enum);
    cmode.enum_ = tenum(&mut b, &["a", "b"]);
    cmode.required = true;
    let cmode = b.id(cmode);
    cases.push((
        b.id(FormNode {
            key: "".into(),
            kind: Kind::Object,
            children: vec![cotype, cmode],
            ..FormNode::default()
        }),
        "{type, mode*}",
    ));
    // a nested oneOf (allOf-distributed detail branch in Cargo.toml)
    // stands in its variants' signatures, not "json"; identical object
    // leaves (has version / path / git / workspace) dedupe to one
    let v1 = b.id(scalar("version", "", "string"));
    let o1 = b.id(FormNode {
        key: "".into(),
        kind: Kind::Object,
        children: vec![v1],
        ..FormNode::default()
    });
    let v2 = b.id(scalar("version", "", "string"));
    let o2 = b.id(FormNode {
        key: "".into(),
        kind: Kind::Object,
        children: vec![v2],
        ..FormNode::default()
    });
    let detailed = b.id(FormNode {
        key: "".into(),
        kind: Kind::OneOf,
        variants: vec![o1, o2],
        ..FormNode::default()
    });
    cases.push((detailed, "{version}"));
    let s1 = b.id(scalar("", "", "string"));
    let s2 = b.id(scalar("", "", "integer"));
    let distinct = b.id(FormNode {
        key: "".into(),
        kind: Kind::OneOf,
        variants: vec![s1, s2],
        ..FormNode::default()
    });
    cases.push((distinct, "string | integer"));

    let schema = b.finish();
    for (f, want) in cases {
        assert_eq!(
            variant_signature(&schema, Some(f)),
            want,
            "VariantSignature"
        );
    }
}

#[test]
fn test_current_variant() {
    let schema = test_schema();
    let root = schema.root;
    let auth = child_of(&schema, root, "auth");
    let cases: Vec<(Option<NodeId>, i64)> = Vec::new();
    let _ = cases;
    let check = |doc: &Doc, want: i64, desc: &str| {
        assert_eq!(
            current_variant(&schema, auth, doc, Some(doc.root)),
            want,
            "{}",
            desc
        );
    };
    // nil value
    assert_eq!(current_variant(&schema, auth, &test_doc(), None), -1);
    // explicit null
    let dn = dbuild(|db| db.push(Value::Null));
    check(&dn, -1, "null");
    let ds = dbuild(|db| db.s("none"));
    check(&ds, 0, "enum variant match");
    let d_obj = dbuild(|db| db.obj(vec![("token", db.s("x"))]));
    check(&d_obj, 1, "key intersection");
    let d_empty = dbuild(|db| db.obj(vec![]));
    check(&d_empty, -1, "token is required → no key hit");
    let d_other = dbuild(|db| db.s("other"));
    check(&d_other, -1, "no variant matches");

    // object variant without required children matches any object
    let mut b = TB::new();
    let a = b.id(fnode("a", "", Kind::Scalar));
    let v = b.id(FormNode {
        key: "".into(),
        kind: Kind::Object,
        children: vec![a],
        ..FormNode::default()
    });
    let free = b.id(FormNode {
        key: "".into(),
        kind: Kind::OneOf,
        variants: vec![v],
        ..FormNode::default()
    });
    let free_schema = b.finish();
    let d = dbuild(|db| db.obj(vec![]));
    assert_eq!(
        current_variant(&free_schema, free, &d, Some(d.root)),
        0,
        "required-free object variant should match"
    );
}
