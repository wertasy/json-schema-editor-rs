use regex::Regex;
use tempfile::tempdir;

use super::*;
use crate::doc::jsondoc::{Member, NodeData};
use crate::doc::{Doc, NodeId, Value};
use crate::smodel::{EnumOpt, FormNode, Kind, Schema};
use crate::ui::{MenuCtx, MenuKind, MenuState};

// --- fixtures -----------------------------------------------------------

/// Builds a Schema arena plus the Doc arena its enum/default values
/// point into (the Go tests build loose *FormNode graphs).
struct TB {
    nodes: Vec<FormNode>,
    doc: Doc,
    root: FormNodeId,
}

impl TB {
    fn new() -> TB {
        TB {
            nodes: Vec::new(),
            doc: Doc::new(),
            root: 0,
        }
    }
    fn id(&mut self, n: FormNode) -> FormNodeId {
        let id = self.nodes.len() as FormNodeId;
        self.nodes.push(n);
        id
    }
    fn s(&mut self, v: &str) -> NodeId {
        self.doc.new_str(v)
    }
    fn n(&mut self, v: &str) -> NodeId {
        self.doc.new_num(v)
    }
    fn b(&mut self, v: bool) -> NodeId {
        self.doc.new_bool(v)
    }
    fn finish(self) -> Schema {
        Schema {
            nodes: self.nodes,
            root: self.root,
            defs: Vec::new(),
            doc: self.doc,
            schema_less: false,
        }
    }
}

/// Builds a document arena (the Go tobj/tstr/tnum helpers). Methods
/// take `&self` so nested expressions like
/// `db.obj(vec![("a", db.s("x"))])` compile.
#[derive(Clone)]
struct DB {
    nodes: std::rc::Rc<std::cell::RefCell<Vec<NodeData>>>,
}

impl DB {
    fn new() -> DB {
        DB {
            nodes: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
        }
    }
    fn push(&self, v: Value) -> NodeId {
        let mut n = self.nodes.borrow_mut();
        let id = n.len() as NodeId;
        n.push(NodeData {
            value: v,
            ..NodeData::default()
        });
        id
    }
    fn s(&self, v: &str) -> NodeId {
        self.push(Value::Str(v.into()))
    }
    fn n(&self, v: &str) -> NodeId {
        self.push(Value::Num(v.into()))
    }
    fn b(&self, v: bool) -> NodeId {
        self.push(Value::Bool(v))
    }
    fn obj(&self, ms: Vec<(&str, NodeId)>) -> NodeId {
        self.push(Value::Object(
            ms.into_iter()
                .map(|(k, v)| Member {
                    key: k.into(),
                    val: v,
                })
                .collect(),
        ))
    }
    fn arr(&self, xs: Vec<NodeId>) -> NodeId {
        self.push(Value::Array(xs))
    }
    fn finish(self, root: NodeId) -> Doc {
        Doc {
            nodes: self.nodes.borrow().clone(),
            root,
        }
    }
}

/// dbuild builds a whole document in one closure.
fn dbuild(f: impl FnOnce(&DB) -> NodeId) -> Doc {
    let db = DB::new();
    let root = f(&db);
    db.finish(root)
}

fn tenum(b: &mut TB, vals: &[&str]) -> Vec<EnumOpt> {
    vals.iter()
        .map(|&v| {
            let val = b.s(v);
            let encoded = b.doc.encode_value(val);
            EnumOpt {
                display: v.into(),
                encoded,
                val,
            }
        })
        .collect()
}

fn fnode(key: &str, title: &str, kind: Kind) -> FormNode {
    FormNode {
        key: key.into(),
        title: title.into(),
        kind,
        ..FormNode::default()
    }
}

fn scalar(key: &str, title: &str, jt: &str) -> FormNode {
    FormNode {
        key: key.into(),
        title: title.into(),
        kind: Kind::Scalar,
        json_type: jt.into(),
        ..FormNode::default()
    }
}

/// testModel mirrors examples/server.schema.json.
fn test_model(b: &mut TB) -> FormNodeId {
    let mut name = scalar("name", "Name", "string");
    name.required = true;
    let name = b.id(name);
    let mut mode = fnode("mode", "Mode", Kind::Enum);
    mode.enum_ = tenum(b, &["http", "grpc", "tcp"]);
    mode.default_ = Some(b.s("http"));
    let mode = b.id(mode);

    let mut port = scalar("port", "Port", "integer");
    port.required = true;
    port.min = Some(1024.0);
    port.max = Some(65535.0);
    port.default_ = Some(b.n("8080"));
    let port = b.id(port);
    let mut host = scalar("host", "Host", "string");
    host.default_ = Some(b.s("0.0.0.0"));
    let host = b.id(host);
    let mut debug = fnode("debug", "Debug", Kind::Boolean);
    debug.default_ = Some(b.b(false));
    let debug = b.id(debug);
    let ratio = b.id(scalar("ratio", "Ratio", "number"));
    let server = b.id(FormNode {
        key: "server".into(),
        title: "Server".into(),
        kind: Kind::Object,
        required: true,
        children: vec![port, host, debug, ratio],
        ..FormNode::default()
    });

    let mut levels_items = fnode("levels", "", Kind::Enum);
    levels_items.enum_ = tenum(b, &["debug", "info", "warn", "error"]);
    let levels_items = b.id(levels_items);
    let levels = b.id(FormNode {
        key: "levels".into(),
        title: "Levels".into(),
        kind: Kind::Array,
        min_items: 1,
        items: Some(levels_items),
        ..FormNode::default()
    });
    let tags_items = b.id(scalar("tags", "", "string"));
    let tags = b.id(FormNode {
        key: "tags".into(),
        title: "Tags".into(),
        kind: Kind::Array,
        items: Some(tags_items),
        ..FormNode::default()
    });
    let logging = b.id(FormNode {
        key: "logging".into(),
        title: "Logging".into(),
        kind: Kind::Object,
        children: vec![levels, tags],
        ..FormNode::default()
    });

    let mut src = scalar("src", "Source", "string");
    src.required = true;
    let src = b.id(src);
    let dst = b.id(scalar("dst", "Target", "string"));
    let mount = b.id(FormNode {
        key: "mounts".into(),
        title: "Mount".into(),
        kind: Kind::Object,
        children: vec![src, dst],
        ..FormNode::default()
    });
    let mounts = b.id(FormNode {
        key: "mounts".into(),
        title: "Mounts".into(),
        kind: Kind::Array,
        items: Some(mount),
        ..FormNode::default()
    });

    let mut none_v = fnode("auth", "None", Kind::Enum);
    none_v.enum_ = tenum(b, &["none"]);
    let none_v = b.id(none_v);
    let mut token = scalar("token", "Token", "string");
    token.required = true;
    let token = b.id(token);
    let token_v = b.id(FormNode {
        key: "auth".into(),
        title: "Token".into(),
        kind: Kind::Object,
        children: vec![token],
        ..FormNode::default()
    });
    let auth = b.id(FormNode {
        key: "auth".into(),
        title: "Auth".into(),
        kind: Kind::OneOf,
        variants: vec![none_v, token_v],
        ..FormNode::default()
    });

    b.id(FormNode {
        key: "$".into(),
        title: "Server Configuration".into(),
        kind: Kind::Object,
        children: vec![name, mode, server, logging, mounts, auth],
        ..FormNode::default()
    })
}

