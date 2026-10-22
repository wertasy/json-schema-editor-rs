//! Model-level tests for the menu TUI: key-message injection plus doc/state
//! assertions. Rust port of the Go internal/ui menu_*_test.go suites — every
//! test that builds the model, sends keys and asserts doc/state. Bubbletea
//! View-string rendering tests are dropped (the Rust view layer emits ANSI
//! and is exercised by the interactive path, not asserted here).

use std::sync::Arc;

use super::*;
use crate::doc::jsondoc::Member;
use crate::doc::{Doc, NodeData, NodeId, PathSeg, Value};
use crate::smodel::{EnumOpt, FormNode, FormNodeId, Kind, Schema};
use crate::ui::menu::{alias_at, same_menu_row};
use crate::ui::{FileSession, Issue, NodeRef, Options, RawCodec};

// --- fixtures ---------------------------------------------------------------

/// Builds a Schema arena plus the Doc arena its enum/default values point
/// into (the Go tests build loose *FormNode graphs).
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

/// Builds a document arena (the Go tobj/tstr/tnum helpers).
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

fn dbuild(f: impl FnOnce(&DB) -> NodeId) -> Doc {
    let db = DB::new();
    let root = f(&db);
    db.finish(root)
}

fn tenum(b: &mut TB, vals: &[&str]) -> Vec<EnumOpt> {
    vals.iter()
        .map(|&v| {
            let val = b.s(v);
            let encoded = encode_value(&b.doc, val);
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

fn test_opts() -> Options {
    Options {
        accessible: false,
        force: false,
        raw: RawCodec {
            label: "",
            ext: ".json",
            marshal_doc: Arc::new(|d: &Doc| Ok(d.write())),
            unmarshal_doc: Arc::new(|s: &str| {
                crate::doc::jsondoc::parse(s).map_err(|e| e.to_string())
            }),
            adopt_doc: None,
            marshal_value: Arc::new(|r: NodeRef<'_>| Ok(r.doc.write_subtree(r.id))),
            unmarshal_value: Arc::new(|s: &str| {
                crate::doc::jsondoc::parse(s).map_err(|e| e.to_string())
            }),
        },
        validate: None,
        save: None,
        path: String::new(),
        write_to: None,
        copy: None,
        paste: None,
        replace_schema_location: None,
        status: String::new(),
        startup_notice: String::new(),
        load_errors: vec![],
    }
}

// --- harness ----------------------------------------------------------------

fn key(k: &str) -> Msg {
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
        "alt+i" => Msg::Char('i', true),
        "alt+x" => Msg::Char('x', true),
        _ => {
            let c = k.chars().next().unwrap_or(' ');
            Msg::Char(c, false)
        }
    }
}

fn press(app: &mut App, keys: &[&str]) {
    for &k in keys {
        match k {
            "enter" | "esc" | "up" | "down" | "left" | "right" | "space" | "tab" | "shift+tab"
            | "backspace" | "delete" | "home" | "end" | "ctrl+c" | "ctrl+j" | "ctrl+k"
            | "ctrl+s" | "ctrl+r" | "ctrl+o" | "ctrl+i" | "ctrl+u" | "ctrl+n" | "ctrl+p"
            | "alt+i" | "alt+x" | "pgup" | "pgdown" | "ctrl+f" | "ctrl+b" => {
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

fn mk_app(doc: Doc, opts: Options) -> App {
    let schema = test_schema();
    let root = schema.root;
    App::new(vec![FileSession {
        schema,
        root,
        doc,
        opts,
    }])
}

fn path(segs: &[&str]) -> Vec<PathSeg> {
    segs.iter().map(|s| PathSeg::key(*s)).collect()
}

fn str_at(doc: &Doc, segs: &[&str]) -> String {
    match doc.get(&path(segs)) {
        Some(n) => match doc.value(n) {
            Value::Str(s) => s.clone(),
            _ => "<missing>".to_string(),
        },
        None => "<missing>".to_string(),
    }
}

fn num_at(doc: &Doc, segs: &[&str]) -> String {
    match doc.get(&path(segs)) {
        Some(n) => match doc.value(n) {
            Value::Num(s) => s.clone(),
            _ => "<missing>".to_string(),
        },
        None => "<missing>".to_string(),
    }
}

fn bool_at(doc: &Doc, segs: &[&str]) -> bool {
    match doc.get(&path(segs)) {
        Some(n) => matches!(doc.value(n), Value::Bool(true)),
        None => false,
    }
}

// --- extended harness -------------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering};

fn app_with(schema: Schema, doc: Doc, opts: Options) -> App {
    let root = schema.root;
    App::new(vec![FileSession {
        schema,
        root,
        doc,
        opts,
    }])
}

/// schema_build builds an arena schema from node constructors.
fn schema_build(f: impl FnOnce(&mut TB) -> FormNodeId) -> Schema {
    let mut b = TB::new();
    let root = f(&mut b);
    b.root = root;
    b.finish()
}

fn ks(s: &str) -> PathSeg {
    PathSeg::key(s)
}

fn sp(segs: &[&str]) -> Vec<PathSeg> {
    path(segs)
}

fn child(schema: &Schema, n: FormNodeId, key: &str) -> FormNodeId {
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

fn getv(doc: &Doc, segs: &[PathSeg]) -> Option<NodeId> {
    doc.get(segs)
}

fn sget(doc: &Doc, segs: &[PathSeg]) -> String {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Str(s) => s.clone(),
            _ => "<missing>".into(),
        },
        None => "<missing>".into(),
    }
}

fn nget(doc: &Doc, segs: &[PathSeg]) -> String {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Num(s) => s.clone(),
            _ => "<missing>".into(),
        },
        None => "<missing>".into(),
    }
}

fn bget(doc: &Doc, segs: &[PathSeg]) -> Option<bool> {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Bool(b) => Some(*b),
            _ => None,
        },
        None => None,
    }
}

fn has(doc: &Doc, segs: &[PathSeg]) -> bool {
    getv(doc, segs).is_some()
}

fn obj_keys(doc: &Doc, segs: &[PathSeg]) -> Vec<String> {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Object(mems) => mems.iter().map(|m| m.key.clone()).collect(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

fn arr_len(doc: &Doc, segs: &[PathSeg]) -> usize {
    match getv(doc, segs) {
        Some(n) => match doc.value(n) {
            Value::Array(xs) => xs.len(),
            _ => 0,
        },
        None => 0,
    }
}

fn arr_strs(doc: &Doc, segs: &[PathSeg]) -> Vec<String> {
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

fn row_at(m: &mut App) -> Row {
    let c = m.top().map(|s| s.cursor).unwrap_or(0);
    m.menu_rows().into_iter().nth(c).unwrap_or_default()
}

fn set_cursor(m: &mut App, pred: impl Fn(&Row) -> bool) {
    let rows = m.menu_rows();
    if let Some(i) = rows.iter().position(pred) {
        if let Some(st) = m.top_mut() {
            st.cursor = i;
        }
    }
}

fn set_cursor_path(m: &mut App, p: &[PathSeg]) {
    set_cursor(m, |r| r.path == p);
}

fn cursor_cursor(m: &mut App) -> usize {
    m.top().map(|s| s.cursor).unwrap_or(0)
}

fn counting_opts() -> (Options, Arc<AtomicUsize>) {
    let mut opts = test_opts();
    let n = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&n);
    opts.save = Some(Arc::new(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    (opts, n)
}

fn opts_validate(msgs: Vec<String>) -> Options {
    let mut opts = test_opts();
    opts.validate = Some(Arc::new(move |_: &Doc| msgs.clone()));
    opts
}

// --- menu_test.go -----------------------------------------------------------

#[test]
fn menu_drill_and_restore_cursor() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["down", "down", "enter"]);
    assert_eq!(
        m.fs().stack.len(),
        2,
        "drill into Server should push a menu"
    );
    press(&mut m, &["esc"]);
    assert_eq!(m.fs().stack.len(), 1, "esc should pop back to root");
    // cursor was restored to row 2 (Server); one down lands on Logging
    press(&mut m, &["down", "enter"]);
    assert_eq!(
        m.fs().stack.len(),
        2,
        "cursor restore should drill into Logging"
    );
}

#[test]
fn menu_inline_edit_scalar() {
    let doc = test_doc();
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "down", "enter"]); // into Server
    press(&mut m, &["enter"]); // edit Port

    // invalid: below minimum keeps doc untouched, still editing
    press(&mut m, &["ctrl+u", "1", "enter"]);
    assert_eq!(
        num_at(&m.fs().doc, &["server", "port"]),
        "8080",
        "invalid input must not write"
    );
    assert!(
        matches!(m.overlay, Some(Overlay::Edit(_))),
        "edit must survive a failed confirm"
    );

    // valid: writes through and closes
    press(&mut m, &["ctrl+u", "9050", "enter"]);
    assert_eq!(
        num_at(&m.fs().doc, &["server", "port"]),
        "9050",
        "valid input should write"
    );
    assert!(
        !matches!(m.overlay, Some(Overlay::Edit(_))),
        "edit must close after success"
    );
}

#[test]
fn menu_enum_cycle() {
    let doc = test_doc();
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down"]); // Mode row
    press(&mut m, &["right"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
    press(&mut m, &["right"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "tcp");
    press(&mut m, &["left"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
}

#[test]
fn menu_enum_overlay() {
    let doc = test_doc();
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "enter"]); // open enum picker on Mode
    assert!(
        matches!(m.overlay, Some(Overlay::Sel(_))),
        "enum overlay should open"
    );
    press(&mut m, &["down", "enter"]); // pick grpc
    assert!(
        !matches!(m.overlay, Some(Overlay::Sel(_))),
        "picker must close on enter"
    );
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
}

#[test]
fn menu_enum_overlay_search_filters_and_selects() {
    let doc = test_doc();
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "enter"]); // open enum picker on Mode
    press(&mut m, &["/", "G", "R"]); // case-insensitive query
    match &m.overlay {
        Some(Overlay::Sel(s)) => {
            assert!(s.searching);
            assert_eq!(s.search_ti.value(), "GR");
            assert_eq!(sel_match_count(s), 1, "only grpc matches");
            let view = m.sel_view(s);
            assert!(view.contains("grpc"));
            assert!(!view.contains("http"), "non-matching options are hidden");
        }
        _ => panic!("enum picker should stay open while searching"),
    }
    press(&mut m, &["enter"]);
    assert!(
        !matches!(m.overlay, Some(Overlay::Sel(_))),
        "enter selects the match and closes the picker"
    );
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
}

#[test]
fn menu_enum_overlay_search_cancel_and_no_match() {
    let doc = test_doc();
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "enter", "/", "z", "z"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => {
            assert_eq!(sel_match_count(s), 0);
            assert!(m.sel_view(s).contains("(no matches)"));
        }
        _ => panic!("enum picker should stay open"),
    }
    press(&mut m, &["enter"]); // no match: must not write the hidden option
    assert!(matches!(m.overlay, Some(Overlay::Sel(_))));
    assert!(!has(&m.fs().doc, &sp(&["mode"])), "no option was written");

    press(&mut m, &["esc"]); // leave search, keep the picker
    match &m.overlay {
        Some(Overlay::Sel(s)) => {
            assert!(!s.searching);
            assert_eq!(s.search_ti.value(), "");
            assert_eq!(sel_match_count(s), 3, "the full option list is back");
        }
        _ => panic!("esc must only cancel the search"),
    }
    press(&mut m, &["down", "enter"]); // grpc from the restored list
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
    assert!(m.overlay.is_none());
}

#[test]
fn menu_enum_overlay_search_takes_q_literally() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "mode": {"type": "string", "enum": ["quiet", "loud", "http"]}
          }
        }"#,
    );
    let doc = dbuild(|db| db.obj(vec![("mode", db.s("http"))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]); // open enum picker on the only field
    press(&mut m, &["/", "q"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => {
            assert!(s.searching, "q must be a query character, not esc");
            assert_eq!(s.search_ti.value(), "q");
            assert_eq!(sel_match_count(s), 1);
        }
        _ => panic!("enum picker should stay open"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "quiet");
}

/// assertHintBottom checks that the view fills the terminal exactly and its
/// last row carries the expected help text.
fn assert_hint_bottom(m: &mut App, needle: &str) {
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

#[test]
fn enum_picker_search_sits_above_the_list() {
    let doc = test_doc();
    let mut m = mk_app(doc, test_opts());
    m.width = 80;
    m.height = 12;
    press(&mut m, &["down", "enter", "/", "g", "r"]);
    let view = m.view();
    let lines: Vec<&str> = view.lines().collect();
    let search = lines
        .iter()
        .position(|l| l.contains("search: gr"))
        .expect("search row");
    let option = lines
        .iter()
        .position(|l| l.contains("grpc"))
        .expect("matched option");
    assert!(search < option, "search must be above the list:\n{view}");
    assert!(
        !lines[search].contains("\x1b[38;5;248m"),
        "the search row must use the default white foreground:\n{view}"
    );
    assert_hint_bottom(&mut m, "esc cancel search");
}

#[test]
fn oneof_picker_search_sits_above_the_list() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "rule": {"oneOf": [
              {"title": "Alpha", "type": "object",
               "properties": {"a": {"type": "string"}}},
              {"title": "Beta", "type": "object",
               "properties": {"b": {"type": "string"}}}
            ]}
          }
        }"#,
    );
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    m.width = 80;
    m.height = 14;
    set_cursor_path(&mut m, &sp(&["rule"]));
    press(&mut m, &["t", "/", "a", "l", "p"]);
    let view = m.view();
    let lines: Vec<&str> = view.lines().collect();
    let search = lines
        .iter()
        .position(|l| l.contains("search: alp"))
        .expect("search row");
    let variant = lines
        .iter()
        .position(|l| l.contains("Alpha"))
        .expect("matched variant");
    assert!(search < variant, "search must be above the list:\n{view}");
    assert!(
        !lines[search].contains("\x1b[38;5;248m"),
        "the search row must use the default white foreground:\n{view}"
    );
    assert_hint_bottom(&mut m, "esc cancel search");
}

#[test]
fn overlay_help_rows_pin_to_the_terminal_bottom() {
    let mut m = mk_app(test_doc(), test_opts());
    m.width = 80;
    m.height = 12;

    m.overlay = Some(Overlay::Issues(IssuesState::default()));
    assert_hint_bottom(&mut m, "esc close");

    m.overlay = Some(Overlay::RefPick(RefPickState {
        name: "x".into(),
        paths: vec![sp(&["name"])],
        cursor: 0,
        search_ti: Default::default(),
        searching: false,
        search_cursor: 0,
    }));
    assert_hint_bottom(&mut m, "enter jump");

    m.overlay = Some(Overlay::Errs(ErrsState {
        lines: vec!["boom".into()],
        paths: vec![Some(sp(&["server"]))],
        cursor: 0,
        exit: false,
        target: String::new(),
        search_ti: Default::default(),
        searching: false,
        search_cursor: 0,
    }));
    assert_hint_bottom(&mut m, "pgup/pgdn page");

    m.overlay = Some(Overlay::Confirm(ConfirmState {
        title: "Sure?".into(),
        hint: String::new(),
        on_yes: None,
        on_no: None,
    }));
    assert_hint_bottom(&mut m, "y/enter/space yes");

    let mut ta = TextArea::new();
    ta.set_width(40);
    ta.set_height(3);
    m.overlay = Some(Overlay::Text(TextState {
        path: None,
        fn_: None,
        ta,
        err: String::new(),
        verbatim_str: false,
    }));
    assert_hint_bottom(&mut m, "ctrl+s apply");

    m.overlay = None;
    m.file_pick = Some(FilePickState {
        cursor: 0,
        search_ti: Default::default(),
        searching: false,
        search_cursor: 0,
    });
    assert_hint_bottom(&mut m, "enter switch");
}

#[test]
fn search_opens_empty_on_the_current_value() {
    // oneOf: the cursor snaps back to the variant matching the document
    // value, even after it was moved away
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "rule": {"oneOf": [
              {"title": "Alpha", "type": "object", "required": ["kind"],
               "properties": {"kind": {"const": "a"}}},
              {"title": "Beta", "type": "object", "required": ["kind"],
               "properties": {"kind": {"const": "b"}}}
            ]}
          }
        }"#,
    );
    let doc = dbuild(|db| {
        let rule = db.obj(vec![("kind", db.s("b"))]);
        db.obj(vec![("rule", rule)])
    });
    let mut m = app_with(schema, doc, test_opts());
    m.width = 80;
    m.height = 16;
    set_cursor_path(&mut m, &sp(&["rule"]));
    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().cursor, 1, "seeded on Beta");
    press(&mut m, &["/"]);
    assert!(m.top().unwrap().searching);
    assert_eq!(m.top().unwrap().search_ti.value(), "", "empty query");
    assert_eq!(m.top().unwrap().cursor, 1, "cursor keeps the value row");
    press(&mut m, &["a", "l"]);
    press(&mut m, &["esc"]);
    assert_eq!(
        m.top().unwrap().cursor,
        1,
        "cancelling restores the value cursor"
    );

    // enum: same rule for the single-select picker
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "mode":{"type":"string","enum":["http","grpc","tcp"]}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("mode", db.s("tcp"))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => assert_eq!(s.cursor, 2, "current value tcp"),
        _ => panic!("enum picker should open"),
    }
    press(&mut m, &["/"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => {
            assert!(s.searching);
            assert_eq!(s.search_ti.value(), "");
            assert_eq!(s.cursor, 2, "cursor stays on the current value");
            assert_eq!(s.search_cursor, 2);
        }
        _ => panic!("enum picker should stay open"),
    }
    press(&mut m, &["g", "r"]);
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
}

#[test]
fn enum_array_search_filters_toggles() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "levels":{"type":"array","items":{"enum":["alpha","beta","gamma"]}}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("levels", db.arr(vec![db.s("beta")]))]));
    let mut m = app_with(schema, doc, test_opts());
    m.width = 80;
    m.height = 14;
    set_cursor_path(&mut m, &sp(&["levels"]));
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::EnumArray);
    assert_eq!(m.menu_rows().len(), 3);

    press(&mut m, &["/", "g", "a"]);
    assert!(m.top().unwrap().searching);
    assert_eq!(m.top().unwrap().search_ti.value(), "ga");
    assert_eq!(m.menu_rows().len(), 1, "only gamma matches");
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));

    press(&mut m, &["enter"]); // toggles the only match, filter stays
    assert_eq!(arr_strs(&m.fs().doc, &sp(&["levels"])), ["beta", "gamma"]);
    assert!(m.top().unwrap().searching);

    press(&mut m, &["esc"]);
    assert!(!m.top().unwrap().searching);
    assert_eq!(m.menu_rows().len(), 3);
}

#[test]
fn issues_list_search_and_cancel() {
    let mk = |msg: &str, path: &[&str]| Issue {
        msg: msg.into(),
        path: Some(sp(path)),
        ..Issue::default()
    };
    let mut m = mk_app(test_doc(), test_opts());
    m.width = 80;
    m.height = 12;
    m.overlay = Some(Overlay::Issues(IssuesState {
        items: vec![
            mk("at '$.name': alpha problem", &["name"]),
            mk("at '$.server': beta problem", &["server"]),
        ],
        cursor: 1,
        ..IssuesState::default()
    }));

    press(&mut m, &["/", "b", "e", "t", "a"]);
    assert!(m.overlay.is_some());
    let view = m.view();
    assert!(view.contains("beta problem"), "{view}");
    assert!(!view.contains("alpha problem"), "{view}");

    press(&mut m, &["esc"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => {
            assert!(!s.searching);
            assert_eq!(s.search_ti.value(), "");
            assert_eq!(s.cursor, 1, "cancel restores the row");
        }
        _ => panic!("issues overlay should stay open"),
    }

    press(&mut m, &["/", "b", "e", "t", "a"]);
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "enter jumps and closes the list");
    assert_eq!(m.top().unwrap().path, sp(&["server"]));
}

#[test]
fn save_error_list_search() {
    let mut m = mk_app(test_doc(), test_opts());
    m.width = 80;
    m.height = 12;
    m.overlay = Some(Overlay::Errs(ErrsState {
        lines: vec!["$.name: bad".into(), "$.server: worse".into()],
        paths: vec![Some(sp(&["name"])), Some(sp(&["server"]))],
        cursor: 0,
        exit: false,
        target: String::new(),
        search_ti: Default::default(),
        searching: false,
        search_cursor: 0,
    }));

    press(&mut m, &["/", "w", "o", "r"]);
    let view = m.view();
    assert!(view.contains("worse"), "{view}");
    assert!(!view.contains("bad"), "{view}");
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().path, sp(&["server"]));
}

#[test]
fn reference_list_search() {
    let mut m = mk_app(test_doc(), test_opts());
    m.width = 80;
    m.height = 12;
    m.overlay = Some(Overlay::RefPick(RefPickState {
        name: "x".into(),
        paths: vec![sp(&["name"]), sp(&["server", "host"])],
        cursor: 0,
        search_ti: Default::default(),
        searching: false,
        search_cursor: 0,
    }));

    press(&mut m, &["/", "h", "o", "s", "t"]);
    let view = m.view();
    assert!(view.contains("$.server.host"), "{view}");
    assert!(!view.contains("$.name"), "{view}");
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
}

#[test]
fn file_picker_search() {
    let opts = |path: &str| {
        let mut o = test_opts();
        o.path = path.into();
        o
    };
    let schema1 = test_schema();
    let root1 = schema1.root;
    let schema2 = test_schema();
    let root2 = schema2.root;
    let mut m = App::new(vec![
        FileSession {
            schema: schema1,
            root: root1,
            doc: test_doc(),
            opts: opts("alpha.json"),
        },
        FileSession {
            schema: schema2,
            root: root2,
            doc: test_doc(),
            opts: opts("beta.json"),
        },
    ]);
    m.width = 80;
    m.height = 12;
    m.file_pick = Some(FilePickState {
        cursor: 0,
        search_ti: Default::default(),
        searching: false,
        search_cursor: 0,
    });

    press(&mut m, &["/", "b", "e", "t", "a"]);
    let view = m.view();
    assert!(view.contains("beta.json"), "{view}");
    assert!(!view.contains("alpha.json"), "{view}");
    press(&mut m, &["enter"]);
    assert_eq!(m.cur, 1);
    assert!(m.file_pick.is_none());
}

#[test]
fn menu_bool_toggle() {
    let doc = test_doc(); // debug=true
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "down", "enter", "down", "down"]); // Server → Debug
    press(&mut m, &["space"]);
    assert!(
        !bool_at(&m.fs().doc, &["server", "debug"]),
        "space should flip debug to false"
    );
    press(&mut m, &["enter"]); // enter toggles too
    assert!(
        bool_at(&m.fs().doc, &["server", "debug"]),
        "enter should toggle bool rows"
    );
}

#[test]
fn menu_ctrl_c() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["down", "enter", "ctrl+c"]);
    assert!(m.aborted, "ctrl+c must abort anywhere");
}

// --- menu_paste_test.go -----------------------------------------------------

fn mk_paste_app(doc: Doc, text: String) -> App {
    let mut opts = test_opts();
    opts.paste = Some(Arc::new(move || Ok(text.clone())));
    mk_app(doc, opts)
}

#[test]
fn paste_partial_keys_into_page_object() {
    let doc = test_doc();
    let mut m = mk_paste_app(
        doc.clone(),
        "{\"zone\":\"us-east\",\"replicas\":2}".to_string(),
    );
    press(&mut m, &["down", "down", "enter"]); // Server
    press(&mut m, &["p"]);
    assert_eq!(str_at(&m.fs().doc, &["server", "zone"]), "us-east");
    assert_eq!(num_at(&m.fs().doc, &["server", "replicas"]), "2");
}

#[test]
fn paste_unparseable() {
    let doc = test_doc();
    let mut m = mk_paste_app(doc.clone(), "not json at all".to_string());
    press(&mut m, &["down", "down", "enter"]); // Server
    press(&mut m, &["p"]);
    assert_eq!(num_at(&m.fs().doc, &["server", "port"]), "8080");
    assert!(!m.notice.is_empty(), "unparseable paste must set a notice");
}

#[test]
fn paste_empty_clipboard() {
    let doc = test_doc();
    let mut m = mk_paste_app(doc.clone(), String::new());
    press(&mut m, &["p"]);
    assert!(!m.notice.is_empty(), "empty clipboard must set a notice");
}

// ===========================================================================
// menu_test.go (model-level remainder)
// ===========================================================================

#[test]
fn startup_notice_hides_on_key() {
    let mut opts = test_opts();
    opts.status = "⚠ using the cached schema: offline".to_string();
    opts.startup_notice =
        "yaml: 1 alias kept\nschema from SchemaStore: GitHub Workflow".to_string();
    let mut m = mk_app(test_doc(), opts);
    assert!(
        m.fs().startup.contains("alias kept"),
        "startup notice must be installed"
    );
    press(&mut m, &["j"]);
    assert!(
        m.fs().startup.is_empty(),
        "first key press must retire info notices"
    );
    assert!(m.fs().opts.status.contains("using the cached schema"));
}

#[test]
fn menu_inline_edit_cancel_and_omit() {
    let doc = test_doc();
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "down", "enter"]); // Server
    press(&mut m, &["enter"]); // edit Port
    press(&mut m, &["ctrl+u", "9050", "enter"]);
    assert_eq!(num_at(&m.fs().doc, &["server", "port"]), "9050");

    // esc cancels without writing
    press(&mut m, &["enter", "ctrl+u", "1234", "esc"]);
    assert!(
        !matches!(m.overlay, Some(Overlay::Edit(_))),
        "esc must close the editor"
    );
    assert_eq!(
        num_at(&m.fs().doc, &["server", "port"]),
        "9050",
        "esc must not write"
    );

    // empty optional input removes the key (omit rule): Ratio has no default
    press(
        &mut m,
        &["down", "down", "down", "enter", "ctrl+u", "enter"],
    );
    assert!(
        !has(&m.fs().doc, &sp(&["server", "ratio"])),
        "empty optional input should remove the key"
    );
}

#[test]
fn menu_root_esc_quit() {
    // clean root esc quits directly
    let mut clean = mk_app(test_doc(), test_opts());
    clean.update(Msg::Esc);
    assert!(clean.exiting && !clean.aborted, "clean root esc must quit");
    assert!(clean.overlay.is_none() && !clean.fs().saved);

    // changed: esc asks whether to save
    let mut discard = mk_app(test_doc(), test_opts());
    press(&mut discard, &["enter", "ctrl+u", "x", "enter"]);
    assert!(discard.dirty(), "editing Name must dirty the document");
    discard.update(Msg::Esc);
    match &discard.overlay {
        Some(Overlay::Confirm(c)) => assert!(c.title.contains("Save changes?")),
        _ => panic!("dirty root esc must ask to save"),
    }
    press(&mut discard, &["n"]);
    assert!(discard.overlay.is_none() && discard.aborted);
    assert!(
        !discard.exiting,
        "n aborts; the process must not report a clean quit"
    );

    // esc dismisses the prompt and stays in the session
    let mut stay = mk_app(test_doc(), test_opts());
    press(&mut stay, &["enter", "ctrl+u", "x", "enter", "esc"]);
    assert!(matches!(stay.overlay, Some(Overlay::Confirm(_))));
    press(&mut stay, &["esc"]);
    assert!(stay.overlay.is_none() && !stay.aborted && !stay.exiting);

    // y saves and exits
    let (opts, saves) = counting_opts();
    let mut save = mk_app(test_doc(), opts);
    press(&mut save, &["enter", "ctrl+u", "x", "enter", "esc"]);
    press(&mut save, &["y"]);
    assert_eq!(saves.load(Ordering::SeqCst), 1);
    assert!(save.fs().saved && save.exiting && !save.aborted);
    assert!(!save.dirty(), "saving must refresh the baseline");
}

#[test]
fn root_esc_clears_selection_before_quit() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["m", "down", "m"]);
    assert_eq!(m.top().unwrap().sel.len(), 2);
    m.update(Msg::Esc);
    assert!(
        !m.exiting && !m.aborted,
        "esc must not quit with a selection"
    );
    assert!(m.top().unwrap().sel.is_empty(), "esc must clear the marks");
    assert_eq!(m.notice, "selection cleared");
    assert!(m.fs().stack.len() == 1);

    // q aliases esc outside inputs
    press(&mut m, &["m", "q"]);
    assert!(!m.exiting && !m.aborted && m.top().unwrap().sel.is_empty());

    // without a selection esc keeps its meaning
    m.update(Msg::Esc);
    assert!(m.exiting);
}

#[test]
fn root_esc_clears_selection_before_save_prompt() {
    // a dirty root with marks clears them first; the next esc asks to save
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["enter", "ctrl+u", "x", "enter", "m"]);
    assert!(m.dirty() && !m.top().unwrap().sel.is_empty());
    m.update(Msg::Esc);
    assert!(m.overlay.is_none() && !m.exiting && !m.aborted);
    assert!(m.top().unwrap().sel.is_empty());
    m.update(Msg::Esc);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
}

#[test]
fn unknown_key_type_picker_object_and_switch() {
    let mut doc = test_doc();
    let extra = doc.new_object();
    let old = doc.new_str("value");
    let _ = doc.set(&sp(&["extra"]), extra);
    let _ = doc.insert_member(&sp(&["extra"]), "old", 0, old);
    let mut m = mk_app(doc.clone(), test_opts());

    // a known object type drills straight into the schema-less object menu
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "extra");
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert!(m.pending_edit.is_none(), "no $EDITOR is launched");
    let nested = find_row(&mut m, |r, _| {
        r.kind == RowKind::Unknown && r.label == "old"
    })
    .expect("unknown nested key");
    let i = m
        .menu_rows()
        .iter()
        .position(|r| same_menu_row(r, &nested))
        .unwrap();
    m.top_mut().unwrap().cursor = i;

    // a nested known string edits directly
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("new"),
        _ => panic!("string editor missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(sget(&m.fs().doc, &sp(&["extra", "old"])), "new");

    // t explicitly opens the type picker even with a value
    press(&mut m, &["esc"]);
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "extra");
    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    let titles: Vec<String> = m
        .menu_rows()
        .iter()
        .filter_map(|r| r.variant)
        .map(|v| m.fs().schema.nodes[v as usize].title.clone())
        .collect();
    assert_eq!(
        titles,
        [
            "String", "Number", "Boolean", "Object", "Array", "Null", "JSON"
        ]
    );
    press(&mut m, &["home", "enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("text"),
        _ => panic!("string editor missing after type switch"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["extra"]), "text");

    // the confirm nulls the old value as one step; the new string is the
    // next, so two undos restore the object
    press(&mut m, &["u"]);
    assert!(matches!(
        m.fs()
            .doc
            .value(getv(&m.fs().doc, &sp(&["extra"])).unwrap()),
        Value::Null
    ));
    press(&mut m, &["u"]);
    assert_eq!(
        sget(&m.fs().doc, &sp(&["extra", "old"])),
        "new",
        "undo must restore the object"
    );
}

#[test]
fn add_unknown_key_uses_type_picker() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["a"]);
    match &mut m.overlay {
        Some(Overlay::Add(s)) => s.ti.set_value("extra"),
        _ => panic!("add-key input missing"),
    }
    press(&mut m, &["enter"]);
    assert!(has(&m.fs().doc, &sp(&["extra"])));
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::OneOf,
        "a new schema-less key picks its type instead of launching $EDITOR"
    );
    assert!(m.pending_edit.is_none());

    // the null placeholder takes the chosen type
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("value"),
        _ => panic!("string editor missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["extra"]), "value");
}

#[test]
fn untitled_schema_variants_render_signature_in_title_column() {
    // The any-type picker shows each type name in the row's title column;
    // schema oneOf variants without a title must use their signature there
    // too instead of leaving an empty, padded title cell.
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "auth": {"oneOf": [
              {"type": "object", "properties": {"token": {"type": "string"}}},
              {"type": "object", "properties": {"user": {"type": "string"}},
               "required": ["user"]}
            ]}
          }
        }"#,
    );
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    m.width = 100;
    m.height = 24;
    set_cursor_path(&mut m, &sp(&["auth"]));
    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf, "variant picker");

    let view = m.view();
    let variant_lines: Vec<&str> = view
        .lines()
        .filter(|l| l.contains('○') || l.contains('●'))
        .collect();
    assert_eq!(variant_lines.len(), 2, "{view}");
    let ansi = regex::Regex::new("\x1b\\[[0-9;]*m").unwrap();
    for line in variant_lines {
        let plain = ansi.replace_all(line, "");
        assert!(
            plain.contains("○ {token}") || plain.contains("○ {user*}"),
            "untitled variant must carry its signature in the title column: {plain:?}"
        );
    }
}

