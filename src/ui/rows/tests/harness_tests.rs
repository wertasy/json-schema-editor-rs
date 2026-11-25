//! Shared rows-test harness: schema/doc fixtures and doc accessors.

use super::*;

// --- fixtures -----------------------------------------------------------

/// Builds a Schema arena plus the Doc arena its enum/default values
/// point into (the Go tests build loose *FormNode graphs).
pub(super) struct TB {
    pub(super) nodes: Vec<FormNode>,
    pub(super) doc: Doc,
    pub(super) root: FormNodeId,
}

impl TB {
    pub(super) fn new() -> TB {
        TB {
            nodes: Vec::new(),
            doc: Doc::new(),
            root: 0,
        }
    }
    pub(super) fn id(&mut self, n: FormNode) -> FormNodeId {
        let id = self.nodes.len() as FormNodeId;
        self.nodes.push(n);
        id
    }
    pub(super) fn s(&mut self, v: &str) -> NodeId {
        self.doc.new_str(v)
    }
    pub(super) fn n(&mut self, v: &str) -> NodeId {
        self.doc.new_num(v)
    }
    pub(super) fn b(&mut self, v: bool) -> NodeId {
        self.doc.new_bool(v)
    }
    pub(super) fn finish(self) -> Schema {
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
pub(super) struct DB {
    pub(super) nodes: std::rc::Rc<std::cell::RefCell<Vec<NodeData>>>,
}

impl DB {
    pub(super) fn new() -> DB {
        DB {
            nodes: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
        }
    }
    pub(super) fn push(&self, v: Value) -> NodeId {
        let mut n = self.nodes.borrow_mut();
        let id = n.len() as NodeId;
        n.push(NodeData {
            value: v,
            ..NodeData::default()
        });
        id
    }
    pub(super) fn s(&self, v: &str) -> NodeId {
        self.push(Value::Str(v.into()))
    }
    pub(super) fn n(&self, v: &str) -> NodeId {
        self.push(Value::Num(v.into()))
    }
    pub(super) fn b(&self, v: bool) -> NodeId {
        self.push(Value::Bool(v))
    }
    pub(super) fn obj(&self, ms: Vec<(&str, NodeId)>) -> NodeId {
        self.push(Value::Object(
            ms.into_iter()
                .map(|(k, v)| Member {
                    key: k.into(),
                    val: v,
                })
                .collect(),
        ))
    }
    pub(super) fn arr(&self, xs: Vec<NodeId>) -> NodeId {
        self.push(Value::Array(xs))
    }
    pub(super) fn finish(self, root: NodeId) -> Doc {
        Doc {
            nodes: self.nodes.borrow().clone(),
            root,
        }
    }
}

/// dbuild builds a whole document in one closure.
pub(super) fn dbuild(f: impl FnOnce(&DB) -> NodeId) -> Doc {
    let db = DB::new();
    let root = f(&db);
    db.finish(root)
}

pub(super) fn tenum(b: &mut TB, vals: &[&str]) -> Vec<EnumOpt> {
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

pub(super) fn fnode(key: &str, title: &str, kind: Kind) -> FormNode {
    FormNode {
        key: key.into(),
        title: title.into(),
        kind,
        ..FormNode::default()
    }
}

pub(super) fn scalar(key: &str, title: &str, jt: &str) -> FormNode {
    FormNode {
        key: key.into(),
        title: title.into(),
        kind: Kind::Scalar,
        json_type: jt.into(),
        ..FormNode::default()
    }
}

/// testModel mirrors examples/server.schema.json.
pub(super) fn test_model(b: &mut TB) -> FormNodeId {
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

pub(super) fn test_schema() -> Schema {
    let mut b = TB::new();
    let root = test_model(&mut b);
    b.root = root;
    b.finish()
}

pub(super) fn test_doc() -> Doc {
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
pub(super) struct Fx {
    pub(super) schema: Schema,
    pub(super) doc: Doc,
}

impl Fx {
    pub(super) fn new() -> Fx {
        Fx {
            schema: test_schema(),
            doc: test_doc(),
        }
    }
    pub(super) fn ctx(&self) -> MenuCtx<'_> {
        MenuCtx {
            schema: &self.schema,
            doc: &self.doc,
        }
    }
}

pub(super) fn st(kind: MenuKind, node: FormNodeId, path: Vec<PathSeg>) -> MenuState {
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

pub(super) fn root_state(schema: &Schema) -> MenuState {
    st(menu_kind_for(schema, schema.root), schema.root, Vec::new())
}

pub(super) fn child_of(schema: &Schema, n: FormNodeId, key: &str) -> FormNodeId {
    schema.nodes[n as usize]
        .children
        .iter()
        .copied()
        .find(|c| schema.nodes[*c as usize].key == key)
        .unwrap_or_else(|| panic!("child {key:?} not found"))
}

/// menuRows over a stack whose top is `states.last()`; is_root mirrors
/// Go's `lm.stack[0] == st` check.
pub(super) fn rows_for(fx: &mut Fx, states: Vec<MenuState>, is_root: bool) -> Vec<Row> {
    let top = states.last().unwrap();
    let mut env = RowsEnv::default();
    env.is_root = is_root;
    menu_rows_for(&mut fx.schema, &fx.doc, false, top, &mut env)
}

pub(super) fn kinds(rows: &[Row]) -> Vec<RowKind> {
    rows.iter().map(|r| r.kind).collect()
}

pub(super) fn paths(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| path_string(&r.path)).collect()
}

pub(super) fn must_schema(src: &str) -> Schema {
    let dir = tempdir().unwrap();
    let p = dir.path().join("schema.json");
    std::fs::write(&p, src).unwrap();
    crate::smodel::load(p.to_str().unwrap()).unwrap()
}