fn test_schema() -> Schema {
    let mut b = TB::new();
    let root = test_model(&mut b);
    b.root = root;
    b.finish()
}

fn test_doc() -> Doc {
    dbuild(|db| {
        db.obj(vec![
            ("name", db.s("webapp")),
            (
                "server",
                db.obj(vec![
                    ("port", db.n("8080")),
                    ("host", db.s("0.0.0.0")),
                    ("debug", db.b(true)),
                ]),
            ),
            (
                "mounts",
                db.arr(vec![
                    db.obj(vec![("src", db.s("/data")), ("dst", db.s("/mnt"))]),
                ]),
            ),
        ])
    })
}

/// A ready-to-use (schema, doc) pair over the test fixture.
struct Fx {
    schema: Schema,
    doc: Doc,
}

impl Fx {
    fn new() -> Fx {
        Fx {
            schema: test_schema(),
            doc: test_doc(),
        }
    }
    fn ctx(&self) -> MenuCtx<'_> {
        MenuCtx {
            schema: &self.schema,
            doc: &self.doc,
        }
    }
}

fn st(kind: MenuKind, node: FormNodeId, path: Vec<PathSeg>) -> MenuState {
    MenuState {
        kind,
        node,
        path,
        cursor: 0,
        offset: 0,
        variant_of: None,
        one_of: None,
        sel: Default::default(),
        visual: false,
        vanchor: 0,
        filter: Default::default(),
    }
}

fn root_state(schema: &Schema) -> MenuState {
    st(menu_kind_for(schema, schema.root), schema.root, Vec::new())
}

fn child_of(schema: &Schema, n: FormNodeId, key: &str) -> FormNodeId {
    schema.nodes[n as usize]
        .children
        .iter()
        .copied()
        .find(|c| schema.nodes[*c as usize].key == key)
        .unwrap_or_else(|| panic!("child {key:?} not found"))
}

/// menuRows over a stack whose top is `states.last()`; is_root mirrors
/// Go's `lm.stack[0] == st` check.
fn rows_for(fx: &mut Fx, states: Vec<MenuState>, is_root: bool) -> Vec<Row> {
    let top = states.last().unwrap();
    let mut env = RowsEnv::default();
    env.is_root = is_root;
    menu_rows_for(&mut fx.schema, &fx.doc, false, top, &mut env)
}

fn kinds(rows: &[Row]) -> Vec<RowKind> {
    rows.iter().map(|r| r.kind).collect()
}

fn paths(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| path_string(&r.path)).collect()
}

fn must_schema(src: &str) -> Schema {
    let dir = tempdir().unwrap();
    let p = dir.path().join("schema.json");
    std::fs::write(&p, src).unwrap();
    crate::smodel::load(p.to_str().unwrap()).unwrap()
}

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

// --- rows & stack --------------------------------------------------------

#[test]
fn test_menu_rows_root_order() {
    let mut fx = Fx::new();
    let root = fx.schema.root;
    let rows = rows_for(&mut fx, vec![st(MenuKind::Object, root, vec![])], true);
    let want = vec![
        RowKind::Field,
        RowKind::Field,
        RowKind::Drill,
        RowKind::Drill,
        RowKind::Drill,
        RowKind::Drill,
    ];
    assert_eq!(kinds(&rows), want, "root rows: {rows:?}");
    assert_eq!(fx.schema.nodes[rows[2].fn_.unwrap() as usize].key, "server");
    assert_eq!(fx.schema.nodes[rows[5].fn_.unwrap() as usize].key, "auth");
}

#[test]
fn test_menu_rows_duplicate_child_titles_fall_back_to_keys() {
    // Cargo.toml: dev/release/bench/test all $ref the same titled
    // definition; their rows must still name the actual profile keys.
    let schema = must_schema(
        r##"{
              "type": "object",
              "properties": {"profile": {"$ref": "#/definitions/Profiles"}},
              "definitions": {
                "Profile": {
                  "title": "Profile with Build Override",
                  "type": "object",
                  "properties": {"opt-level": {"type": "integer"}}
                },
                "Special": {
                  "title": "Special profile",
                  "type": "object",
                  "properties": {"x": {"type": "string"}}
                },
                "Profiles": {
                  "title": "Profiles",
                  "type": "object",
                  "properties": {
                    "bench": {"$ref": "#/definitions/Profile"},
                    "dev": {"$ref": "#/definitions/Profile"},
                    "release": {"$ref": "#/definitions/Profile"},
                    "test": {"$ref": "#/definitions/Profile"},
                    "special": {"$ref": "#/definitions/Special"},
                    "documented": {
                      "title": "Documented field",
                      "type": "object",
                      "properties": {"y": {"type": "string"}}
                    }
                  },
                  "additionalProperties": {"$ref": "#/definitions/Profile"}
                }
              }
            }"##,
    );
    let doc = dbuild(|db| db.obj(vec![("profile", db.obj(vec![]))]));
    let mut fx = Fx { schema, doc };
    let root_st = root_state(&fx.schema);
    let root_rows = rows_for(&mut fx, vec![root_st], true);
    let profile_row = root_rows
        .iter()
        .find(|r| path_string(&r.path) == "$.profile")
        .expect("profile row");
    assert_eq!(
        profile_row.label, "profile",
        "a bare $ref property must keep its key as the row label"
    );

    let profile = child_of(&fx.schema, fx.schema.root, "profile");
    let rows = rows_for(
        &mut fx,
        vec![st(MenuKind::Object, profile, vec![PathSeg::key("profile")])],
        false,
    );
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        ["bench", "dev", "release", "test", "special", ""],
        "ref-derived children use keys; an own title still wins: {rows:?}"
    );
}