#[test]
fn comment_preview_is_capped_and_c_expands_it() {
    let desc = "L1\nL2\nL3\nL4\nL5\nTAILMARK";
    let src = serde_json::json!({
        "type": "object",
        "properties": {"name": {"type": "string", "title": "Name", "description": desc}}
    });
    let schema = schema_from_json(&src.to_string());
    let mut m = app_with(schema, test_doc(), test_opts());
    m.width = 40;
    m.height = 24;
    set_cursor_path(&mut m, &sp(&["name"]));

    let view = m.view();
    assert!(view.contains("L1"), "{view}");
    assert!(
        view.contains("..."),
        "collapsed preview must mark the cut:\n{view}"
    );
    assert!(
        !view.contains("TAILMARK"),
        "preview leaked the tail:\n{view}"
    );

    press(&mut m, &["c"]);
    assert!(
        m.desc_expanded,
        "c must expand a description that fits one page"
    );
    let view = m.view();
    assert!(
        view.contains("TAILMARK"),
        "expanded description lost text:\n{view}"
    );

    // Any other key folds the description and still performs its action.
    let before = m.top().unwrap().cursor;
    press(&mut m, &["j"]);
    assert!(!m.desc_expanded, "j must collapse the expanded description");
    assert_eq!(
        m.top().unwrap().cursor,
        before + 1,
        "j must still move the cursor"
    );
    let view = m.view();
    assert!(
        !view.contains("TAILMARK"),
        "collapsed view still expanded:\n{view}"
    );
}

#[test]
fn long_comment_opens_a_scrollable_page() {
    let desc = (1..=40)
        .map(|i| format!("L{i:02}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\nTAILMARK";
    let src = serde_json::json!({
        "type": "object",
        "properties": {"name": {"type": "string", "title": "Name", "description": desc}}
    });
    let schema = schema_from_json(&src.to_string());
    let mut m = app_with(schema, test_doc(), test_opts());
    m.width = 40;
    m.height = 24;
    set_cursor_path(&mut m, &sp(&["name"]));

    press(&mut m, &["c"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => {
            assert_eq!(c.lines.len(), 41);
            assert_eq!(c.scroll, 0);
        }
        _ => panic!("long comment did not open the comment page"),
    }
    let view = m.view();
    assert!(view.contains("Comment: Name"), "{view}");
    assert!(view.contains("L01"), "{view}");
    assert!(
        !view.contains("TAILMARK"),
        "first page leaks the tail:\n{view}"
    );
    assert!(view.contains("c/q/esc close"), "page hint missing:\n{view}");

    // A page key jumps a full screen and clamps at the ends.
    press(&mut m, &["pgdown"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => {
            assert_eq!(c.scroll, 19);
            assert!(m.view().contains("TAILMARK"));
        }
        _ => panic!("pgdown closed the comment page"),
    }
    press(&mut m, &["ctrl+b"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => assert_eq!(c.scroll, 0),
        _ => panic!("ctrl+b closed the comment page"),
    }
    press(&mut m, &["ctrl+f"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => assert_eq!(c.scroll, 19),
        _ => panic!("ctrl+f closed the comment page"),
    }
    press(&mut m, &["pgup"]);
    match &m.overlay {
        Some(Overlay::Comment(c)) => assert_eq!(c.scroll, 0),
        _ => panic!("pgup closed the comment page"),
    }

    for _ in 0..25 {
        press(&mut m, &["down"]);
    }
    match &m.overlay {
        Some(Overlay::Comment(c)) => {
            assert_eq!(c.scroll, 19, "scroll must clamp to the last page");
            let view = m.view();
            assert!(!view.contains("L01"), "old lines still visible:\n{view}");
            assert!(
                view.contains("TAILMARK"),
                "last page lost the tail:\n{view}"
            );
        }
        _ => panic!("comment page closed unexpectedly"),
    }

    press(&mut m, &["c"]);
    assert!(m.overlay.is_none(), "c must close the comment page");

    // q aliases esc outside inputs and also closes the page.
    press(&mut m, &["c"]);
    assert!(matches!(m.overlay, Some(Overlay::Comment(_))));
    press(&mut m, &["q"]);
    assert!(m.overlay.is_none(), "q must close the comment page");
}

#[test]
fn page_keys_move_the_menu_cursor_one_screen() {
    let mut props = serde_json::Map::new();
    for i in 0..30 {
        props.insert(format!("k{i:02}"), serde_json::json!({"type": "string"}));
    }
    let src = serde_json::json!({"type": "object", "properties": props});
    let schema = schema_from_json(&src.to_string());
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    m.width = 80;
    m.height = 12; // title + breadcrumb + bottom help leave 9 rows

    press(&mut m, &["pgdown"]);
    assert_eq!(m.top().unwrap().cursor, 9);
    press(&mut m, &["ctrl+f"]);
    assert_eq!(m.top().unwrap().cursor, 18);
    press(&mut m, &["pgdown"]);
    assert_eq!(m.top().unwrap().cursor, 27);
    // no wrap at the end: stop on the last row
    let last = m.menu_rows().len() - 1;
    press(&mut m, &["pgdown"]);
    assert_eq!(m.top().unwrap().cursor, last);
    press(&mut m, &["pgup"]);
    assert_eq!(m.top().unwrap().cursor, last - 9);
    press(&mut m, &["ctrl+b"]);
    assert_eq!(m.top().unwrap().cursor, last - 18);
    // and no wrap at the top either
    for _ in 0..3 {
        press(&mut m, &["pgup"]);
    }
    assert_eq!(m.top().unwrap().cursor, 0);
    // paging is motion only: the document is untouched and no undo step exists
    assert!(!m.dirty());
}

#[test]
fn help_page_scrolls_when_taller_than_the_screen() {
    let mut m = app_with(test_schema(), test_doc(), test_opts());
    m.width = 80;
    m.height = 24;
    press(&mut m, &["?"]);
    assert!(m.help);
    assert!(!m.view().contains("merge keys: merge anchors"));

    press(&mut m, &["pgdown"]);
    assert_eq!(m.help_scroll, 19, "one full help screen");
    assert!(m.view().contains("merge keys: merge anchors"));

    press(&mut m, &["pgup"]);
    assert_eq!(m.help_scroll, 0);
    press(&mut m, &["q"]);
    assert!(!m.help, "q must close the help page");
}

#[test]
fn root_title_uses_file_name_and_submenus_keep_schema_titles() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "server": {
              "type": "object",
              "title": "Server Title",
              "properties": {"port": {"type": "integer"}}
            },
            "plain": {
              "type": "object",
              "properties": {"x": {"type": "string"}}
            }
          }
        }"#,
    );
    let mut opts = test_opts();
    opts.path = "/tmp/demo.json".into();
    let m = app_with(schema, test_doc(), opts);
    let root = m.fs().schema.root;

    let st = mstate(MenuKind::Object, root, vec![]);
    assert_eq!(
        m.menu_title(&st).0,
        "demo.json",
        "root fallback must be the file name"
    );

    let server = child(&m.fs().schema, root, "server");
    let st = mstate(MenuKind::Object, server, sp(&["server"]));
    assert_eq!(
        m.menu_title(&st).0,
        "Server Title",
        "submenu keeps the schema title"
    );

    let plain = child(&m.fs().schema, root, "plain");
    let st = mstate(MenuKind::Object, plain, sp(&["plain"]));
    assert_eq!(
        m.menu_title(&st).0,
        "plain",
        "untitled submenu falls back to its key"
    );

    // The root screen always names the file, even when the schema has a
    // title of its own.
    let schema = schema_from_json(r#"{"title":"My Config","type":"object"}"#);
    let mut opts = test_opts();
    opts.path = "/tmp/other.json".into();
    let m = app_with(schema, test_doc(), opts);
    let root = m.fs().schema.root;
    let st = mstate(MenuKind::Object, root, vec![]);
    assert_eq!(m.menu_title(&st).0, "other.json");
}

#[test]
fn schema_less_root_title_uses_file_name() {
    let doc = test_doc();
    let schema = crate::smodel::untyped(&doc);
    let mut opts = test_opts();
    opts.path = "/tmp/plain.json".into();
    let m = app_with(schema, doc, opts);
    let root = m.fs().schema.root;
    let st = mstate(MenuKind::Object, root, vec![]);
    assert_eq!(m.menu_title(&st).0, "plain.json");
}

#[test]
fn schema_typed_field_does_not_switch_type() {
    let mut m = mk_app(test_doc(), test_opts());
    set_cursor_path(&mut m, &sp(&["name"]));
    press(&mut m, &["t"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::Object,
        "a schema-typed field must not open a type picker"
    );
    // enter still edits the declared type in place
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
}

#[test]
fn any_type_picker_includes_schema_definitions() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {"value": {}},
          "$defs": {
            "server": {
              "title": "Server",
              "type": "object",
              "properties": {"port": {"type": "integer", "title": "Port"}},
              "required": ["port"]
            },
            "mode": {"title": "Mode", "enum": ["a", "b"]},
            "blob": {"type": "string"},
            "pick": {
              "title": "Pick",
              "oneOf": [
                {"type": "string"},
                {"type": "object", "properties": {"n": {"type": "number"}}}
              ]
            }
          }
        }"#,
    );
    let titles: Vec<String> = schema
        .defs
        .iter()
        .map(|id| schema.nodes[*id as usize].title.clone())
        .collect();
    assert_eq!(titles, ["Server", "Mode", "blob", "Pick"]);

    let doc = dbuild(|db| {
        let value = db.obj(vec![]);
        let extra = db.obj(vec![("x", db.n("1"))]);
        db.obj(vec![("value", value), ("extra", extra)])
    });
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["value"]));
    press(&mut m, &["t"]);
    let got: Vec<String> = m
        .menu_rows()
        .iter()
        .filter_map(|r| r.variant)
        .map(|v| m.fs().schema.nodes[v as usize].title.clone())
        .collect();
    assert_eq!(
        got,
        [
            "String", "Number", "Boolean", "Object", "Array", "Null", "Server", "Mode", "blob",
            "Pick", "JSON"
        ]
    );

    // picking a schema-defined object type opens its real form
    press(&mut m, &["down", "down", "down", "enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    set_cursor_path(&mut m, &sp(&["value", "port"]));
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("8080"),
        _ => panic!("schema-defined Port editor missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(num_at(&m.fs().doc, &["value", "port"]), "8080");

    // a schema definition that is itself a oneOf opens its variant picker
    press(&mut m, &["esc"]);
    set_cursor_path(&mut m, &sp(&["value"]));
    press(&mut m, &["t"]);
    press(&mut m, &["down", "down", "down", "down", "down", "down"]);
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
}

#[test]
fn type_picker_search_filters_variants_inline() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {"value": {}},
          "$defs": {
            "server": {
              "title": "Server",
              "type": "object",
              "properties": {"port": {"type": "integer"}}
            },
            "mode": {"title": "Mode", "enum": ["a", "b"]}
          }
        }"#,
    );
    let doc = dbuild(|db| {
        let value = db.obj(vec![("x", db.n("1"))]);
        db.obj(vec![("value", value)])
    });
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["value"]));
    press(&mut m, &["t"]); // explicitly switch the untyped value's type
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    let before = m.top().unwrap().cursor;

    // "/" opens an empty search and keeps the cursor on the selected type
    press(&mut m, &["/"]);
    assert!(m.overlay.is_none());
    assert!(m.top().unwrap().searching);
    assert_eq!(m.top().unwrap().search_ti.value(), "");
    assert_eq!(m.top().unwrap().cursor, before);
    assert_eq!(m.top().unwrap().search_cursor, before);

    // "u" is a filter character, not undo
    press(&mut m, &["u"]);
    assert_eq!(m.top().unwrap().search_ti.value(), "u");
    press(&mut m, &["x"]);
    assert_eq!(m.top().unwrap().search_ti.value(), "ux");
    assert!(m.top().unwrap().searching);

    // no match keeps search mode alive so backspace can undo the query
    let rows = m.menu_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, RowKind::NoItems);
    assert!(rows[0].label.contains("no matching"));
    press(&mut m, &["backspace", "backspace"]);
    assert_eq!(m.top().unwrap().search_ti.value(), "");
    press(&mut m, &["esc"]);
    assert!(!m.top().unwrap().searching);
    assert!(m.top().unwrap().search_ti.value().is_empty());

    // a fresh search narrows to the schema-defined type
    press(&mut m, &["/", "S", "e", "r", "v", "e", "r"]);
    let rows = m.menu_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, RowKind::Variant);
    let variant = rows[0].variant.unwrap();
    assert_eq!(m.fs().schema.nodes[variant as usize].title, "Server");

    // enter selects the filtered type (confirming the replacement first)
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    let node = m.top().unwrap().node;
    assert_eq!(m.fs().schema.nodes[node as usize].key, "server");
    assert_eq!(m.fs().schema.nodes[node as usize].title, "Server");
}

#[test]
fn schema_oneof_picker_uses_variant_search() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "rule": {
              "oneOf": [
                {
                  "title": "DnsRule",
                  "type": "object",
                  "properties": {
                    "type": {"type": "string"},
                    "inbound": {"type": "string"},
                    "ip_version": {"type": "string"},
                    "query_type": {"type": "string"},
                    "extra": {"type": "string"}
                  }
                },
                {
                  "title": "HashRule",
                  "type": "object",
                  "properties": {"hash": {"type": "string"}}
                },
                {"title": "TextRule", "type": "string"}
              ]
            }
          },
          "$defs": {
            "Unused": {
              "title": "Unused",
              "type": "object",
              "properties": {"z": {"type": "string"}}
            }
          }
        }"#,
    );
    let doc = dbuild(|db| {
        let rule = db.obj(vec![
            ("type", db.s("dns")),
            ("inbound", db.s("in")),
            ("ip_version", db.s("4")),
            ("query_type", db.s("A")),
            ("extra", db.s("e")),
        ]);
        db.obj(vec![("rule", rule)])
    });
    let mut m = app_with(schema, doc, test_opts());
    set_cursor_path(&mut m, &sp(&["rule"]));
    press(&mut m, &["enter"]); // DnsRule editor
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    let st = m.top().unwrap().clone();
    let (_title, summary) = m.menu_title(&st);
    assert_eq!(
        summary, "DnsRule {type, inbound, ip_version, query_type, +1}",
        "the variant editor shows the type name in its bottom info"
    );
    press(&mut m, &["t"]); // back to the schema's variant picker
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);

    // only the oneOf's own variants: no built-in JSON types, no $defs
    let rows = m.menu_rows();
    let titles: Vec<String> = rows
        .iter()
        .filter_map(|r| r.variant)
        .map(|v| m.fs().schema.nodes[v as usize].title.clone())
        .collect();
    assert_eq!(titles, ["DnsRule", "HashRule", "TextRule"]);

    // the bottom variant line names the matched type
    let st = m.top().unwrap().clone();
    let (_title, summary) = m.menu_title(&st);
    assert_eq!(
        summary,
        "DnsRule {type, inbound, ip_version, query_type, +1}"
    );

    // the same search component as the any-type picker, over this list only
    press(&mut m, &["/", "H", "a", "s", "h"]);
    let rows = m.menu_rows();
    assert_eq!(rows.len(), 1);
    let v = rows[0].variant.unwrap();
    assert_eq!(m.fs().schema.nodes[v as usize].title, "HashRule");
    assert!(!rows.iter().any(|r| {
        r.variant
            .map(|v| m.fs().schema.nodes[v as usize].title == "Unused")
            .unwrap_or(false)
    }));
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
}

#[test]
fn unknown_array_add_string_edits_inline() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["a"]);
    match &mut m.overlay {
        Some(Overlay::Add(s)) => s.ti.set_value("arr"),
        _ => panic!("add-key input missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    // String, Number, Boolean, Object, Array -> Array
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);

    // < Add item > inserts a null slot, then the type picker opens
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m.top().unwrap().path, vec![ks("arr"), PathSeg::Idx(0)]);
    press(&mut m, &["enter"]); // String
    let rows = m.menu_rows();
    let rendered = match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.path, vec![ks("arr"), PathSeg::Idx(0)]);
            m.has_inline_edit_row(&rows, e)
        }
        _ => panic!("string editor missing"),
    };
    assert!(rendered, "arr[0] must get a rendered inline editor row");
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("hello"),
        _ => unreachable!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(sget(&m.fs().doc, &[ks("arr"), PathSeg::Idx(0)]), "hello");
}

#[test]
fn search_jump_to_variant_key_stays_on_row() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "rule": {
              "oneOf": [
                {
                  "title": "DnsRule",
                  "type": "object",
                  "properties": {"type": {"type": "string"}}
                },
                {"title": "TextRule", "type": "string"}
              ]
            }
          }
        }"#,
    );
    let doc = dbuild(|db| {
        let rule = db.obj(vec![("type", db.s("dns"))]);
        db.obj(vec![("rule", rule)])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "r", "u", "l", "e"]);
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.kind == RowKind::Field && r.path == vec![ks("rule")])
        .unwrap_or_else(|| {
            panic!(
                "variant key row in results: {:?}",
                search_state(&m)
                    .results
                    .iter()
                    .map(|r| (r.kind, r.path.clone(), r.label.clone()))
                    .collect::<Vec<_>>()
            )
        });
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        m.fs().stack.len(),
        1,
        "jump stays in the parent menu, not the old picker"
    );
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert_eq!(row_at(&mut m).path, vec![ks("rule")]);
    assert_eq!(row_at(&mut m).kind, RowKind::Drill);
}

#[test]
fn unknown_key_scalar_inline_edit() {
    let mut doc = test_doc();
    for (k, v) in [
        ("text", Value::Str("old".into())),
        ("count", Value::Num("42".into())),
        ("flag", Value::Bool(true)),
        ("nothing", Value::Null),
    ] {
        let n = doc.push(NodeData {
            value: v,
            ..NodeData::default()
        });
        let _ = doc.set(&sp(&[k]), n);
    }
    let objval = doc.new_object();
    let _ = doc.set(&sp(&["objval"]), objval);
    let a = doc.new_num("1");
    let _ = doc.insert_member(&sp(&["objval"]), "a", 0, a);
    let arr = doc.new_array();
    let one = doc.new_num("1");
    let _ = doc.set(&sp(&["arrval"]), arr);
    let _ = doc.insert(&sp(&["arrval"]), 0, one);
    let mut m = mk_app(doc.clone(), test_opts());

    // string: known type edits directly
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "text");
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.synth.as_ref().unwrap().json_type, "string");
            assert_eq!(e.ti.value(), "old");
        }
        _ => panic!("string value must edit in place"),
    }
    assert!(m.pending_edit.is_none());
    press(&mut m, &["ctrl+u", "n", "e", "w", "enter"]);
    assert_eq!(str_at(&m.fs().doc, &["text"]), "new");

    // number: known type edits directly; invalid text rejected
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "count");
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.synth.as_ref().unwrap().json_type, "number");
            assert_eq!(e.ti.value(), "42");
        }
        _ => panic!("number editor missing"),
    }
    press(&mut m, &["ctrl+u", "n", "o", "p", "enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.err, "must be a number"),
        _ => panic!("invalid number must stay in the editor"),
    }
    press(&mut m, &["ctrl+u", "4", "3", "enter"]);
    assert_eq!(num_at(&m.fs().doc, &["count"]), "43");

    // boolean toggles directly
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "flag");
    press(&mut m, &["enter"]);
    assert_eq!(bget(&m.fs().doc, &sp(&["flag"])), Some(false));
    press(&mut m, &["space"]);
    assert_eq!(bget(&m.fs().doc, &sp(&["flag"])), Some(true));

    // null: choose the Null type (last but one row)
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "nothing"
    });
    press(&mut m, &["enter", "G", "up", "enter"]);
    assert!(matches!(
        m.fs()
            .doc
            .value(getv(&m.fs().doc, &sp(&["nothing"])).unwrap()),
        Value::Null
    ));

    // object: known type drills into a schema-less object menu
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "objval"
    });
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    set_cursor(&mut m, |r| r.kind == RowKind::Unknown && r.label == "a");
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("2"),
        _ => panic!("nested number editor missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(num_at(&m.fs().doc, &["objval", "a"]), "2");
    press(&mut m, &["esc"]);

    // array: known type drills into an items menu
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "arrval"
    });
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["down", "enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("2"),
        _ => panic!("array item number editor missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(n_at_idx(&m.fs().doc, "arrval", 0), "2");
}

fn n_at_idx(doc: &Doc, key: &str, idx: usize) -> String {
    match getv(doc, &[ks(key), PathSeg::Idx(idx)]) {
        Some(n) => match doc.value(n) {
            Value::Num(s) => s.clone(),
            _ => "<missing>".into(),
        },
        None => "<missing>".into(),
    }
}

#[test]
fn external_raw_edit() {
    let mut m = mk_app(test_doc(), test_opts());
    set_cursor(&mut m, |r| r.kind == RowKind::Raw);
    press(&mut m, &["enter"]);
    assert!(m.pending_edit.as_ref().map_or(false, |e| e.whole_doc));

    // a successful edit replaces the document as one undo step
    m.finish_external_value(r#"{"name":"edited"}"#);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "edited");
    assert!(m.dirty());
    press(&mut m, &["u"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "webapp");

    // invalid JSON keeps the document and reports the parse error
    set_cursor(&mut m, |r| r.kind == RowKind::Raw);
    press(&mut m, &["enter"]);
    m.finish_external_value("{oops");
    assert_eq!(
        str_at(&m.fs().doc, &["name"]),
        "webapp",
        "invalid JSON must not replace the document"
    );
    assert!(m.notice.contains("invalid JSON"), "notice = {}", m.notice);

    // an editor failure keeps the document too
    set_cursor(&mut m, |r| r.kind == RowKind::Raw);
    press(&mut m, &["enter"]);
    m.external_edit_failed("boom");
    assert!(m.notice.contains("boom"), "notice = {}", m.notice);
}

#[test]
fn menu_save_flow() {
    // clean ctrl+s writes and stays
    let (opts, saves) = counting_opts();
    let mut m = mk_app(test_doc(), opts);
    press(&mut m, &["ctrl+s"]);
    assert_eq!(saves.load(Ordering::SeqCst), 1);
    assert!(m.fs().saved && !m.fs().with_errors && !m.exiting);

    // w aliases ctrl+s
    let (opts, saves_w) = counting_opts();
    let mut mw = mk_app(test_doc(), opts);
    press(&mut mw, &["w"]);
    assert_eq!(saves_w.load(Ordering::SeqCst), 1);
    assert!(mw.fs().saved && !mw.exiting);

    // validation errors block, then dismiss
    let mut m2 = mk_app(
        test_doc(),
        opts_validate(vec!["bad thing A".into(), "bad thing B".into()]),
    );
    press(&mut m2, &["ctrl+s"]);
    assert!(!m2.fs().saved && matches!(m2.overlay, Some(Overlay::Errs(_))));
    press(&mut m2, &["esc"]);
    assert!(m2.overlay.is_none(), "esc must dismiss the error overlay");

    // force path: y writes with errors and stays
    let mut fopts = opts_validate(vec!["bad thing A".into()]);
    fopts.force = true;
    {
        let c = Arc::new(AtomicUsize::new(0));
        fopts.save = Some(Arc::new(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
    }
    let mut m3 = mk_app(test_doc(), fopts);
    press(&mut m3, &["ctrl+s"]);
    press(&mut m3, &["y"]);
    assert!(m3.fs().saved && m3.fs().with_errors && !m3.exiting);

    // force path: n goes back
    let mut nopts = opts_validate(vec!["bad thing A".into()]);
    nopts.force = true;
    let mut m4 = mk_app(test_doc(), nopts);
    press(&mut m4, &["ctrl+s", "n"]);
    assert!(!m4.fs().saved && m4.overlay.is_none());

    // Save and exit writes and quits
    let (opts, exits) = counting_opts();
    let mut m5 = mk_app(test_doc(), opts);
    set_cursor(&mut m5, |r| r.kind == RowKind::Save);
    press(&mut m5, &["enter"]);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(m5.fs().saved && m5.exiting);
}

#[test]
fn jump_keys() {
    let mut m = mk_app(test_doc(), test_opts());
    let last = m.menu_rows().len() - 1;
    press(&mut m, &["G"]);
    assert_eq!(cursor_cursor(&mut m), last);
    press(&mut m, &["end"]);
    assert_eq!(cursor_cursor(&mut m), last);
    press(&mut m, &["g", "g"]);
    assert_eq!(cursor_cursor(&mut m), 0);
    press(&mut m, &["home"]);
    assert_eq!(cursor_cursor(&mut m), 0);

    // a lone g arms but does not move; another key disarms it
    press(&mut m, &["j"]);
    press(&mut m, &["g"]);
    assert_eq!(cursor_cursor(&mut m), 1);
    assert_eq!(m.cmd_buf, vec!["g".to_string()]);
    press(&mut m, &["j"]);
    assert_eq!(cursor_cursor(&mut m), 2);
    assert!(m.cmd_buf.is_empty());

    // an armed g still fires after a jump key
    press(&mut m, &["G", "g", "g"]);
    assert_eq!(cursor_cursor(&mut m), 0);

    // the buffer never expires
    press(&mut m, &["G", "g"]);
    assert_eq!(m.cmd_buf, vec!["g".to_string()]);
    press(&mut m, &["g"]);
    assert_eq!(cursor_cursor(&mut m), 0);

    // recording and a pending command coexist
    press(&mut m, &["g", "@"]);
    assert!(m.recording);
    press(&mut m, &["@"]);
    assert!(!m.recording);
    assert_eq!(
        m.cmd_buf,
        vec!["g".to_string()],
        "macro control must leave the pending command alone"
    );
    press(&mut m, &["g"]);
    assert_eq!(cursor_cursor(&mut m), 0);
    assert!(m.cmd_buf.is_empty());
}

#[test]
fn esc_clears_command_buffer() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["g"]);
    assert_eq!(m.cmd_buf.len(), 1);
    press(&mut m, &["esc"]);
    assert!(m.cmd_buf.is_empty());
    assert!(!m.exiting && !m.aborted && m.overlay.is_none() && m.fs().stack.len() == 1);

    // q aliases esc outside inputs
    press(&mut m, &["g", "q"]);
    assert!(m.cmd_buf.is_empty() && !m.exiting && !m.aborted);

    // without a pending combo esc keeps its meaning
    press(&mut m, &["esc"]);
    assert!(m.exiting);
}

#[test]
fn esc_clears_buffer_in_submenu_and_overlay() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["down", "down", "enter"]); // Server
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["g", "esc"]);
    assert!(m.cmd_buf.is_empty() && m.fs().stack.len() == 2);
    press(&mut m, &["esc"]);
    assert_eq!(m.fs().stack.len(), 1);

    // the issues overlay shares the g buffer
    let mut opts = test_opts();
    opts.load_errors = vec!["at '$.name': boom".into()];
    let mut mi = mk_app(test_doc(), opts);
    press(&mut mi, &["enter"]);
    assert!(matches!(mi.overlay, Some(Overlay::Issues(_))));
    press(&mut mi, &["g"]);
    assert_eq!(mi.cmd_buf.len(), 1);
    press(&mut mi, &["esc"]);
    assert!(
        matches!(mi.overlay, Some(Overlay::Issues(_))),
        "esc with a pending combo must not close the issues list"
    );
    assert!(mi.cmd_buf.is_empty());
    press(&mut mi, &["esc"]);
    assert!(mi.overlay.is_none());
}

#[test]
fn jump_keys_overlays() {
    // issues overlay
    let msgs = vec![
        "at '$.server.port': minimum value is 1024".to_string(),
        "at '$.name': got number, want string".to_string(),
    ];
    let mut opts = test_opts();
    opts.load_errors = msgs;
    let mut m = mk_app(test_doc(), opts);
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!("issues overlay should open"),
    }
    press(&mut m, &["G"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 1),
        _ => panic!(),
    }
    press(&mut m, &["end"]);
    press(&mut m, &["g", "g"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
    press(&mut m, &["home"]);
    press(&mut m, &["esc"]);

    // enum select overlay
    press(&mut m, &["down", "down"]); // issues row → Name → Mode
    press(&mut m, &["enter"]);
    let last = match &m.overlay {
        Some(Overlay::Sel(s)) => s.opts.len() - 1,
        _ => panic!("enum select should open"),
    };
    press(&mut m, &["G"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => assert_eq!(s.cursor, last),
        _ => panic!(),
    }
    press(&mut m, &["g", "g"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
    press(&mut m, &["esc"]);

    // save-error overlay
    let mut eopts = opts_validate(vec![
        "at '$.name': got number, want string".into(),
        "at '$.server.port': minimum value is 1024".into(),
        "at '$.mode': bad mode".into(),
    ]);
    eopts.force = true;
    eopts.save = Some(Arc::new(|_| Ok(())));
    let mut m2 = mk_app(test_doc(), eopts);
    press(&mut m2, &["ctrl+s"]);
    press(&mut m2, &["G"]);
    match &m2.overlay {
        Some(Overlay::Errs(s)) => assert_eq!(s.cursor, 2),
        _ => panic!("errs overlay should open"),
    }
    press(&mut m2, &["g", "g"]);
    match &m2.overlay {
        Some(Overlay::Errs(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
    press(&mut m2, &["home"]);
    press(&mut m2, &["end"]);
    match &m2.overlay {
        Some(Overlay::Errs(s)) => assert_eq!(s.cursor, 2),
        _ => panic!(),
    }
}

#[test]
fn menu_next_prev_non_null() {
    let mut m = mk_app(test_doc(), test_opts());
    let cursor = |m: &mut App| cursor_cursor(m);
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 2, "n from Name lands on Server");
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 4, "n from Server lands on Mounts");
    press(&mut m, &["n"]);
    assert_eq!(cursor(&mut m), 0, "n wraps to Name");
    press(&mut m, &["N"]);
    assert_eq!(cursor(&mut m), 4, "N wraps backwards to Mounts");

    // explicit nulls and schema-less nulls are skipped
    let mut doc = test_doc();
    let n = doc.new_null();
    let _ = doc.set(&sp(&["mode"]), n);
    let n = doc.new_null();
    let _ = doc.set(&sp(&["null_extra"]), n);
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["extra"]), n);
    let mut m = mk_app(doc, test_opts());
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        cursor(&mut m),
        1,
        "ctrl+j from the issues row lands on Name"
    );
    press(&mut m, &["ctrl+j"]);
    assert_eq!(cursor(&mut m), 3, "ctrl+j skips the explicit null mode");
    press(&mut m, &["ctrl+j"]);
    assert_eq!(cursor(&mut m), 5, "ctrl+j skips unset Logging/Auth");
    press(&mut m, &["ctrl+j"]);
    let r = row_at(&mut m);
    assert_eq!(r.kind, RowKind::Unknown);
    assert_eq!(
        r.path,
        sp(&["extra"]),
        "ctrl+j lands on the set unknown key"
    );
    press(&mut m, &["ctrl+j"]);
    assert_eq!(cursor(&mut m), 1, "ctrl+j wraps to Name");

    // no non-null keys: n/N leave the cursor alone
    let schema = schema_build(|b| {
        let a = b.id(scalar("a", "A", "string"));
        let bb = b.id(scalar("b", "B", "string"));
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![a, bb],
            ..FormNode::default()
        })
    });
    let mut empty = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut empty, &["n"]);
    assert_eq!(cursor(&mut empty), 0);
    press(&mut empty, &["N"]);
    assert_eq!(cursor(&mut empty), 0);

    // array items are entries too; null placeholders are skipped
    let schema = schema_build(|b| {
        let x = b.id(scalar("x", "X", "string"));
        let item = b.id(FormNode {
            key: "items".into(),
            kind: Kind::Object,
            children: vec![x],
            ..FormNode::default()
        });
        let items = b.id(FormNode {
            key: "items".into(),
            title: "Items".into(),
            kind: Kind::Array,
            items: Some(item),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![items],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| {
        db.obj(vec![(
            "items",
            db.arr(vec![
                db.push(Value::Null),
                db.obj(vec![("x", db.s("v"))]),
                db.obj(vec![("x", db.s("w"))]),
            ]),
        )])
    });
    let mut im = app_with(schema, doc, test_opts());
    press(&mut im, &["enter"]);
    press(&mut im, &["ctrl+j"]);
    assert_eq!(
        cursor_cursor(&mut im),
        2,
        "ctrl+j must skip a null array item"
    );
    press(&mut im, &["ctrl+j"]);
    assert_eq!(cursor_cursor(&mut im), 3);
    press(&mut im, &["ctrl+k"]);
    assert_eq!(cursor_cursor(&mut im), 2);
}

fn render_path(p: &[PathSeg]) -> String {
    p.iter()
        .map(|s| match s {
            PathSeg::Key(k) => k.clone(),
            PathSeg::Idx(i) => i.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[test]
fn parse_issue_missing_properties() {
    let plural = "at '$': missing properties 'name', 'description', 'runs'";
    let p = crate::ui::rows::parse_issue(plural);
    assert_eq!(
        p.as_ref().map(|p| jsondoc::path_string(p)),
        Some("$.name".to_string())
    );
    let got: Vec<String> = crate::ui::rows::issue_flag_paths(plural)
        .iter()
        .map(|f| jsondoc::path_string(f))
        .collect();
    assert_eq!(got.join(","), "$.name,$.description,$.runs");

    let single = "at '$.server': missing property 'port'";
    let p = crate::ui::rows::parse_issue(single);
    assert_eq!(
        p.as_ref().map(|p| jsondoc::path_string(p)),
        Some("$.server.port".to_string())
    );
    let got = crate::ui::rows::issue_flag_paths(single);
    assert_eq!(got.len(), 1);
    assert_eq!(jsondoc::path_string(&got[0]), "$.server.port");

    // parse_issue table (Go TestParseIssue)
    let cases: &[(&str, &str, bool)] = &[
        (
            "at '$.server.port': minimum value is 1024",
            "server/port",
            true,
        ),
        ("at '$': missing property 'port'", "port", true),
        (
            "at '$.inner': additional properties 'extra_key' not allowed",
            "inner/extra_key",
            true,
        ),
        (
            "at '$': additional properties 'a', 'b' not allowed",
            "a",
            true,
        ),
        ("at '$.a[0].b': got number, want string", "a/0/b", true),
        ("at '$.a/b': unexpected", "a/b", true),
        ("at '$': got string, want object", "", true),
        ("jsonschema validation failed", "", false),
    ];
    for (msg, want, ok) in cases {
        let p = crate::ui::rows::parse_issue(msg);
        assert_eq!(p.is_some(), *ok, "{msg}: ok");
        assert_eq!(render_path(p.as_deref().unwrap_or(&[])), *want, "{msg}");
    }
}

#[test]
fn save_error_jump_real_multiple_required() {
    let schema = schema_build(|b| {
        let mut name = scalar("name", "Name", "string");
        name.required = true;
        let mut desc = scalar("description", "Description", "string");
        desc.required = true;
        let mut runs = scalar("runs", "Runs", "integer");
        runs.required = true;
        let (name, desc, runs) = (b.id(name), b.id(desc), b.id(runs));
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![name, desc, runs],
            ..FormNode::default()
        })
    });
    let opts = opts_validate(vec![
        "at '$': missing properties 'name', 'description', 'runs'".into(),
    ]);
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), opts);
    press(&mut m, &["ctrl+s"]);
    assert!(matches!(m.overlay, Some(Overlay::Errs(_))));
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "enter must jump");
    assert_eq!(jsondoc::path_string(&m.top().unwrap().path), "$");
    let r = row_at(&mut m);
    assert_eq!(jsondoc::path_string(&r.path), "$.name");
}

