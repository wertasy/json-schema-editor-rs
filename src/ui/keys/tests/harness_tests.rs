//! Shared test harness: schema/doc fixtures, App builders, key press
//! helpers and doc accessors used by every topic module.

use super::*;

pub(super) fn yaml_doc(src: &str) -> Doc {
    crate::doc::yamldoc::parse(src).expect("yaml").0
}

/// plain strips ANSI SGR sequences from a view string: the suggestion rows
/// interleave red highlight runs, so plain-text assertions read the stripped
/// view.
pub(super) fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut esc = false;
    for c in s.chars() {
        if esc {
            if c == 'm' {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            esc = true;
            continue;
        }
        out.push(c);
    }
    out
}

// --- fixtures ---------------------------------------------------------------

/// Builds a Schema arena plus the Doc arena its enum/default values point
/// into (the Go tests build loose *FormNode graphs).
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

/// Builds a document arena (the Go tobj/tstr/tnum helpers).
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

pub(super) fn test_opts() -> Options {
    Options {
        path: String::new(),
        ..Options::default()
    }
}

// --- harness ----------------------------------------------------------------

pub(super) fn key(k: &str) -> Msg {
    match k {
        "enter" => Msg::Enter,
        "esc" => Msg::Esc,
        "up" => Msg::Up,
        "down" => Msg::Down,
        "left" => Msg::Left,
        "right" => Msg::Right,
        "space" => Msg::Char(' ', false),
        "tab" => Msg::Tab,
        "shift+tab" => Msg::BackTab,
        "backspace" => Msg::Backspace,
        "delete" => Msg::Delete,
        "home" => Msg::Home,
        "end" => Msg::End,
        "pgup" => Msg::PageUp,
        "pgdown" => Msg::PageDown,
        "ctrl+f" => Msg::Ctrl('f'),
        "ctrl+b" => Msg::Ctrl('b'),
        "ctrl+c" => Msg::Ctrl('c'),
        "ctrl+j" => Msg::Ctrl('j'),
        "ctrl+k" => Msg::Ctrl('k'),
        "ctrl+s" => Msg::Ctrl('s'),
        "ctrl+r" => Msg::Ctrl('r'),
        "ctrl+o" => Msg::Ctrl('o'),
        "ctrl+i" => Msg::Ctrl('i'),
        "ctrl+u" => Msg::Ctrl('u'),
        "ctrl+n" => Msg::Ctrl('n'),
        "ctrl+p" => Msg::Ctrl('p'),
        "ctrl+a" => Msg::Ctrl('a'),
        "ctrl+w" => Msg::Ctrl('w'),
        "ctrl+y" => Msg::Ctrl('y'),
        "ctrl+e" => Msg::Ctrl('e'),
        "alt+i" => Msg::Char('i', true),
        "alt+x" => Msg::Char('x', true),
        "alt+f" => Msg::Char('f', true),
        "alt+b" => Msg::Char('b', true),
        _ => {
            let c = k.chars().next().unwrap_or(' ');
            Msg::Char(c, false)
        }
    }
}

pub(super) fn press(app: &mut App, keys: &[&str]) {
    for &k in keys {
        match k {
            "enter" | "esc" | "up" | "down" | "left" | "right" | "space" | "tab" | "shift+tab"
            | "backspace" | "delete" | "home" | "end" | "ctrl+c" | "ctrl+j" | "ctrl+k"
            | "ctrl+s" | "ctrl+r" | "ctrl+o" | "ctrl+i" | "ctrl+u" | "ctrl+n" | "ctrl+p"
            | "ctrl+a" | "ctrl+w" | "ctrl+y" | "ctrl+e" | "alt+i" | "alt+x" | "alt+f" | "alt+b"
            | "pgup" | "pgdown" | "ctrl+f" | "ctrl+b" => {
                app.update(key(k));
            }
            _ => {
                for c in k.chars() {
                    app.update(Msg::Char(c, false));
                }
            }
        }
    }
}

pub(super) fn help_scroll(m: &App) -> usize {
    match &m.overlay {
        Some(Overlay::Help(h)) => h.scroll,
        _ => panic!("help page is not open"),
    }
}

pub(super) fn mk_app(doc: Doc, opts: Options) -> App {
    app_with(test_schema(), doc, opts)
}

pub(super) fn path(segs: &[&str]) -> Vec<PathSeg> {
    segs.iter().map(|s| PathSeg::key(*s)).collect()
}