#[test]
fn test_menu_rows_item_menus() {
    let mut fx = Fx::new();
    let root = fx.schema.root;
    let mounts = child_of(&fx.schema, root, "mounts");
    let levels = child_of(&fx.schema, child_of(&fx.schema, root, "logging"), "levels");
    let mounts_items = fx.schema.nodes[mounts as usize].items.unwrap();

    // array manager: add + one drill row per item, no save rows (not root)
    let rows = rows_for(
        &mut fx,
        vec![st(MenuKind::Items, mounts, vec![PathSeg::key("mounts")])],
        false,
    );
    assert_eq!(rows.len(), 1, "non-empty array: item rows only");
    assert_eq!(rows[0].kind, RowKind::Drill);
    assert_eq!(rows[0].idx, 0);

    // object item menu: fields only (D removes the item)
    let rows2 = rows_for(
        &mut fx,
        vec![st(
            MenuKind::Object,
            mounts_items,
            vec![PathSeg::key("mounts"), PathSeg::Idx(0)],
        )],
        false,
    );
    assert_eq!(rows2.len(), 2);
    assert_eq!(fx.schema.nodes[rows2[0].fn_.unwrap() as usize].key, "src");
    assert_eq!(fx.schema.nodes[rows2[1].fn_.unwrap() as usize].key, "dst");

    // scalar item menu: item rows only
    let tags = child_of(&fx.schema, child_of(&fx.schema, root, "logging"), "tags");
    let mut fx3 = Fx::new();
    fx3.doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("tags", db.arr(vec![db.s("prod"), db.s("dev")]))]),
        )])
    });
    let rows3 = rows_for(
        &mut fx3,
        vec![st(
            MenuKind::Items,
            tags,
            vec![PathSeg::key("logging"), PathSeg::key("tags")],
        )],
        false,
    );
    assert_eq!(rows3.len(), 2);
    assert_eq!(rows3[0].kind, RowKind::Field);
    assert_eq!(rows3[0].idx, 0);
    assert_eq!(rows3[1].idx, 1);

    // empty array: the append action is the only row
    let mut fx4 = Fx::new();
    fx4.doc = dbuild(|db| db.obj(vec![("logging", db.obj(vec![("tags", db.arr(vec![]))]))]));
    let rows_empty = rows_for(
        &mut fx4,
        vec![st(
            MenuKind::Items,
            tags,
            vec![PathSeg::key("logging"), PathSeg::key("tags")],
        )],
        false,
    );
    assert_eq!(rows_empty.len(), 1);
    assert_eq!(rows_empty[0].kind, RowKind::Add);

    // enum array: one toggle row per option
    let rows4 = rows_for(
        &mut fx,
        vec![st(
            MenuKind::EnumArray,
            levels,
            vec![PathSeg::key("logging"), PathSeg::key("levels")],
        )],
        false,
    );
    assert_eq!(rows4.len(), 4, "levels rows");
    for (i, r) in rows4.iter().enumerate() {
        assert_eq!(r.kind, RowKind::Toggle);
        assert_eq!(r.idx, i as i64);
    }
}

#[test]
fn test_menu_rows_one_of() {
    let mut fx = Fx::new();
    let root = fx.schema.root;
    let auth = child_of(&fx.schema, root, "auth");
    let rows = rows_for(
        &mut fx,
        vec![st(MenuKind::OneOf, auth, vec![PathSeg::key("auth")])],
        false,
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].kind, RowKind::Variant);
    assert_eq!(rows[1].kind, RowKind::Variant);
    let v0 = &fx.schema.nodes[rows[0].variant.unwrap() as usize];
    let v1 = &fx.schema.nodes[rows[1].variant.unwrap() as usize];
    assert_eq!(v0.title, "None");
    assert_eq!(v1.title, "Token");
}

#[test]
fn test_rebuild_stack() {
    let mut fx = Fx::new();
    let _root = fx.schema.root;

    let stx = rebuild_stack(&mut fx.schema, &fx.doc, &[PathSeg::key("server")]);
    assert_eq!(stx.len(), 2);
    assert_eq!(stx[1].kind, MenuKind::Object);
    assert_eq!(fx.schema.nodes[stx[1].node as usize].key, "server");

    // leaf field stops at the containing menu
    let stx = rebuild_stack(
        &mut fx.schema,
        &fx.doc,
        &[PathSeg::key("server"), PathSeg::key("port")],
    );
    assert_eq!(stx.len(), 2, "port stack depth");

    // array item menu
    let stx = rebuild_stack(
        &mut fx.schema,
        &fx.doc,
        &[PathSeg::key("mounts"), PathSeg::Idx(0), PathSeg::key("src")],
    );
    assert_eq!(stx.len(), 3);
    assert_eq!(stx[1].kind, MenuKind::Items);
    assert_eq!(stx[2].kind, MenuKind::Object);
    assert_eq!(fx.schema.nodes[stx[2].node as usize].key, "mounts");
    assert_eq!(stx[2].path[1], PathSeg::Idx(0), "item menu path");

    // scalar array item: row lives in the array menu
    let tags_doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("tags", db.arr(vec![db.s("x")]))]),
        )])
    });
    let stx = rebuild_stack(
        &mut fx.schema,
        &tags_doc,
        &[
            PathSeg::key("logging"),
            PathSeg::key("tags"),
            PathSeg::Idx(0),
        ],
    );
    assert_eq!(stx.len(), 3);
    assert_eq!(stx[2].kind, MenuKind::Items);

    // oneOf descent follows the current variant; an existing value edits
    // it directly (oneOf carried for the t type switch)
    let doc_auth = dbuild(|db| db.obj(vec![("auth", db.obj(vec![("token", db.s("x"))]))]));
    let stx = rebuild_stack(
        &mut fx.schema,
        &doc_auth,
        &[PathSeg::key("auth"), PathSeg::key("token")],
    );
    assert_eq!(stx.len(), 2);
    assert_eq!(fx.schema.nodes[stx[1].node as usize].title, "Token");
    assert!(stx[1].one_of.is_some());

    // unset oneOf cannot descend past the picker
    let stx = rebuild_stack(
        &mut fx.schema,
        &fx.doc,
        &[PathSeg::key("auth"), PathSeg::key("token")],
    );
    assert_eq!(stx.len(), 2);
    assert_eq!(stx[1].kind, MenuKind::OneOf);

    // unknown keys are skipped
    let stx = rebuild_stack(&mut fx.schema, &fx.doc, &[PathSeg::key("nope")]);
    assert_eq!(stx.len(), 1);
}

#[test]
fn test_search_index_and_filter() {
    let mut fx = Fx::new();
    let _root = fx.schema.root;
    let rst = root_state(&fx.schema);
    let all = search_index(&mut fx.schema, &fx.doc, &rst);
    let ctx = fx.ctx();
    let find = |title: &str| -> String {
        all.iter()
            .find(|r| r.fn_.map(|f| fx.schema.nodes[f as usize].title.as_str()) == Some(title))
            .map(|r| r.label.clone())
            .unwrap_or_default()
    };
    // labels are breadcrumb-style paths plus the menu value overview
    assert_eq!(find("Source"), "$.mounts[0].src  /data");
    assert_eq!(find("Server"), "$.server  port=8080 host=0.0.0.0 +1");
    // no phantom child rows below an absent oneOf parent
    assert_eq!(find("Token"), "");
    assert!(
        !all.iter()
            .any(|r| path_eq(&r.path, &[PathSeg::key("auth"), PathSeg::key("token")]))
    );
    assert!(
        all.iter().any(|r| {
            path_eq(&r.path, &[PathSeg::key("auth")])
                && r.kind == RowKind::Field
                && r.label.ends_with("—")
        }),
        "unset oneOf key row must be searchable"
    );
    for r in &all {
        assert!(
            !r.label.contains("variant "),
            "label leaks a fallback variant title: {:?}",
            r.label
        );
    }
    // filter by title and key, case-insensitive
    assert_eq!(filter_search(&ctx, &all, "token", true, None).len(), 0); // parent absent
    assert_eq!(filter_search(&ctx, &all, "SRC", true, None).len(), 1); // Source row only
    // fold=false demands the exact case
    assert_eq!(filter_search(&ctx, &all, "SRC", false, None).len(), 0);
    assert_eq!(filter_search(&ctx, &all, "src", false, None).len(), 1); // $.mounts[0].src
    assert_eq!(filter_search(&ctx, &all, "", true, None).len(), all.len());
    // a row matches with its own value: the container above it only
    // rolls its child values up, so the roll-up is no match target
    assert_eq!(filter_search(&ctx, &all, "8080", true, None).len(), 1); // $.server.port
    assert_eq!(filter_search(&ctx, &all, "port=8080", true, None).len(), 0);
    // values with a slash stay ordinary text: only a "$" prefix switches
    // to path mode
    assert_eq!(filter_search(&ctx, &all, "mnt", true, None).len(), 1); // $.mounts[0].dst
    assert_eq!(filter_search(&ctx, &all, "/mnt", true, None).len(), 1);

    // scope: searching from inside Server stays below the breadcrumb
    let mut inner_fx = Fx::new();
    let server = child_of(&inner_fx.schema, inner_fx.schema.root, "server");
    let inner_all = search_index(
        &mut inner_fx.schema,
        &inner_fx.doc,
        &st(MenuKind::Object, server, vec![PathSeg::key("server")]),
    );
    assert_eq!(inner_all.len(), 4, "scoped index"); // port, host, debug, ratio
    let inner_ctx = inner_fx.ctx();
    assert_eq!(
        filter_search(&inner_ctx, &inner_all, "name", true, None).len(),
        0,
        "scoped search must not leave the current menu"
    );
}