#[test]
fn save_error_jump_missing_properties() {
    let cases = [
        (
            "root plural",
            "at '$': missing properties 'name', 'description', 'runs'",
            "$",
            "$.name",
        ),
        (
            "nested plural",
            "at '$.server': missing properties 'port', 'host'",
            "$.server",
            "$.server.port",
        ),
    ];
    for (name, msg, top, row) in cases {
        let mut m = mk_app(test_doc(), opts_validate(vec![msg.to_string()]));
        press(&mut m, &["ctrl+s"]);
        assert!(
            matches!(m.overlay, Some(Overlay::Errs(_))),
            "{name}: overlay"
        );
        press(&mut m, &["enter"]);
        assert!(m.overlay.is_none(), "{name}: enter must jump");
        assert_eq!(jsondoc::path_string(&m.top().unwrap().path), top, "{name}");
        let r = row_at(&mut m);
        assert_eq!(jsondoc::path_string(&r.path), row, "{name}");
    }
}

// ===========================================================================
// menu_paste_test.go
// ===========================================================================

fn paste_app(doc: Doc, text: &str) -> App {
    let mut opts = test_opts();
    let t = text.to_string();
    opts.paste = Some(Arc::new(move || Ok(t.clone())));
    app_with(test_schema(), doc, opts)
}

fn yaml_codec() -> RawCodec {
    RawCodec {
        label: "YAML",
        ext: ".yaml",
        marshal_doc: Arc::new(|d: &Doc| {
            crate::doc::yamldoc::write(d, None).map_err(|e| e.to_string())
        }),
        unmarshal_doc: Arc::new(|s: &str| {
            crate::doc::yamldoc::parse(s)
                .map(|(d, _)| d)
                .map_err(|e| e.to_string())
        }),
        adopt_doc: None,
        marshal_value: Arc::new(|_| Err("unused".into())),
        unmarshal_value: Arc::new(|s: &str| {
            crate::doc::yamldoc::parse(s)
                .map(|(d, _)| d)
                .map_err(|e| e.to_string())
        }),
    }
}

fn yaml_paste_app(doc: Doc, text: &str) -> App {
    let mut opts = test_opts();
    opts.raw = yaml_codec();
    let t = text.to_string();
    opts.paste = Some(Arc::new(move || Ok(t.clone())));
    app_with(test_schema(), doc, opts)
}

fn toml_codec() -> RawCodec {
    RawCodec {
        label: "TOML",
        ext: ".toml",
        marshal_doc: Arc::new(|d: &Doc| {
            crate::doc::tomldoc::write(d, &crate::doc::Meta::default()).map_err(|e| e.to_string())
        }),
        unmarshal_doc: Arc::new(|s: &str| {
            crate::doc::tomldoc::parse(s)
                .map(|(d, _)| d)
                .map_err(|e| e.to_string())
        }),
        adopt_doc: None,
        marshal_value: Arc::new(|r| {
            crate::doc::tomldoc::write_value(r.doc, r.id).map_err(|e| e.to_string())
        }),
        unmarshal_value: Arc::new(|s: &str| {
            crate::doc::tomldoc::parse_value(s)
                .map(|(d, _)| d)
                .map_err(|e| e.to_string())
        }),
    }
}

fn toml_paste_app(doc: Doc, text: &str) -> App {
    let mut opts = test_opts();
    opts.raw = toml_codec();
    let t = text.to_string();
    opts.paste = Some(Arc::new(move || Ok(t.clone())));
    app_with(test_schema(), doc, opts)
}

fn two_mount_doc() -> Doc {
    let mut doc = test_doc();
    let arr = doc.new_array();
    let _ = doc.set(&sp(&["mounts"]), arr);
    for (i, (src, dst)) in [("/a", "/1"), ("/b", "/2")].iter().enumerate() {
        let obj = doc.new_object();
        let _ = doc.insert(&sp(&["mounts"]), i, obj);
        let p = vec![ks("mounts"), PathSeg::Idx(i)];
        let s = doc.new_str(*src);
        let _ = doc.insert_member(&p, "src", 0, s);
        let d = doc.new_str(*dst);
        let _ = doc.insert_member(&p, "dst", 1, d);
    }
    doc
}

fn mount_src(doc: &Doc, i: usize) -> String {
    sget(doc, &[ks("mounts"), PathSeg::Idx(i), ks("src")])
}

#[test]
fn paste_partial_keys_member_order_and_undo() {
    let mut m = paste_app(test_doc(), r#""x":1,"y":"2","#);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 keys into $");
    assert_eq!(
        obj_keys(&m.fs().doc, &[]).join(" "),
        "name x y server mounts"
    );
    assert_eq!(num_at(&m.fs().doc, &["x"]), "1");
    assert_eq!(str_at(&m.fs().doc, &["y"]), "2");
    press(&mut m, &["u"]);
    assert!(
        nodes_equal_doc(&test_doc(), &m.fs().doc),
        "one undo must restore the pre-paste document"
    );
}

#[test]
fn paste_partial_keys_no_trailing_comma() {
    let mut m = paste_app(test_doc(), r#""a":1,"b":2"#);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 keys into $");
    assert_eq!(
        obj_keys(&m.fs().doc, &[]).join(" "),
        "name a b server mounts"
    );
    assert_eq!(num_at(&m.fs().doc, &["a"]), "1");
    assert_eq!(num_at(&m.fs().doc, &["b"]), "2");
}

#[test]
fn paste_full_object_into_drill_row() {
    let mut m = paste_app(test_doc(), r#"{"port":9090,"ratio":0.5}"#);
    press(&mut m, &["down", "down"]);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 2 keys into $.server");
    assert_eq!(
        obj_keys(&m.fs().doc, &sp(&["server"])).join(" "),
        "port host debug ratio"
    );
    assert_eq!(num_at(&m.fs().doc, &["server", "port"]), "9090");
    assert_eq!(num_at(&m.fs().doc, &["server", "ratio"]), "0.5");
}

#[test]
fn paste_full_object_onto_page_root() {
    let mut m = paste_app(test_doc(), r#"{"port":9090}"#);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 1 keys into $");
    assert_eq!(
        obj_keys(&m.fs().doc, &[]).join(" "),
        "name port server mounts"
    );
    assert_eq!(num_at(&m.fs().doc, &["port"]), "9090");
}

#[test]
fn paste_array_items_at_cursor() {
    let mut m = paste_app(two_mount_doc(), r#"1, 2, "three""#);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down", "down"]);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 3 items into $.mounts");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 5);
    assert_eq!(mount_src(&m.fs().doc, 0), "/a");
    assert_eq!(mount_src(&m.fs().doc, 1), "/b");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 2), "1");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 3), "2");
    assert_eq!(sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(4)]), "three");
}

#[test]
fn paste_full_array_onto_array_page() {
    let mut m = paste_app(two_mount_doc(), "[4,5]");
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 4);
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 2), "4");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 3), "5");
}

#[test]
fn paste_menu_vs_cursor_target() {
    let mut m = paste_app(test_doc(), r#"{"x":1}"#);
    press(&mut m, &["down", "down"]);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 1 keys into $");
    assert_eq!(obj_keys(&m.fs().doc, &[]).join(" "), "name server x mounts");
    assert_eq!(num_at(&m.fs().doc, &["x"]), "1");

    let mut m = paste_app(test_doc(), r#"{"y":2}"#);
    press(&mut m, &["down", "down"]);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 1 keys into $.server");
    assert_eq!(
        obj_keys(&m.fs().doc, &sp(&["server"])).join(" "),
        "port host debug y"
    );
    assert_eq!(num_at(&m.fs().doc, &["server", "y"]), "2");
}

#[test]
fn paste_into_unset_drill_row() {
    let mut m = paste_app(test_doc(), r#""levels":["debug"]"#);
    press(&mut m, &["down", "down", "down"]);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 1 keys into $.logging");
    assert_eq!(
        sget(&m.fs().doc, &[ks("logging"), ks("levels"), PathSeg::Idx(0)]),
        "debug"
    );
}

#[test]
fn paste_mismatch_notices() {
    // a complete mapping lands in an items page as one object item
    let mut m = paste_app(two_mount_doc(), r#"{"src":"/c","dst":"/d"}"#);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down"]);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 1 items into $.mounts");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 3);
    assert_eq!(mount_src(&m.fs().doc, 1), "/c");
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(1), ks("dst")]),
        "/d"
    );
    assert_eq!(mount_src(&m.fs().doc, 2), "/b");

    // P above the cursor item
    let mut m = paste_app(two_mount_doc(), r#"{"src":"/c","dst":"/d"}"#);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down"]);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 1 items into $.mounts");
    assert_eq!(mount_src(&m.fs().doc, 0), "/c");
    assert_eq!(mount_src(&m.fs().doc, 1), "/a");

    // array items pasted onto a mapping target are refused
    let mut m = paste_app(test_doc(), "1,2");
    press(&mut m, &["p"]);
    assert_eq!(
        m.notice,
        "clipboard holds array items, the target is a mapping"
    );
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));

    // enum arrays keep refusing key-value pairs
    let mut m = paste_app(test_doc(), r#"{"a":1}"#);
    press(&mut m, &["down", "down", "down", "enter", "enter"]);
    press(&mut m, &["p"]);
    assert_eq!(
        m.notice,
        "clipboard holds key-value pairs, the target is an array"
    );
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
}

#[test]
fn paste_unparseable_messages() {
    for text in [r#"{"a":"#, "garbage"] {
        let mut m = paste_app(test_doc(), text);
        press(&mut m, &["p"]);
        assert_eq!(
            m.notice, "clipboard is not JSON keys or items",
            "clipboard {text}"
        );
        assert!(
            nodes_equal_doc(&test_doc(), &m.fs().doc),
            "failed paste must not touch the document"
        );
    }
}

#[test]
fn paste_empty_clipboard_messages() {
    for text in ["", ","] {
        let mut m = paste_app(test_doc(), text);
        press(&mut m, &["p"]);
        assert_eq!(m.notice, "clipboard is empty", "clipboard {text:?}");
        assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
    }
}

#[test]
fn paste_clipboard_error() {
    let mut opts = test_opts();
    opts.paste = Some(Arc::new(|| Err("no clipboard tool".to_string())));
    let mut m = app_with(test_schema(), test_doc(), opts);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "paste failed: no clipboard tool");
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
}

#[test]
fn paste_uppercase_p() {
    let mut m = paste_app(test_doc(), r#""x":1,"y":"2","#);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 2 keys into $");
    assert_eq!(
        obj_keys(&m.fs().doc, &[]).join(" "),
        "x y name server mounts"
    );
}

#[test]
fn paste_complete_mapping_around_cursor() {
    let schema = schema_build(|b| {
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            ..FormNode::default()
        })
    });
    let mk = |text: &str| {
        let doc = dbuild(|db| db.obj(vec![("a", db.n("1")), ("b", db.n("2")), ("c", db.n("3"))]));
        let mut opts = test_opts();
        let t = text.to_string();
        opts.paste = Some(Arc::new(move || Ok(t.clone())));
        app_with(
            schema_build(|b| {
                b.id(FormNode {
                    key: "$".into(),
                    title: "$".into(),
                    kind: Kind::Object,
                    ..FormNode::default()
                })
            }),
            doc,
            opts,
        )
    };
    let _ = schema;

    // row 0 is the issues status row; down down lands on b
    let mut m = mk(r#"{"x":9,}"#);
    press(&mut m, &["down", "down"]);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 1 keys into $");
    assert_eq!(obj_keys(&m.fs().doc, &[]).join(" "), "a b x c");

    let mut m = mk(r#"{"x":9,}"#);
    press(&mut m, &["down", "down"]);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 1 keys into $");
    assert_eq!(obj_keys(&m.fs().doc, &[]).join(" "), "a x b c");
}

#[test]
fn paste_duplicate_key_in_clipboard() {
    let mut m = paste_app(test_doc(), r#""a":1,"a":2"#);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 keys into $");
    assert_eq!(obj_keys(&m.fs().doc, &[]).join(" "), "name a server mounts");
    assert_eq!(num_at(&m.fs().doc, &["a"]), "2");
}

#[test]
fn paste_no_changes() {
    let mut m = paste_app(test_doc(), r#"{"name":"webapp"}"#);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "paste made no changes");
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
}

#[test]
fn paste_yaml_block_pairs() {
    let mut m = yaml_paste_app(test_doc(), "x: 1\ny: \"2\"");
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 keys into $");
    assert_eq!(
        obj_keys(&m.fs().doc, &[]).join(" "),
        "name x y server mounts"
    );
    assert_eq!(num_at(&m.fs().doc, &["x"]), "1");
    assert_eq!(str_at(&m.fs().doc, &["y"]), "2");
}

#[test]
fn paste_yaml_flow_mapping() {
    let mut m = yaml_paste_app(test_doc(), "{x: 1}");
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 1 keys into $");
    assert_eq!(obj_keys(&m.fs().doc, &[]).join(" "), "name x server mounts");
    assert_eq!(num_at(&m.fs().doc, &["x"]), "1");
}

#[test]
fn paste_yaml_block_items() {
    let mut m = yaml_paste_app(two_mount_doc(), "- 1\n- 2\n- three");
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down", "down"]);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 3 items into $.mounts");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 5);
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 2), "1");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 3), "2");
    assert_eq!(sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(4)]), "three");
}

#[test]
fn paste_yaml_keeps_json_working() {
    let mut m = yaml_paste_app(test_doc(), r#""a":1,"b":2"#);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 keys into $");
    assert_eq!(
        obj_keys(&m.fs().doc, &[]).join(" "),
        "name a b server mounts"
    );
}

#[test]
fn paste_toml_block_pairs() {
    let mut m = toml_paste_app(test_doc(), "x = 1\ny = \"2\"");
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 keys into $");
    assert_eq!(
        obj_keys(&m.fs().doc, &[]).join(" "),
        "name x y server mounts"
    );
    assert_eq!(num_at(&m.fs().doc, &["x"]), "1");
    assert_eq!(str_at(&m.fs().doc, &["y"]), "2");
}

#[test]
fn paste_toml_value_array() {
    let mut m = toml_paste_app(two_mount_doc(), r#"[{ src = "/c" }, { src = "/d" }]"#);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down", "down"]);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 4);
    assert_eq!(
        sget(
            &m.fs().doc,
            &[ks("mounts"), PathSeg::Idx(2), PathSeg::Key("src".into())]
        ),
        "/c"
    );
}

#[test]
fn paste_toml_scalar_value() {
    let mut m = toml_paste_app(test_doc(), "1979-05-27T07:32:00Z");
    press(&mut m, &["p"]);
    // A bare value needs P on a value row; p only explains the rule.
    assert!(m.notice.contains("single value"), "notice = {}", m.notice);

    let mut m = toml_paste_app(test_doc(), "1979-05-27T07:32:00Z");
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.name");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "1979-05-27T07:32:00Z");
}

#[test]
fn paste_toml_plain_string_value() {
    // y/Y copies a string without quotes; P must accept that text back.
    let mut m = toml_paste_app(test_doc(), "hello world");
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.name");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "hello world");
}

#[test]
fn paste_yaml_detaches_anchors() {
    let mut m = yaml_paste_app(test_doc(), "x: &a 1\ny: *a");
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 keys into $");
    let x = getv(&m.fs().doc, &sp(&["x"])).expect("x");
    let y = getv(&m.fs().doc, &sp(&["y"])).expect("y");
    assert_eq!(n_at_idx(&m.fs().doc, "x", 0), "<missing>");
    let (nx, ny) = (m.fs().doc.value(x).clone(), m.fs().doc.value(y).clone());
    assert_eq!(nx, Value::Num("1".into()));
    assert_eq!(ny, Value::Num("1".into()));
    assert_eq!(
        m.fs().doc.node(x).anchor,
        "",
        "pasted anchors must dissolve"
    );
    assert_eq!(m.fs().doc.node(y).anchor, "");
    assert_ne!(x, y, "the alias must paste as an independent copy");
}

#[test]
fn paste_array_p_below_p_above() {
    let mk = |text: &str| {
        let mut m = paste_app(two_mount_doc(), text);
        press(&mut m, &["down", "down", "down", "down", "enter"]);
        press(&mut m, &["down", "down"]);
        m
    };

    let mut m = mk("8, 9");
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 4);
    assert_eq!(mount_src(&m.fs().doc, 1), "/b");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 2), "8");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 3), "9");

    let mut m = mk("8, 9");
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 1), "8");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 2), "9");
    assert_eq!(mount_src(&m.fs().doc, 3), "/b");

    let mut m = mk("[6,7,]");
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 1), "6");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 2), "7");
    assert_eq!(mount_src(&m.fs().doc, 3), "/b");

    let mut m = mk("[6,7,]");
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(mount_src(&m.fs().doc, 1), "/b");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 2), "6");

    // a cursor on < Add item > is the list end: p and P append
    let mut m = mk("8,");
    press(&mut m, &["up", "up"]);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 1 items into $.mounts");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 2), "8");
}

#[test]
fn paste_trailing_comma_in_complete_object() {
    let mut m = paste_app(test_doc(), r#"{"a":1,"b":"2",}"#);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 keys into $");
    assert_eq!(
        obj_keys(&m.fs().doc, &[]).join(" "),
        "name a b server mounts"
    );
    assert_eq!(num_at(&m.fs().doc, &["a"]), "1");
    assert_eq!(str_at(&m.fs().doc, &["b"]), "2");
}

#[test]
fn paste_trailing_comma_in_complete_array_appends() {
    let mut m = paste_app(two_mount_doc(), "[4,5,]");
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 4);
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 2), "4");
}

#[test]
fn paste_trailing_comma_still_rejects_garbage() {
    for text in [r#"{"a":1,,}"#, "[1,2,,]", "{1:2,}"] {
        let mut m = paste_app(test_doc(), text);
        press(&mut m, &["p"]);
        assert_eq!(
            m.notice, "clipboard is not JSON keys or items",
            "clipboard {text}"
        );
        assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
    }
}

#[test]
fn paste_yaml_scalar() {
    let mut m = yaml_paste_app(test_doc(), "hello");
    press(&mut m, &["p"]);
    assert_eq!(
        m.notice,
        "a single value replaces a row's value — press P on the row"
    );
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.name");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "hello");

    // a YAML clip is refused in JSON documents
    let mut m = paste_app(test_doc(), "x: 1");
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "clipboard is not JSON keys or items");
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
}

#[test]
fn paste_value_onto_cursor_row() {
    // JSON number replaces the scalar value in one step
    let mut m = paste_app(test_doc(), "123");
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.name");
    assert_eq!(num_at(&m.fs().doc, &["name"]), "123");
    press(&mut m, &["u"]);
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));

    // quoted JSON string is a single value too
    let mut m = paste_app(test_doc(), r#""hello""#);
    press(&mut m, &["P"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "hello");

    // array items: P on a scalar item replaces that item
    let schema = schema_build(|b| {
        let it = b.id(scalar("items", "", "number"));
        let items = b.id(FormNode {
            key: "items".into(),
            title: "Items".into(),
            kind: Kind::Array,
            items: Some(it),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![items],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("items", db.arr(vec![db.n("1"), db.n("2")]))]));
    let mut opts = test_opts();
    opts.paste = Some(Arc::new(|| Ok("99".to_string())));
    let mut m = app_with(schema, doc, opts);
    press(&mut m, &["enter"]);
    press(&mut m, &["down", "down"]);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.items[1]");
    assert_eq!(n_at_idx(&m.fs().doc, "items", 1), "99");
    assert_eq!(n_at_idx(&m.fs().doc, "items", 0), "1");

    // p and value-less rows refuse a single value
    let mut m = paste_app(test_doc(), "123");
    press(&mut m, &["p"]);
    assert_eq!(
        m.notice,
        "a single value replaces a row's value — press P on the row"
    );
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
    press(&mut m, &["G"]);
    press(&mut m, &["P"]);
    assert_eq!(
        m.notice,
        "a single value replaces a row's value — press P on the row"
    );
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
}

#[test]
fn batch_value_paste() {
    let mut m = paste_app(test_doc(), "123");
    press(&mut m, &["m", "down", "m"]);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into 2 keys");
    assert_eq!(num_at(&m.fs().doc, &["name"]), "123");
    assert_eq!(num_at(&m.fs().doc, &["mode"]), "123");
    press(&mut m, &["u"]);
    assert!(
        nodes_equal_doc(&test_doc(), &m.fs().doc),
        "one undo must restore the batch"
    );

    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.mode");
    assert_eq!(num_at(&m.fs().doc, &["mode"]), "123");
}

// ===========================================================================
// menu_merge_test.go
// ===========================================================================

const MERGE_SRC: &str = r#"jobs:
  build_base1: &base1
    env:
      env1: val1
  build_base2: &base2
    outputs:
      version: v1
  build2:
    <<: [*base1, *base2]
    name: Build Apple
"#;

fn yaml_doc(src: &str) -> Doc {
    crate::doc::yamldoc::parse(src).expect("yaml").0
}

fn merge_schema() -> Schema {
    schema_build(|b| {
        let name = b.id(scalar("name", "Name", "string"));
        let env1 = b.id(scalar("env1", "Env1", "string"));
        let env = b.id(FormNode {
            key: "env".into(),
            title: "Env".into(),
            kind: Kind::Object,
            children: vec![env1],
            ..FormNode::default()
        });
        let version = b.id(scalar("version", "Version", "string"));
        let outputs = b.id(FormNode {
            key: "outputs".into(),
            title: "Outputs".into(),
            kind: Kind::Object,
            children: vec![version],
            ..FormNode::default()
        });
        let job = b.id(FormNode {
            key: "job".into(),
            title: "Job".into(),
            kind: Kind::Object,
            children: vec![name, env, outputs],
            ..FormNode::default()
        });
        let jobs = b.id(FormNode {
            key: "jobs".into(),
            title: "Jobs".into(),
            kind: Kind::Map,
            items: Some(job),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Jobs".into(),
            kind: Kind::Object,
            children: vec![jobs],
            ..FormNode::default()
        })
    })
}

fn merge_model(src: &str) -> App {
    app_with(merge_schema(), yaml_doc(src), test_opts())
}

fn drill_build2(m: &mut App) {
    press(m, &["enter"]);
    press(m, &["down", "down", "enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "build2"]));
}

fn cursor_on_merged(m: &mut App, key: &str) {
    set_cursor(m, |r| r.label == key && !r.merged.is_empty());
}

fn find_row(m: &mut App, pred: impl Fn(&Row, &Schema) -> bool) -> Option<Row> {
    let rows = m.menu_rows();
    let schema = &m.fs().schema;
    rows.into_iter().find(|r| pred(r, schema))
}

#[test]
fn merge_rows_show_provenance() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    let rows = m.menu_rows();
    let schema = &m.fs().schema;
    let names: Vec<String> = rows
        .iter()
        .filter(|r| matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown))
        .map(|r| match r.fn_ {
            Some(f) => schema.nodes[f as usize].key.clone(),
            None => r.label.clone(),
        })
        .collect();
    assert_eq!(names.join(","), "name,env,outputs");

    let name = rows
        .iter()
        .find(|r| r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "name")
        .unwrap();
    assert!(name.merged.is_empty(), "own key must not be merged");
    assert_eq!(name.path, sp(&["jobs", "build2", "name"]));
    let env = rows
        .iter()
        .find(|r| r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "env")
        .unwrap();
    assert_eq!(env.merged, "base1");
    assert_eq!(env.path, sp(&["jobs", "build_base1", "env"]));
    let outputs = rows
        .iter()
        .find(|r| r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "outputs")
        .unwrap();
    assert_eq!(outputs.merged, "base2");

    // document layout does not change the schema-declared order
    let tail = MERGE_SRC.replace(
        "    <<: [*base1, *base2]\n    name: Build Apple\n",
        "    name: Build Apple\n    <<: [*base1, *base2]\n",
    );
    let mut m2 = merge_model(&tail);
    drill_build2(&mut m2);
    let rows = m2.menu_rows();
    let schema = &m2.fs().schema;
    let got: Vec<String> = rows
        .iter()
        .filter(|r| matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown))
        .map(|r| match r.fn_ {
            Some(f) => schema.nodes[f as usize].key.clone(),
            None => r.label.clone(),
        })
        .collect();
    assert_eq!(got.join(","), "name,env,outputs");
}

#[test]
fn merge_own_key_shadows_merged() {
    let src = r#"jobs:
  b1: &base1
    env:
      env1: from-anchor
  b2: &base2
    out: o
  child:
    <<: [*base1, *base2]
    env:
      env1: own-value
"#;
    let mut m = merge_model(src);
    // the unknown key in b2 gives the root a status row: navigate by path
    set_cursor_path(&mut m, &sp(&["jobs"]));
    press(&mut m, &["enter"]);
    set_cursor(&mut m, |r| r.label == "child");
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "child"]));
    let env = find_row(&mut m, |r, schema| {
        r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "env"
    })
    .expect("env row");
    assert!(env.merged.is_empty(), "own env must shadow the merged one");
    assert_eq!(env.path, sp(&["jobs", "child", "env"]));
}

#[test]
fn merge_earlier_anchor_wins() {
    let src = r#"jobs:
  b1: &base1
    env:
      env1: first
  b2: &base2
    env:
      env1: second
  child:
    <<: [*base1, *base2]
"#;
    let mut m = merge_model(src);
    set_cursor_path(&mut m, &sp(&["jobs"]));
    press(&mut m, &["enter"]);
    set_cursor(&mut m, |r| r.label == "child");
    press(&mut m, &["enter"]);
    let env = find_row(&mut m, |r, schema| {
        r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "env"
    })
    .expect("env row");
    assert_eq!(env.merged, "base1", "the earlier anchor must win");
}

#[test]
fn merge_breadcrumb_follows_cursor() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    assert_eq!(m.breadcrumb(), "$.jobs.build2");
    press(&mut m, &["down"]);
    assert_eq!(m.breadcrumb(), "$.jobs.build2 → $.jobs.build_base1");
    press(&mut m, &["down"]);
    assert_eq!(m.breadcrumb(), "$.jobs.build2 → $.jobs.build_base2");
    press(&mut m, &["up", "up"]);
    assert_eq!(m.breadcrumb(), "$.jobs.build2");
}

#[test]
fn merge_edit_writes_through_to_anchor() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    press(&mut m, &["down"]);
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "build_base1", "env"]));
    assert!(
        !has(&m.fs().doc, &sp(&["jobs", "build2", "env"])),
        "editing must not materialize a local copy"
    );

    press(&mut m, &["enter"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Edit(_))),
        "in-place edit did not open"
    );
    if let Some(Overlay::Edit(e)) = &mut m.overlay {
        e.ti.set_value("val2");
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &sp(&["jobs", "build_base1", "env", "env1"])),
        "val2"
    );
    assert!(!has(&m.fs().doc, &sp(&["jobs", "build2", "env"])));
}

fn bool_merge_model() -> App {
    let doc = yaml_doc("base: &b\n  flag: true\nchild:\n  <<: *b\n");
    let schema = schema_build(|b| {
        let flag = b.id(FormNode {
            key: "flag".into(),
            title: "Flag".into(),
            kind: Kind::Boolean,
            ..FormNode::default()
        });
        let base = b.id(FormNode {
            key: "base".into(),
            title: "Base".into(),
            kind: Kind::Object,
            children: vec![flag],
            ..FormNode::default()
        });
        let child = b.id(FormNode {
            key: "child".into(),
            title: "Child".into(),
            kind: Kind::Object,
            children: vec![flag],
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

#[test]
fn merge_toggle_writes_through_to_anchor() {
    let mut m = bool_merge_model();
    press(&mut m, &["down", "enter"]);
    press(&mut m, &["space"]);
    assert_eq!(
        bget(&m.fs().doc, &sp(&["base", "flag"])),
        Some(false),
        "space must toggle the anchor field"
    );
    assert!(
        !has(&m.fs().doc, &sp(&["child", "flag"])),
        "space must not materialize a local copy"
    );
}

fn required_merge_model() -> App {
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

#[test]
fn merge_delete_writes_through_to_anchor() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    press(&mut m, &["down", "d"]);
    assert!(
        !has(&m.fs().doc, &sp(&["jobs", "build_base1", "env"])),
        "d must delete the anchor field"
    );
    assert!(!has(&m.fs().doc, &sp(&["jobs", "build2", "env"])));
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "env"
            && !r.merged.is_empty())
        .is_none(),
        "merged env row must disappear"
    );

    // a required merged key refuses
    let mut m2 = required_merge_model();
    press(&mut m2, &["down", "enter"]);
    press(&mut m2, &["d"]);
    assert!(
        has(&m2.fs().doc, &sp(&["base", "name"])),
        "required merged key must survive d"
    );
    assert!(
        m2.notice.contains("Name is required"),
        "notice = {}",
        m2.notice
    );
}

#[test]
fn merge_expand_key() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    press(&mut m, &["down"]);
    press(&mut m, &["E"]);
    assert!(
        has(&m.fs().doc, &sp(&["jobs", "build2", "env"])),
        "E must materialize an own env member"
    );
    assert_eq!(
        sget(&m.fs().doc, &sp(&["jobs", "build_base1", "env", "env1"])),
        "val1"
    );
    assert!(m.notice.contains("merged key expanded"));
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "env"
            && !r.merged.is_empty())
        .is_none(),
        "expanded key must not be merged anymore"
    );
}