pub(super) fn str_at(doc: &Doc, segs: &[&str]) -> String {
    match doc.get(&path(segs)) {
        Some(n) => match doc.value(n) {
            Value::Str(s) => s.clone(),
            _ => "<missing>".to_string(),
        },
        None => "<missing>".to_string(),
    }
}

pub(super) fn num_at(doc: &Doc, segs: &[&str]) -> String {
    match doc.get(&path(segs)) {
        Some(n) => match doc.value(n) {
            Value::Num(s) => s.clone(),
            _ => "<missing>".to_string(),
        },
        None => "<missing>".to_string(),
    }
}

pub(super) fn bool_at(doc: &Doc, segs: &[&str]) -> bool {
    match doc.get(&path(segs)) {
        Some(n) => matches!(doc.value(n), Value::Bool(true)),
        None => false,
    }
}

pub(super) fn app_with(schema: Schema, doc: Doc, opts: Options) -> App {
    app_with_meta(schema, doc, opts, crate::doc::Meta::default())
}

pub(super) fn session(schema: Schema, doc: Doc, opts: Options) -> FileSession {
    let root = schema.root;
    let codec = crate::doc::codec_for(&opts.path);
    FileSession {
        schema,
        root,
        doc,
        meta: crate::doc::Meta::default(),
        codec,
        opts,
    }
}

pub(super) fn app_with_meta(
    schema: Schema,
    doc: Doc,
    opts: Options,
    meta: crate::doc::Meta,
) -> App {
    let mut s = session(schema, doc, opts);
    s.meta = meta;
    App::new(vec![s])
}

pub(super) fn app_with_codec(
    schema: Schema,
    doc: Doc,
    opts: Options,
    codec: &'static dyn crate::doc::Codec,
) -> App {
    let mut s = session(schema, doc, opts);
    s.codec = codec;
    App::new(vec![s])
}

/// schema_build builds an arena schema from node constructors.
pub(super) fn schema_build(f: impl FnOnce(&mut TB) -> FormNodeId) -> Schema {
    let mut b = TB::new();
    let root = f(&mut b);
    b.root = root;
    b.finish()
}

pub(super) fn ks(s: &str) -> PathSeg {
    PathSeg::key(s)
}

pub(super) fn sp(segs: &[&str]) -> Vec<PathSeg> {
    path(segs)
}

pub(super) fn child(schema: &Schema, n: FormNodeId, key: &str) -> FormNodeId {
    let node = &schema.nodes[n as usize];
    for &c in &node.children {
        if schema.nodes[c as usize].key == key {
            return c;
        }
    }
    for &v in &node.variants {
        if schema.nodes[v as usize].key == key {
            return v;
        }
    }
    panic!("no child {key}");
}

pub(super) fn getv(doc: &Doc, segs: &[PathSeg]) -> Option<NodeId> {
    doc.get(segs)
}

pub(super) fn sget(doc: &Doc, segs: &[PathSeg]) -> String {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Str(s) => s.clone(),
            _ => "<missing>".into(),
        },
        None => "<missing>".into(),
    }
}

pub(super) fn nget(doc: &Doc, segs: &[PathSeg]) -> String {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Num(s) => s.clone(),
            _ => "<missing>".into(),
        },
        None => "<missing>".into(),
    }
}

pub(super) fn bget(doc: &Doc, segs: &[PathSeg]) -> Option<bool> {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Bool(b) => Some(*b),
            _ => None,
        },
        None => None,
    }
}

pub(super) fn has(doc: &Doc, segs: &[PathSeg]) -> bool {
    getv(doc, segs).is_some()
}

pub(super) fn obj_keys(doc: &Doc, segs: &[PathSeg]) -> Vec<String> {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Object(mems) => mems.iter().map(|m| m.key.clone()).collect(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

pub(super) fn arr_len(doc: &Doc, segs: &[PathSeg]) -> usize {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Array(xs) => xs.len(),
            _ => 0,
        },
        None => 0,
    }
}