#[test]
fn test_search_index_null_object_keeps_menu_fields() {
    let mut schema = test_schema();
    let root = schema.root;
    let server = child_of(&schema, root, "server");
    // The object exists but is null (a freshly created placeholder).
    let doc = dbuild(|db| db.obj(vec![("server", db.push(Value::Null))]));
    let all = search_index(
        &mut schema,
        &doc,
        &st(MenuKind::Object, server, vec![PathSeg::key("server")]),
    );
    let has = |key: &str| {
        all.iter()
            .any(|r| path_eq(&r.path, &[PathSeg::key("server"), PathSeg::key(key)]))
    };
    for key in ["port", "host", "debug", "ratio"] {
        assert!(has(key), "null object must still list its menu field {key}");
    }
    let ctx = MenuCtx {
        schema: &schema,
        doc: &doc,
    };
    let hits = filter_search(&ctx, &all, "port", true, None);
    assert!(
        hits.iter()
            .any(|r| path_eq(&r.path, &[PathSeg::key("server"), PathSeg::key("port")])),
        "search must find the field of a null object"
    );
}

/// TestSearchDotQuery: a "."-prefixed query matches the keyword against
/// path segments from any node — the node itself first, its descendants
/// after — and both path queries fall back to values carrying the query
/// text (with its "$"/"." prefix) after their path hits.
#[test]
fn test_search_dot_query() {
    let schema = must_schema(
        r#"{
	  "type": "object",
	  "properties": {
	    "jobs": {"type": "object", "properties": {
	      "build_apple_app_store": {"type": "object", "properties": {
	        "runs_on": {"type": "string"},
	        "timeout": {"type": "number"}
	      }},
	      "lint": {"type": "object", "properties": {
	        "runs_on": {"type": "string"}
	      }}
	    }},
	    "notes": {"type": "string"}
	  }
	}"#,
    );
    let doc = dbuild(|db| {
        db.obj(vec![
            (
                "jobs",
                db.obj(vec![
                    (
                        "build_apple_app_store",
                        db.obj(vec![("runs_on", db.s("macos-14")), ("timeout", db.n("30"))]),
                    ),
                    ("lint", db.obj(vec![("runs_on", db.s("ubuntu-24.04"))])),
                ]),
            ),
            ("notes", db.s("release: see $.jobs.build_apple_app_store")),
        ])
    });
    let mut schema = schema;
    let rst = root_state(&schema);
    let all = search_index(&mut schema, &doc, &rst);
    let ctx = MenuCtx {
        schema: &schema,
        doc: &doc,
    };
    let join = |rows: &[Row]| -> String {
        rows.iter()
            .map(|r| path_string(&r.path))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let subtree = "$.jobs.build_apple_app_store $.jobs.build_apple_app_store.runs_on $.jobs.build_apple_app_store.timeout";

    // the node and its subtree, node first — the keyword need not be the
    // whole key ("build_app_store" lands on "build_apple_app_store");
    // the notes value carries no ".build_app_store" text, so no fallback
    assert_eq!(
        join(&filter_search(&ctx, &all, ".build_app_store", true, None)),
        subtree
    );
    // an exact key prefix hits the same rows plus the value fallback:
    // the notes value contains ".build_app"
    assert_eq!(
        join(&filter_search(&ctx, &all, ".build_app", true, None)),
        format!("{subtree} $.notes")
    );
    // dotted parts match a run of consecutive segments from any node
    assert_eq!(
        join(&filter_search(&ctx, &all, ".lint.runs_on", true, None)),
        "$.jobs.lint.runs_on"
    );
    // fold off demands the exact case
    assert_eq!(
        filter_search(&ctx, &all, ".BUILD_APP", false, None).len(),
        0
    );
    // regex mode keeps the tiered literal matcher on the path (the re
    // only feeds ordinary text queries), and the fallback stays a
    // literal containment: identical results to the literal query above
    let re = Regex::new(&format!("(?i){}", path_pattern(".build_app"))).unwrap();
    assert_eq!(
        join(&filter_search(&ctx, &all, ".build_app", true, Some(&re))),
        format!("{subtree} $.notes")
    );

    // "$" query: root-anchored path hits first (exact path on top), then
    // the value fallback — the notes value carries the queried path text
    let want = "$.jobs.build_apple_app_store $.jobs.build_apple_app_store.runs_on $.jobs.build_apple_app_store.timeout $.notes";
    assert_eq!(
        join(&filter_search(
            &ctx,
            &all,
            "$.jobs.build_apple_app_store",
            true,
            None
        )),
        want
    );
}

#[test]
fn test_search_index_variant_rows_only_without_value() {
    let key_row = |schema: &mut Schema, doc: &Doc| -> Row {
        let st = root_state(schema);
        search_index(schema, doc, &st)
            .into_iter()
            .find(|r| {
                r.fn_.map(|f| schema.nodes[f as usize].key.as_str()) == Some("auth")
                    && path_eq(&r.path, &[PathSeg::key("auth")])
            })
            .expect("auth key row")
    };

    // Unset oneOf: exactly one key row, no per-variant duplicates.
    let mut fx = Fx::new();
    let st = root_state(&fx.schema);
    let all = search_index(&mut fx.schema, &fx.doc, &st);
    assert!(
        all.iter()
            .filter(|r| r.kind == RowKind::Variant && path_eq(&r.path, &[PathSeg::key("auth")]))
            .count()
            == 0,
        "no variant rows for an unset key"
    );
    let row = key_row(&mut fx.schema, &fx.doc);
    assert!(row.label.ends_with("—"), "unset key label: {}", row.label);

    // Set oneOf: the key row stays searchable, still without duplicates.
    let mut fx2 = Fx::new();
    let tok = fx2.doc.new_str("secret");
    let obj = fx2.doc.new_object();
    if let Value::Object(m) = &mut fx2.doc.node_mut(obj).value {
        m.push(Member {
            key: "token".into(),
            val: tok,
        });
    }
    fx2.doc.set(&[PathSeg::key("auth")], obj).unwrap();
    let st = root_state(&fx2.schema);
    let all = search_index(&mut fx2.schema, &fx2.doc, &st);
    assert!(
        all.iter()
            .filter(|r| r.kind == RowKind::Variant && path_eq(&r.path, &[PathSeg::key("auth")]))
            .count()
            == 0,
        "no duplicate variant rows for a set key"
    );
    let row = key_row(&mut fx2.schema, &fx2.doc);
    assert_ne!(row.label, "");
}