fn mrg_value(m: &App) -> String {
    match &m.overlay {
        Some(Overlay::Mrg(s)) => s.ti.value().to_string(),
        _ => panic!("merge input not open"),
    }
}

fn mrg_err(m: &App) -> String {
    match &m.overlay {
        Some(Overlay::Mrg(s)) => s.err.clone(),
        _ => String::new(),
    }
}

fn merge_refs(doc: &Doc, path: &[PathSeg]) -> Vec<String> {
    match doc.get(path) {
        Some(n) => doc
            .merge_refs(n)
            .iter()
            .map(|&r| doc.node(r).anchor.clone())
            .collect(),
        None => Vec::new(),
    }
}

#[test]
fn merge_input_edits_merge_list() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    press(&mut m, &["<"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Mrg(_))),
        "< must open the merge-keys input"
    );
    assert_eq!(mrg_value(&m), "base1 base2");

    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("base");
    }
    press(&mut m, &["tab"]);
    let v = mrg_value(&m);
    assert!(v == "base1 " || v == "base2 ", "tab completion = {v:?}");
    press(&mut m, &["tab"]);
    let v = mrg_value(&m);
    assert!(
        v == "base1 base2 " || v == "base2 base1 ",
        "second tab = {v:?}"
    );
    press(&mut m, &["enter"]);
    assert_eq!(merge_refs(&m.fs().doc, &sp(&["jobs", "build2"])).len(), 2);
    assert!(m.overlay.is_none(), "enter closes the merge input");

    // "*" prefixes and commas are accepted
    press(&mut m, &["<"]);
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("*base2, *base1");
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        merge_refs(&m.fs().doc, &sp(&["jobs", "build2"])),
        vec!["base2", "base1"]
    );

    // a single-anchor merge
    press(&mut m, &["<"]);
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("base2");
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "enter must close the input");
    assert_eq!(
        merge_refs(&m.fs().doc, &sp(&["jobs", "build2"])),
        vec!["base2"]
    );
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "env"
            && !r.merged.is_empty())
        .is_none(),
        "env comes from base1 and must be gone"
    );
    assert!(find_row(&mut m, |r, _| r.label == "outputs" && r.merged == "base2").is_some());

    // empty enter clears the merges
    press(&mut m, &["<"]);
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("");
    }
    press(&mut m, &["enter"]);
    assert!(merge_refs(&m.fs().doc, &sp(&["jobs", "build2"])).is_empty());
    assert!(find_row(&mut m, |r, _| !r.merged.is_empty()).is_none());

    // adding merges back
    press(&mut m, &["<"]);
    assert_eq!(mrg_value(&m), "");
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("base1");
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        merge_refs(&m.fs().doc, &sp(&["jobs", "build2"])),
        vec!["base1"]
    );
    assert!(find_row(&mut m, |r, _| r.label == "env" && r.merged == "base1").is_some());
}

#[test]
fn merge_input_errors() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    press(&mut m, &["<"]);
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("nope");
    }
    press(&mut m, &["enter"]);
    assert!(
        mrg_err(&m).contains("unknown anchor"),
        "err = {}",
        mrg_err(&m)
    );
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("base1 base2");
    }
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none(), "esc must close the input");

    // a merge cycle
    let cyc = r#"jobs:
  a: &b1
    name: from-a
  b: &b2
    <<: *b1
    name: from-b
"#;
    let mut m2 = merge_model(cyc);
    press(&mut m2, &["enter", "enter"]);
    assert_eq!(m2.top().unwrap().path, sp(&["jobs", "a"]));
    press(&mut m2, &["<"]);
    if let Some(Overlay::Mrg(s)) = &mut m2.overlay {
        s.ti.set_value("b2");
    }
    press(&mut m2, &["enter"]);
    assert!(mrg_err(&m2).contains("circular"), "err = {}", mrg_err(&m2));
}

#[test]
fn merge_self_reference_refused() {
    let src = r#"jobs:
  a: &b1
    name: x
"#;
    let mut m = merge_model(src);
    press(&mut m, &["enter", "enter"]);
    press(&mut m, &["<"]);
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("b1");
    }
    press(&mut m, &["enter"]);
    assert!(mrg_err(&m).contains("itself"), "err = {}", mrg_err(&m));
}

#[test]
fn merge_anchor_deletion_guards() {
    let mut m = merge_model(MERGE_SRC);
    press(&mut m, &["enter"]); // jobs map menu
    press(&mut m, &["d"]);
    assert!(
        has(&m.fs().doc, &sp(&["jobs", "build_base1"])),
        "anchored mapping must survive d"
    );
    assert!(
        m.notice.contains("merge key referencing"),
        "notice = {}",
        m.notice
    );

    // clearing the anchor via & is refused
    press(&mut m, &["&"]);
    assert!(matches!(m.overlay, Some(Overlay::Anc(_))));
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("");
    }
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Anc(s)) => assert!(s.err.contains("merge key"), "err = {}", s.err),
        _ => panic!("anchor removal must be refused"),
    }
    press(&mut m, &["esc"]);

    // removing the merge first frees the anchor
    press(&mut m, &["down", "down", "enter"]);
    press(&mut m, &["<"]);
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("");
    }
    press(&mut m, &["enter"]);
    press(&mut m, &["esc"]);
    press(&mut m, &["up", "up"]);
    press(&mut m, &["&"]);
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("");
    }
    press(&mut m, &["enter"]);
    assert!(
        m.overlay.is_none(),
        "anchor removal must succeed after the merge is gone"
    );
    let n = getv(&m.fs().doc, &sp(&["jobs", "build_base1"])).unwrap();
    assert_eq!(m.fs().doc.node(n).anchor, "");
}

#[test]
fn merge_undo_restores_merge_list() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    press(&mut m, &["<"]);
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("");
    }
    press(&mut m, &["enter"]);
    assert!(merge_refs(&m.fs().doc, &sp(&["jobs", "build2"])).is_empty());
    press(&mut m, &["u"]);
    assert_eq!(
        merge_refs(&m.fs().doc, &sp(&["jobs", "build2"])).len(),
        2,
        "undo must restore the merge list"
    );
}

#[test]
fn merge_summary_shows_effective_content() {
    let mut m = merge_model(MERGE_SRC);
    press(&mut m, &["enter"]); // jobs map
    let r = find_row(&mut m, |r, _| r.label == "build2").expect("build2 row");
    let ctx = m.ctx();
    let s = crate::ui::rows::summary(&ctx, r.fn_.unwrap(), &r.path);
    assert!(s.contains("name=Build Apple"), "summary = {s}");
    assert!(s.contains("env={…}"), "summary = {s}");
}

#[test]
fn merge_ga_jumps_to_anchor_carrier() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    cursor_on_merged(&mut m, "env");
    press(&mut m, &["g", "a"]);
    assert!(m.fs().stack.len() >= 2);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "build_base1"]));
    let r = row_at(&mut m);
    assert_eq!(r.path, sp(&["jobs", "build_base1", "env"]));
    press(&mut m, &["esc"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "build2"]));
}

#[test]
fn merge_gr_jumps_to_other_merge_users() {
    let src = r#"jobs:
  build_base1: &base1
    env:
      env1: val1
  build3:
    <<: *base1
    name: other
  build2:
    <<: *base1
    name: Build Apple
"#;
    let mut m = merge_model(src);
    drill_build2(&mut m);
    cursor_on_merged(&mut m, "env");
    press(&mut m, &["g", "r"]);
    assert_eq!(
        m.top().unwrap().path,
        sp(&["jobs", "build3"]),
        "notice {}",
        m.notice
    );
}

#[test]
fn merge_gr_picks_among_several_users() {
    let src = r#"jobs:
  build_base1: &base1
    env:
      env1: val1
  build3:
    <<: *base1
  build4:
    <<: *base1
  direct: *base1
  build2:
    <<: *base1
"#;
    let mut m = merge_model(src);
    press(&mut m, &["enter"]);
    set_cursor(&mut m, |r| r.label == "build2");
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "build2"]));
    cursor_on_merged(&mut m, "env");

    press(&mut m, &["g", "r"]);
    let got: Vec<String> = match &m.overlay {
        Some(Overlay::RefPick(p)) => p.paths.iter().map(|p| jsondoc::path_string(p)).collect(),
        _ => panic!("several users must open the picker, notice {}", m.notice),
    };
    assert_eq!(got, vec!["$.jobs.build3", "$.jobs.build4", "$.jobs.direct"]);
    if let Some(Overlay::RefPick(p)) = &mut m.overlay {
        p.cursor = 1;
    }
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "build4"]));
}

#[test]
fn merge_gr_without_other_users() {
    let mut m = merge_model(MERGE_SRC);
    drill_build2(&mut m);
    cursor_on_merged(&mut m, "env");
    press(&mut m, &["g", "r"]);
    assert!(m.overlay.is_none(), "no picker without other users");
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "build2"]));
    assert!(
        m.notice.contains("no other references to &base1"),
        "notice = {}",
        m.notice
    );
}

#[test]
fn gr_on_reference_includes_merge_users() {
    let src = "base: &x\n  p: 1\ncopy: *x\nmerger:\n  <<: *x\n";
    let doc = yaml_doc(src);
    let schema = schema_build(|b| {
        let p = b.id(scalar("p", "P", "integer"));
        let base = b.id(FormNode {
            key: "base".into(),
            title: "Base".into(),
            kind: Kind::Object,
            children: vec![p],
            ..FormNode::default()
        });
        let copy = b.id(FormNode {
            key: "copy".into(),
            title: "Copy".into(),
            kind: Kind::Object,
            children: vec![p],
            ..FormNode::default()
        });
        let merger = b.id(FormNode {
            key: "merger".into(),
            title: "Merger".into(),
            kind: Kind::Object,
            children: vec![p],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "X".into(),
            kind: Kind::Object,
            children: vec![base, copy, merger],
            ..FormNode::default()
        })
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["down", "g", "r"]);
    assert_eq!(
        m.top().unwrap().path,
        sp(&["merger"]),
        "notice {}",
        m.notice
    );
}

fn fresh_mapping_model() -> App {
    let doc = yaml_doc("base: &b\n  env:\n    env1: v1\nsettings:\n");
    let schema = schema_build(|b| {
        let mk_env = |b: &mut TB| {
            let env1 = b.id(scalar("env1", "Env1", "string"));
            b.id(FormNode {
                key: "env".into(),
                title: "Env".into(),
                kind: Kind::Object,
                children: vec![env1],
                ..FormNode::default()
            })
        };
        let e1 = mk_env(b);
        let base = b.id(FormNode {
            key: "base".into(),
            title: "Base".into(),
            kind: Kind::Object,
            children: vec![e1],
            ..FormNode::default()
        });
        let e2 = mk_env(b);
        let settings = b.id(FormNode {
            key: "settings".into(),
            title: "Settings".into(),
            kind: Kind::Object,
            children: vec![e2],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![base, settings],
            ..FormNode::default()
        })
    });
    app_with(schema, doc, test_opts())
}

#[test]
fn merge_creates_missing_mapping() {
    let mut m = fresh_mapping_model();
    press(&mut m, &["down", "enter"]);
    press(&mut m, &["<"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Mrg(_))),
        "merge input must open on an unset mapping"
    );
    press(&mut m, &["esc"]);
    let made_obj = getv(&m.fs().doc, &sp(&["settings"]))
        .map_or(false, |n| matches!(m.fs().doc.value(n), Value::Object(_)));
    assert!(!made_obj, "cancelling must not create the mapping");
    press(&mut m, &["<"]);
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("b");
    }
    press(&mut m, &["enter"]);
    let n = getv(&m.fs().doc, &sp(&["settings"])).expect("settings created");
    assert!(matches!(m.fs().doc.value(n), Value::Object(_)));
    assert_eq!(merge_refs(&m.fs().doc, &sp(&["settings"])), vec!["b"]);
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "env"
            && r.merged == "b")
        .is_some()
    );
}

#[test]
fn delete_expanded_merged_key_allowed_when_required() {
    let mut m = required_merge_model();
    press(&mut m, &["down", "enter"]);
    press(&mut m, &["E"]);
    assert!(
        has(&m.fs().doc, &sp(&["child", "name"])),
        "E must materialize the merged name"
    );
    press(&mut m, &["d"]);
    assert!(
        !has(&m.fs().doc, &sp(&["child", "name"])),
        "the expanded copy must be deletable"
    );
    assert!(!m.notice.contains("required"), "notice = {}", m.notice);
    assert!(find_row(&mut m, |r, _| r.label == "name" && r.merged == "b").is_some());
}

#[test]
fn merge_chain_rows_follow_the_defining_anchor() {
    let src = r#"jobs:
  b1: &b1
    name: from-b1
    env:
      env1: v1
  b2: &b2
    <<: *b1
    name: from-b2
  job1:
    <<: *b2
"#;
    let mut m = merge_model(src);
    press(&mut m, &["enter"]);
    set_cursor(&mut m, |r| r.label == "job1");
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "job1"]));
    let name_row = find_row(&mut m, |r, schema| {
        r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "name"
    })
    .unwrap();
    assert_eq!(name_row.merged, "b2");
    assert_eq!(name_row.path, sp(&["jobs", "b2", "name"]));
    let ctx = m.ctx();
    assert_eq!(
        crate::ui::rows::summary(&ctx, name_row.fn_.unwrap(), &name_row.path),
        "from-b2"
    );
    let env_row = find_row(&mut m, |r, schema| {
        r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "env"
    })
    .unwrap();
    assert_eq!(env_row.merged, "b1");
    assert_eq!(env_row.path, sp(&["jobs", "b1", "env"]));

    // the chain-provided key is expandable and deletable even if required
    if let Some(st) = m.top_mut() {
        st.cursor = 0;
    }
    press(&mut m, &["E"]);
    press(&mut m, &["d"]);
    assert!(
        !has(&m.fs().doc, &sp(&["jobs", "job1", "name"])),
        "expanded copy must be deletable"
    );
    assert!(find_row(&mut m, |r, _| r.label == "name" && r.merged == "b2").is_some());
}

fn merge_variant_model(src: &str) -> App {
    let doc = yaml_doc(src);
    let schema = schema_build(|b| {
        let name = b.id(scalar("name", "Name", "string"));
        let mut runs_on = scalar("runs-on", "Runs on", "string");
        runs_on.required = true;
        let runs_on = b.id(runs_on);
        let run = b.id(scalar("run", "Run", "string"));
        let steps_items = b.id(FormNode {
            kind: Kind::Object,
            children: vec![run],
            ..FormNode::default()
        });
        let steps = b.id(FormNode {
            key: "steps".into(),
            title: "Steps".into(),
            kind: Kind::Array,
            items: Some(steps_items),
            ..FormNode::default()
        });
        let normal = b.id(FormNode {
            key: "job".into(),
            title: "Normal".into(),
            kind: Kind::Object,
            children: vec![name, runs_on, steps],
            ..FormNode::default()
        });
        let name2 = b.id(scalar("name", "Name", "string"));
        let mut uses = scalar("uses", "Uses", "string");
        uses.required = true;
        let uses = b.id(uses);
        let reusable = b.id(FormNode {
            key: "job".into(),
            title: "Reusable".into(),
            kind: Kind::Object,
            children: vec![name2, uses],
            ..FormNode::default()
        });
        let one_of = b.id(FormNode {
            kind: Kind::OneOf,
            variants: vec![normal, reusable],
            ..FormNode::default()
        });
        let jobs = b.id(FormNode {
            key: "jobs".into(),
            title: "Jobs".into(),
            kind: Kind::Map,
            items: Some(one_of),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![jobs],
            ..FormNode::default()
        })
    });
    app_with(schema, doc, test_opts())
}

#[test]
fn merge_drives_variant_inference() {
    let mut m = merge_variant_model(
        "jobs:\n  base: &b\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n  build:\n    <<: *b\n",
    );
    press(&mut m, &["enter"]);
    press(&mut m, &["down", "enter"]);
    let runs_on = find_row(&mut m, |r, schema| {
        r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "runs-on"
    });
    let runs_on = runs_on.expect("merged runs-on must identify the normal variant");
    assert_eq!(runs_on.merged, "b");
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "uses")
        .is_none(),
        "the other variant's required field must not leak"
    );

    // a missing required key stays visible and the variant stays pinned
    let mut m2 = merge_variant_model(
        "jobs:\n  base: &b\n    name: base\n    steps:\n      - run: echo hi\n  build:\n    <<: *b\n",
    );
    press(&mut m2, &["enter"]);
    press(&mut m2, &["down", "enter"]);
    let r = find_row(&mut m2, |r, schema| {
        r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "runs-on"
    })
    .expect("runs-on row");
    assert!(
        !has(&m2.fs().doc, &r.path),
        "missing required runs-on must stay visible"
    );
}

#[test]
fn merge_ga_lands_on_defining_field() {
    let src = "jobs:\n  b1: &b1\n    name: from-b1\n  build2:\n    <<: *b1\n";
    let mut m = merge_model(src);
    press(&mut m, &["enter"]);
    press(&mut m, &["down", "enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "build2"]));
    cursor_on_merged(&mut m, "name");
    press(&mut m, &["g", "a"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "b1"]));
    let r = row_at(&mut m);
    assert_eq!(r.path, sp(&["jobs", "b1", "name"]));
    assert!(
        r.merged.is_empty(),
        "the defining row is the anchor's own key"
    );
    press(&mut m, &["esc"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "build2"]));
}

#[test]
fn merge_ga_follows_the_chain_to_the_defining_field() {
    let src = "jobs:\n  b1: &b1\n    env:\n      env1: v1\n  b2: &b2\n    <<: *b1\n    name: from-b2\n  job1:\n    <<: *b2\n";
    let mut m = merge_model(src);
    press(&mut m, &["enter"]);
    set_cursor(&mut m, |r| r.label == "job1");
    press(&mut m, &["enter"]);
    cursor_on_merged(&mut m, "env");
    press(&mut m, &["g", "a"]);
    assert_eq!(m.top().unwrap().path, sp(&["jobs", "b1"]));
    assert_eq!(row_at(&mut m).path, sp(&["jobs", "b1", "env"]));
}

#[test]
fn merge_completion_filters_candidates() {
    let doc = yaml_doc(
        "base: &b\n  x: 1\nscalar: &s hello\narr: &a\n  - 1\n  - 2\njob: &j\n  <<: *b\n  name: j\nwrap: &w\n  <<: *j\n  k: v\n",
    );
    let schema = schema_build(|b| {
        let x = b.id(scalar("x", "X", "integer"));
        let base = b.id(FormNode {
            key: "base".into(),
            title: "Base".into(),
            kind: Kind::Object,
            children: vec![x],
            ..FormNode::default()
        });
        let scal = b.id(scalar("scalar", "Scalar", "string"));
        let item = b.id(scalar("arr", "", "integer"));
        let arr = b.id(FormNode {
            key: "arr".into(),
            title: "Arr".into(),
            kind: Kind::Array,
            items: Some(item),
            ..FormNode::default()
        });
        let name = b.id(scalar("name", "Name", "string"));
        let job = b.id(FormNode {
            key: "job".into(),
            title: "Job".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        });
        let k = b.id(scalar("k", "K", "string"));
        let wrap = b.id(FormNode {
            key: "wrap".into(),
            title: "Wrap".into(),
            kind: Kind::Object,
            children: vec![k],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![base, scal, arr, job, wrap],
            ..FormNode::default()
        })
    });
    let mut m = app_with(schema, doc, test_opts());
    let rows = m.menu_rows();
    let i = rows
        .iter()
        .position(|r| r.fn_.is_some() && m.fs().schema.nodes[r.fn_.unwrap() as usize].key == "job")
        .unwrap();
    m.top_mut().unwrap().cursor = i;
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["job"]));
    press(&mut m, &["<"]);
    match &m.overlay {
        Some(Overlay::Mrg(s)) => assert_eq!(s.cands.join(","), "b", "merge candidates"),
        _ => panic!("merge input did not open"),
    }
    if let Some(Overlay::Mrg(s)) = &mut m.overlay {
        s.ti.set_value("B");
    }
    let s = match m.overlay.take() {
        Some(Overlay::Mrg(s)) => s,
        _ => panic!(),
    };
    assert_eq!(
        merge_filtered(&s).join(","),
        "b",
        "filtering is case-insensitive"
    );
    m.overlay = Some(Overlay::Mrg(s));
    press(&mut m, &["tab"]);
    assert_eq!(mrg_value(&m), "b ", "tab must complete the mapping anchor");
}

// ===========================================================================
// menu_alias_test.go
// ===========================================================================

fn alias_schema() -> Schema {
    schema_build(|b| {
        let p = b.id(scalar("p", "P", "integer"));
        let base = b.id(FormNode {
            key: "base".into(),
            title: "Base".into(),
            kind: Kind::Object,
            children: vec![p],
            ..FormNode::default()
        });
        let copy = b.id(FormNode {
            key: "copy".into(),
            title: "Copy".into(),
            kind: Kind::Object,
            children: vec![p],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Aliases".into(),
            kind: Kind::Object,
            children: vec![base, copy],
            ..FormNode::default()
        })
    })
}

fn alias_model(src: &str) -> App {
    app_with(alias_schema(), yaml_doc(src), test_opts())
}

fn drill_copy(m: &mut App) {
    press(m, &["down", "enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["copy"]));
}

fn anc_err(m: &App) -> String {
    match &m.overlay {
        Some(Overlay::Anc(s)) => s.err.clone(),
        _ => String::new(),
    }
}

#[test]
fn alias_menu_offers_expand_on_top() {
    let mut m = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    drill_copy(&mut m);
    let rows = m.menu_rows();
    assert_eq!(rows[0].kind, RowKind::Expand);
    assert_eq!(rows[0].label, "< Expand >");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].kind, RowKind::Field);
    assert_eq!(m.fs().schema.nodes[rows[1].fn_.unwrap() as usize].key, "p");

    // the anchored original is not a reference: no Expand there
    press(&mut m, &["esc"]);
    press(&mut m, &["up", "enter"]);
    assert_eq!(m.top().unwrap().path, sp(&["base"]));
    assert!(find_row(&mut m, |r, _| r.kind == RowKind::Expand).is_none());
}

#[test]
fn alias_edit_writes_anchored_original() {
    let mut m = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    drill_copy(&mut m);
    press(&mut m, &["down", "enter"]);
    if let Some(Overlay::Edit(e)) = &mut m.overlay {
        e.ti.set_value("2");
    }
    press(&mut m, &["enter"]);
    assert_eq!(num_at(&m.fs().doc, &["base", "p"]), "2");
    assert_eq!(num_at(&m.fs().doc, &["copy", "p"]), "2");
    assert_eq!(
        getv(&m.fs().doc, &sp(&["base"])),
        getv(&m.fs().doc, &sp(&["copy"])),
        "edit must keep the alias sharing"
    );
}

#[test]
fn alias_expand_detaches_copy() {
    let mut m = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    drill_copy(&mut m);
    press(&mut m, &["enter"]);
    let cp = getv(&m.fs().doc, &sp(&["copy"])).unwrap();
    assert_eq!(m.fs().doc.node(cp).anchor, "");
    assert!(find_row(&mut m, |r, _| r.kind == RowKind::Expand).is_none());

    press(&mut m, &["down", "enter"]);
    if let Some(Overlay::Edit(e)) = &mut m.overlay {
        e.ti.set_value("3");
    }
    press(&mut m, &["enter"]);
    assert_eq!(num_at(&m.fs().doc, &["copy", "p"]), "3");
    assert_eq!(num_at(&m.fs().doc, &["base", "p"]), "1");

    let out =
        crate::doc::yamldoc::write(&m.fs().doc, Some(&Default::default())).expect("yaml write");
    assert_eq!(out, "base: &x\n  p: 1\ncopy:\n  p: 3\n");
}

#[test]
fn alias_expand_shortcut_e() {
    let mut m = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    drill_copy(&mut m);
    press(&mut m, &["E"]);
    let cp = getv(&m.fs().doc, &sp(&["copy"])).unwrap();
    assert_eq!(m.fs().doc.node(cp).anchor, "");
    assert!(alias_at(&m.fs().doc, &sp(&["copy"])).is_none());

    // E on a reference row in the parent menu expands that row
    let mut m2 = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    press(&mut m2, &["down"]);
    press(&mut m2, &["E"]);
    let cp = getv(&m2.fs().doc, &sp(&["copy"])).unwrap();
    assert_eq!(m2.fs().doc.node(cp).anchor, "");
    let b = getv(&m2.fs().doc, &sp(&["base"])).unwrap();
    assert_eq!(m2.fs().doc.node(b).anchor, "x");

    // without a reference under the cursor e does nothing
    let mut m3 = alias_model("base: &x\n  p: 1\n");
    press(&mut m3, &["e"]);
    let b = getv(&m3.fs().doc, &sp(&["base"])).unwrap();
    assert_eq!(m3.fs().doc.node(b).anchor, "x");
}

#[test]
fn anchor_set_and_rename() {
    let mut m = alias_model("base:\n  p: 1\n");
    press(&mut m, &["&"]);
    assert!(matches!(m.overlay, Some(Overlay::Anc(_))));
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("x");
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "enter must commit the anchor input");
    let base = getv(&m.fs().doc, &sp(&["base"])).unwrap();
    assert_eq!(m.fs().doc.node(base).anchor, "x");

    // a reference sharing the node, then renaming through it
    let _ = m.fs_mut().doc.set(&sp(&["copy"]), base);
    press(&mut m, &["down", "&"]);
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("y");
    }
    press(&mut m, &["enter"]);
    assert_eq!(m.fs().doc.node(base).anchor, "y");
    assert_eq!(getv(&m.fs().doc, &sp(&["copy"])), Some(base));
}

#[test]
fn anchor_delete_rules() {
    let mut m = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    press(&mut m, &["&"]);
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("");
    }
    press(&mut m, &["enter"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Anc(_))),
        "deleting a referenced anchor must be refused"
    );
    assert!(anc_err(&m).contains("referenced"), "err = {}", anc_err(&m));
    press(&mut m, &["esc"]);

    press(&mut m, &["down", "*"]);
    match &m.overlay {
        Some(Overlay::Anc(s)) => assert_eq!(s.ti.value(), "x"),
        _ => panic!("reference input must open"),
    }
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("");
    }
    press(&mut m, &["enter"]);
    assert!(
        !has(&m.fs().doc, &sp(&["copy"])),
        "empty reference input removes the reference"
    );

    press(&mut m, &["up", "&"]);
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("");
    }
    press(&mut m, &["enter"]);
    let base = getv(&m.fs().doc, &sp(&["base"])).unwrap();
    assert_eq!(m.fs().doc.node(base).anchor, "");
}

#[test]
fn reference_set_with_completion() {
    let mut m = alias_model("base: &x\n  p: 1\n");
    press(&mut m, &["down"]);
    press(&mut m, &["*"]);
    match &m.overlay {
        Some(Overlay::Anc(s)) => assert_eq!(s.cands, vec!["x".to_string()]),
        _ => panic!("* must open the reference input"),
    }
    press(&mut m, &["tab"]);
    match &m.overlay {
        Some(Overlay::Anc(s)) => assert_eq!(s.ti.value(), "x"),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        getv(&m.fs().doc, &sp(&["copy"])),
        getv(&m.fs().doc, &sp(&["base"]))
    );

    // editing through the new reference writes the anchored original
    press(&mut m, &["enter"]);
    press(&mut m, &["down", "enter"]);
    if let Some(Overlay::Edit(e)) = &mut m.overlay {
        e.ti.set_value("9");
    }
    press(&mut m, &["enter"]);
    assert_eq!(num_at(&m.fs().doc, &["base", "p"]), "9");
    assert_eq!(num_at(&m.fs().doc, &["copy", "p"]), "9");
}

#[test]
fn reference_set_refusals() {
    let mut m = alias_model("base: &x\n  p: 1\ncopy:\n  p: 2\n");
    press(&mut m, &["enter"]);
    press(&mut m, &["*"]);
    assert!(matches!(m.overlay, Some(Overlay::Anc(_))));
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("x");
    }
    press(&mut m, &["enter"]);
    assert!(anc_err(&m).contains("circular"), "err = {}", anc_err(&m));
    press(&mut m, &["esc"]);
    press(&mut m, &["esc"]);
    assert_eq!(m.top().unwrap().path, sp(&[]));

    // renaming an anchor to a name another node holds is refused
    press(&mut m, &["down", "&"]);
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("y");
    }
    press(&mut m, &["enter"]);
    let copy = getv(&m.fs().doc, &sp(&["copy"])).unwrap();
    assert_eq!(m.fs().doc.node(copy).anchor, "y");
    press(&mut m, &["up", "&"]);
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("y");
    }
    press(&mut m, &["enter"]);
    assert!(
        anc_err(&m).contains("already in use"),
        "err = {}",
        anc_err(&m)
    );
    press(&mut m, &["esc"]);
    let base = getv(&m.fs().doc, &sp(&["base"])).unwrap();
    assert_eq!(m.fs().doc.node(base).anchor, "x");
}

#[test]
fn anchor_jump_ga_gr() {
    let four = "base: &x\n  p: 1\ncopy: *x\nagain: *x\nthird: *x\n";

    // ga jumps to the carrier and esc returns
    let mut m = alias_jump_model(four, &["base", "copy", "again", "third"]);
    set_cursor(&mut m, |r| r.path == sp(&["copy"]));
    press(&mut m, &["g", "a"]);
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(m.top().unwrap().path, sp(&["base"]));
    press(&mut m, &["esc"]);
    assert_eq!(m.fs().stack.len(), 1);
    assert!(m.fs().jump_origin.is_none());
    assert_eq!(row_at(&mut m).path, sp(&["copy"]));

    // q returns too
    let mut m = alias_jump_model(four, &["base", "copy", "again", "third"]);
    set_cursor(&mut m, |r| r.path == sp(&["copy"]));
    press(&mut m, &["g", "a", "q"]);
    assert_eq!(m.fs().stack.len(), 1);
    assert_eq!(row_at(&mut m).path, sp(&["copy"]));

    // gr with one other reference jumps directly
    let mut m = alias_jump_model(
        "base: &x\n  p: 1\ncopy: *x\nagain: *x\n",
        &["base", "copy", "again"],
    );
    set_cursor(&mut m, |r| r.path == sp(&["again"]));
    press(&mut m, &["g", "r"]);
    assert!(
        m.overlay.is_none(),
        "a single target jumps without the picker"
    );
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(m.top().unwrap().path, sp(&["copy"]));

    // several references open the picker
    let mut m = alias_jump_model(four, &["base", "copy", "again", "third"]);
    set_cursor(&mut m, |r| r.path == sp(&["copy"]));
    press(&mut m, &["g", "r"]);
    match &m.overlay {
        Some(Overlay::RefPick(p)) => assert_eq!(p.paths.len(), 2),
        _ => panic!("two targets must open the picker"),
    }
    press(&mut m, &["down", "enter"]);
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(m.top().unwrap().path, sp(&["third"]));
    press(&mut m, &["esc"]);
    assert_eq!(m.fs().stack.len(), 1);
    assert_eq!(row_at(&mut m).path, sp(&["copy"]));

    // gr from the carrier lists all references and esc just closes
    let mut m = alias_jump_model(four, &["base", "copy", "again", "third"]);
    set_cursor(&mut m, |r| r.path == sp(&["base"]));
    press(&mut m, &["g", "r"]);
    match &m.overlay {
        Some(Overlay::RefPick(p)) => assert_eq!(p.paths.len(), 3),
        _ => panic!("carrier gr must pick among 3 references"),
    }
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.fs().stack.len(), 1);

    // notices
    let mut m = alias_jump_model(four, &["base", "copy", "again", "third"]);
    set_cursor(&mut m, |r| r.path == sp(&["base"]));
    press(&mut m, &["g", "a"]);
    assert!(
        m.notice.contains("already at the anchor &x"),
        "notice = {}",
        m.notice
    );
    press(&mut m, &["enter"]);
    press(&mut m, &["g", "a"]);
    assert!(
        m.notice.contains("not on an anchor or reference"),
        "notice = {}",
        m.notice
    );
    press(&mut m, &["g", "r"]);
    assert!(
        m.notice.contains("not on an anchor or reference"),
        "notice = {}",
        m.notice
    );
    press(&mut m, &["esc"]);

    // gr with no other references
    let mut m = alias_jump_model("base: &x\n  p: 1\ncopy: *x\n", &["base", "copy"]);
    set_cursor(&mut m, |r| r.path == sp(&["copy"]));
    press(&mut m, &["g", "r"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.fs().stack.len(), 1);
    assert!(
        m.notice.contains("no other references to &x"),
        "notice = {}",
        m.notice
    );

    // plain a and r keys are unaffected
    let mut m = alias_jump_model(four, &["base", "copy", "again", "third"]);
    set_cursor(&mut m, |r| r.path == sp(&["again"]));
    press(&mut m, &["enter"]);
    press(&mut m, &["a"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Add(_))),
        "plain a must still open the add-key input"
    );
    press(&mut m, &["esc", "esc"]);
    press(&mut m, &["r"]);
    match &m.overlay {
        Some(Overlay::Search(s)) => assert!(s.replace),
        _ => panic!("plain r must still open the replace search"),
    }
}