pub(super) fn arr_strs(doc: &Doc, segs: &[PathSeg]) -> Vec<String> {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Array(xs) => xs
                .iter()
                .map(|&x| match doc.value(x) {
                    Value::Str(s) => s.clone(),
                    Value::Num(n) => n.clone(),
                    _ => "<other>".into(),
                })
                .collect(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

pub(super) fn row_at(m: &mut App) -> Row {
    let c = m.top().map(|s| s.cursor).unwrap_or(0);
    m.menu_rows().into_iter().nth(c).unwrap_or_default()
}

pub(super) fn set_cursor(m: &mut App, pred: impl Fn(&Row) -> bool) {
    let rows = m.menu_rows();
    if let Some(i) = rows.iter().position(pred) {
        if let Some(st) = m.top_mut() {
            st.cursor = i;
        }
    }
}

pub(super) fn set_cursor_path(m: &mut App, p: &[PathSeg]) {
    set_cursor(m, |r| r.path == p);
}

pub(super) fn cursor_cursor(m: &mut App) -> usize {
    m.top().map(|s| s.cursor).unwrap_or(0)
}

pub(super) fn counting_opts() -> (Options, Arc<AtomicUsize>) {
    let mut opts = test_opts();
    let n = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&n);
    opts.save = Some(Arc::new(move |_, _| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    (opts, n)
}

pub(super) fn opts_validate(msgs: Vec<String>) -> Options {
    let mut opts = test_opts();
    opts.validate = Some(Arc::new(move |_: &Doc| msgs.clone()));
    opts
}

/// assertHintBottom checks that the view fills the terminal exactly and its
/// last row carries the expected help text.
pub(super) fn assert_hint_bottom(m: &mut App, needle: &str) {
    let view = m.view();
    let lines: Vec<&str> = view.lines().collect();
    assert_eq!(
        lines.len(),
        m.height,
        "view must end on the terminal's last row:\n{view}"
    );
    assert!(
        lines.last().unwrap().contains(needle),
        "help row missing {needle:?}:\n{view}"
    );
}

pub(super) fn n_at_idx(doc: &Doc, key: &str, idx: usize) -> String {
    match getv(doc, &[ks(key), PathSeg::Idx(idx)]) {
        Some(n) => match doc.value(n) {
            Value::Num(s) => s.clone(),
            _ => "<missing>".into(),
        },
        None => "<missing>".into(),
    }
}

pub(super) fn mount_src(doc: &Doc, i: usize) -> String {
    sget(doc, &[ks("mounts"), PathSeg::Idx(i), ks("src")])
}

pub(super) fn find_row(m: &mut App, pred: impl Fn(&Row, &Schema) -> bool) -> Option<Row> {
    let rows = m.menu_rows();
    let schema = &m.fs().schema;
    rows.into_iter().find(|r| pred(r, schema))
}

pub(super) fn required_merge_model() -> App {
    let doc = yaml_doc("base: &b\n  name: from-base\nchild:\n  <<: *b\n");
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.required = true;
        let name = b.id(name);
        let base = b.id(FormNode {
            key: "base".into(),
            title: "Base".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        });
        let child = b.id(FormNode {
            key: "child".into(),
            title: "Child".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![base, child],
            ..FormNode::default()
        })
    });
    app_with(schema, doc, test_opts())
}

pub(super) fn path_marked(paths: &[Vec<PathSeg>], p: &[PathSeg]) -> bool {
    paths.iter().any(|q| is_path_prefix(p, q))
}

// ===========================================================================
// menu_variant_infer_test.go
// ===========================================================================

pub(super) fn infer_schema() -> (Schema, FormNodeId, FormNodeId) {
    let mut b = TB::new();
    let mut t1 = fnode("type", "Type", Kind::Enum);
    t1.enum_ = tenum(&mut b, &["direct"]);
    t1.required = true;
    let t1 = b.id(t1);
    let l1 = b.id(scalar("listen", "Listen", "string"));
    let v1 = b.id(FormNode {
        key: "inbounds".into(),
        title: "variant 1".into(),
        kind: Kind::Object,
        children: vec![t1, l1],
        ..FormNode::default()
    });

    let mut t2 = fnode("type", "Type", Kind::Enum);
    t2.enum_ = tenum(&mut b, &["trojan"]);
    t2.required = true;
    let t2 = b.id(t2);
    let l2 = b.id(scalar("listen", "Listen", "string"));
    let mut p2 = scalar("password", "Password", "string");
    p2.required = true;
    let p2 = b.id(p2);
    let v2 = b.id(FormNode {
        key: "inbounds".into(),
        title: "variant 2".into(),
        kind: Kind::Object,
        children: vec![t2, l2, p2],
        ..FormNode::default()
    });
    let inbounds_one = b.id(FormNode {
        key: "inbounds".into(),
        title: "Inbound".into(),
        kind: Kind::OneOf,
        variants: vec![v1, v2],
        ..FormNode::default()
    });

    let mut none = fnode("alerts", "None", Kind::Enum);
    none.enum_ = tenum(&mut b, &["off"]);
    let none = b.id(none);
    let mut url = scalar("url", "Url", "string");
    url.required = true;
    let url = b.id(url);
    let webhook = b.id(FormNode {
        key: "alerts".into(),
        title: "Webhook".into(),
        kind: Kind::Object,
        children: vec![url],
        ..FormNode::default()
    });
    let alerts_one = b.id(FormNode {
        key: "alerts".into(),
        title: "Alert".into(),
        kind: Kind::OneOf,
        variants: vec![none, webhook],
        ..FormNode::default()
    });

    let inbounds = b.id(FormNode {
        key: "inbounds".into(),
        title: "Inbounds".into(),
        kind: Kind::Array,
        items: Some(inbounds_one),
        ..FormNode::default()
    });
    let alerts = b.id(FormNode {
        key: "alerts".into(),
        title: "Alerts".into(),
        kind: Kind::Array,
        items: Some(alerts_one),
        ..FormNode::default()
    });
    let root = b.id(FormNode {
        key: "$".into(),
        title: "Config".into(),
        kind: Kind::Object,
        children: vec![inbounds, alerts],
        ..FormNode::default()
    });
    b.root = root;
    (b.finish(), inbounds_one, alerts_one)
}

pub(super) fn mstate(kind: MenuKind, node: FormNodeId, path: Vec<PathSeg>) -> MenuState {
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

pub(super) fn body_val(f: impl FnOnce(&DB) -> NodeId) -> (Doc, NodeId) {
    let db = DB::new();
    let v = f(&db);
    (db.finish(v), v)
}

pub(super) fn schema_from_json(src: &str) -> Schema {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("schema.json");
    std::fs::write(&p, src).expect("schema write");
    crate::smodel::load(p.to_str().expect("path")).expect("schema load")
}

pub(super) fn live_check(doc: &Doc) -> Vec<String> {
    let mut out = Vec::new();
    if nget(doc, &sp(&["server", "port"])) == "8080" {
        out.push("at '$.server.port': minimum value is 1024".to_string());
    }
    if sget(doc, &sp(&["name"])) == "webapp" {
        out.push("at '$.name': invalid name".to_string());
    }
    out
}

pub(super) fn map_one_of_schema() -> Schema {
    schema_build(|b| {
        let v1 = b.id(scalar("headers", "string", "string"));
        let it = b.id(scalar("headers", "", "string"));
        let v2 = b.id(FormNode {
            key: "headers".into(),
            title: "list".into(),
            kind: Kind::Array,
            items: Some(it),
            ..FormNode::default()
        });
        let item = b.id(FormNode {
            key: "headers".into(),
            title: "Header".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        let headers = b.id(FormNode {
            key: "headers".into(),
            title: "Headers".into(),
            kind: Kind::Map,
            items: Some(item),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![headers],
            ..FormNode::default()
        })
    })
}

// ===========================================================================
// menu_items_test.go
// ===========================================================================

pub(super) fn items_app(doc: Doc) -> App {
    app_with(test_schema(), doc, test_opts())
}

pub(super) fn empty_doc() -> Doc {
    dbuild(|db| db.obj(vec![]))
}

pub(super) fn search_state(m: &App) -> &SearchState {
    match &m.overlay {
        Some(Overlay::Search(s)) => s,
        _ => panic!("search overlay not open"),
    }
}

// --- shell command insertion (`!`) -----------------------------------------

pub(super) fn shell_schema() -> Schema {
    schema_build(|b| {
        let name = b.id(scalar("name", "Name", "string"));
        let count = b.id(scalar("count", "Count", "integer"));
        let item = b.id(scalar("tags", "", "string"));
        let tags = b.id(FormNode {
            key: "tags".into(),
            title: "Tags".into(),
            kind: Kind::Array,
            items: Some(item),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![name, tags, count],
            ..FormNode::default()
        })
    })
}

pub(super) fn shell_doc() -> Doc {
    dbuild(|db| {
        db.obj(vec![
            ("name", db.s("old")),
            ("tags", db.arr(vec![db.s("a")])),
            ("count", db.n("1")),
        ])
    })
}

pub(super) fn two_file_app() -> (App, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let (opts1, n1) = counting_opts();
    let (opts2, n2) = counting_opts();
    let mut app = App::new(vec![
        session(shell_schema(), shell_doc(), opts1),
        session(shell_schema(), shell_doc(), opts2),
    ]);
    for i in 0..2 {
        let name = app.files[i].doc.new_str("changed");
        app.files[i].doc.set(&sp(&["name"]), name).unwrap();
    }
    (app, n1, n2)
}