#[test]
fn test_search_index_only_matched_variant_fields() {
    // an array of discriminated variants (sing-box inbounds): each
    // item's current type defines its fields
    let mut b = TB::new();
    let mut direct_type = fnode("type", "Type", Kind::Enum);
    direct_type.enum_ = tenum(&mut b, &["direct"]);
    direct_type.required = true;
    let direct_type = b.id(direct_type);
    let listen_port = b.id(scalar("listen_port", "Listen port", "string"));
    let direct = b.id(FormNode {
        key: "inbounds".into(),
        title: "Direct".into(),
        kind: Kind::Object,
        children: vec![direct_type, listen_port],
        ..FormNode::default()
    });
    let mut tun_type = fnode("type", "Type", Kind::Enum);
    tun_type.enum_ = tenum(&mut b, &["tun"]);
    tun_type.required = true;
    let tun_type = b.id(tun_type);
    let ip_scalar = b.id(scalar("include_package", "Include package", "string"));
    let ip_item = b.id(scalar("include_package", "Include package", "string"));
    let ip_array = b.id(FormNode {
        key: "include_package".into(),
        title: "Include package".into(),
        kind: Kind::Array,
        items: Some(ip_item),
        ..FormNode::default()
    });
    let include_package = b.id(FormNode {
        key: "include_package".into(),
        title: "Include package".into(),
        kind: Kind::OneOf,
        variants: vec![ip_scalar, ip_array],
        ..FormNode::default()
    });
    let tun = b.id(FormNode {
        key: "inbounds".into(),
        title: "Tun".into(),
        kind: Kind::Object,
        children: vec![tun_type, include_package],
        ..FormNode::default()
    });
    let inbounds_items = b.id(FormNode {
        key: "inbounds".into(),
        kind: Kind::OneOf,
        variants: vec![direct, tun],
        ..FormNode::default()
    });
    let inbounds = b.id(FormNode {
        key: "inbounds".into(),
        title: "Inbounds".into(),
        kind: Kind::Array,
        items: Some(inbounds_items),
        ..FormNode::default()
    });
    let proot = b.id(FormNode {
        key: "$".into(),
        kind: Kind::Object,
        children: vec![inbounds],
        ..FormNode::default()
    });
    b.root = proot;
    let mut schema = b.finish();

    let doc = dbuild(|db| {
        db.obj(vec![(
            "inbounds",
            db.arr(vec![
                db.obj(vec![
                    ("type", db.s("direct")),
                    ("listen_port", db.s("1080")),
                ]),
                db.obj(vec![
                    ("type", db.s("tun")),
                    ("include_package", db.arr(vec![db.s("com.foo")])),
                ]),
            ]),
        )])
    });

    let rst = root_state(&schema);
    let all = search_index(&mut schema, &doc, &rst);
    let foreign = all
        .iter()
        .filter(|r| {
            path_eq(
                &r.path,
                &[
                    PathSeg::key("inbounds"),
                    PathSeg::Idx(0),
                    PathSeg::key("include_package"),
                ],
            )
        })
        .count();
    let own = all
        .iter()
        .filter(|r| {
            path_eq(
                &r.path,
                &[
                    PathSeg::key("inbounds"),
                    PathSeg::Idx(1),
                    PathSeg::key("include_package"),
                ],
            )
        })
        .count();
    assert_eq!(foreign, 0, "direct inbound indexed tun fields");
    assert_eq!(own, 1, "tun include_package rows");

    // the direct inbound's own field survives the restriction
    let ctx = MenuCtx {
        schema: &schema,
        doc: &doc,
    };
    let hits = filter_search(&ctx, &all, "listen_port", true, None);
    assert_eq!(hits.len(), 1);
    assert!(path_eq(
        &hits[0].path,
        &[
            PathSeg::key("inbounds"),
            PathSeg::Idx(0),
            PathSeg::key("listen_port")
        ]
    ));
}

#[test]
fn test_search_rank_and_path_queries() {
    let mut fx = Fx::new();
    let rst = root_state(&fx.schema);
    let all = search_index(&mut fx.schema, &fx.doc, &rst);
    let ctx = fx.ctx();
    let eq = |got: &[String], want: &[&str]| {
        let w: Vec<String> = want.iter().map(|s| s.to_string()).collect();
        assert_eq!(*got, w, "paths");
    };

    // exact-key priority: the row whose key IS the query outranks rows
    // that merely mention it
    let got = filter_search(&ctx, &all, "port", true, None);
    assert!(
        !got.is_empty() && paths(&got)[0] == "$.server.port",
        "port first hit = {:?}",
        paths(&got)
    );
    let got = filter_search(&ctx, &all, "src", true, None);
    assert!(
        !got.is_empty() && paths(&got)[0] == "$.mounts[0].src",
        "src first hit"
    );
    // a value only its own row carries matches that row alone
    let got = filter_search(&ctx, &all, "8080", true, None);
    assert_eq!(got.len(), 1);
    assert_eq!(paths(&got)[0], "$.server.port");
    // the tier is stable: without an exact key the order is unchanged
    eq(
        &paths(&filter_search(&ctx, &all, "0", true, None)),
        &["$.server.port", "$.server.host"],
    );

    // path queries: "$"-prefixed, literal prefix match from the root
    eq(
        &paths(&filter_search(&ctx, &all, "$.server.port", true, None)),
        &["$.server.port"],
    );
    let anch = filter_search(&ctx, &all, "$.server", true, None);
    for p in paths(&anch) {
        assert!(
            p.starts_with("$.server"),
            "path hit {p:?} lacks the $.server prefix"
        );
    }
    assert_eq!(paths(&anch)[0], "$.server"); // exact full path ranks first
    assert_eq!(
        paths(&filter_search(&ctx, &all, "$.serv", true, None))[0],
        "$.server"
    );
    // array indexes use the rendered [N] syntax
    eq(
        &paths(&filter_search(&ctx, &all, "$.mounts[0]", true, None)),
        &["$.mounts[0]", "$.mounts[0].src", "$.mounts[0].dst"],
    );
    eq(
        &paths(&filter_search(&ctx, &all, "$.mounts[0].src", true, None)),
        &["$.mounts[0].src"],
    );

    // dotted and slashed values remain ordinary text queries
    assert_eq!(filter_search(&ctx, &all, "0.0.0.0", true, None).len(), 1);
    assert_eq!(filter_search(&ctx, &all, "/mnt", true, None).len(), 1);
}

// --- search & replace helpers ----------------------------------------------