fn alias_jump_model(src: &str, keys: &[&str]) -> App {
    let doc = yaml_doc(src);
    let schema = schema_build(|b| {
        let mut children = Vec::new();
        for k in keys {
            let p = b.id(scalar("p", "P", "integer"));
            let title = format!("{}{}", &k[..1].to_uppercase(), &k[1..]);
            children.push(b.id(FormNode {
                key: (*k).into(),
                title,
                kind: Kind::Object,
                children: vec![p],
                ..FormNode::default()
            }));
        }
        b.id(FormNode {
            key: "$".into(),
            title: "Aliases".into(),
            kind: Kind::Object,
            children,
            ..FormNode::default()
        })
    });
    app_with(schema, doc, test_opts())
}

fn path_marked(paths: &[Vec<PathSeg>], p: &[PathSeg]) -> bool {
    paths.iter().any(|q| is_path_prefix(p, q))
}

fn alias_issue_model(validate: Vec<String>) -> App {
    let doc = yaml_doc("base: &b\n  a: 1\nfield:\n  b: x\n");
    let schema = schema_build(|b| {
        let a = b.id(scalar("a", "A", "integer"));
        let base = b.id(FormNode {
            key: "base".into(),
            title: "Base".into(),
            kind: Kind::Object,
            children: vec![a],
            ..FormNode::default()
        });
        let mut bb = scalar("b", "B", "string");
        bb.required = true;
        let bb = b.id(bb);
        let field = b.id(FormNode {
            key: "field".into(),
            title: "Field".into(),
            kind: Kind::Object,
            children: vec![bb],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![base, field],
            ..FormNode::default()
        })
    });
    let mut opts = test_opts();
    opts.validate = Some(Arc::new(move |_: &Doc| validate.clone()));
    app_with(schema, doc, opts)
}

fn point_field_at_reference(m: &mut App) {
    set_cursor_path(m, &sp(&["field"]));
    press(m, &["*"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Anc(_))),
        "reference input did not open"
    );
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("b");
    }
    press(m, &["enter"]);
}

#[test]
fn alias_warning_reaches_parent_row() {
    let mut m = alias_issue_model(vec![]);
    assert!(m.fs().issues_now.is_empty());
    point_field_at_reference(&mut m);

    // the new status row must not shift the cursor off the edited row
    assert_eq!(row_at(&mut m).path, sp(&["field"]));
    assert!(
        path_marked(&m.fs().warn_paths, &sp(&["field"])),
        "warn paths = {:?}",
        m.fs().warn_paths
    );
    let rows = m.menu_rows();
    assert!(
        rows[0].label.contains("1 warning"),
        "status label = {}",
        rows[0].label
    );

    // inside the field menu the unknown key is listed
    press(&mut m, &["enter"]);
    assert!(find_row(&mut m, |r, _| r.kind == RowKind::Unknown && r.label == "a").is_some());
}

#[test]
fn alias_error_reaches_parent_row() {
    // recompute against the referenced content
    let mut oopts = test_opts();
    oopts.validate = Some(Arc::new(|d: &Doc| {
        if has(d, &sp(&["field", "a"])) {
            vec!["at '$.field.a': got integer, want string".to_string()]
        } else {
            Vec::new()
        }
    }));
    // rebuild the app around the same schema/doc fixture with that validator
    let doc = yaml_doc("base: &b\n  a: 1\nfield:\n  b: x\n");
    let mut b = TB::new();
    let a = b.id(scalar("a", "A", "integer"));
    let base = b.id(FormNode {
        key: "base".into(),
        title: "Base".into(),
        kind: Kind::Object,
        children: vec![a],
        ..FormNode::default()
    });
    let mut bb = scalar("b", "B", "string");
    bb.required = true;
    let bb = b.id(bb);
    let field = b.id(FormNode {
        key: "field".into(),
        title: "Field".into(),
        kind: Kind::Object,
        children: vec![bb],
        ..FormNode::default()
    });
    let root = b.id(FormNode {
        key: "$".into(),
        title: "Root".into(),
        kind: Kind::Object,
        children: vec![base, field],
        ..FormNode::default()
    });
    b.root = root;
    let mut m = app_with(b.finish(), doc, oopts);
    point_field_at_reference(&mut m);
    assert!(
        path_marked(&m.fs().err_paths, &sp(&["field"])),
        "err paths = {:?}",
        m.fs().err_paths
    );
}

#[test]
fn alias_expand_undoable() {
    let mut m = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    drill_copy(&mut m);
    press(&mut m, &["enter"]);
    let cp = getv(&m.fs().doc, &sp(&["copy"])).unwrap();
    assert_eq!(m.fs().doc.node(cp).anchor, "");
    assert!(m.dirty(), "expand must mark the document dirty");
    press(&mut m, &["u"]);
    assert!(
        alias_at(&m.fs().doc, &sp(&["copy"])).is_some(),
        "undo must restore the reference"
    );
    assert!(!m.dirty());
    assert!(find_row(&mut m, |r, _| r.kind == RowKind::Expand).is_some());
}

#[test]
fn anchor_edit_undoable_and_dirty() {
    let mut m = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    press(&mut m, &["&"]);
    assert!(matches!(m.overlay, Some(Overlay::Anc(_))));
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("renamed");
    }
    press(&mut m, &["enter"]);
    let base = getv(&m.fs().doc, &sp(&["base"])).unwrap();
    assert_eq!(m.fs().doc.node(base).anchor, "renamed");
    assert!(m.dirty(), "anchor rename must be dirty");
    press(&mut m, &["u"]);
    assert_eq!(m.fs().doc.node(base).anchor, "x");
    assert!(!m.dirty());
}

fn completion_model(src: &str) -> App {
    let doc = yaml_doc(src);
    let schema = schema_build(|b| {
        let sname = b.id(scalar("sname", "Sname", "string"));
        let snum = b.id(scalar("snum", "Snum", "integer"));
        let k = b.id(scalar("k", "K", "string"));
        let sobj = b.id(FormNode {
            key: "sobj".into(),
            title: "Sobj".into(),
            kind: Kind::Object,
            children: vec![k],
            ..FormNode::default()
        });
        let mut senum = fnode("senum", "Senum", Kind::Enum);
        senum.enum_ = tenum(b, &["a", "b"]);
        let senum = b.id(senum);
        let target = b.id(scalar("target", "Target", "string"));
        let int_target = b.id(scalar("intTarget", "Int target", "integer"));
        let k2 = b.id(scalar("k", "K", "string"));
        let obj_target = b.id(FormNode {
            key: "objTarget".into(),
            title: "Obj target".into(),
            kind: Kind::Object,
            children: vec![k2],
            ..FormNode::default()
        });
        let mut enum_target = fnode("enumTarget", "Enum target", Kind::Enum);
        enum_target.enum_ = tenum(b, &["a", "b"]);
        let enum_target = b.id(enum_target);
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![
                sname,
                snum,
                sobj,
                senum,
                target,
                int_target,
                obj_target,
                enum_target,
            ],
            ..FormNode::default()
        })
    });
    app_with(schema, doc, test_opts())
}

fn open_reference_on(m: &mut App, key: &str) {
    if m.overlay.is_some() {
        press(m, &["esc"]);
    }
    let rows = m.menu_rows();
    let i = rows
        .iter()
        .position(|r| r.fn_.is_some() && m.fs().schema.nodes[r.fn_.unwrap() as usize].key == key)
        .unwrap_or_else(|| panic!("row {key} not found"));
    m.top_mut().unwrap().cursor = i;
    press(m, &["*"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Anc(_))),
        "* did not open on {key}"
    );
}

fn anc_cands(m: &App) -> Vec<String> {
    match &m.overlay {
        Some(Overlay::Anc(s)) => s.cands.clone(),
        _ => Vec::new(),
    }
}

#[test]
fn reference_completion_filters_by_type() {
    let mut m =
        completion_model("sname: &n1 hello\nsnum: &n2 42\nsobj: &n3\n  k: v\nsenum: &n4 a\n");
    for (field, want) in [
        ("target", "n1,n4"),
        ("intTarget", "n2"),
        ("objTarget", "n3"),
        ("enumTarget", "n4"),
    ] {
        open_reference_on(&mut m, field);
        assert_eq!(anc_cands(&m).join(","), want, "candidates for {field}");
    }
    // a schema-less key has no type to check: every anchor is offered
    let mut m2 = completion_model("sname: &n1 hello\nsnum: &n2 42\nextra: x\n");
    set_cursor(&mut m2, |r| {
        r.kind == RowKind::Unknown && r.label == "extra"
    });
    press(&mut m2, &["*"]);
    assert_eq!(anc_cands(&m2).join(","), "n1,n2");
}

#[test]
fn reference_completion_follows_input() {
    let mut m = completion_model("sname: &alpha one\nsnum: &beta 2\nsobj: &gamma\n  k: v\n");
    open_reference_on(&mut m, "target");
    assert_eq!(anc_cands(&m).join(","), "alpha");

    open_reference_on(&mut m, "senum");
    assert!(anc_cands(&m).is_empty());
    if let Some(Overlay::Anc(s)) = &mut m.overlay {
        s.ti.set_value("al");
    }
    let filtered = match &m.overlay {
        Some(Overlay::Anc(s)) => App::anchor_filtered(s),
        _ => Vec::new(),
    };
    assert!(filtered.is_empty());
    press(&mut m, &["tab"]);
    match &m.overlay {
        Some(Overlay::Anc(s)) => assert_eq!(
            s.ti.value(),
            "al",
            "tab without candidates changed the input"
        ),
        _ => panic!(),
    }
    press(&mut m, &["esc"]);

    let mut m2 =
        completion_model("sname: &alpha one\nsnum: &beta 2\nsobj: &gamma\n  k: v\nextra: x\n");
    set_cursor(&mut m2, |r| {
        r.kind == RowKind::Unknown && r.label == "extra"
    });
    press(&mut m2, &["*"]);
    assert_eq!(anc_cands(&m2).join(","), "alpha,beta,gamma");
    if let Some(Overlay::Anc(s)) = &mut m2.overlay {
        s.ti.set_value("be");
    }
    let filtered = match &m2.overlay {
        Some(Overlay::Anc(s)) => App::anchor_filtered(s),
        _ => Vec::new(),
    };
    assert_eq!(filtered.join(","), "beta");
    press(&mut m2, &["tab"]);
    match &m2.overlay {
        Some(Overlay::Anc(s)) => assert_eq!(s.ti.value(), "beta"),
        _ => panic!(),
    }
    if let Some(Overlay::Anc(s)) = &mut m2.overlay {
        s.ti.set_value("g");
    }
    let filtered = match &m2.overlay {
        Some(Overlay::Anc(s)) => App::anchor_filtered(s),
        _ => Vec::new(),
    };
    assert_eq!(filtered.join(","), "gamma");
    if let Some(Overlay::Anc(s)) = &mut m2.overlay {
        s.ti.set_value("AL");
    }
    press(&mut m2, &["tab"]);
    match &m2.overlay {
        Some(Overlay::Anc(s)) => assert_eq!(s.ti.value(), "alpha", "case-insensitive completion"),
        _ => panic!(),
    }
}

// ===========================================================================
// menu_variant_infer_test.go
// ===========================================================================

fn infer_schema() -> (Schema, FormNodeId, FormNodeId) {
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

fn mstate(kind: MenuKind, node: FormNodeId, path: Vec<PathSeg>) -> MenuState {
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
        search_ti: Default::default(),
        searching: false,
        search_cursor: 0,
    }
}

fn infer_app() -> App {
    let (schema, _, _) = infer_schema();
    app_with(schema, dbuild(|db| db.obj(vec![])), test_opts())
}

fn variant_titles(schema: &Schema, cands: &[FormNodeId]) -> String {
    cands
        .iter()
        .map(|&c| schema.nodes[c as usize].title.clone())
        .collect::<Vec<_>>()
        .join(",")
}

fn body_val(f: impl FnOnce(&DB) -> NodeId) -> (Doc, NodeId) {
    let db = DB::new();
    let v = f(&db);
    (db.finish(v), v)
}

#[test]
fn matching_variants() {
    let (schema, one_of, _) = infer_schema();
    let cases: Vec<(&str, Option<(Doc, NodeId)>, &str)> = vec![
        ("nil value", None, "variant 1,variant 2"),
        (
            "discriminator direct",
            Some(body_val(|db| db.obj(vec![("type", db.s("direct"))]))),
            "variant 1",
        ),
        (
            "discriminator trojan plus field",
            Some(body_val(|db| {
                db.obj(vec![("type", db.s("trojan")), ("password", db.s("x"))])
            })),
            "variant 2",
        ),
        (
            "shared field only is ambiguous",
            Some(body_val(|db| db.obj(vec![("listen", db.s("1.2.3.4"))]))),
            "variant 1,variant 2",
        ),
        (
            "undefined key ignored",
            Some(body_val(|db| {
                db.obj(vec![("type", db.s("direct")), ("password", db.s("x"))])
            })),
            "variant 1",
        ),
    ];
    for (name, val, want) in cases {
        let got = match &val {
            None => variant_titles(
                &schema,
                &crate::ui::rows::matching_variants(&schema, one_of, &Doc::new(), None),
            ),
            Some((doc, v)) => variant_titles(
                &schema,
                &crate::ui::rows::matching_variants(&schema, one_of, doc, Some(*v)),
            ),
        };
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn current_variant_value_aware() {
    let (schema, one_of, _) = infer_schema();
    let (d1, v1) = body_val(|db| db.obj(vec![("type", db.s("direct")), ("listen", db.s("::"))]));
    assert_eq!(
        crate::ui::rows::current_variant(&schema, one_of, &d1, Some(v1)),
        0
    );
    let (d2, v2) = body_val(|db| db.obj(vec![("type", db.s("trojan"))]));
    assert_eq!(
        crate::ui::rows::current_variant(&schema, one_of, &d2, Some(v2)),
        1
    );

    // examples-style mixed oneOf keeps its semantics
    let examples = test_schema();
    let auth = child(&examples, examples.root, "auth");
    let (ds, vs) = body_val(|db| db.s("none"));
    assert_eq!(
        crate::ui::rows::current_variant(&examples, auth, &ds, Some(vs)),
        0
    );
    let (dt, vt) = body_val(|db| db.obj(vec![("token", db.s("t"))]));
    assert_eq!(
        crate::ui::rows::current_variant(&examples, auth, &dt, Some(vt)),
        1
    );
    assert_eq!(
        crate::ui::rows::current_variant(&examples, auth, &Doc::new(), None),
        -1
    );
}

fn schema_from_json(src: &str) -> Schema {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("schema.json");
    std::fs::write(&p, src).expect("schema write");
    crate::smodel::load(p.to_str().expect("path")).expect("schema load")
}

#[test]
fn null_only_variant_does_not_shadow_object() {
    let root_schema = schema_from_json(
        r#"{
	  "type": "object",
	  "properties": {
	    "push": {"oneOf": [
	      {"type": "null"},
	      {"type": "object", "properties": {
	        "tags": {"type": "array", "items": {"type": "string"}}}}]}}}"#,
    );
    let push = child(&root_schema, root_schema.root, "push");
    assert_eq!(root_schema.nodes[push as usize].variants.len(), 2);
    let nullv = root_schema.nodes[push as usize].variants[0];
    assert_eq!(root_schema.nodes[nullv as usize].kind, Kind::Raw);
    assert!(root_schema.nodes[nullv as usize].null_only);

    let (dn, vn) = body_val(|db| db.push(Value::Null));
    assert!(crate::ui::rows::variant_matches(
        &root_schema,
        nullv,
        &dn,
        Some(vn)
    ));
    let (dv, vv) = body_val(|db| db.obj(vec![("tags", db.arr(vec![db.s("v*")]))]));
    assert!(!crate::ui::rows::variant_matches(
        &root_schema,
        nullv,
        &dv,
        Some(vv)
    ));
    assert_eq!(
        crate::ui::rows::current_variant(&root_schema, push, &dv, Some(vv)),
        1
    );

    let doc = dbuild(|db| {
        db.obj(vec![(
            "push",
            db.obj(vec![("tags", db.arr(vec![db.s("v*")]))]),
        )])
    });
    let mut m = app_with(root_schema, doc, test_opts());
    press(&mut m, &["enter"]);
    assert!(
        !matches!(m.overlay, Some(Overlay::Text(_))),
        "push must not open the raw textarea"
    );
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert!(find_row(&mut m, |r, _| r.path == sp(&["push", "tags"])).is_some());
}

#[test]
fn shared_ref_row_labels() {
    let schema = schema_from_json(
        r##"{
  "type": "object",
  "properties": {
    "push": {"type": "object", "properties": {
      "branches": {"$ref": "#/definitions/branch"},
      "branches-ignore": {"$ref": "#/definitions/branch"},
      "tags": {"$ref": "#/definitions/branch"},
      "tags-ignore": {"$ref": "#/definitions/branch"},
      "paths": {"$ref": "#/definitions/path"},
      "paths-ignore": {"$ref": "#/definitions/path"}
    }}
  },
  "definitions": {
    "branch": {"$ref": "#/definitions/globs"},
    "path": {"$ref": "#/definitions/globs"},
    "globs": {"type": "array", "items": {"type": "string"}}
  }
}"##,
    );
    let doc = dbuild(|db| {
        db.obj(vec![(
            "push",
            db.obj(vec![("branches", db.arr(vec![db.s("main")]))]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    let labels: Vec<String> = m
        .menu_rows()
        .iter()
        .map(|r| match r.fn_ {
            Some(f) => m.fs().schema.nodes[f as usize].key.clone(),
            None => r.label.clone(),
        })
        .collect();
    for label in [
        "branches",
        "branches-ignore",
        "tags",
        "tags-ignore",
        "paths",
        "paths-ignore",
    ] {
        assert!(
            labels.iter().any(|l| l == label),
            "row label {label:?} missing: {labels:?}"
        );
    }
}

#[test]
fn one_of_item_add_infer() {
    let (schema, one_of, _) = infer_schema();
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut m, &["enter"]);
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().variant_of, Some(one_of));
    let rows = m.menu_rows();
    let keys: Vec<String> = rows
        .iter()
        .filter_map(|r| r.fn_.map(|f| m.fs().schema.nodes[f as usize].key.clone()))
        .collect();
    assert!(keys.iter().any(|k| k == "type"));
    assert!(keys.iter().any(|k| k == "listen"));
    assert!(keys.iter().any(|k| k == "password"));

    press(&mut m, &["down", "enter"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Sel(_))),
        "type picker should open"
    );
    if let Some(Overlay::Sel(s)) = &m.overlay {
        assert!(
            s.opts.iter().any(|o| o.display == "trojan"),
            "union discriminator"
        );
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("inbounds"), PathSeg::Idx(0), ks("type")]),
        "direct"
    );
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "password")
        .is_none()
    );
    let r = row_at(&mut m);
    assert_eq!(m.fs().schema.nodes[r.fn_.unwrap() as usize].key, "type");

    // switching in place keeps the union discriminator
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => assert!(s.opts.iter().any(|o| o.display == "trojan")),
        _ => panic!("pinned menu must keep the union discriminator"),
    }
    press(&mut m, &["down", "enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("inbounds"), PathSeg::Idx(0), ks("type")]),
        "trojan"
    );
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "password")
        .is_some()
    );
    let r = row_at(&mut m);
    assert_eq!(m.fs().schema.nodes[r.fn_.unwrap() as usize].key, "type");
}

#[test]
fn one_of_union_enum_keeps_all_discriminator_options() {
    // sing-box RuleSet: the inline variant allows enum ["inline", ""] while
    // local/remote are consts. The merged type picker must offer all of them.
    let schema = schema_from_json(
        r#"{
      "type": "object",
      "properties": {
        "route": {"type": "object", "properties": {
          "rule_set": {"type": "array", "items": {"oneOf": [
            {"type": "object", "properties": {
              "type": {"type": "string", "enum": ["inline", ""]},
              "tag": {"type": "string"},
              "rules": {"type": "array", "items": {}}},
             "required": ["tag"]},
            {"type": "object", "properties": {
              "type": {"const": "local"},
              "tag": {"type": "string"},
              "format": {"type": "string", "enum": ["source", "binary"]},
              "path": {"type": "string"}},
             "required": ["type", "tag"]},
            {"type": "object", "properties": {
              "type": {"const": "remote"},
              "tag": {"type": "string"},
              "format": {"type": "string", "enum": ["source", "binary"]},
              "url": {"type": "string"}},
             "required": ["type", "tag"]}
          ]}}
        }}
      }
    }"#,
    );
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    // route -> rule_set -> < Add item >
    press(&mut m, &["enter", "enter", "enter"]);
    assert_eq!(
        m.top().unwrap().path,
        vec![ks("route"), ks("rule_set"), PathSeg::Idx(0)]
    );
    assert!(m.top().unwrap().variant_of.is_some());

    let type_idx = {
        let rows = m.menu_rows();
        let schema = &m.fs().schema;
        rows.iter()
            .position(|r| r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "type")
            .expect("type row")
    };
    m.top_mut().unwrap().cursor = type_idx;
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => {
            let labels: Vec<&str> = s.opts.iter().map(|o| o.display.as_str()).collect();
            assert_eq!(labels, ["inline", "", "local", "remote"]);
        }
        _ => panic!("type picker should open"),
    }
    if let Some(Overlay::Sel(s)) = &mut m.overlay {
        s.cursor = 3; // remote
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(
            &m.fs().doc,
            &[ks("route"), ks("rule_set"), PathSeg::Idx(0), ks("type")]
        ),
        "remote"
    );
    assert!(
        find_row(&mut m, |r, schema| {
            r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "url"
        })
        .is_some(),
        "choosing remote must pin the remote variant"
    );
    // the pinned variant keeps the union picker so the type stays switchable
    let type_idx = {
        let rows = m.menu_rows();
        let schema = &m.fs().schema;
        rows.iter()
            .position(|r| r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "type")
            .expect("type row")
    };
    m.top_mut().unwrap().cursor = type_idx;
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => {
            let labels: Vec<&str> = s.opts.iter().map(|o| o.display.as_str()).collect();
            assert_eq!(labels, ["inline", "", "local", "remote"]);
        }
        _ => panic!("pinned type picker should open"),
    }
}

#[test]
fn variant_flip_keeps_cursor() {
    let mut m = infer_app();
    press(&mut m, &["enter", "enter"]);
    let rows = m.menu_rows();
    let i = rows
        .iter()
        .position(|r| {
            r.fn_.is_some() && m.fs().schema.nodes[r.fn_.unwrap() as usize].key == "password"
        })
        .unwrap();
    m.top_mut().unwrap().cursor = i;
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["x", "y", "enter"]);
    assert_eq!(
        sget(
            &m.fs().doc,
            &[ks("inbounds"), PathSeg::Idx(0), ks("password")]
        ),
        "xy"
    );
    let node = m.top().unwrap().node;
    assert_eq!(
        m.fs().schema.nodes[node as usize].children.len(),
        3,
        "menu should pin the trojan variant"
    );
    let r = row_at(&mut m);
    assert_eq!(m.fs().schema.nodes[r.fn_.unwrap() as usize].key, "password");
}

fn scalar_one_of_schema() -> Schema {
    schema_build(|b| {
        let v1 = b.id(scalar("http_client", "variant 1", "string"));
        let engine = b.id(scalar("engine", "Engine", "string"));
        let v2 = b.id(FormNode {
            key: "http_client".into(),
            title: "variant 2".into(),
            kind: Kind::Object,
            children: vec![engine],
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "http_client".into(),
            title: "HTTP Client".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one],
            ..FormNode::default()
        })
    })
}

#[test]
fn search_jump_to_array_item_variant_field() {
    // sing-box-like dns.rules: an array whose items are classic oneOf
    // objects, with a scalar fallback so the picker is not a dynamic menu.
    let mut variants = String::new();
    for i in 0..6 {
        if i > 0 {
            variants.push(',');
        }
        variants.push_str(&format!(
            r#"{{
              "title": "Rule{i}",
              "type": "object",
              "properties": {{
                "type": {{"type": "string"}},
                "inbound": {{"type": "string"}},
                "ip_version": {{"type": "string"}},
                "domain_suffix": {{"type": "array", "items": {{"type": "string"}}}},
                "server": {{"type": "string"}}
              }},
              "required": ["type", "server"]
            }}"#
        ));
    }
    let src = format!(
        r#"{{
          "type": "object",
          "properties": {{
            "dns": {{
              "type": "object",
              "properties": {{
                "rules": {{
                  "type": "array",
                  "items": {{ "oneOf": [{variants}] }}
                }}
              }}
            }}
          }}
        }}"#
    );
    let schema = schema_from_json(&src);
    let doc = dbuild(|db| {
        let rule = db.obj(vec![
            ("type", db.s("default")),
            ("server", db.s("dns-proxy")),
            ("domain_suffix", db.arr(vec![db.s("a.com")])),
        ]);
        let first = db.obj(vec![("type", db.s("default")), ("server", db.s("other"))]);
        db.obj(vec![(
            "dns",
            db.obj(vec![("rules", db.arr(vec![first, rule]))]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "d", "n", "s"]);
    let want = vec![ks("dns"), ks("rules"), PathSeg::Idx(1), ks("server")];
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.path == want)
        .expect("rules[1].server search row");
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::Object,
        "jump must open the array item menu, not the variant picker"
    );
    assert_eq!(
        m.top().unwrap().path,
        vec![ks("dns"), ks("rules"), PathSeg::Idx(1)]
    );
    let row = row_at(&mut m);
    assert_eq!(row.path, want);
    assert_eq!(row.kind, RowKind::Field);
}

#[test]
fn search_jump_to_union_discriminator_field() {
    // sing-box Rule shape: nested oneOf groups flatten to object leaves.
    // "sniff" matches two leaves, so the item menu is the merged menu and
    // the search index must walk the flattened leaves (a top-level group
    // node has no children and used to leave the key unknown). Every leaf
    // defines "action" as a different const, so the merged menu synthesizes
    // a union enum node; the search hit still records the leaf's node. The
    // jump must land on the key row either way.
    let schema = schema_from_json(
        r##"{
	  "type": "object",
	  "properties": {
	    "rules": {"type": "array", "items": {
	      "oneOf": [
	        {"oneOf": [
	          {"type": "object", "properties": {
	            "action": {"const": "route"},
	            "outbound": {"type": "string"}}, "required": ["action"]},
	          {"type": "object", "properties": {
	            "action": {"const": "sniff"},
	            "inbound": {"type": "string"}}, "required": ["action"]}
	        ]},
	        {"oneOf": [
	          {"type": "object", "properties": {
	            "action": {"const": "sniff"},
	            "protocol": {"type": "string"}}, "required": ["action"]}
	        ]}
	      ]
	    }}
	  }
	}"##,
    );
    let doc = dbuild(|db| {
        db.obj(vec![(
            "rules",
            db.arr(vec![db.obj(vec![("action", db.s("sniff"))])]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "a", "c", "t", "i", "o", "n"]);
    let want = vec![ks("rules"), PathSeg::Idx(0), ks("action")];
    let hits: Vec<&Row> = search_state(&m)
        .results
        .iter()
        .filter(|r| r.path == want)
        .collect();
    assert_eq!(hits.len(), 1, "one search row per key");
    assert_eq!(
        hits[0].kind,
        RowKind::Field,
        "the key is schema-defined, not an unknown document key"
    );
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.path == want)
        .expect("action search row");
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        m.top().unwrap().path,
        vec![ks("rules"), PathSeg::Idx(0)],
        "jump opens the array item menu"
    );
    let row = row_at(&mut m);
    assert_eq!(row.path, want, "cursor must land on the key row");
}

#[test]
fn search_skips_missing_parent_paths() {
    let src = r#"{
      "type": "object",
      "properties": {
        "endpoints": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "servers": {
                "type": "array",
                "items": {
                  "type": "object",
                  "properties": {
                    "server": {
                      "oneOf": [
                        {"type": "string"},
                        {
                          "title": "ServerObject",
                          "type": "object",
                          "properties": {"addr": {"type": "string"}}
                        }
                      ]
                    }
                  }
                }
              }
            }
          }
        }
      }
    }"#;

    // servers is absent under endpoints[0]
    let schema = schema_from_json(src);
    let doc = dbuild(|db| db.obj(vec![("endpoints", db.arr(vec![db.obj(vec![])]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "s", "e", "r", "v", "e", "r"]);
    let deep = [
        ks("endpoints"),
        PathSeg::Idx(0),
        ks("servers"),
        PathSeg::Idx(0),
        ks("server"),
    ];
    assert!(
        !search_state(&m).results.iter().any(|r| r.path == deep),
        "do not index below a missing parent object"
    );

    // the array itself is empty: no endpoints[0] rows at all
    let schema = schema_from_json(src);
    let doc = dbuild(|db| db.obj(vec![("endpoints", db.arr(vec![]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "s", "e", "r", "v", "e", "r"]);
    let prefix = [ks("endpoints"), PathSeg::Idx(0)];
    assert!(
        !search_state(&m)
            .results
            .iter()
            .any(|r| r.path.starts_with(&prefix)),
        "empty arrays must not synthesize item paths"
    );
}

#[test]
fn array_variant_key_search_rows_and_jump() {
    let src = r#"{
      "type": "object",
      "properties": {
        "dns": {
          "type": "object",
          "properties": {
            "rules": {
              "type": "array",
              "items": {
                "oneOf": [
                  {
                    "title": "RuleA",
                    "type": "object",
                    "properties": {
                      "server": {
                        "oneOf": [
                          {"title": "ServerName", "type": "string"},
                          {
                            "title": "ServerObject",
                            "type": "object",
                            "properties": {"addr": {"type": "string"}}
                          }
                        ]
                      }
                    },
                    "required": ["server"]
                  },
                  {
                    "title": "RuleB",
                    "type": "object",
                    "properties": {"other": {"type": "string"}},
                    "required": ["other"]
                  },
                  {"title": "RuleText", "type": "string"}
                ]
              }
            }
          }
        }
      }
    }"#;
    let server_path = vec![ks("dns"), ks("rules"), PathSeg::Idx(0), ks("server")];

    // Unset variant key inside an array item: one search row, ending in "—".
    let schema = schema_from_json(src);
    let doc = dbuild(|db| {
        db.obj(vec![(
            "dns",
            db.obj(vec![("rules", db.arr(vec![db.obj(vec![])]))]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "s", "e", "r", "v", "e", "r"]);
    let hits: Vec<Row> = search_state(&m)
        .results
        .iter()
        .filter(|r| r.path == server_path)
        .cloned()
        .collect();
    assert_eq!(hits.len(), 1, "one row per unset variant key");
    assert!(hits[0].label.ends_with("—"), "label = {}", hits[0].label);

    // Set variant key: jumping lands on the key row in the variant editor.
    let schema = schema_from_json(src);
    let doc = dbuild(|db| {
        let rule = db.obj(vec![("server", db.s("addr"))]);
        db.obj(vec![("dns", db.obj(vec![("rules", db.arr(vec![rule]))]))])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "s", "e", "r", "v", "e", "r"]);
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.path == server_path && r.kind == RowKind::Field)
        .expect("server key row in search results");
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert!(m.top().unwrap().one_of.is_some());
    let row = row_at(&mut m);
    assert_eq!(row.path, server_path);
    assert_eq!(row.kind, RowKind::Drill);
}

#[test]
fn t_on_variant_key_opens_picker() {
    // scalar value: t switches without entering the inline editor
    let schema = scalar_one_of_schema();
    let mut m = app_with(
        schema,
        dbuild(|db| db.obj(vec![("http_client", db.s("abc"))])),
        test_opts(),
    );
    set_cursor_path(&mut m, &sp(&["http_client"]));
    press(&mut m, &["t"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m.top().unwrap().cursor, 0, "current scalar variant");

    // object value: the same shortcut works without drilling in first
    let schema = scalar_one_of_schema();
    let mut m = app_with(
        schema,
        dbuild(|db| db.obj(vec![("http_client", db.obj(vec![("engine", db.s("x"))]))])),
        test_opts(),
    );
    set_cursor_path(&mut m, &sp(&["http_client"]));
    press(&mut m, &["t"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m.top().unwrap().cursor, 1, "current object variant");
}

#[test]
fn scalar_variant_selection() {
    let schema = scalar_one_of_schema();
    let client = child(&schema, schema.root, "http_client");
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);

    press(&mut m, &["enter"]);
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::Object,
        "scalar variant returns to the parent menu"
    );
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["a", "b", "c"]);
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["http_client"]), "abc");

    let (ds, vs) = body_val(|db| db.s("abc"));
    assert_eq!(
        crate::ui::rows::current_variant(&m.fs().schema, client, &ds, Some(vs)),
        0
    );

    // re-entering with a value skips the picker
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "abc"),
        _ => panic!("editor should open prefilled"),
    }
    // t on the variant key switches without entering the scalar editor
    press(&mut m, &["esc"]);
    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m.top().unwrap().cursor, 0);
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_some(), "no confirm for the current variant");
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "abc"),
        _ => panic!("editor should reopen prefilled"),
    }
    press(&mut m, &["esc"]);

    // switching to the object variant asks first
    press(&mut m, &["t"]);
    press(&mut m, &["down"]);
    press(&mut m, &["enter"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Confirm(_))),
        "switching variants should confirm"
    );
    press(&mut m, &["y"]);
    assert!(
        !has(&m.fs().doc, &sp(&["http_client"])),
        "the default-less object variant clears the value first"
    );
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
}