#[test]
fn test_filter_search_regex() {
    let mut fx = Fx::new();
    let rst = root_state(&fx.schema);
    let all = search_index(&mut fx.schema, &fx.doc, &rst);
    let eq = |got: &[String], want: &[&str]| {
        let w: Vec<String> = want.iter().map(|s| s.to_string()).collect();
        assert_eq!(*got, w, "paths");
    };
    let matchq = |fx: &Fx, all: &[Row], q: &str| -> Vec<String> {
        let pat = if is_path_query(q) {
            path_pattern(q)
        } else {
            q.to_string()
        };
        let re = Regex::new(&pat).unwrap();
        let ctx = fx.ctx();
        paths(&filter_search(&ctx, all, q, false, Some(&re)))
    };

    // the regex targets the same text literal mode matches: title/key
    // plus the row's own scalar value; a full leaf-key match ranks first
    eq(&matchq(&fx, &all, r"(?i)token"), &[]); // parent is absent
    eq(&matchq(&fx, &all, r"\bport\b"), &["$.server.port"]); // $.server's roll-up is no target
    eq(&matchq(&fx, &all, r"0\.0\.0\.0"), &["$.server.host"]);
    // a "$."-prefixed query targets the document path, regex included
    eq(&matchq(&fx, &all, "$.server.port"), &["$.server.port"]);
    eq(
        &matchq(&fx, &all, "$.server"),
        &[
            "$.server",
            "$.server.port",
            "$.server.host",
            "$.server.debug",
            "$.server.ratio",
        ],
    );
    // [N] index brackets in a pasted path stay literal in regex mode
    eq(&matchq(&fx, &all, "$.mounts[0].src"), &["$.mounts[0].src"]);
    eq(
        &matchq(&fx, &all, r"$.mounts\[0\].src"),
        &["$.mounts[0].src"],
    ); // explicit escape
    // the pattern matches original-case text: folding is the caller's (?i)
    let re = Regex::new(r"(?i)SRC").unwrap();
    eq(
        &paths(&filter_search(
            &fx.ctx(),
            &all,
            r"(?i)SRC",
            false,
            Some(&re),
        )),
        &["$.mounts[0].src"],
    );
    assert_eq!(matchq(&fx, &all, "SRC").len(), 0, "exact-case SRC");
    // a pattern matching only a container roll-up finds nothing
    assert_eq!(matchq(&fx, &all, "port=8080").len(), 0);
}

#[test]
fn test_replace_all() {
    let cases: Vec<(&str, &str, &str, bool, &str, usize)> = vec![
        ("Hello world HELLO", "hello", "hi", true, "hi world hi", 2),
        ("Hello", "hello", "x", true, "x", 1),
        ("Hello", "hello", "x", false, "Hello", 0),
        ("Grösse GRÖSSE", "grösse", "S", true, "S S", 2),
        ("Größe", "GRÖSSE", "X", true, "Größe", 0), // simple fold: ß is not ss
        ("aaa", "aa", "b", true, "ba", 1),          // non-overlapping scan
        ("ab", "a", "aa", true, "aab", 1),          // rep containing old: no rescan
        ("abc", "", "x", true, "abc", 0),           // empty old
        ("abc", "z", "x", false, "abc", 0),         // no match
        ("a§b A§B", "§", "§§", true, "a§§b A§§B", 2), // multibyte-safe
    ];
    for (s, old, rep, fold, want, want_n) in cases {
        let (got, n) = replace_all(s, old, rep, fold, None);
        assert_eq!(
            (got.as_str(), n),
            (want, want_n),
            "replaceAll({s:?}, {old:?}, {rep:?}, {fold})"
        );
    }
}

#[test]
fn test_replace_all_regex() {
    let cases: Vec<(&str, &str, &str, &str, usize)> = vec![
        ("hello", "l+", "x", "hexo", 1),
        ("foo1bar2", "[0-9]+", "#", "foo#bar#", 2),
        ("abc123", "(b)(c)", "[$2$1]", "a[cb]123", 1), // $N groups expand
        ("HELLO hello", "(?i)hello", "x", "x x", 2),   // folding is the caller's (?i)
        ("bbb", "a*", "x", "xbxbxbx", 4),              // empty match at each boundary
    ];
    for (s, pat, rep, want, want_n) in cases {
        let re = Regex::new(pat).unwrap();
        let (got, n) = replace_all(s, pat, rep, false, Some(&re));
        assert_eq!(
            (got.as_str(), n),
            (want, want_n),
            "replaceAll({s:?}, {pat:?}, regexp)"
        );
    }
}

#[test]
fn test_replace_hits() {
    let mut fx = Fx::new();
    let rst = root_state(&fx.schema);
    let all = search_index(&mut fx.schema, &fx.doc, &rst);
    let ctx = fx.ctx();

    // non-empty query: only string values containing q, relabeled
    let hits = replace_hits(&ctx, &all, "/data", "/opt", true, None);
    assert_eq!(hits.len(), 1, "hits");
    assert_eq!(hits[0].path.len(), 3, "hit path");
    assert_eq!(hits[0].label, "$.mounts[0].src  /data → /opt");

    // fold=false demands the exact case
    assert_eq!(
        replace_hits(&ctx, &all, "/DATA", "/opt", false, None).len(),
        0
    );
    assert_eq!(
        replace_hits(&ctx, &all, "/DATA", "/opt", true, None).len(),
        1
    );

    // empty query: every string value, original labels, numbers excluded
    let strs = replace_hits(&ctx, &all, "", "", true, None);
    assert_eq!(strs.len(), 4); // name, server/host, mounts/0/src, mounts/0/dst
    for r in &strs {
        if let Some(f) = r.fn_ {
            assert_ne!(
                fx.schema.nodes[f as usize].key, "port",
                "the number port must not be a replace hit"
            );
        }
        assert!(
            !r.label.is_empty(),
            "empty-query hits keep their original labels"
        );
    }

    // duplicate paths (oneOf variants walking the same location) are
    // kept once
    let mut b = TB::new();
    let va = b.id(scalar("auth", "A", "string"));
    let vb = b.id(scalar("auth", "B", "string"));
    let auth = b.id(FormNode {
        key: "auth".into(),
        title: "Auth".into(),
        kind: Kind::OneOf,
        variants: vec![va, vb],
        ..FormNode::default()
    });
    let proot = b.id(FormNode {
        key: "$".into(),
        title: "$".into(),
        kind: Kind::Object,
        children: vec![auth],
        ..FormNode::default()
    });
    b.root = proot;
    let mut fx2 = Fx {
        schema: b.finish(),
        doc: dbuild(|db| db.obj(vec![("auth", db.s("secret"))])),
    };
    let rst = root_state(&fx2.schema);
    let all2 = search_index(&mut fx2.schema, &fx2.doc, &rst);
    let ctx2 = fx2.ctx();
    let got = replace_hits(&ctx2, &all2, "secret", "x", true, None);
    assert_eq!(got.len(), 1, "duplicate variant paths");
}

#[test]
fn test_search_index_unknown_keys() {
    // schema-less document keys (with nested values) are indexed,
    // matched and offered to replace like any other row
    let mut fx = Fx::new();
    {
        let note = fx.doc.new_str("alpha");
        let beta = fx.doc.new_str("beta");
        let tags = fx.doc.new_array();
        fx.doc.node_mut(tags).value = Value::Array(vec![beta]);
        let extra = fx.doc.new_object();
        fx.doc.node_mut(extra).value = Value::Object(vec![
            Member {
                key: "note".into(),
                val: note,
            },
            Member {
                key: "tags".into(),
                val: tags,
            },
        ]);
        fx.doc.set(&[PathSeg::key("extra")], extra).unwrap();
        let legacy = fx.doc.new_str("gamma");
        fx.doc
            .set(&[PathSeg::key("server"), PathSeg::key("legacy")], legacy)
            .unwrap();
    }
    let rst = root_state(&fx.schema);
    let all = search_index(&mut fx.schema, &fx.doc, &rst);
    let ctx = fx.ctx();

    let labels: std::collections::HashSet<&str> = all
        .iter()
        .filter(|r| r.kind == RowKind::Unknown)
        .map(|r| r.label.as_str())
        .collect();
    for want in [
        "$.extra.note  alpha",
        "$.extra.tags[0]  beta",
        "$.server.legacy  gamma",
    ] {
        assert!(
            labels.contains(want),
            "unknown row {want:?} missing from index: {labels:?}"
        );
    }

    // search matches the unknown key name and its value overview
    for q in ["legacy", "gamma", "extra"] {
        assert!(
            !filter_search(&ctx, &all, q, true, None).is_empty(),
            "filter {q:?} found no unknown row"
        );
    }

    // replace previews target unknown string values, nested included
    let hits = replace_hits(&ctx, &all, "a", "A", true, None);
    let hit_paths: std::collections::HashSet<String> =
        hits.iter().map(|h| path_string(&h.path)).collect();
    for want in ["$.extra.note", "$.extra.tags[0]", "$.server.legacy"] {
        assert!(
            hit_paths.contains(want),
            "replace hit {want:?} missing: {hit_paths:?}"
        );
    }
}

#[test]
fn test_search_index_container_variant_elements() {
    // `anyOf: [string, array[string]]` is one value with two shapes: the
    // matched array variant must be walked as the value itself, so its
    // elements stay searchable (sing-box route.rules[].rule_set)
    let schema_src = r#"{
          "type": "object",
          "properties": {
            "rule_set": {"anyOf": [
              {"type": "string"},
              {"type": "array", "items": {"type": "string"}}
            ]}
          }
        }"#;
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("schema.json");
    std::fs::write(&p, schema_src).expect("schema write");
    let mut schema = crate::smodel::load(p.to_str().expect("path")).expect("schema load");
    let doc = dbuild(|db| {
        db.obj(vec![(
            "rule_set",
            db.arr(vec![db.s("ruleset-proxy-ai"), db.s("ruleset-proxy-github")]),
        )])
    });
    let st = root_state(&schema);
    let all = search_index(&mut schema, &doc, &st);
    let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();
    assert!(
        labels.contains(&"$.rule_set[0]  ruleset-proxy-ai")
            && labels.contains(&"$.rule_set[1]  ruleset-proxy-github"),
        "array variant elements missing: {labels:?}"
    );
    let ctx = MenuCtx {
        schema: &schema,
        doc: &doc,
    };
    assert_eq!(
        filter_search(&ctx, &all, "ruleset-proxy-github", true, None).len(),
        1,
        "the element value must match"
    );
}

#[test]
fn test_search_index_array_elements() {
    // every document element is searchable, whatever shape its schema
    // has: enum items, missing items, nested arrays (degraded to Raw) and
    // plain object items
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("schema.json");
    std::fs::write(
            &p,
            r#"{
              "type": "object",
              "properties": {
                "levels": {"type": "array", "items": {"type": "string", "enum": ["debug", "info"]}},
                "free": {"type": "array"},
                "matrix": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}},
                "mounts": {"type": "array", "items": {"type": "object", "properties": {"src": {"type": "string"}}}}
              }
            }"#,
        )
        .expect("schema write");
    let mut schema = crate::smodel::load(p.to_str().expect("path")).expect("schema load");
    let doc = dbuild(|db| {
        db.obj(vec![
            ("levels", db.arr(vec![db.s("debug"), db.s("info")])),
            (
                "free",
                db.arr(vec![db.s("alpha"), db.obj(vec![("k", db.n("7"))])]),
            ),
            ("matrix", db.arr(vec![db.arr(vec![db.n("1"), db.n("2")])])),
            ("mounts", db.arr(vec![db.obj(vec![("src", db.s("/data"))])])),
        ])
    });
    let st = root_state(&schema);
    let all = search_index(&mut schema, &doc, &st);
    let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();

    // enum items are document values, not schema-only options
    assert!(
        labels.contains(&"$.levels[0]  debug") && labels.contains(&"$.levels[1]  info"),
        "enum array elements missing: {labels:?}"
    );
    // no items schema: element rows plus nested values, indexed raw
    assert!(
        labels.contains(&"$.free[0]  alpha")
            && labels.contains(&"$.free[1]  k=7")
            && labels.contains(&"$.free[1].k  7"),
        "schema-less array elements missing: {labels:?}"
    );
    // nested arrays degrade to Raw; the inner elements stay searchable
    assert!(
        labels.contains(&"$.matrix[0]  2 items")
            && labels.contains(&"$.matrix[0][0]  1")
            && labels.contains(&"$.matrix[0][1]  2"),
        "nested array elements missing: {labels:?}"
    );
    // object items keep their per-field rows
    assert!(
        labels.contains(&"$.mounts[0].src  /data"),
        "object item fields missing: {labels:?}"
    );

    // the new rows are ordinary text matches
    let ctx = MenuCtx {
        schema: &schema,
        doc: &doc,
    };
    for q in ["info", "alpha", "7", "/data"] {
        assert!(
            !filter_search(&ctx, &all, q, true, None).is_empty(),
            "filter {q:?} found no array element row"
        );
    }
}

#[test]
fn test_search_index_raw_value_subtrees() {
    // a node the form layer degrades to Raw is edited as one blob, but
    // its nested values must still be searchable: object children and
    // map values include everything below the raw node
    let mut b = TB::new();
    let extra = b.id(fnode("extra", "Extra", Kind::Raw));
    let env = b.id(FormNode {
        key: "env".into(),
        title: "Env".into(),
        kind: Kind::Map,
        ..FormNode::default()
    });
    let root = b.id(FormNode {
        key: "$".into(),
        title: "$".into(),
        kind: Kind::Object,
        children: vec![extra, env],
        ..FormNode::default()
    });
    b.root = root;
    let mut schema = b.finish();
    let doc = dbuild(|db| {
        db.obj(vec![
            (
                "extra",
                db.obj(vec![
                    ("note", db.s("needle")),
                    ("items", db.arr(vec![db.obj(vec![("name", db.s("deep"))])])),
                ]),
            ),
            (
                "env",
                db.obj(vec![("A", db.obj(vec![("deep", db.s("hidden"))]))]),
            ),
        ])
    });
    let st = root_state(&schema);
    let all = search_index(&mut schema, &doc, &st);
    let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();
    assert!(
        labels.contains(&"$.extra.note  needle") && labels.contains(&"$.extra.items[0].name  deep"),
        "raw child subtree missing: {labels:?}"
    );
    assert!(
        labels.contains(&"$.env.A.deep  hidden"),
        "raw map value subtree missing: {labels:?}"
    );
    let ctx = MenuCtx {
        schema: &schema,
        doc: &doc,
    };
    for q in ["needle", "deep", "hidden"] {
        assert!(
            !filter_search(&ctx, &all, q, true, None).is_empty(),
            "filter {q:?} found no raw subtree row"
        );
    }
}