#[test]
fn variant_picker_cursor_on_current_value() {
    let schema = schema_build(|b| {
        let v1 = b.id(scalar("ip_cidr", "", "string"));
        let it = b.id(scalar("ip_cidr", "", "string"));
        let v2 = b.id(FormNode {
            key: "ip_cidr".into(),
            kind: Kind::Array,
            items: Some(it),
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "ip_cidr".into(),
            title: "IP CIDR".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one],
            ..FormNode::default()
        })
    });
    let mut m = app_with(
        schema,
        dbuild(|db| db.obj(vec![("ip_cidr", db.arr(vec![db.s("1.2.3.4")]))])),
        test_opts(),
    );
    press(&mut m, &["enter"]);
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::Items,
        "enter with a list value edits the list variant"
    );
    assert!(m.top().unwrap().one_of.is_some());
    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(
        m.top().unwrap().cursor,
        1,
        "picker opens on the current list variant"
    );

    // no value: the picker keeps row 0
    let schema3 = schema_build(|b| {
        let v1 = b.id(scalar("ip_cidr", "", "string"));
        let it = b.id(scalar("ip_cidr", "", "string"));
        let v2 = b.id(FormNode {
            key: "ip_cidr".into(),
            kind: Kind::Array,
            items: Some(it),
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "ip_cidr".into(),
            title: "IP CIDR".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one],
            ..FormNode::default()
        })
    });
    let mut m2 = app_with(schema3, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut m2, &["enter"]);
    assert_eq!(m2.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m2.top().unwrap().cursor, 0);

    // a search jump reaches the variant editor with the same rule
    let schema4 = schema_build(|b| {
        let v1 = b.id(scalar("ip_cidr", "", "string"));
        let it = b.id(scalar("ip_cidr", "", "string"));
        let v2 = b.id(FormNode {
            key: "ip_cidr".into(),
            kind: Kind::Array,
            items: Some(it),
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "ip_cidr".into(),
            title: "IP CIDR".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one],
            ..FormNode::default()
        })
    });
    let mut m3 = app_with(
        schema4,
        dbuild(|db| db.obj(vec![("ip_cidr", db.arr(vec![db.s("1.2.3.4")]))])),
        test_opts(),
    );
    press(&mut m3, &["/", "c", "i", "d", "r", "enter"]);
    assert_eq!(
        m3.top().unwrap().kind,
        MenuKind::Object,
        "search jump stays on the variant key row"
    );
    press(&mut m3, &["t"]);
    assert_eq!(m3.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m3.top().unwrap().cursor, 1);
}

#[test]
fn tag_reference_completion() {
    let schema = schema_build(|b| {
        let mut v1 = scalar("http_client", "variant 1", "string");
        v1.tag_ref = "http_client".into();
        let v1 = b.id(v1);
        let engine = b.id(scalar("engine", "Engine", "string"));
        let v2 = b.id(FormNode {
            key: "http_client".into(),
            title: "variant 2".into(),
            kind: Kind::Object,
            children: vec![engine],
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "http_client".into(),
            title: "HTTP Client".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        let tag = b.id(scalar("tag", "Tag", "string"));
        let client = b.id(FormNode {
            key: "http_clients".into(),
            title: "Client".into(),
            kind: Kind::Object,
            children: vec![tag],
            ..FormNode::default()
        });
        let clients = b.id(FormNode {
            key: "http_clients".into(),
            title: "HTTP Clients".into(),
            kind: Kind::Array,
            items: Some(client),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one, clients],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| {
        db.obj(vec![(
            "http_clients",
            db.arr(vec![
                db.obj(vec![("tag", db.s("c1"))]),
                db.obj(vec![("tag", db.s("c2"))]),
            ]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter", "enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.cands, vec!["c1".to_string(), "c2".to_string()]),
        _ => panic!("scalar variant editor did not open"),
    }
    press(&mut m, &["c", "2"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sugs, vec!["c2".to_string()]),
        _ => panic!(),
    }
    press(&mut m, &["tab", "enter"]);
    assert_eq!(str_at(&m.fs().doc, &["http_client"]), "c2");
}

#[test]
fn completion_scrolling() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("v1"))]),
                db.obj(vec![("src", db.s("v2"))]),
                db.obj(vec![("src", db.s("v3"))]),
                db.obj(vec![("src", db.s("v4"))]),
                db.obj(vec![("src", db.s("v5"))]),
            ]),
        )])
    });
    let mut m = app_with(test_schema(), doc, test_opts());
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["enter"]);
    press(&mut m, &["down", "enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.cands.len(), 5),
        _ => panic!("candidates missing"),
    }
    press(&mut m, &["tab", "tab", "tab", "tab"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "v4"),
        _ => panic!(),
    }
    press(&mut m, &["tab"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "v5"),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(5), ks("src")]),
        "v5"
    );
}

#[test]
fn completion_dedup() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/data"))]),
                db.obj(vec![("src", db.s("/data"))]),
                db.obj(vec![("src", db.s("/data2"))]),
                db.obj(vec![("src", db.s(""))]),
            ]),
        )])
    });
    let mut m = app_with(test_schema(), doc, test_opts());
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["enter"]);
    press(&mut m, &["down", "enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.cands, vec!["/data".to_string(), "/data2".to_string()])
        }
        _ => panic!(),
    }
    press(&mut m, &["esc", "esc", "esc"]);

    let schema = schema_build(|b| {
        let mut v1 = scalar("http_client", "variant 1", "string");
        v1.tag_ref = "http_client".into();
        let v1 = b.id(v1);
        let engine = b.id(scalar("engine", "Engine", "string"));
        let v2 = b.id(FormNode {
            key: "http_client".into(),
            title: "variant 2".into(),
            kind: Kind::Object,
            children: vec![engine],
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "http_client".into(),
            title: "HTTP Client".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![one],
            ..FormNode::default()
        })
    });
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "http_clients",
            db.arr(vec![
                db.obj(vec![("tag", db.s("c1"))]),
                db.obj(vec![("tag", db.s("c1"))]),
                db.obj(vec![("tag", db.s("c2"))]),
            ]),
        )])
    });
    let mut m2 = app_with(schema, doc2, test_opts());
    press(&mut m2, &["down", "enter", "enter"]);
    match &m2.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.cands, vec!["c1".to_string(), "c2".to_string()]),
        _ => panic!("tag candidates missing"),
    }
}

#[test]
fn one_of_leaf_summaries() {
    let schema = schema_build(|b| {
        let v1 = b.id(scalar("tag", "variant 1", "string"));
        let it = b.id(scalar("tag", "", "string"));
        let v2 = b.id(FormNode {
            key: "tag".into(),
            title: "variant 2".into(),
            kind: Kind::Array,
            items: Some(it),
            ..FormNode::default()
        });
        let tag = b.id(FormNode {
            key: "tag".into(),
            title: "Tag".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![tag],
            ..FormNode::default()
        })
    });
    let tag = child(&schema, schema.root, "tag");
    let (d1, _) = body_val(|db| db.obj(vec![("tag", db.s("geosite-github"))]));
    let m = app_with(schema, d1, test_opts());
    let ctx = m.ctx();
    let s = crate::ui::rows::summary(&ctx, tag, &sp(&["tag"]));
    assert_eq!(s, "geosite-github");

    // array variant summary
    let schema2 = schema_build(|b| {
        let v1 = b.id(scalar("tag", "variant 1", "string"));
        let it = b.id(scalar("tag", "", "string"));
        let v2 = b.id(FormNode {
            key: "tag".into(),
            title: "variant 2".into(),
            kind: Kind::Array,
            items: Some(it),
            ..FormNode::default()
        });
        let tag = b.id(FormNode {
            key: "tag".into(),
            title: "Tag".into(),
            kind: Kind::OneOf,
            variants: vec![v1, v2],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "Root".into(),
            kind: Kind::Object,
            children: vec![tag],
            ..FormNode::default()
        })
    });
    let tag2 = child(&schema2, schema2.root, "tag");
    let d2 = dbuild(|db| db.obj(vec![("tag", db.arr(vec![db.s("a"), db.s("b")]))]));
    let m2 = app_with(schema2, d2, test_opts());
    let ctx2 = m2.ctx();
    assert_eq!(
        crate::ui::rows::summary(&ctx2, tag2, &sp(&["tag"])),
        "[2] a b"
    );
}

#[test]
fn breadcrumb_path() {
    let (schema, one_of, _) = infer_schema();
    let root = schema.root;
    let inbounds = child(&schema, root, "inbounds");
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    m.fs_mut().stack = vec![
        mstate(MenuKind::Object, root, Vec::new()),
        mstate(MenuKind::Items, inbounds, sp(&["inbounds"])),
        mstate(
            MenuKind::Object,
            one_of,
            vec![ks("inbounds"), PathSeg::Idx(0)],
        ),
    ];
    assert_eq!(m.breadcrumb(), "$.inbounds[0]");
    m.fs_mut().stack.push(mstate(
        MenuKind::OneOf,
        one_of,
        vec![ks("inbounds"), PathSeg::Idx(0)],
    ));
    assert_eq!(m.breadcrumb(), "$.inbounds[0]");
    m.fs_mut().stack.push(mstate(
        MenuKind::Object,
        one_of,
        vec![ks("inbounds"), PathSeg::Idx(3), ks("listen")],
    ));
    assert_eq!(m.breadcrumb(), "$.inbounds[3].listen");
    m.width = 12;
    assert_eq!(m.breadcrumb(), "…s[3].listen");
    m.fs_mut().stack.truncate(1);
    assert_eq!(m.breadcrumb(), "$");
}

#[test]
fn issues_jump_return() {
    let mut opts = test_opts();
    opts.load_errors = vec![
        "at '$.server.port': minimum value is 1024".into(),
        "at '$.name': got number, want string".into(),
    ];
    let mut m = mk_app(test_doc(), opts);

    press(&mut m, &["enter"]);
    press(&mut m, &["down"]);
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "jump closes the overlay");
    assert_eq!(row_at(&mut m).path, sp(&["name"]));
    press(&mut m, &["esc"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 1),
        _ => panic!("esc at the jump target returns to the issues list"),
    }
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());

    // deeper drill from the jump target pops first, then returns
    m.top_mut().unwrap().cursor = 0;
    press(&mut m, &["enter", "enter"]);
    assert_eq!(
        m.fs().schema.nodes[m.top().unwrap().node as usize].key,
        "server"
    );
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["esc"]);
    assert!(
        m.overlay.is_none(),
        "closing the input must not return to the list"
    );
    press(&mut m, &["esc"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!("esc at the jump target returns to the list"),
    }

    // a search jump clears the parked issues list
    press(&mut m, &["esc"]);
    m.top_mut().unwrap().cursor = 0;
    press(&mut m, &["enter", "enter"]);
    press(&mut m, &["/", "p", "o", "r", "t", "enter"]);
    press(&mut m, &["esc"]);
    assert!(
        !matches!(m.overlay, Some(Overlay::Issues(_))),
        "a search jump must clear the parked issues list"
    );
}

fn live_check(doc: &Doc) -> Vec<String> {
    let mut out = Vec::new();
    if nget(doc, &sp(&["server", "port"])) == "8080" {
        out.push("at '$.server.port': minimum value is 1024".to_string());
    }
    if sget(doc, &sp(&["name"])) == "webapp" {
        out.push("at '$.name': invalid name".to_string());
    }
    out
}

#[test]
fn issues_live_refresh() {
    let doc = test_doc();
    let mut opts = test_opts();
    opts.load_errors = live_check(&doc);
    opts.validate = Some(Arc::new(|d: &Doc| live_check(d)));
    let mut m = mk_app(doc, opts);

    assert!(m.fs().issues_now.len() == 2);
    let rows = m.menu_rows();
    assert!(
        rows[0].label.contains("2 validation errors"),
        "label = {}",
        rows[0].label
    );
    assert!(path_marked(&m.fs().err_paths, &sp(&["server", "port"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["name"])));

    // fix the port
    set_cursor_path(&mut m, &sp(&["server"]));
    press(&mut m, &["enter"]);
    set_cursor_path(&mut m, &sp(&["server", "port"]));
    press(&mut m, &["enter", "ctrl+u", "9", "0", "5", "0", "enter"]);
    assert_eq!(m.fs().issues_now.len(), 1);
    assert!(
        !path_marked(&m.fs().err_paths, &sp(&["server", "port"])),
        "the fixed row must lose its mark"
    );
    press(&mut m, &["esc"]);
    let rows = m.menu_rows();
    assert!(
        rows[0].label.contains("1 validation error"),
        "label = {}",
        rows[0].label
    );

    // fix the name too
    set_cursor_path(&mut m, &sp(&["name"]));
    press(
        &mut m,
        &["enter", "ctrl+u", "f", "i", "x", "e", "d", "enter"],
    );
    assert!(m.fs().issues_now.is_empty());
    assert!(m.fs().err_paths.is_empty() && m.fs().warn_paths.is_empty());

    // returning to a parked list refreshes it
    let doc2 = test_doc();
    let mut o2 = test_opts();
    o2.load_errors = live_check(&doc2);
    o2.validate = Some(Arc::new(|d: &Doc| live_check(d)));
    let mut m2 = mk_app(doc2, o2);
    press(&mut m2, &["enter"]);
    press(&mut m2, &["enter"]);
    set_cursor_path(&mut m2, &sp(&["server", "port"]));
    press(&mut m2, &["enter", "ctrl+u", "9", "0", "5", "0", "enter"]);
    press(&mut m2, &["esc"]);
    match &m2.overlay {
        Some(Overlay::Issues(s)) => {
            assert_eq!(s.items.len(), 1);
            assert!(
                !s.items[0].msg.contains("$.server.port"),
                "fixed entry still listed"
            );
        }
        _ => panic!("returned list should hold the remaining issue"),
    }
}

#[test]
fn one_of_item_drill_existing() {
    let (schema, one_of, _) = infer_schema();
    let doc = dbuild(|db| {
        db.obj(vec![(
            "inbounds",
            db.arr(vec![
                db.obj(vec![("type", db.s("trojan")), ("password", db.s("x"))]),
            ]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    press(&mut m, &["enter"]);
    let top = m.top().unwrap();
    assert_eq!(top.variant_of, Some(one_of));
    assert_eq!(top.kind, MenuKind::Object);
    assert_eq!(m.fs().stack.len(), 3);
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "password")
        .is_some()
    );
}

#[test]
fn mixed_one_of_still_picker() {
    let (schema, _, _) = infer_schema();
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut m, &["down", "enter"]);
    press(&mut m, &["enter"]);
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::OneOf,
        "mixed oneOf add keeps the picker"
    );
}

#[test]
fn one_of_search_jump() {
    let (schema, _, _) = infer_schema();
    let doc = dbuild(|db| {
        db.obj(vec![(
            "inbounds",
            db.arr(vec![
                db.obj(vec![("type", db.s("trojan")), ("password", db.s("x"))]),
            ]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(
        &mut m,
        &["/", "p", "a", "s", "s", "w", "o", "r", "d", "enter"],
    );
    assert!(
        !matches!(m.overlay, Some(Overlay::Search(_))),
        "enter should close search"
    );
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "password")
        .is_some()
    );
}

#[test]
fn field_required() {
    let (schema, _, _) = infer_schema();
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut m, &["enter", "enter"]);

    let row_of = |m: &mut App, k: &str| {
        find_row(m, |r, schema| {
            r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == k
        })
        .unwrap_or_else(|| panic!("row {k} not found"))
    };
    let r = row_of(&mut m, "type");
    assert!(m.field_required(&r));
    let r = row_of(&mut m, "listen");
    assert!(!m.field_required(&r));
    let r = row_of(&mut m, "password");
    assert!(m.field_required(&r));

    let n = m.fs_mut().doc.new_str("trojan");
    let _ = m
        .fs_mut()
        .doc
        .set(&[ks("inbounds"), PathSeg::Idx(0), ks("type")], n);
    m.refresh_variant_menu();
    let r = row_of(&mut m, "password");
    assert!(m.field_required(&r));

    let n = m.fs_mut().doc.new_str("direct");
    let _ = m
        .fs_mut()
        .doc
        .set(&[ks("inbounds"), PathSeg::Idx(0), ks("type")], n);
    m.refresh_variant_menu();
    let r = row_of(&mut m, "listen");
    assert!(!m.field_required(&r));
}

#[test]
fn nested_one_of_group_flatten() {
    let mut b = TB::new();
    let mut t1 = fnode("type", "Type", Kind::Enum);
    t1.enum_ = tenum(&mut b, &["direct"]);
    t1.required = true;
    let t1 = b.id(t1);
    let v1 = b.id(FormNode {
        key: "inbounds".into(),
        title: "variant 1".into(),
        kind: Kind::Object,
        children: vec![t1],
        ..FormNode::default()
    });
    let mut t2 = fnode("type", "Type", Kind::Enum);
    t2.enum_ = tenum(&mut b, &["vless"]);
    t2.required = true;
    let t2 = b.id(t2);
    let flow = b.id(scalar("flow", "Flow", "string"));
    let v2 = b.id(FormNode {
        key: "inbounds".into(),
        title: "variant 2".into(),
        kind: Kind::Object,
        children: vec![t2, flow],
        ..FormNode::default()
    });
    let mut t3 = fnode("type", "Type", Kind::Enum);
    t3.enum_ = tenum(&mut b, &["vmess"]);
    t3.required = true;
    let t3 = b.id(t3);
    let v3 = b.id(FormNode {
        key: "inbounds".into(),
        title: "variant 3".into(),
        kind: Kind::Object,
        children: vec![t3],
        ..FormNode::default()
    });
    let group = b.id(FormNode {
        key: "inbounds".into(),
        title: "group".into(),
        kind: Kind::OneOf,
        variants: vec![v2, v3],
        ..FormNode::default()
    });
    let one_of = b.id(FormNode {
        key: "inbounds".into(),
        title: "Inbound".into(),
        kind: Kind::OneOf,
        variants: vec![v1, group],
        ..FormNode::default()
    });
    let arr = b.id(FormNode {
        key: "inbounds".into(),
        title: "Inbounds".into(),
        kind: Kind::Array,
        items: Some(one_of),
        ..FormNode::default()
    });
    let root = b.id(FormNode {
        key: "$".into(),
        title: "Config".into(),
        kind: Kind::Object,
        children: vec![arr],
        ..FormNode::default()
    });
    b.root = root;
    let schema = b.finish();

    assert_eq!(
        crate::ui::rows::dynamic_variants(&schema, one_of)
            .unwrap()
            .len(),
        3
    );
    let (dv, vv) = body_val(|db| {
        db.obj(vec![
            ("type", db.s("vless")),
            ("flow", db.s("xtls-rprx-vision")),
        ])
    });
    assert_eq!(
        crate::ui::rows::current_variant(&schema, one_of, &dv, Some(vv)),
        1
    );

    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut m, &["enter", "enter"]);
    assert_eq!(m.top().unwrap().variant_of, Some(one_of));
    let merged = m.top().unwrap().node;
    let children = m.fs().schema.nodes[merged as usize].children.clone();
    assert_eq!(children.len(), 2);
    let u = &m.fs().schema.nodes[children[0] as usize];
    assert_eq!(u.kind, Kind::Enum);
    assert_eq!(u.enum_.len(), 3);
    assert_eq!(u.enum_[0].display, "direct");
    assert_eq!(u.enum_[1].display, "vless");
    assert_eq!(u.enum_[2].display, "vmess");

    let n = m.fs_mut().doc.new_str("vless");
    let _ = m
        .fs_mut()
        .doc
        .set(&[ks("inbounds"), PathSeg::Idx(0), ks("type")], n);
    m.refresh_variant_menu();
    let node = m.top().unwrap().node;
    let children = m.fs().schema.nodes[node as usize].children.clone();
    assert_eq!(children.len(), 2);
    assert_eq!(m.fs().schema.nodes[children[1] as usize].key, "flow");
}

#[test]
fn all_of_combo_unknown_keys_and_menu() {
    let schema = schema_from_json(
        r##"{
	  "type": "object",
	  "properties": {
	    "rules": {"type": "array", "items": {
	      "oneOf": [
	        {"type": "object", "allOf": [
	          {"properties": {"domain": {"type": "string"}}},
	          {"$ref": "#/$defs/act"}]},
	        {"type": "object", "properties": {"mode": {"type": "string"}}, "required": ["mode"]}
	      ]
	    }}
	  },
	  "$defs": {"act": {"oneOf": [
	    {"type": "object", "properties": {"action": {"const": "route"}, "outbound": {"type": "string"}}, "required": ["action"]},
	    {"type": "object", "properties": {"action": {"const": "reject"}}, "required": ["action"]}
	  ]}}
	}"##,
    );
    let doc = dbuild(|db| {
        db.obj(vec![(
            "rules",
            db.arr(vec![db.obj(vec![
                ("domain", db.s("a.com")),
                ("action", db.s("route")),
                ("outbound", db.s("o1")),
                ("bogus", db.s("x")),
            ])]),
        )])
    });
    let issues = crate::ui::rows::unknown_key_issues(&schema, &doc);
    assert_eq!(issues.len(), 1);
    assert!(issues[0].msg.contains("bogus"));

    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["down", "enter"]);
    press(&mut m, &["down", "enter"]);
    assert!(
        find_row(&mut m, |r, schema| r.fn_.is_some()
            && schema.nodes[r.fn_.unwrap() as usize].key == "outbound")
        .is_some()
    );
    let unknowns: Vec<Row> = m
        .menu_rows()
        .into_iter()
        .filter(|r| r.kind == RowKind::Unknown)
        .collect();
    assert_eq!(unknowns.len(), 1);
    assert_eq!(unknowns[0].label, "bogus");
}

fn map_one_of_schema() -> Schema {
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

#[test]
fn map_keys_known_and_editable() {
    let schema = map_one_of_schema();
    let doc = dbuild(|db| {
        db.obj(vec![(
            "headers",
            db.obj(vec![("Host", db.s("canhui.wang"))]),
        )])
    });
    assert!(crate::ui::rows::unknown_key_issues(&schema, &doc).is_empty());

    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    let host = find_row(&mut m, |r, _| r.label == "Host").expect("Host row");
    assert!(host.fn_.is_some());
    assert!(
        !m.menu_rows()
            .iter()
            .any(|r| r.label.contains("not in schema"))
    );
    assert!(m.delete_pair(&host));
    assert!(obj_keys(&m.fs().doc, &sp(&["headers"])).is_empty());

    // a types a key name; the map value seeds per the value schema
    press(&mut m, &["a"]);
    assert!(matches!(m.overlay, Some(Overlay::Add(_))));
    if let Some(Overlay::Add(s)) = &mut m.overlay {
        s.ti.set_value("X-Tag");
    }
    press(&mut m, &["enter"]);
    assert!(
        has(&m.fs().doc, &sp(&["headers", "X-Tag"])),
        "addKey must seed the map value"
    );
}

#[test]
fn map_list_value_add_element_after_type_switch() {
    let schema = map_one_of_schema();
    let headers = child(&schema, schema.root, "headers");
    let one_of = schema.nodes[headers as usize].items.unwrap();
    let list = schema.nodes[one_of as usize].variants[1];
    let doc = dbuild(|db| {
        db.obj(vec![(
            "headers",
            db.obj(vec![("Host", db.push(Value::Null))]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());

    press(&mut m, &["enter"]);
    let path = sp(&["headers", "Host"]);
    m.push(one_of, path.clone());
    m.enter_variant(&Row {
        kind: RowKind::Variant,
        variant: Some(list),
        path: path.clone(),
        idx: -1,
        ..Row::default()
    });
    assert_eq!(
        m.top().unwrap().node,
        list,
        "top should be the pushed list variant"
    );

    m.insert_item_at(0, false);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    if let Some(Overlay::Edit(e)) = &mut m.overlay {
        e.ti.set_value("8.8.8.8");
    }
    press(&mut m, &["enter"]);

    let p0 = [ks("headers"), ks("Host"), PathSeg::Idx(0)];
    assert_eq!(sget(&m.fs().doc, &p0), "8.8.8.8");
    assert_eq!(arr_len(&m.fs().doc, &[ks("headers"), ks("Host")]), 1);
}

#[test]
fn map_null_seed_list_add_converts_cleanly() {
    let schema = map_one_of_schema();
    let doc = dbuild(|db| db.obj(vec![("headers", db.obj(vec![]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    press(&mut m, &["a"]);
    if let Some(Overlay::Add(s)) = &mut m.overlay {
        s.ti.set_value("Host");
    }
    press(&mut m, &["enter"]);
    press(&mut m, &["down"]);
    press(&mut m, &["enter"]);
    press(&mut m, &["enter"]);
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("8.8.8.8"),
        _ => panic!("element editor did not open"),
    }
    press(&mut m, &["enter"]);
    press(&mut m, &["esc", "esc"]);

    let v = getv(&m.fs().doc, &sp(&["headers", "Host"])).expect("Host");
    assert!(
        !matches!(m.fs().doc.value(v), Value::Null),
        "Host must be the converted array"
    );
    assert_eq!(arr_len(&m.fs().doc, &sp(&["headers", "Host"])), 1);
    assert_eq!(
        sget(&m.fs().doc, &[ks("headers"), ks("Host"), PathSeg::Idx(0)]),
        "8.8.8.8"
    );
}

#[test]
fn element_labels_and_edit_title() {
    let schema = map_one_of_schema();
    let doc = dbuild(|db| {
        db.obj(vec![(
            "headers",
            db.obj(vec![("Host", db.arr(vec![db.s("a"), db.s("b")]))]),
        )])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    press(&mut m, &["enter"]);
    let labels: Vec<String> = m.menu_rows().iter().map(|r| r.label.clone()).collect();
    assert!(labels.iter().any(|l| l == "Host[0]"), "labels = {labels:?}");
    assert!(labels.iter().any(|l| l == "Host[1]"), "labels = {labels:?}");
}

#[test]
fn variant_entry_replaces_picker_in_stack() {
    let schema = map_one_of_schema();
    let headers = child(&schema, schema.root, "headers");
    let one_of = schema.nodes[headers as usize].items.unwrap();
    let list = schema.nodes[one_of as usize].variants[1];
    let doc = dbuild(|db| db.obj(vec![("headers", db.obj(vec![("Host", db.s("old"))]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);

    let path = sp(&["headers", "Host"]);
    m.push(one_of, path.clone());
    m.enter_variant(&Row {
        kind: RowKind::Variant,
        variant: Some(list),
        path: path.clone(),
        idx: -1,
        ..Row::default()
    });
    assert!(
        matches!(m.overlay, Some(Overlay::Confirm(_))),
        "switching an existing value should confirm"
    );
    press(&mut m, &["y"]);
    assert_eq!(m.top().unwrap().node, list);
    assert_eq!(m.top().unwrap().one_of, Some(one_of));
    assert_eq!(m.fs().stack.len(), 3);
    assert_eq!(m.fs().stack[1].node, headers);
    press(&mut m, &["esc"]);
    assert_eq!(
        m.top().unwrap().node,
        headers,
        "esc lands on the headers menu, not the picker"
    );
}

// ===========================================================================
// menu_issues_test.go
// ===========================================================================

#[test]
fn worst_issue_icon_selection() {
    let cases: Vec<(Vec<Issue>, &str)> = vec![
        (vec![], WARN),
        (
            vec![Issue {
                msg: "unknown key".into(),
                warn: true,
                ..Issue::default()
            }],
            WARN,
        ),
        (
            vec![
                Issue {
                    msg: "unknown key".into(),
                    warn: true,
                    ..Issue::default()
                },
                Issue {
                    msg: "bad value".into(),
                    ..Issue::default()
                },
            ],
            ERR,
        ),
        (
            vec![Issue {
                msg: "bad value".into(),
                ..Issue::default()
            }],
            ERR,
        ),
    ];
    for (items, want) in cases {
        assert_eq!(worst_issue_icon(&items), want);
    }
}

#[test]
fn fast_move_takes_u_as_target() {
    let mut doc = test_doc();
    let n = doc.new_str("x");
    let _ = doc.set(&sp(&["u_key"]), n);
    let mut m = mk_app(doc, test_opts());

    // create one undoable edit so a stray undo is observable
    set_cursor_path(&mut m, &sp(&["name"]));
    press(&mut m, &["enter"]);
    press(&mut m, &["ctrl+u"]);
    for c in "changed".chars() {
        press(&mut m, &[&c.to_string()]);
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "changed");

    // f waits for a target character: u must move, not undo
    press(&mut m, &["home", "f", "u"]);
    assert!(m.fast_move.is_none(), "fast move consumed u");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "changed", "u must not undo");
    assert_eq!(row_at(&mut m).label, "u_key");
}

#[test]
fn unknown_key_rows() {
    let mut doc = test_doc();
    let n = doc.new_str("x");
    let _ = doc.set(&sp(&["extra_key"]), n);
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["server", "legacy"]), n);
    let mut m = mk_app(doc, test_opts());

    let extra = find_row(&mut m, |r, _| {
        r.kind == RowKind::Unknown && r.label == "extra_key"
    })
    .expect("unknown row");
    // enter edits a scalar unknown key in place
    let i = m
        .menu_rows()
        .iter()
        .position(|r| same_menu_row(r, &extra))
        .unwrap();
    m.top_mut().unwrap().cursor = i;
    // a known string value edits in place without the type picker
    press(&mut m, &["enter"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Edit(_))),
        "string unknown key should edit in place"
    );
    press(&mut m, &["ctrl+u", "y", "enter"]);
    assert_eq!(str_at(&m.fs().doc, &["extra_key"]), "y");

    // nested unknown key shows in the sub menu
    set_cursor_path(&mut m, &sp(&["server"]));
    press(&mut m, &["enter"]);
    assert!(
        find_row(&mut m, |r, _| r.kind == RowKind::Unknown
            && r.label == "legacy")
        .is_some()
    );
    press(&mut m, &["esc"]);

    // delete removes the root unknown key
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "extra_key"
    });
    press(&mut m, &["delete"]);
    assert!(!has(&m.fs().doc, &sp(&["extra_key"])));
}

#[test]
fn load_issues_row_and_jump() {
    let mut doc = test_doc();
    let n = doc.new_str("x");
    let _ = doc.set(&sp(&["extra_key"]), n);
    let mut opts = test_opts();
    opts.load_errors = vec![
        "at '$.server.port': minimum value is 1024".into(),
        "at '$': additional properties 'extra_key' not allowed".into(),
    ];
    let mut m = mk_app(doc, opts);
    let rows = m.menu_rows();
    assert_eq!(rows[0].kind, RowKind::Issues);
    assert!(
        rows[0].label.contains("2 validation errors"),
        "label = {}",
        rows[0].label
    );

    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.items.len(), 2),
        _ => panic!("issues overlay should list both messages"),
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "jump should close the overlay");
    assert_eq!(
        m.fs().schema.nodes[m.top().unwrap().node as usize].key,
        "server"
    );
    assert_eq!(row_at(&mut m).path, sp(&["server", "port"]));

    press(&mut m, &["esc"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Issues(_))),
        "esc at the jump target returns to the list"
    );
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none() && m.fs().stack.len() == 1);

    // the second entry jumps to the unknown-key row
    m.top_mut().unwrap().cursor = 0;
    press(&mut m, &["enter", "down", "enter"]);
    assert!(m.overlay.is_none());
    let r = row_at(&mut m);
    assert_eq!(r.kind, RowKind::Unknown);
    assert_eq!(r.label, "extra_key");

    // the overlay recomputes against the current doc when a validator exists
    m.fs_mut().opts.validate = Some(Arc::new(|_: &Doc| {
        vec!["at '$.name': got number, want string".into()]
    }));
    m.top_mut().unwrap().cursor = 0;
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Issues(s)) => {
            assert_eq!(s.items.len(), 2);
            assert!(
                s.items[0].msg.contains("$.name"),
                "items = {:?}",
                s.items[0].msg
            );
            assert!(
                s.items[1].msg.contains(r#"unknown key "extra_key""#),
                "items = {:?}",
                s.items[1].msg
            );
        }
        _ => panic!("overlay should recompute from the validator"),
    }
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());
}

#[test]
fn additional_properties_dedupes_all_names() {
    let mut doc = test_doc();
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["server", "legacy"]), n);
    let n = doc.new_num("2");
    let _ = doc.set(&sp(&["server", "other"]), n);
    let mut m = mk_app(doc, test_opts());

    for msg in [
        "at '$.server': additional properties 'legacy', 'other' not allowed",
        "at '$.server': additional properties 'other', 'legacy' not allowed",
        "at '$.server': additional properties 'other', 'legacy', 'x' not allowed",
    ] {
        for it in m.live_issues(vec![msg.to_string()]) {
            assert!(!it.warn, "{msg}: warning {:?} was not deduped", it.msg);
        }
    }

    let warned = m
        .live_issues(vec![
            "at '$.server': additional properties 'legacy' not allowed".into(),
        ])
        .into_iter()
        .any(|it| it.warn && it.msg.contains(r#"unknown key "other""#));
    assert!(warned, "unlisted unknown key should keep its warning");
}

#[test]
fn additional_properties_marks_real_unknowns_only() {
    let mut doc = test_doc();
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["server", "legacy"]), n);
    let n = doc.new_num("2");
    let _ = doc.set(&sp(&["server", "other"]), n);
    let mut m = mk_app(doc, test_opts());

    let items = m.live_issues(vec![
        "at '$.server': additional properties 'port', 'host', 'legacy' not allowed".into(),
    ]);
    m.set_issues(items);
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "port"])));
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "host"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["server", "legacy"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["server"])));

    let items = m.live_issues(vec![
        "at '$.server': additional properties 'legacy', 'host', 'port' not allowed".into(),
    ]);
    m.set_issues(items);
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "port"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["server", "legacy"])));

    let items = m.live_issues(vec![
        "at '$.server': additional properties 'port', 'host' not allowed".into(),
    ]);
    m.set_issues(items);
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "port"])));
    assert!(!path_marked(&m.fs().err_paths, &sp(&["server", "host"])));
    assert!(path_marked(&m.fs().err_paths, &sp(&["server"])));

    let items = m.live_issues(vec![
        "at '$': additional properties 'name' not allowed".into(),
    ]);
    m.set_issues(items);
    assert!(
        m.fs().err_paths.is_empty(),
        "root-level mark leaked to rows: {:?}",
        m.fs().err_paths
    );
}

#[test]
fn unknown_key_issues_listed() {
    let mut doc = test_doc();
    let n = doc.new_str("x");
    let _ = doc.set(&sp(&["extra_key"]), n);
    let n = doc.new_num("1");
    let _ = doc.set(&sp(&["server", "legacy"]), n);
    let mut m = mk_app(doc, test_opts());

    assert_eq!(m.fs().issues_now.len(), 2);
    assert!(m.fs().issues_now.iter().all(|it| it.warn));
    assert_eq!(m.fs().issues_now[0].path, Some(sp(&["server", "legacy"])));
    assert_eq!(m.fs().issues_now[1].path, Some(sp(&["extra_key"])));
    let rows = m.menu_rows();
    assert!(
        rows[0].label.contains("2 warnings"),
        "label = {}",
        rows[0].label
    );
    assert!(path_marked(&m.fs().warn_paths, &sp(&["server", "legacy"])));
    assert!(path_marked(&m.fs().warn_paths, &sp(&["extra_key"])));

    // validator output combines with the warnings without double-flagging
    m.fs_mut().opts.validate = Some(Arc::new(|d: &Doc| {
        if has(d, &sp(&["extra_key"])) {
            vec!["at '$': additional properties 'extra_key' not allowed".into()]
        } else {
            Vec::new()
        }
    }));
    m.refresh_issues();
    assert_eq!(m.fs().issues_now.len(), 2);
    assert!(m.fs().issues_now[0].msg.contains("additional properties"));
    assert!(m.fs().issues_now[1].msg.contains(r#"unknown key "legacy""#));
    assert!(!m.fs().issues_now[0].warn && m.fs().issues_now[1].warn);
    let rows = m.menu_rows();
    assert!(
        rows[0].label.contains("1 validation error"),
        "label = {}",
        rows[0].label
    );
    assert!(
        rows[0].label.contains("1 warning"),
        "label = {}",
        rows[0].label
    );
    assert!(path_marked(&m.fs().err_paths, &sp(&["extra_key"])));
    assert!(path_marked(&m.fs().warn_paths, &sp(&["server", "legacy"])));

    // removing the keys clears their warnings
    m.fs_mut().doc.remove(&sp(&["extra_key"]));
    m.fs_mut().doc.remove(&sp(&["server", "legacy"]));
    m.refresh_issues();
    assert!(m.fs().issues_now.is_empty());
}

#[test]
fn unknown_key_issues_schema_pointer_exempt() {
    let schema = schema_build(|b| {
        let name = b.id(scalar("name", "Name", "string"));
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![name],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| {
        db.obj(vec![
            ("$schema", db.s("mini.schema.json")),
            ("name", db.s("z")),
        ])
    });
    assert!(
        crate::ui::rows::unknown_key_issues(&schema, &doc).is_empty(),
        "$schema must not warn"
    );

    let schema2 = schema_build(|b| {
        let server = b.id(FormNode {
            key: "server".into(),
            title: "Server".into(),
            kind: Kind::Object,
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![server],
            ..FormNode::default()
        })
    });
    let doc2 = dbuild(|db| db.obj(vec![("server", db.obj(vec![("$schema", db.s("x"))]))]));
    let got = crate::ui::rows::unknown_key_issues(&schema2, &doc2);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].path, Some(sp(&["server", "$schema"])));
}

// ===========================================================================
// menu_write_test.go
// ===========================================================================

use parking_lot::Mutex;

fn write_opts(path: &str, save: Arc<AtomicUsize>, paths: Arc<Mutex<Vec<String>>>) -> Options {
    let mut opts = test_opts();
    opts.path = path.to_string();
    opts.save = Some(Arc::new(move |_| {
        save.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    opts.write_to = Some(Arc::new(move |p: &str, _: &Doc| {
        paths.lock().push(p.to_string());
        Ok(())
    }));
    opts
}

fn write_model() -> (App, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    let save = Arc::new(AtomicUsize::new(0));
    let paths = Arc::new(Mutex::new(Vec::new()));
    let opts = write_opts(
        "/origin/path/to/file.ext",
        Arc::clone(&save),
        Arc::clone(&paths),
    );
    (mk_app(test_doc(), opts), save, paths)
}

#[test]
fn write_to_opens_prefilled_row() {
    let (mut m, _, paths) = write_model();
    press(&mut m, &["W"]);
    match &m.overlay {
        Some(Overlay::Write(w)) => assert_eq!(w.ti.value(), "/origin/path/to/file.ext"),
        _ => panic!("W must open the write-to input"),
    }
    press(&mut m, &["ctrl+u"]);
    press(&mut m, &["w", "e", "i"]);
    match &m.overlay {
        Some(Overlay::Write(w)) => assert_eq!(w.ti.value(), "wei"),
        _ => panic!(),
    }
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());
    assert!(paths.lock().is_empty(), "esc must not write");
}

#[test]
fn save_notice_shows_target() {
    let (mut m, save, _) = write_model();
    press(&mut m, &["w"]);
    assert!(m.overlay.is_none(), "w must save directly");
    assert_eq!(save.load(Ordering::SeqCst), 1);
    assert_eq!(m.notice, "Write to /origin/path/to/file.ext");
    assert!(m.fs().saved);
    press(&mut m, &["ctrl+s"]);
    assert_eq!(save.load(Ordering::SeqCst), 2);
    assert_eq!(m.notice, "Write to /origin/path/to/file.ext");
}

#[test]
fn write_to_own_path_saves_session() {
    let (mut m, save, paths) = write_model();
    let n = m.fs_mut().doc.new_str("changed");
    let _ = m.fs_mut().doc.set(&sp(&["name"]), n);
    press(&mut m, &["W"]);
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "enter must close the input");
    assert_eq!(save.load(Ordering::SeqCst), 1);
    assert!(
        paths.lock().is_empty(),
        "own-path write must not go through WriteTo"
    );
    assert_eq!(m.notice, "Write to /origin/path/to/file.ext");
    assert!(nodes_equal_doc(
        m.fs().baseline.as_ref().unwrap(),
        &m.fs().doc
    ));
    assert!(m.fs().saved);
}

#[test]
fn write_to_commits_edited_path() {
    let (mut m, save, paths) = write_model();
    let n = m.fs_mut().doc.new_str("changed");
    let _ = m.fs_mut().doc.set(&sp(&["name"]), n);
    let pre = m.fs().baseline.clone().unwrap();
    press(&mut m, &["W"]);
    press(&mut m, &["ctrl+u"]);
    press(&mut m, &["a", "/", "b", ".", "j", "s", "o", "n"]);
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(*paths.lock(), vec!["a/b.json".to_string()]);
    assert_eq!(m.notice, "Wrote to a/b.json");
    assert_eq!(save.load(Ordering::SeqCst), 0);
    assert!(nodes_equal_doc(&pre, m.fs().baseline.as_ref().unwrap()));
    assert!(!m.fs().saved);

    // empty path is refused and keeps the input open
    press(&mut m, &["W"]);
    press(&mut m, &["ctrl+u"]);
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Write(w)) => assert_eq!(w.err, "path required"),
        _ => panic!("empty path must keep the input open"),
    }
}

#[test]
fn write_to_validation_gates_and_forces() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let p = Arc::clone(&paths);
    let mut opts = test_opts();
    opts.path = "/origin/f.json".into();
    opts.validate = Some(Arc::new(|_: &Doc| vec!["x is required".into()]));
    opts.force = true;
    opts.write_to = Some(Arc::new(move |path: &str, _: &Doc| {
        p.lock().push(path.to_string());
        Ok(())
    }));
    let mut m = mk_app(test_doc(), opts);
    press(&mut m, &["W"]);
    press(&mut m, &["ctrl+u"]);
    press(&mut m, &["c", ".", "j", "s", "o", "n"]);
    press(&mut m, &["enter"]);
    match &m.overlay {
        Some(Overlay::Errs(e)) => assert_eq!(e.target, "c.json"),
        _ => panic!("validation errors must open the errors overlay"),
    }
    assert!(paths.lock().is_empty(), "gated W must not write");
    press(&mut m, &["y"]);
    assert_eq!(*paths.lock(), vec!["c.json".to_string()]);
    assert!(
        !m.fs().saved,
        "a forced W copy must not mark the session as saved"
    );
}

#[test]
fn write_to_error_is_reported() {
    let mut opts = test_opts();
    opts.path = "/origin/f.json".into();
    opts.write_to = Some(Arc::new(|_: &str, _: &Doc| Err("disk full".to_string())));
    let mut m = mk_app(test_doc(), opts);
    press(&mut m, &["W"]);
    press(&mut m, &["ctrl+u"]);
    press(&mut m, &["c", ".", "j", "s", "o", "n"]);
    press(&mut m, &["enter"]);
    assert!(
        m.overlay.is_none(),
        "enter must close the input even on failure"
    );
    assert!(
        m.notice.contains("write failed: disk full"),
        "notice = {}",
        m.notice
    );
}

#[test]
fn write_to_path_completion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    for name in ["alpha.json", "beta.json", ".hidden", "sub/inner.json"] {
        std::fs::write(root.join(name), "{}").unwrap();
    }
    let paths = Arc::new(Mutex::new(Vec::new()));
    let p = Arc::clone(&paths);
    let mut opts = test_opts();
    opts.path = root.join("alpha.json").to_string_lossy().to_string();
    opts.write_to = Some(Arc::new(move |path: &str, _: &Doc| {
        p.lock().push(path.to_string());
        Ok(())
    }));
    let mut m = mk_app(test_doc(), opts);

    press(&mut m, &["W"]);
    match &m.overlay {
        Some(Overlay::Write(w)) => assert!(!w.sugs.is_empty(), "W must compute candidates"),
        _ => panic!(),
    }
    press(&mut m, &["ctrl+u"]);
    let typed = format!("{}/s", root.display());
    press(&mut m, &[&typed]);
    match &m.overlay {
        Some(Overlay::Write(w)) => {
            let want = format!("{}/", root.join("sub").display());
            assert_eq!(w.sugs, vec![want.clone()]);
            assert_eq!(w.ti.value(), typed);
        }
        _ => panic!(),
    }
    press(&mut m, &["tab"]);
    let sub = format!("{}/", root.join("sub").display());
    match &m.overlay {
        Some(Overlay::Write(w)) => assert_eq!(w.ti.value(), sub),
        _ => panic!(),
    }
    press(&mut m, &["i"]);
    press(&mut m, &["tab"]);
    let inner = root.join("sub/inner.json").to_string_lossy().to_string();
    match &m.overlay {
        Some(Overlay::Write(w)) => assert_eq!(w.ti.value(), inner),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "enter must commit the completed path");
    assert_eq!(*paths.lock(), vec![inner]);
    assert!(m.notice.contains("Wrote to "), "notice = {}", m.notice);

    // dotfiles stay hidden unless the typed segment starts with a dot
    press(&mut m, &["W"]);
    press(&mut m, &["ctrl+u"]);
    let typed = format!("{}/", root.display());
    press(&mut m, &[&typed]);
    match &m.overlay {
        Some(Overlay::Write(w)) => {
            assert!(
                w.sugs.iter().all(|s| !std::path::Path::new(s)
                    .file_name()
                    .map_or(false, |f| f.to_string_lossy().starts_with('.'))),
                "dotfile leaked: {:?}",
                w.sugs
            );
        }
        _ => panic!(),
    }
    press(&mut m, &["."]);
    match &m.overlay {
        Some(Overlay::Write(w)) => assert!(
            w.sugs.iter().any(|s| s.ends_with(".hidden")),
            "candidates = {:?}",
            w.sugs
        ),
        _ => panic!(),
    }
}

#[test]
fn write_to_unavailable() {
    let (opts, saves) = counting_opts();
    let mut m = mk_app(test_doc(), opts);
    press(&mut m, &["w"]);
    assert!(
        m.overlay.is_none(),
        "without WriteTo the input must not open"
    );
    assert_eq!(saves.load(Ordering::SeqCst), 1);
    assert_eq!(m.notice, "Saved");
}

// ===========================================================================
// menu_items_test.go
// ===========================================================================

fn items_app(doc: Doc) -> App {
    app_with(test_schema(), doc, test_opts())
}

fn empty_doc() -> Doc {
    dbuild(|db| db.obj(vec![]))
}

fn search_state(m: &App) -> &SearchState {
    match &m.overlay {
        Some(Overlay::Search(s)) => s,
        _ => panic!("search overlay not open"),
    }
}

#[test]
fn array_item_selection_colors_labels_and_deletes_batch() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/a")), ("dst", db.s("/1"))]),
                db.obj(vec![("src", db.s("/b")), ("dst", db.s("/2"))]),
                db.obj(vec![("src", db.s("/c")), ("dst", db.s("/3"))]),
            ]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["down", "m", "down", "down", "m"]);
    let st = m.top().unwrap();
    assert_eq!(st.sel.len(), 2, "first and last items marked");
    assert!(!st.visual);

    // the "mounts[N]" label is styled like an object key
    let rows = m.menu_rows();
    let tw = m.menu_title_col_width(&rows);
    let off = m.row_line(&rows[1], false, tw);
    assert!(
        off.contains(&crate::ui::menu::style::fg9("mounts[0]")),
        "unfocused marked item must be red: {off:?}"
    );
    assert!(
        off.starts_with(&crate::ui::menu::style::fg9("> ")),
        "marked item needs the red marker: {off:?}"
    );
    let focused = m.row_line(&rows[3], true, tw);
    assert!(
        focused.contains(&crate::ui::menu::style::bold(&crate::ui::menu::style::fg9(
            "mounts[2]"
        ))),
        "focused marked item must be bold red: {focused:?}"
    );

    // batch delete splices from the end: the surviving item is the middle one
    press(&mut m, &["d"]);
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 1);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("src")]),
        "/b"
    );
    assert!(m.top().unwrap().sel.is_empty());
}

#[test]
fn array_item_visual_select_marks_items() {
    let mut m = items_app(test_doc());
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down", "v", "down"]);
    let st = m.top().unwrap();
    assert!(st.visual);
    assert_eq!(st.sel.len(), 1, "one item in the visual range");
    press(&mut m, &["v"]);
    assert!(m.top().unwrap().visual == false);
}

#[test]
fn array_item_selection_copies_values() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("tags", db.arr(vec![db.s("prod"), db.s("dev")]))]),
        )])
    });
    let mut opts = test_opts();
    let copies = Arc::new(parking_lot::Mutex::new(String::new()));
    let sink = copies.clone();
    opts.copy = Some(Arc::new(move |t: &str| {
        *sink.lock() = t.to_string();
        Ok(())
    }));
    let mut m = app_with(test_schema(), doc, opts);
    press(&mut m, &["down", "down", "down", "enter"]); // Logging
    press(&mut m, &["down", "enter"]); // Tags
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["down", "m", "down", "m"]);
    press(&mut m, &["y"]);
    assert_eq!(*copies.lock(), "prod\ndev");
    assert_eq!(m.notice, "Copied 2 values");
    assert!(m.top().unwrap().sel.is_empty());
}

#[test]
fn menu_mounts_add_fill_back() {
    let mut m = items_app(empty_doc());
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert_eq!(m.top().unwrap().path, vec![ks("mounts"), PathSeg::Idx(0)]);
    assert!(find_row(&mut m, |r, _| r.kind == RowKind::Remove).is_some());

    press(&mut m, &["down", "enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["/", "d", "a", "t", "a", "enter"]);
    assert!(m.overlay.is_none(), "edit should close");
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("src")]),
        "/data"
    );

    press(&mut m, &["esc"]);
    press(&mut m, &["down", "enter"]);
    let src = find_row(&mut m, |r, schema| {
        r.fn_.is_some() && schema.nodes[r.fn_.unwrap() as usize].key == "src"
    })
    .expect("Source row");
    assert_eq!(sget(&m.fs().doc, &src.path), "/data");
}

#[test]
fn menu_delete_item() {
    let mut m = items_app(test_doc());
    press(&mut m, &["down", "down", "down", "down", "enter", "down"]);
    press(&mut m, &["d"]);
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 0);

    // scalar array items delete the same way
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("tags", db.arr(vec![db.s("prod"), db.s("dev")]))]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(
        &mut m2,
        &["down", "down", "down", "enter", "down", "enter", "down"],
    );
    press(&mut m2, &["d"]);
    assert_eq!(
        arr_strs(&m2.fs().doc, &sp(&["logging", "tags"])),
        vec!["dev".to_string()]
    );
}

#[test]
fn item_count_jump() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![(
                "tags",
                db.arr(vec![db.s("a"), db.s("b"), db.s("c")]),
            )]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "enter", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);

    press(&mut m, &["2"]);
    assert_eq!(m.cmd_buf.join(""), "2");
    press(&mut m, &["g"]);
    assert_eq!(m.cmd_buf.join(""), "2g");
    press(&mut m, &["g"]);
    assert_eq!(row_at(&mut m).idx, 2);

    press(&mut m, &["0", "g", "g"]);
    assert_eq!(row_at(&mut m).idx, 0);
    press(&mut m, &["9", "g", "g"]);
    assert_eq!(row_at(&mut m).idx, 2);
    press(&mut m, &["1", "2", "g", "g"]);
    assert_eq!(row_at(&mut m).idx, 2);
    press(&mut m, &["g", "g"]);
    assert_eq!(row_at(&mut m).kind, RowKind::Add);

    // digits outside array menus do not arm a count
    press(&mut m, &["esc", "esc"]);
    set_cursor_path(&mut m, &sp(&["auth"]));
    press(&mut m, &["3"]);
    assert!(
        m.cmd_buf.is_empty(),
        "digits outside array menus must not arm a count"
    );
    press(&mut m, &["g", "g"]);
    assert_eq!(cursor_cursor(&mut m), 0);
}

#[test]
fn menu_move_item() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![(
                "tags",
                db.arr(vec![db.s("prod"), db.s("dev"), db.s("stage")]),
            )]),
        )])
    });
    let mut m = items_app(doc);
    press(
        &mut m,
        &["down", "down", "down", "enter", "down", "enter", "down"],
    );
    let tags = sp(&["logging", "tags"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["prod", "dev", "stage"]);

    press(&mut m, &["J"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["dev", "prod", "stage"]);
    assert_eq!(cursor_cursor(&mut m), 2);
    press(&mut m, &["K"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["prod", "dev", "stage"]);
    assert_eq!(cursor_cursor(&mut m), 1);
    press(&mut m, &["K"]);
    assert_eq!(m.notice, "already first item");
    press(&mut m, &["down", "down"]);
    press(&mut m, &["J"]);
    assert_eq!(m.notice, "already last item");

    // one key = one undo step
    press(&mut m, &["up", "up"]);
    press(&mut m, &["J"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["dev", "prod", "stage"]);
    press(&mut m, &["u"]);
    assert_eq!(arr_strs(&m.fs().doc, &tags), vec!["prod", "dev", "stage"]);

    // single-item array: both directions refuse
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("tags", db.arr(vec![db.s("solo")]))]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(
        &mut m2,
        &["down", "down", "down", "enter", "down", "enter", "down"],
    );
    press(&mut m2, &["J"]);
    assert_eq!(m2.notice, "already last item");
    press(&mut m2, &["K"]);
    assert_eq!(m2.notice, "already first item");

    // object item rows move the same way; undo restores
    let doc3 = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/a"))]),
                db.obj(vec![("src", db.s("/b"))]),
            ]),
        )])
    });
    let mut m3 = items_app(doc3);
    press(&mut m3, &["down", "down", "down", "down", "enter", "down"]);
    press(&mut m3, &["J"]);
    assert_eq!(mount_src(&m3.fs().doc, 0), "/b");
    assert_eq!(cursor_cursor(&mut m3), 2);
    press(&mut m3, &["u"]);
    assert_eq!(mount_src(&m3.fs().doc, 0), "/a");
}

#[test]
fn menu_scalar_array_add() {
    let mut m = items_app(empty_doc());
    press(
        &mut m,
        &["down", "down", "down", "enter", "down", "enter", "enter"],
    );
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert!(e.is_new),
        _ => panic!("scalar add should open the new-item editor"),
    }
    press(&mut m, &["esc"]);
    assert!(
        !has(&m.fs().doc, &sp(&["logging", "tags"])),
        "cancelled add must not create the array"
    );

    press(&mut m, &["enter"]);
    press(&mut m, &["p", "r", "o", "d", "enter"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &sp(&["logging", "tags"])),
        vec!["prod".to_string()]
    );
}

#[test]
fn menu_enum_array_toggle() {
    let mut m = items_app(empty_doc());
    press(&mut m, &["down", "down", "down", "enter", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::EnumArray);
    let levels = sp(&["logging", "levels"]);
    press(&mut m, &["space"]);
    assert_eq!(arr_strs(&m.fs().doc, &levels), vec!["debug".to_string()]);
    press(&mut m, &["down", "space"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &levels),
        vec!["debug".to_string(), "info".to_string()]
    );
    press(&mut m, &["space"]);
    assert_eq!(arr_strs(&m.fs().doc, &levels), vec!["debug".to_string()]);
    press(&mut m, &["up", "space"]);
    assert!(
        !has(&m.fs().doc, &levels),
        "empty optional enum array should be removed"
    );
}

#[test]
fn menu_one_of_variants() {
    let doc = dbuild(|db| db.obj(vec![("auth", db.obj(vec![("token", db.s("secret123"))]))]));
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "down", "down", "enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert_eq!(
        m.fs().schema.nodes[m.top().unwrap().node as usize].title,
        "Token"
    );
    assert!(m.top().unwrap().one_of.is_some());

    press(&mut m, &["t"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m.top().unwrap().cursor, 1);
    press(&mut m, &["enter"]);
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(
        m.fs().schema.nodes[m.top().unwrap().node as usize].title,
        "Token"
    );
    press(&mut m, &["esc"]);

    // picker, cursor onto None, confirm, then the enum picker
    press(&mut m, &["enter", "t", "up"]);
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["y"]);
    assert!(!has(&m.fs().doc, &sp(&["auth"])));
    assert!(matches!(m.overlay, Some(Overlay::Sel(_))));
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["auth"]), "none");

    // n keeps the value
    let doc2 = dbuild(|db| db.obj(vec![("auth", db.s("none"))]));
    let mut m2 = items_app(doc2);
    press(
        &mut m2,
        &[
            "down", "down", "down", "down", "down", "enter", "t", "down", "enter",
        ],
    );
    assert!(matches!(m2.overlay, Some(Overlay::Confirm(_))));
    press(&mut m2, &["n"]);
    assert!(m2.overlay.is_none());
    assert_eq!(str_at(&m2.fs().doc, &["auth"]), "none");
}

#[test]
fn cursor_wrap() {
    let mut m = items_app(test_doc());
    let last = m.menu_rows().len() - 1;
    press(&mut m, &["up"]);
    assert_eq!(cursor_cursor(&mut m), last);
    assert_eq!(row_at(&mut m).kind, RowKind::Raw);
    press(&mut m, &["down"]);
    assert_eq!(cursor_cursor(&mut m), 0);

    // enum picker wraps
    press(&mut m, &["down", "enter"]);
    let n = match &m.overlay {
        Some(Overlay::Sel(s)) => s.opts.len(),
        _ => panic!("picker did not open"),
    };
    press(&mut m, &["up"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => assert_eq!(s.cursor, n - 1),
        _ => panic!(),
    }
    press(&mut m, &["down"]);
    match &m.overlay {
        Some(Overlay::Sel(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
    press(&mut m, &["esc"]);

    // search results wrap
    let mut m2 = items_app(test_doc());
    press(&mut m2, &["/", "e"]);
    let n = search_state(&m2).results.len();
    assert!(n >= 2);
    press(&mut m2, &["up"]);
    assert_eq!(search_state(&m2).cursor, n - 1);
    press(&mut m2, &["down"]);
    assert_eq!(search_state(&m2).cursor, 0);
    press(&mut m2, &["ctrl+k"]);
    assert_eq!(search_state(&m2).cursor, n - 1);
    press(&mut m2, &["ctrl+j"]);
    assert_eq!(search_state(&m2).cursor, 0);
    assert_eq!(search_state(&m2).ti.value(), "e");

    // issues overlay wraps
    let mut opts = test_opts();
    opts.load_errors = vec![
        "at '$.server.port': minimum value is 1024".into(),
        "at '$.name': got number, want string".into(),
    ];
    let mut m3 = mk_app(test_doc(), opts);
    press(&mut m3, &["enter"]);
    press(&mut m3, &["up"]);
    match &m3.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 1),
        _ => panic!(),
    }
    press(&mut m3, &["down"]);
    match &m3.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
    press(&mut m3, &["ctrl+j"]);
    match &m3.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 1),
        _ => panic!(),
    }
    press(&mut m3, &["ctrl+k"]);
    match &m3.overlay {
        Some(Overlay::Issues(s)) => assert_eq!(s.cursor, 0),
        _ => panic!(),
    }
}