#[test]
fn test_search_index_recursive_raw_field() {
    // a recursive $ref degrades the inner field to Raw ("edit as JSON");
    // values inside that raw subtree must still be searchable
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("schema.json");
    std::fs::write(
        &p,
        r##"{
              "type": "object",
              "properties": {"node": {"$ref": "#/$defs/Node"}},
              "$defs": {
                "Node": {
                  "type": "object",
                  "properties": {
                    "name": {"type": "string"},
                    "child": {"$ref": "#/$defs/Node"}
                  }
                }
              }
            }"##,
    )
    .expect("schema write");
    let mut schema = crate::smodel::load(p.to_str().expect("path")).expect("schema load");
    let doc = dbuild(|db| {
        db.obj(vec![(
            "node",
            db.obj(vec![
                ("name", db.s("root")),
                ("child", db.obj(vec![("name", db.s("needle"))])),
            ]),
        )])
    });
    let st = root_state(&schema);
    let all = search_index(&mut schema, &doc, &st);
    let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();
    assert!(
        labels.contains(&"$.node.child.name  needle"),
        "recursive raw subtree missing: {labels:?}"
    );
}

#[test]
fn test_search_index_multi_match_array_variants() {
    // two array variants both match the value (a tie): the union has no
    // object fields to merge, so the elements must still be indexed
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("schema.json");
    std::fs::write(
        &p,
        r#"{
              "type": "object",
              "properties": {
                "ids": {"anyOf": [
                  {"type": "array", "items": {"type": "string"}},
                  {"type": "array", "items": {"type": "string"}, "minItems": 1}
                ]}
              }
            }"#,
    )
    .expect("schema write");
    let mut schema = crate::smodel::load(p.to_str().expect("path")).expect("schema load");
    let doc = dbuild(|db| db.obj(vec![("ids", db.arr(vec![db.s("x"), db.s("y")]))]));
    let st = root_state(&schema);
    let all = search_index(&mut schema, &doc, &st);
    let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();
    assert!(
        labels.contains(&"$.ids[0]  x") && labels.contains(&"$.ids[1]  y"),
        "multi-match array variant elements missing: {labels:?}"
    );
}

#[test]
fn test_rebuild_stack_schema_less() {
    // The runtime menus Enter opens for unknown values are the ones a
    // jump must rebuild, or a schema-less search hit lands only on the
    // top-level key.
    let doc = dbuild(|db| {
        db.obj(vec![
            ("server", db.obj(vec![("port", db.n("8080"))])),
            ("tags", db.arr(vec![db.s("alpha"), db.s("beta")])),
            ("arr", db.arr(vec![db.obj(vec![("name", db.s("deep"))])])),
        ])
    });
    let mut schema = crate::smodel::untyped(&doc);

    // object member: jump target is the member row in the value's menu
    let stx = rebuild_stack(
        &mut schema,
        &doc,
        &[PathSeg::key("server"), PathSeg::key("port")],
    );
    assert_eq!(stx.len(), 2);
    assert_eq!(stx[1].kind, MenuKind::Object);
    assert_eq!(stx[1].path, vec![PathSeg::key("server")]);
    assert_eq!(schema.nodes[stx[1].node as usize].kind, Kind::Object);

    // array item: the runtime items menu
    let stx = rebuild_stack(&mut schema, &doc, &[PathSeg::key("tags"), PathSeg::Idx(1)]);
    assert_eq!(stx.len(), 2);
    assert_eq!(stx[1].kind, MenuKind::Items);
    assert_eq!(stx[1].path, vec![PathSeg::key("tags")]);

    // object nested in an array item: item menu, then its field menu
    let stx = rebuild_stack(
        &mut schema,
        &doc,
        &[PathSeg::key("arr"), PathSeg::Idx(0), PathSeg::key("name")],
    );
    assert_eq!(stx.len(), 3);
    assert_eq!(stx[2].kind, MenuKind::Object);
    assert_eq!(stx[2].path, vec![PathSeg::key("arr"), PathSeg::Idx(0)]);
}

/// Map rows edit against the value schema, so the schema node's key is
/// the map's own key ("dependencies"), not the document member's. The
/// member key must still be searchable (Cargo.toml `ratatui`).
#[test]
fn test_search_map_member_key() {
    let mut schema = must_schema(
        r#"{
              "type": "object",
              "properties": {
                "dependencies": {
                  "type": "object",
                  "additionalProperties": {
                    "oneOf": [
                      {"type": "string"},
                      {"type": "object", "properties": {"version": {"type": "string"}}}
                    ]
                  }
                }
              }
            }"#,
    );
    let doc = dbuild(|db| {
        db.obj(vec![(
            "dependencies",
            db.obj(vec![("ratatui", db.s("=0.30.2"))]),
        )])
    });
    let rst = root_state(&schema);
    let all = search_index(&mut schema, &doc, &rst);
    let ctx = MenuCtx {
        schema: &schema,
        doc: &doc,
    };
    let hits = filter_search(&ctx, &all, "ratatui", true, None);
    assert_eq!(
        hits.iter().map(|r| r.label.clone()).collect::<Vec<_>>(),
        ["$.dependencies.ratatui  =0.30.2"],
        "map member key must be a text-search target"
    );
    assert_eq!(
        filter_search(&ctx, &all, "RATATUI", false, None).len(),
        0,
        "case-sensitive search must not match a differently cased key"
    );
}

#[test]
fn test_search_multi_pattern_keys_in_schema() {
    let schema = must_schema(
        r##"{
              "type": "object",
              "properties": {
                "jobs": {
                  "type": "object",
                  "patternProperties": {
                    "^a": {"type": "string"},
                    "^b": {"type": "integer"}
                  },
                  "additionalProperties": false
                }
              }
            }"##,
    );
    let doc = dbuild(|db| {
        db.obj(vec![(
            "jobs",
            db.obj(vec![
                ("a1", db.obj(vec![("nested", db.s("x"))])),
                ("b1", db.n("2")),
                ("c1", db.b(true)),
            ]),
        )])
    });
    let jobs = child_of(&schema, schema.root, "jobs");
    let mut schema = schema;
    let st = st(MenuKind::Object, jobs, vec![PathSeg::key("jobs")]);
    let all = search_index(&mut schema, &doc, &st);
    let find = |key: &str| {
        all.iter()
            .find(|r| r.path == vec![PathSeg::key("jobs"), PathSeg::key(key)])
            .unwrap_or_else(|| panic!("{key} search row missing"))
    };
    assert!(find("a1").in_schema);
    assert!(find("b1").in_schema);
    assert!(!find("c1").in_schema);
    let nested = all
        .iter()
        .find(|r| {
            r.path
                == vec![
                    PathSeg::key("jobs"),
                    PathSeg::key("a1"),
                    PathSeg::key("nested"),
                ]
        })
        .expect("nested row");
    assert!(
        nested.in_schema,
        "rows below a pattern-defined key inherit its schema standing"
    );
}

#[test]
fn test_search_open_additional_keys_in_schema() {
    let schema = must_schema(
        r#"{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":{"type":"string"}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("a", db.s("x")), ("extra", db.s("y"))]));
    let mut schema = schema;
    let st = root_state(&schema);
    let all = search_index(&mut schema, &doc, &st);
    let row = all
        .iter()
        .find(|r| r.path == vec![PathSeg::key("extra")])
        .expect("extra search row");
    assert!(
        row.in_schema,
        "keys of an open additionalProperties object are schema-defined"
    );
}