#[test]
fn vim_keys() {
    let mut m = items_app(test_doc());
    press(&mut m, &["j", "j"]);
    assert_eq!(cursor_cursor(&mut m), 2);
    press(&mut m, &["k"]);
    assert_eq!(cursor_cursor(&mut m), 1);

    assert_eq!(str_at(&m.fs().doc, &["mode"]), "<missing>");
    press(&mut m, &["l"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
    press(&mut m, &["h"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "http");

    set_cursor_path(&mut m, &sp(&["server"]));
    press(&mut m, &["l"]);
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["h"]);
    assert_eq!(m.fs().stack.len(), 1);

    press(&mut m, &["up", "up", "enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["h", "l"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert!(e.ti.value().contains("hl"), "value = {}", e.ti.value()),
        _ => panic!(),
    }
    press(&mut m, &["esc"]);

    // enum picker: j/k move like arrows
    let mut m2 = items_app(test_doc());
    set_cursor_path(&mut m2, &sp(&["mode"]));
    press(&mut m2, &["enter"]);
    let start = match &m2.overlay {
        Some(Overlay::Sel(s)) => s.cursor,
        _ => panic!("picker should open"),
    };
    press(&mut m2, &["j"]);
    match &m2.overlay {
        Some(Overlay::Sel(s)) => assert_ne!(s.cursor, start),
        _ => panic!(),
    }
    press(&mut m2, &["k"]);
    match &m2.overlay {
        Some(Overlay::Sel(s)) => assert_eq!(s.cursor, start),
        _ => panic!(),
    }
}

#[test]
fn menu_search_jump() {
    let mut m = items_app(test_doc());
    press(&mut m, &["/"]);
    assert_eq!(
        m.overlay.as_ref().map(|o| matches!(o, Overlay::Search(_))),
        Some(true)
    );
    press(&mut m, &["p", "o", "r", "t"]);
    assert_eq!(search_state(&m).results.len(), 1);
    press(&mut m, &["enter"]);
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(m.top().unwrap().path, sp(&["server"]));
    assert_eq!(row_at(&mut m).path, sp(&["server", "port"]));

    // esc clears search without moving
    let mut m2 = items_app(test_doc());
    press(&mut m2, &["/", "t", "o", "k", "esc"]);
    assert!(!matches!(m2.overlay, Some(Overlay::Search(_))));
    assert_eq!(m2.fs().stack.len(), 1);

    // search is scoped to the breadcrumb path
    let mut m3 = items_app(test_doc());
    press(&mut m3, &["down", "down", "enter"]);
    press(&mut m3, &["/", "n", "a", "m", "e"]);
    assert_eq!(search_state(&m3).results.len(), 0);
    press(&mut m3, &["esc"]);
    press(&mut m3, &["/", "0", ".", "0"]);
    assert_eq!(search_state(&m3).results.len(), 2);
    assert!(
        search_state(&m3)
            .results
            .iter()
            .any(|r| r.path == sp(&["server", "host"]))
    );
    press(&mut m3, &["esc"]);
    press(&mut m3, &["/", "0", "\\", ".", "0"]);
    assert_eq!(search_state(&m3).results.len(), 1);
    press(&mut m3, &["esc"]);
}

#[test]
fn menu_search_jump_return() {
    let mut m = items_app(test_doc());
    press(&mut m, &["/", "8", "0", "8", "0"]);
    press(&mut m, &["enter"]);
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(row_at(&mut m).path, sp(&["server", "port"]));

    press(&mut m, &["esc"]);
    let s = search_state(&m);
    assert_eq!(s.results.len(), 1);
    assert_eq!(s.cursor, 0);
    assert_eq!(s.ti.value(), "8080");

    press(&mut m, &["esc"]);
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 1);
    assert_eq!(row_at(&mut m).path, sp(&["name"]));

    // edits at the landing refresh the list on return
    let mut m2 = items_app(test_doc());
    press(&mut m2, &["/", "8", "0", "8", "0", "enter"]);
    press(&mut m2, &["enter", "ctrl+u", "9", "9", "9", "9", "enter"]);
    assert_eq!(num_at(&m2.fs().doc, &["server", "port"]), "9999");
    press(&mut m2, &["esc"]);
    assert_eq!(search_state(&m2).results.len(), 0);

    // landing on a container: drill deeper first, the overlay comes last
    let mut m3 = items_app(test_doc());
    press(&mut m3, &["/", "m", "o", "u", "n", "t", "s"]);
    press(&mut m3, &["enter"]);
    assert_eq!(m3.fs().stack.len(), 2);
    press(&mut m3, &["enter"]);
    assert_eq!(m3.fs().stack.len(), 3);
    press(&mut m3, &["esc"]);
    assert!(!matches!(m3.overlay, Some(Overlay::Search(_))));
    assert_eq!(m3.fs().stack.len(), 2);
    press(&mut m3, &["esc"]);
    assert!(matches!(m3.overlay, Some(Overlay::Search(_))));

    // replace mode parks and returns the same way
    let mut m4 = items_app(test_doc());
    press(&mut m4, &["r", "w", "e", "b", "a", "p", "p", "enter"]);
    press(&mut m4, &["esc"]);
    let s = search_state(&m4);
    assert!(s.replace);
}

#[test]
fn menu_search_second_jump_return() {
    let mut m = items_app(test_doc());
    press(&mut m, &["/", "8", "0", "8", "0", "enter"]);
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["esc"]);
    assert!(matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 1);

    press(
        &mut m,
        &["ctrl+u", "0", ".", "0", ".", "0", ".", "0", "enter"],
    );
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["esc"]);
    assert!(matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 1);
    press(&mut m, &["esc"]);
    assert!(!matches!(m.overlay, Some(Overlay::Search(_))));
    assert_eq!(m.fs().stack.len(), 1);
    assert_eq!(row_at(&mut m).path, sp(&["name"]));
}

#[test]
fn edit_completion() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/data")), ("dst", db.s("/mnt"))]),
                db.obj(vec![("src", db.s("/data2"))]),
            ]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["enter"]);
    press(&mut m, &["down", "enter"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(
                e.cands,
                vec![
                    "/data".to_string(),
                    "/data2".to_string(),
                    "/mnt".to_string()
                ],
                "candidate order"
            );
        }
        _ => panic!(),
    }
    press(&mut m, &["/", "d", "a", "t", "a", "2"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sugs, vec!["/data2".to_string()]),
        _ => panic!(),
    }
    press(&mut m, &["ctrl+u"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.sugs.len(), 3),
        _ => panic!(),
    }
    press(&mut m, &["tab"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "/data"),
        _ => panic!(),
    }
    press(&mut m, &["tab"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "/data2"),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(2), ks("src")]),
        "/data2"
    );

    // no prefix match: no suggestions, plain input still commits
    press(&mut m, &["down", "enter"]);
    press(&mut m, &["z", "z"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert!(e.sugs.is_empty()),
        _ => panic!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(2), ks("dst")]),
        "zz"
    );
}

#[test]
fn menu_remove_item_confirm() {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![db.obj(vec![("src", db.s("/data"))])]),
        )])
    });
    let mut m = items_app(doc);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down", "enter"]);
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
    press(&mut m, &["n"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert!(has(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0)]));

    press(&mut m, &["enter", "y"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::Items);
    assert!(!has(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0)]));

    // esc also cancels
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![db.obj(vec![("src", db.s("/a"))])]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(
        &mut m2,
        &[
            "down", "down", "down", "down", "enter", "down", "enter", "enter", "esc",
        ],
    );
    assert!(m2.overlay.is_none());
    assert!(has(&m2.fs().doc, &[ks("mounts"), PathSeg::Idx(0)]));
}

#[test]
fn menu_delete_key() {
    let mut m = items_app(test_doc());
    press(&mut m, &["down", "down", "enter"]);
    press(&mut m, &["down", "delete"]);
    assert!(!has(&m.fs().doc, &sp(&["server", "host"])));
    assert!(m.notice.is_empty());
    press(&mut m, &["up", "delete"]);
    assert!(!has(&m.fs().doc, &sp(&["server", "port"])));
    assert!(m.notice.is_empty());

    // optional object field
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![("levels", db.arr(vec![db.s("info")]))]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(&mut m2, &["down", "down", "down", "delete"]);
    assert!(!has(&m2.fs().doc, &sp(&["logging"])));

    // array item rows delete like d
    let mut m3 = items_app(test_doc());
    press(&mut m3, &["down", "down", "down", "down", "enter"]);
    press(&mut m3, &["down", "delete"]);
    assert!(!has(&m3.fs().doc, &[ks("mounts"), PathSeg::Idx(0)]));
}

#[test]
fn menu_search_replace() {
    // r opens replace mode with the search input focused
    let mut m = items_app(test_doc());
    press(&mut m, &["r"]);
    assert!(search_state(&m).replace);
    assert!(!search_state(&m).replace_focused);

    press(&mut m, &["/", "d", "a", "t", "a"]);
    assert_eq!(search_state(&m).results.len(), 1);
    press(&mut m, &["tab"]);
    assert!(search_state(&m).replace_focused);
    press(&mut m, &["/", "o", "p", "t"]);
    press(&mut m, &["enter"]);
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("src")]),
        "/opt"
    );
    assert!(
        matches!(m.overlay, Some(Overlay::Search(_))),
        "enter keeps the overlay open"
    );
    assert_eq!(search_state(&m).results.len(), 0);
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("dst")]),
        "/mnt"
    );

    // ctrl+r replaces every hit in scope; numbers stay untouched
    let doc2 = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("alpha")),
            (
                "server",
                db.obj(vec![
                    ("host", db.s("alpha.example.com")),
                    ("port", db.n("8080")),
                ]),
            ),
        ])
    });
    let mut m2 = items_app(doc2);
    press(
        &mut m2,
        &[
            "r", "a", "l", "p", "h", "a", "tab", "b", "e", "t", "a", "ctrl+r",
        ],
    );
    assert_eq!(str_at(&m2.fs().doc, &["name"]), "beta");
    assert_eq!(
        str_at(&m2.fs().doc, &["server", "host"]),
        "beta.example.com"
    );
    assert_eq!(num_at(&m2.fs().doc, &["server", "port"]), "8080");
    assert!(m2.overlay.is_none());
    assert_eq!(m2.notice, "replaced 2 occurrence(s) in 2 value(s)");

    // alt+i toggles case sensitivity
    let doc3 = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("alpha")),
            ("server", db.obj(vec![("host", db.s("alpha.example.com"))])),
        ])
    });
    let mut m3 = items_app(doc3);
    press(&mut m3, &["r", "A", "L", "P", "H", "A"]);
    assert_eq!(search_state(&m3).results.len(), 2);
    press(&mut m3, &["alt+i"]);
    assert_eq!(search_state(&m3).results.len(), 0);
    press(&mut m3, &["enter", "ctrl+r"]);
    assert_eq!(str_at(&m3.fs().doc, &["name"]), "alpha");
    press(&mut m3, &["alt+i"]);
    assert_eq!(search_state(&m3).results.len(), 2);

    // the replace scope follows the breadcrumb
    let doc6 = dbuild(|db| {
        db.obj(vec![
            ("name", db.s("same")),
            ("server", db.obj(vec![("host", db.s("same"))])),
        ])
    });
    let mut m6 = items_app(doc6);
    press(&mut m6, &["down", "down", "enter"]);
    press(
        &mut m6,
        &[
            "r", "s", "a", "m", "e", "tab", "o", "t", "h", "e", "r", "ctrl+r",
        ],
    );
    assert_eq!(str_at(&m6.fs().doc, &["server", "host"]), "other");
    assert_eq!(str_at(&m6.fs().doc, &["name"]), "same");

    // / arms replace with tab
    let mut m4 = items_app(test_doc());
    press(&mut m4, &["/", "t", "a", "b"]);
    assert!(!search_state(&m4).replace);
    press(&mut m4, &["tab"]);
    assert!(search_state(&m4).replace && search_state(&m4).replace_focused);

    // esc aborts without writing
    let doc5 = test_doc();
    let mut m5 = items_app(doc5);
    press(
        &mut m5,
        &["r", "/", "d", "a", "t", "a", "tab", "z", "z", "z", "esc"],
    );
    assert_eq!(
        sget(&m5.fs().doc, &[ks("mounts"), PathSeg::Idx(0), ks("src")]),
        "/data"
    );

    // with replace armed, enter on the search input still jumps
    press(&mut m4, &["esc"]);
    press(&mut m4, &["/", "w", "e", "b", "a", "p", "p"]);
    assert_eq!(search_state(&m4).results.len(), 1);
    press(&mut m4, &["tab", "tab"]);
    assert!(search_state(&m4).replace && !search_state(&m4).replace_focused);
    press(&mut m4, &["enter"]);
    assert!(!matches!(m4.overlay, Some(Overlay::Search(_))));
    assert_eq!(cursor_cursor(&mut m4), 0);
    assert_eq!(row_at(&mut m4).path, sp(&["name"]));
}

#[test]
fn menu_search_regex() {
    // regex is the default query mode; alt+x toggles literal matching
    let mut m = items_app(test_doc());
    press(&mut m, &["/"]);
    assert!(search_state(&m).regex);
    press(&mut m, &["0", "\\", ".", "0"]);
    assert_eq!(search_state(&m).results.len(), 1);
    press(&mut m, &["alt+x"]);
    assert_eq!(search_state(&m).results.len(), 0);
    press(&mut m, &["alt+x"]);
    assert_eq!(search_state(&m).results.len(), 1);
    press(&mut m, &["esc"]);

    // "$." anchors the match at the document root
    let mut m5 = items_app(test_doc());
    press(&mut m5, &["/"]);
    press(&mut m5, &["$", ".", "s", "e", "r", "v", "e", "r"]);
    let results = &search_state(&m5).results;
    assert!(!results.is_empty());
    for r in results {
        assert!(
            jsondoc::path_string(&r.path).starts_with("$.server"),
            "hit {:?}",
            r.path
        );
    }
    assert_eq!(search_state(&m5).results[0].path, sp(&["server"]));
    press(&mut m5, &["esc"]);

    // deeper than the root: an anchored query is not a substring search
    let mut m6 = items_app(test_doc());
    press(&mut m6, &["/"]);
    press(&mut m6, &["$", ".", "p", "o", "r", "t"]);
    assert_eq!(search_state(&m6).results.len(), 0);
    press(&mut m6, &["esc"]);

    // a path index still being typed keeps the regex valid
    let mut m7 = items_app(test_doc());
    press(&mut m7, &["/"]);
    press(&mut m7, &["$", ".", "m", "o", "u", "n", "t", "s", "["]);
    assert!(
        search_state(&m7).re_err.is_empty(),
        "re_err = {}",
        search_state(&m7).re_err
    );
    press(&mut m7, &["0", "]"]);
    assert_eq!(search_state(&m7).results.len(), 3);
    assert_eq!(
        search_state(&m7).results[0].path,
        vec![ks("mounts"), PathSeg::Idx(0)]
    );
    press(&mut m7, &["esc"]);

    // an uncompilable pattern yields no results plus an error
    let mut m2 = items_app(test_doc());
    press(&mut m2, &["/", "("]);
    assert_eq!(search_state(&m2).results.len(), 0);
    assert!(!search_state(&m2).re_err.is_empty());
    press(&mut m2, &["esc"]);

    // replace mode: regex by default, $1 groups expand
    let mut m3 = items_app(test_doc());
    press(&mut m3, &["r", "(", "\\", "d", "+", ")"]);
    assert_eq!(search_state(&m3).results.len(), 1);
    press(&mut m3, &["tab", "[", "$", "1", "]"]);
    press(&mut m3, &["enter"]);
    assert_eq!(str_at(&m3.fs().doc, &["server", "host"]), "[0].[0].[0].[0]");
}

#[test]
fn search_replace_unknown_key() {
    let mut doc = test_doc();
    let n = doc.new_str("hello world");
    let _ = doc.set(&sp(&["comment"]), n);
    let mut m = items_app(doc);
    press(&mut m, &["r"]);
    assert!(search_state(&m).replace);
    press(&mut m, &["h", "e", "l", "l", "o"]);
    assert!(
        search_state(&m)
            .results
            .iter()
            .any(|r| r.kind == RowKind::Unknown && r.path == sp(&["comment"]))
    );
    press(&mut m, &["tab"]);
    press(&mut m, &["g", "o", "o", "d", "b", "y", "e"]);
    press(&mut m, &["ctrl+r"]);
    assert_eq!(str_at(&m.fs().doc, &["comment"]), "goodbye world");
    assert!(
        m.notice.contains("replaced 1 occurrence"),
        "notice = {}",
        m.notice
    );
}

fn tags_items_model() -> (App, Doc) {
    let doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![(
                "tags",
                db.arr(vec![db.s("a"), db.s("b"), db.s("c")]),
            )]),
        )])
    });
    let mut m = items_app(doc.clone());
    press(&mut m, &["down", "down", "down", "enter", "down", "enter"]);
    (m, doc)
}

#[test]
fn insert_item_around_cursor() {
    // A after item 0
    let (mut m, _) = tags_items_model();
    press(&mut m, &["down", "A"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["x", "enter"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &sp(&["logging", "tags"])),
        vec![
            "a".to_string(),
            "x".to_string(),
            "b".to_string(),
            "c".to_string()
        ]
    );

    // a before item 1
    let (mut m, _) = tags_items_model();
    press(&mut m, &["down", "down", "a"]);
    press(&mut m, &["x", "enter"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &sp(&["logging", "tags"])),
        vec![
            "a".to_string(),
            "x".to_string(),
            "b".to_string(),
            "c".to_string()
        ]
    );

    // A clamps to append on < Add item >
    let (mut m, _) = tags_items_model();
    press(&mut m, &["A"]);
    press(&mut m, &["x", "enter"]);
    assert_eq!(
        arr_strs(&m.fs().doc, &sp(&["logging", "tags"])),
        vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "x".to_string()
        ]
    );

    // object items: A splices an empty container at idx+1 and drills in
    let doc2 = dbuild(|db| {
        db.obj(vec![(
            "mounts",
            db.arr(vec![
                db.obj(vec![("src", db.s("/a"))]),
                db.obj(vec![("src", db.s("/b"))]),
            ]),
        )])
    });
    let mut m2 = items_app(doc2);
    press(&mut m2, &["down", "down", "down", "down", "enter"]);
    press(&mut m2, &["down", "A"]);
    assert_eq!(m2.top().unwrap().kind, MenuKind::Object);
    assert_eq!(m2.top().unwrap().path, vec![ks("mounts"), PathSeg::Idx(1)]);
    assert_eq!(arr_len(&m2.fs().doc, &sp(&["mounts"])), 3);
    let spliced = getv(&m2.fs().doc, &[ks("mounts"), PathSeg::Idx(1)]).unwrap();
    match m2.fs().doc.value(spliced) {
        Value::Object(m) => assert!(m.is_empty()),
        _ => panic!("spliced item must be an empty object"),
    }
    assert_eq!(mount_src(&m2.fs().doc, 2), "/b");

    // < Add item > stays traceless until a field is written
    press(&mut m2, &["esc"]);
    press(&mut m2, &["up", "enter"]);
    assert_eq!(m2.top().unwrap().kind, MenuKind::Object);
    assert_eq!(m2.top().unwrap().path, vec![ks("mounts"), PathSeg::Idx(3)]);
    assert_eq!(arr_len(&m2.fs().doc, &sp(&["mounts"])), 3);
}

// --- regression: the text input renders on the edited row (bug: enter on a
// scalar set fn_ = None, so inline_edit_row never matched and no input box
// appeared) ---------------------------------------------------------------

#[test]
fn inline_edit_renders_on_the_field_row() {
    let mut m = mk_app(test_doc(), test_opts());
    m.height = 30;
    press(&mut m, &["enter"]);
    let fn_id = match &m.overlay {
        Some(Overlay::Edit(e)) => e.fn_,
        _ => panic!("enter on a scalar must open the inline editor"),
    };
    assert!(fn_id.is_some(), "schema field edits keep the real node id");
    let rows = m.menu_rows();
    let view = m.view();
    let matched = match &m.overlay {
        Some(Overlay::Edit(e)) => m.inline_edit_row(&rows[0], e),
        _ => false,
    };
    assert!(matched, "the edited row must render the input");
    // the input row shows the current value with a reverse-video cursor cell
    assert!(
        view.contains("\u{1b}[7m") && view.contains("webapp"),
        "inline editor must render the value with a cursor cell:\n{view}"
    );
}

#[test]
fn schema_suggestions_reach_the_editor() {
    // x-tag-reference/completion candidates come from the field's own node
    let mut tb = TB::new();
    let mut f = scalar("tag", "Tag", "string");
    f.suggestions = vec!["alpha".into(), "beta".into()];
    let tag = tb.id(f);
    let root = tb.id(crate::smodel::FormNode {
        key: String::new(),
        kind: Kind::Object,
        children: vec![tag],
        ..Default::default()
    });
    tb.root = root;
    let schema = tb.finish();
    let doc = dbuild(|db| db.obj(vec![]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]);
    let sugs = match &m.overlay {
        Some(Overlay::Edit(e)) => e.sugs.clone(),
        _ => panic!("editor must open"),
    };
    assert_eq!(
        sugs,
        vec!["alpha".to_string(), "beta".to_string()],
        "schema suggestions missing"
    );
}

// --- regression: the drill marker on oneOf rows ---------------------------
// Reported against the Go implementation too: a oneOf field resolved to a
// filled-in leaf value edits in place — the "--->" after its summary only
// belongs to mapping variants (objects/maps/arrays) and undetermined ones.

#[test]
fn oneof_leaf_value_hides_the_drill_marker() {
    let mut tb = TB::new();
    let leaf = tb.id(scalar("token", "Token", "string"));
    let obj = tb.id(crate::smodel::FormNode {
        key: "group".into(),
        title: "Group".into(),
        kind: Kind::Object,
        children: vec![],
        ..Default::default()
    });
    let one_of = tb.id(crate::smodel::FormNode {
        key: "auth".into(),
        title: "Auth".into(),
        kind: Kind::OneOf,
        variants: vec![leaf, obj],
        ..Default::default()
    });
    let root = tb.id(crate::smodel::FormNode {
        key: String::new(),
        kind: Kind::Object,
        children: vec![one_of],
        ..Default::default()
    });
    tb.root = root;
    let schema = tb.finish();
    // a scalar oneOf variant is the value itself: {"auth": "secret"}
    let doc = dbuild(|db| {
        let v = db.push(Value::Str("secret".into()));
        db.obj(vec![("auth", v)])
    });
    let mut m = app_with(schema, doc, test_opts());
    let row = m
        .menu_rows()
        .into_iter()
        .find(|r| r.path == vec![PathSeg::Key("auth".into())])
        .expect("auth row");
    assert_eq!(
        row.kind,
        RowKind::Drill,
        "oneOf rows stay drillable for enter"
    );
    assert!(
        !m.drill_marker(&row),
        "a filled scalar variant must not show --->"
    );
    let view = m.view();
    let line = view
        .lines()
        .find(|l| l.contains("Auth"))
        .expect("auth line");
    assert!(
        !line.contains("--->"),
        "row line still shows the marker: {line:?}"
    );
}

#[test]
fn oneof_mapping_and_undetermined_variants_keep_the_marker() {
    // undetermined (no value): marker shown
    let mut tb = TB::new();
    let leaf = tb.id(scalar("token", "Token", "string"));
    let obj = tb.id(crate::smodel::FormNode {
        key: "group".into(),
        title: "Group".into(),
        kind: Kind::Object,
        children: vec![],
        ..Default::default()
    });
    let one_of = tb.id(crate::smodel::FormNode {
        key: "auth".into(),
        title: "Auth".into(),
        kind: Kind::OneOf,
        variants: vec![leaf, obj],
        ..Default::default()
    });
    let root = tb.id(crate::smodel::FormNode {
        key: String::new(),
        kind: Kind::Object,
        children: vec![one_of],
        ..Default::default()
    });
    tb.root = root;
    let schema = tb.finish();
    let m_schema = schema;
    let doc = dbuild(|db| db.obj(vec![]));
    let mut m = app_with(m_schema, doc, test_opts());
    let row = m
        .menu_rows()
        .into_iter()
        .find(|r| r.path == vec![PathSeg::Key("auth".into())])
        .expect("auth row");
    assert!(
        m.drill_marker(&row),
        "undetermined variants keep the marker"
    );

    // mapping variant selected: marker shown
    let doc = dbuild(|db| {
        let g = db.obj(vec![]);
        let auth = db.obj(vec![("group", g)]);
        db.obj(vec![("auth", auth)])
    });
    let mut m = app_with(
        {
            let mut tb = TB::new();
            let leaf = tb.id(scalar("token", "Token", "string"));
            let obj = tb.id(crate::smodel::FormNode {
                key: "group".into(),
                title: "Group".into(),
                kind: Kind::Object,
                children: vec![],
                ..Default::default()
            });
            let one_of = tb.id(crate::smodel::FormNode {
                key: "auth".into(),
                title: "Auth".into(),
                kind: Kind::OneOf,
                variants: vec![leaf, obj],
                ..Default::default()
            });
            let root = tb.id(crate::smodel::FormNode {
                key: String::new(),
                kind: Kind::Object,
                children: vec![one_of],
                ..Default::default()
            });
            tb.root = root;
            tb.finish()
        },
        doc,
        test_opts(),
    );
    let row = m
        .menu_rows()
        .into_iter()
        .find(|r| r.path == vec![PathSeg::Key("auth".into())])
        .expect("auth row");
    assert!(m.drill_marker(&row), "mapping variants keep the marker");
}

// --- regression: the raw editor renders only its viewport -----------------
// It used to emit the whole buffer every keystroke, and a cursor below the
// terminal's height was invisible (Go/bubbles showed one textarea page).

#[test]
fn raw_textarea_view_is_windowed() {
    use crate::ui::widgets::TextArea;
    let mut ta = TextArea::new();
    ta.set_width(20);
    ta.set_height(5);
    ta.set_value(
        &(0..50)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let v = crate::ui::keys::ta_view(&ta);
    let lines: Vec<&str> = v.split('\n').collect();
    assert_eq!(lines.len(), 5, "only the viewport is rendered: {lines:?}");
    assert_eq!(lines[0], "line-0");
    // the page follows the cursor row: after 20 newlines it rests on line 0's
    // row (line 20 of the buffer) and the old page top is gone
    for _ in 0..20 {
        ta.insert_newline();
    }
    let v = crate::ui::keys::ta_view(&ta);
    let page: Vec<&str> = v.split('\n').collect();
    assert_eq!(page.len(), 5);
    assert_eq!(
        page[4], "line-0",
        "cursor row is the last visible line: {page:?}"
    );
    assert!(
        !v.contains("line-30"),
        "off-page lines are not rendered: {v:?}"
    );
    // long lines are clipped to the textarea width, never wrapped
    let mut ta = TextArea::new();
    ta.set_width(10);
    ta.set_height(3);
    ta.set_value(&"x".repeat(100));
    let v = crate::ui::keys::ta_view(&ta);
    assert_eq!(v.chars().count(), 10);
}

// --- YAML string values edit as plain text --------------------------------
// A string is text, not a document: prefilling the raw textarea with its YAML
// encoding gave multiline values a "|" header plus the alignment spaces, and
// quoted values their quotes. Strings now open as their own content and apply
// verbatim; mappings/sequences still open as YAML.

fn yaml_value_codec() -> RawCodec {
    let mut c = yaml_codec();
    c.marshal_value = Arc::new(|r: NodeRef<'_>| {
        let mut sub = Doc::new();
        sub.root = copy_subtree(r.doc, r.id, &mut sub);
        crate::doc::yamldoc::write(&sub, None).map_err(|e| e.to_string())
    });
    c
}

fn copy_subtree(src: &Doc, id: NodeId, dst: &mut Doc) -> NodeId {
    use crate::doc::{Member, Value as V};
    let merge: Vec<NodeId> = src
        .node(id)
        .merge
        .iter()
        .map(|&m| copy_subtree(src, m, dst))
        .collect();
    let value = match src.value(id) {
        V::Object(ms) => V::Object(
            ms.iter()
                .map(|m| Member {
                    key: m.key.clone(),
                    val: copy_subtree(src, m.val, dst),
                })
                .collect(),
        ),
        V::Array(is) => V::Array(is.iter().map(|&i| copy_subtree(src, i, dst)).collect()),
        v => v.clone(),
    };
    dst.push(NodeData {
        value,
        anchor: String::new(),
        merge,
        raw: String::new(),
    })
}

fn raw_field_schema() -> Schema {
    schema_build(|b| {
        let msg = b.id(fnode("msg", "Msg", Kind::Raw));
        b.id(FormNode {
            key: String::new(),
            kind: Kind::Object,
            children: vec![msg],
            ..FormNode::default()
        })
    })
}

fn open_msg_textarea(schema: Schema, doc: Doc, codec: RawCodec) -> App {
    let mut opts = test_opts();
    opts.raw = codec;
    let mut m = app_with(schema, doc, opts);
    // t opens the type picker; the JSON variant is the explicit
    // raw-textarea escape hatch
    press(&mut m, &["t"]);
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::OneOf,
        "t selects a different value type"
    );
    press(&mut m, &["G", "enter"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Text(_))),
        "the JSON variant edits in the built-in textarea"
    );
    m
}

fn textarea_text(m: &App) -> String {
    match &m.overlay {
        Some(Overlay::Text(s)) => s.ta.value(),
        _ => panic!("no textarea open"),
    }
}

#[test]
fn untyped_field_with_value_edits_directly() {
    let doc = dbuild(|db| db.obj(vec![("msg", db.s("old"))]));
    let mut m = app_with(raw_field_schema(), doc, test_opts());
    // a known runtime type edits directly; no picker, no external editor
    press(&mut m, &["enter"]);
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    assert!(m.pending_edit.is_none());
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("new"),
        _ => unreachable!(),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["msg"]), "new");

    // null has no inferable type: enter opens the picker
    let doc = dbuild(|db| db.obj(vec![("msg", db.push(Value::Null))]));
    let mut m = app_with(raw_field_schema(), doc, test_opts());
    press(&mut m, &["enter"]);
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
}

#[test]
fn yaml_multiline_string_opens_as_plain_text() {
    let (doc, _) = crate::doc::yamldoc::parse("msg: |\n  hello\n  world\n").unwrap();
    let m = open_msg_textarea(raw_field_schema(), doc, yaml_value_codec());
    assert_eq!(
        textarea_text(&m),
        "hello\nworld\n",
        "the YAML literal header and its alignment spaces must not reach the editor"
    );
}

#[test]
fn yaml_string_edits_apply_verbatim() {
    let (doc, _) = crate::doc::yamldoc::parse("msg: |\n  hello\n  world\n").unwrap();
    let mut m = open_msg_textarea(raw_field_schema(), doc, yaml_value_codec());
    // unchanged text keeps the value (no YAML re-parse surprises)
    press(&mut m, &["ctrl+s"]);
    assert_eq!(str_at(&m.fs().doc, &["msg"]), "hello\nworld\n");
    // a multiline edit is stored exactly, indentation and all
    let mut m2 = open_msg_textarea(
        raw_field_schema(),
        crate::doc::yamldoc::parse("msg: hello\n").unwrap().0,
        yaml_value_codec(),
    );
    if let Some(Overlay::Text(s)) = &mut m2.overlay {
        s.ta.set_value("  spaced\nline2");
    }
    press(&mut m2, &["ctrl+s"]);
    assert_eq!(str_at(&m2.fs().doc, &["msg"]), "  spaced\nline2");
}

#[test]
fn yaml_mapping_values_still_open_as_yaml() {
    let (doc, _) = crate::doc::yamldoc::parse("msg:\n  a: 1\n  b: two\n").unwrap();
    let m = open_msg_textarea(raw_field_schema(), doc, yaml_value_codec());
    assert_eq!(textarea_text(&m), "a: 1\nb: two\n");
}

#[test]
fn json_string_values_open_as_plain_text_too() {
    // the same rule holds for JSON: the editor shows the string, not "\"v\""
    let doc = dbuild(|db| {
        let v = db.push(Value::Str("v".into()));
        db.obj(vec![("msg", v)])
    });
    let m = open_msg_textarea(raw_field_schema(), doc, test_opts().raw);
    assert_eq!(textarea_text(&m), "v");
}

#[test]
fn repeated_move_matches_individual_moves_and_wraps() {
    let mut one = mk_app(test_doc(), test_opts());
    let mut many = mk_app(test_doc(), test_opts());
    for _ in 0..9 {
        one.update(Msg::Char('j', false));
    }
    many.update_repeat(Msg::Char('j', false), 9);
    assert_eq!(one.top().unwrap().cursor, many.top().unwrap().cursor);
    // a run longer than the row count wraps exactly like separate presses
    for _ in 0..20 {
        one.update(Msg::Down);
    }
    many.update_repeat(Msg::Down, 20);
    assert_eq!(one.top().unwrap().cursor, many.top().unwrap().cursor);
}

#[test]
fn repeated_move_extends_visual_selection_once() {
    let mut one = mk_app(test_doc(), test_opts());
    let mut many = mk_app(test_doc(), test_opts());
    for app in [&mut one, &mut many] {
        app.update(Msg::Char('v', false));
    }
    for _ in 0..3 {
        one.update(Msg::Char('j', false));
    }
    many.update_repeat(Msg::Char('j', false), 3);
    assert_eq!(
        one.top().unwrap().cursor,
        many.top().unwrap().cursor,
        "visual range ends on the same row"
    );
    assert_eq!(one.top().unwrap().sel, many.top().unwrap().sel);
}

#[test]
fn repeated_move_still_records_every_macro_key() {
    let mut app = mk_app(test_doc(), test_opts());
    app.update(Msg::Char('@', false));
    assert!(app.recording);
    app.update_repeat(Msg::Char('j', false), 3);
    assert_eq!(app.rec_buf.len(), 3);
    assert_eq!(app.top().unwrap().cursor, 3);
}

#[test]
fn repeated_typing_replays_every_character() {
    let mut app = mk_app(test_doc(), test_opts());
    press(&mut app, &["enter"]); // edit the first field
    app.update(Msg::Ctrl('u'));
    app.update_repeat(Msg::Char('a', false), 3);
    press(&mut app, &["enter"]);
    assert_eq!(str_at(&app.fs().doc, &["name"]), "aaa");
}
