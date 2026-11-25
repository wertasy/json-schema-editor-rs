//! Paste (P/ctrl+v) and clipboard copy/cut: objects, arrays, YAML/TOML
//! payloads, multi-selection round trips.

use super::*;
use super::{
    app_with, app_with_codec, arr_len, arr_strs, dbuild, empty_doc, getv, ks, mk_app, mount_src,
    n_at_idx, num_at, obj_keys, path, press, row_at, scalar, schema_build, sget, sp, str_at,
    test_doc, test_opts, test_schema,
};

use super::*;

use parking_lot::Mutex;

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
// menu_paste_test.go
// ===========================================================================

fn paste_app(doc: Doc, text: &str) -> App {
    let mut opts = test_opts();
    let t = text.to_string();
    opts.paste = Some(Arc::new(move || Ok(t.clone())));
    app_with(test_schema(), doc, opts)
}

fn yaml_paste_app(doc: Doc, text: &str) -> App {
    let mut opts = test_opts();
    let t = text.to_string();
    opts.paste = Some(Arc::new(move || Ok(t.clone())));
    app_with_codec(test_schema(), doc, opts, crate::doc::codec_for("a.yaml"))
}

fn toml_paste_app(doc: Doc, text: &str) -> App {
    let mut opts = test_opts();
    let t = text.to_string();
    opts.paste = Some(Arc::new(move || Ok(t.clone())));
    app_with_codec(test_schema(), doc, opts, crate::doc::codec_for("a.toml"))
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
    press(&mut m, &["down"]);
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
    press(&mut m, &["down", "p"]);
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
    press(&mut m, &["down"]);
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
    press(&mut m, &["down"]);
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
        press(&mut m, &["down"]);
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

    // an empty array shows < Add item >: P there appends the first item
    let mut m = paste_app(empty_doc(), "8,");
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    assert_eq!(row_at(&mut m).kind, RowKind::Add);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted 1 items into $.mounts");
    assert_eq!(n_at_idx(&m.fs().doc, "mounts", 0), "8");
}

// --- copied item selections paste back as items ----------------------------

/// tags_doc holds logging.tags = ["alpha", "beta"].
fn tags_doc() -> Doc {
    dbuild(|db| {
        db.obj(vec![
            ("name", db.s("webapp")),
            (
                "logging",
                db.obj(vec![("tags", db.arr(vec![db.s("alpha"), db.s("beta")]))]),
            ),
        ])
    })
}

/// clip_app wires opts.copy/opts.paste to one in-memory clipboard so a test
/// can copy in one menu and paste in another.
fn clip_app(doc: Doc) -> (App, Arc<Mutex<Option<String>>>) {
    let clip = Arc::new(Mutex::new(None));
    let mut opts = test_opts();
    {
        let clip = clip.clone();
        opts.copy = Some(Arc::new(move |t| {
            *clip.lock() = Some(t.to_string());
            Ok(())
        }));
    }
    {
        let clip = clip.clone();
        opts.paste = Some(Arc::new(move || {
            clip.lock()
                .clone()
                .ok_or_else(|| "clipboard is empty".to_string())
        }));
    }
    (app_with(test_schema(), doc, opts), clip)
}

/// clip_app_codec is clip_app with the session codec overridden.
fn clip_app_codec(
    doc: Doc,
    codec: &'static dyn crate::doc::Codec,
) -> (App, Arc<Mutex<Option<String>>>) {
    let (mut m, clip) = clip_app(doc);
    m.fs_mut().codec = codec;
    (m, clip)
}

/// enter_tags drills root → logging → tags items menu (cursor on item 0).
/// Root rows follow schema order: name, mode, server, logging, mounts, auth;
/// logging rows are levels, tags.
fn enter_tags(m: &mut App) {
    press(m, &["down", "down", "down", "enter"]);
    press(m, &["down", "enter"]);
}

#[test]
fn paste_multi_selected_string_items_round_trip() {
    let (mut m, clip) = clip_app(tags_doc());
    enter_tags(&mut m);
    press(&mut m, &["v", "j", "y"]);
    assert_eq!(m.notice, "Copied 2 values");
    assert_eq!(clip.lock().as_deref(), Some("alpha,beta"));
    // the copy ends the visual select; the marks go with it
    assert!(!m.top().unwrap().visual);
    assert!(m.top().unwrap().sel.is_empty());

    // cursor still on item 1: p splices below it
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 items into $.logging.tags");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["logging", "tags"])), 4);
    for (i, want) in ["alpha", "beta", "alpha", "beta"].iter().enumerate() {
        assert_eq!(
            sget(&m.fs().doc, &[ks("logging"), ks("tags"), PathSeg::Idx(i)]),
            *want
        );
    }

    // j after the operation must not resume marking
    press(&mut m, &["j"]);
    assert!(!m.top().unwrap().visual);
    assert!(m.top().unwrap().sel.is_empty());
}

#[test]
fn paste_newline_joined_items_yaml_and_toml_arrays() {
    // bare multi-line text also classifies as one YAML scalar and is no TOML
    // document at all: an array target must still get the copied items back
    let tag_at =
        |m: &App, i: usize| sget(&m.fs().doc, &[ks("logging"), ks("tags"), PathSeg::Idx(i)]);
    let mut m = yaml_paste_app(tags_doc(), "alpha\nbeta");
    enter_tags(&mut m);
    press(&mut m, &["down", "p"]); // cursor on beta: p splices below it
    assert_eq!(m.notice, "pasted 2 items into $.logging.tags");
    assert_eq!(tag_at(&m, 2), "alpha");
    assert_eq!(tag_at(&m, 3), "beta");

    let mut m = toml_paste_app(tags_doc(), "alpha\nbeta");
    enter_tags(&mut m);
    press(&mut m, &["down", "p"]);
    assert_eq!(m.notice, "pasted 2 items into $.logging.tags");
    assert_eq!(tag_at(&m, 2), "alpha");
    assert_eq!(tag_at(&m, 3), "beta");
}

#[test]
fn paste_newline_joined_items_are_strings_and_literals() {
    // strings stay unquoted (how y copies them), JSON lines become values
    let tag_at =
        |m: &App, i: usize| sget(&m.fs().doc, &[ks("logging"), ks("tags"), PathSeg::Idx(i)]);
    let mut m = paste_app(tags_doc(), "alpha\n7\ntrue\n{\"k\":1}");
    enter_tags(&mut m);
    press(&mut m, &["down", "p"]);
    assert_eq!(m.notice, "pasted 4 items into $.logging.tags");
    assert_eq!(tag_at(&m, 2), "alpha");
    let at = |m: &App, i: usize| -> Option<Value> {
        m.fs()
            .doc
            .get(&[ks("logging"), ks("tags"), PathSeg::Idx(i)])
            .map(|n| m.fs().doc.value(n).clone())
    };
    assert!(matches!(at(&m, 3), Some(Value::Num(text)) if text == "7"));
    assert!(matches!(at(&m, 4), Some(Value::Bool(true))));
    let obj_at = m
        .fs()
        .doc
        .get(&[ks("logging"), ks("tags"), PathSeg::Idx(5)])
        .expect("object item");
    assert_eq!(m.fs().doc.compact(obj_at), r#"{"k":1}"#);
}

#[test]
fn visual_select_off_after_copy_cut_delete() {
    // object menu: v + j + j + y copies the marked keys and ends the visual
    // select (unset mode has no value, so two pairs come back)
    let (mut m, clip) = clip_app(test_doc());
    press(&mut m, &["v", "j", "j", "y"]);
    assert_eq!(m.notice, "Copied 2 key-value pairs");
    assert!(clip.lock().is_some());
    assert!(!m.top().unwrap().visual);
    assert!(m.top().unwrap().sel.is_empty());
    press(&mut m, &["j"]);
    assert!(!m.top().unwrap().visual, "j must keep navigating plainly");
    assert!(m.top().unwrap().sel.is_empty());

    // items menu: x cuts the marked elements and ends the visual select
    let (mut m2, _clip) = clip_app(tags_doc());
    enter_tags(&mut m2);
    press(&mut m2, &["v", "j", "x"]);
    assert_eq!(m2.notice, "Cut 2 values");
    assert!(!m2.top().unwrap().visual);
    assert!(m2.top().unwrap().sel.is_empty());
    assert_eq!(arr_len(&m2.fs().doc, &sp(&["logging", "tags"])), 0);

    // d deletes the marked keys and ends the visual select
    let mut m3 = mk_app(test_doc(), test_opts());
    press(&mut m3, &["v", "j", "j", "d"]);
    assert!(!m3.top().unwrap().visual);
    assert!(m3.top().unwrap().sel.is_empty());
    assert_eq!(obj_keys(&m3.fs().doc, &[]).join(" "), "mounts");
}

#[test]
fn paste_clears_marked_selection() {
    // an insert shifts the marked index paths: a successful paste must not
    // leave marks pointing at rows that moved
    let mut m = paste_app(tags_doc(), "[\"x\",\"y\"]");
    enter_tags(&mut m);
    press(&mut m, &["m", "down", "m"]);
    assert_eq!(m.top().unwrap().sel.len(), 2);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 items into $.logging.tags");
    assert!(m.top().unwrap().sel.is_empty());
}

#[test]
fn paste_multi_selected_object_items_round_trip() {
    // containers copy one compact line per element so the selection splices
    // back as items in every format
    for file in ["a.json", "a.jsonc", "a.yaml", "a.toml"] {
        let (mut m, clip) = clip_app_codec(two_mount_doc(), crate::doc::codec_for(file));
        press(&mut m, &["down", "down", "down", "down", "enter"]);
        press(&mut m, &["v", "j", "y"]);
        assert_eq!(m.notice, "Copied 2 values", "{file}");
        assert_eq!(
            clip.lock().as_deref(),
            Some(r#"{"src":"/a","dst":"/1"},{"src":"/b","dst":"/2"}"#),
            "{file}"
        );

        // cursor on item 1: p splices the copied objects below it
        press(&mut m, &["p"]);
        assert_eq!(m.notice, "pasted 2 items into $.mounts", "{file}");
        assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 4, "{file}");
        assert_eq!(mount_src(&m.fs().doc, 2), "/a", "{file}");
        assert_eq!(mount_src(&m.fs().doc, 3), "/b", "{file}");
        assert_eq!(
            sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(3), ks("dst")]),
            "/2",
            "{file}"
        );
    }
}

#[test]
fn cut_multi_selected_object_items_clipboard() {
    let (mut m, clip) = clip_app_codec(two_mount_doc(), crate::doc::codec_for("a.jsonc"));
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["v", "j", "x"]);
    assert_eq!(m.notice, "Cut 2 values");
    assert_eq!(
        clip.lock().as_deref(),
        Some(r#"{"src":"/a","dst":"/1"},{"src":"/b","dst":"/2"}"#)
    );
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 0);
}

#[test]
fn paste_separator_split_into_array() {
    let tag_at =
        |m: &App, i: usize| sget(&m.fs().doc, &[ks("logging"), ks("tags"), PathSeg::Idx(i)]);

    // commas split bare strings into items
    let mut m = paste_app(tags_doc(), "alpha, beta");
    enter_tags(&mut m);
    press(&mut m, &["down", "p"]);
    assert_eq!(m.notice, "pasted 2 items into $.logging.tags");
    assert_eq!(tag_at(&m, 2), "alpha");
    assert_eq!(tag_at(&m, 3), "beta");

    // spaces split too
    let mut m = paste_app(tags_doc(), "alpha beta gamma");
    enter_tags(&mut m);
    press(&mut m, &["down", "p"]);
    assert_eq!(m.notice, "pasted 3 items into $.logging.tags");
    assert_eq!(tag_at(&m, 2), "alpha");
    assert_eq!(tag_at(&m, 3), "beta");
    assert_eq!(tag_at(&m, 4), "gamma");

    // pieces parse as JSON when they can
    let mut m = paste_app(tags_doc(), "x, 7, true");
    enter_tags(&mut m);
    press(&mut m, &["down", "p"]);
    assert_eq!(m.notice, "pasted 3 items into $.logging.tags");
    assert_eq!(tag_at(&m, 2), "x");
    let num = m
        .fs()
        .doc
        .get(&[ks("logging"), ks("tags"), PathSeg::Idx(3)])
        .map(|n| m.fs().doc.value(n));
    assert!(matches!(num, Some(Value::Num(text)) if text == "7"));
    let flag = m
        .fs()
        .doc
        .get(&[ks("logging"), ks("tags"), PathSeg::Idx(4)])
        .map(|n| m.fs().doc.value(n));
    assert!(matches!(flag, Some(Value::Bool(true))));
}

#[test]
fn paste_p_keeps_value_semantics_on_string_rows() {
    // P on a string item row replaces that value with the whole text:
    // single strings with separators must not split
    let mut m = paste_app(tags_doc(), "alpha, beta");
    enter_tags(&mut m);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.logging.tags[0]");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["logging", "tags"])), 2);
    assert_eq!(
        sget(&m.fs().doc, &[ks("logging"), ks("tags"), PathSeg::Idx(0)]),
        "alpha, beta"
    );

    // a parseable single value keeps the p notice, even in an items menu
    let mut m = paste_app(tags_doc(), "99");
    enter_tags(&mut m);
    press(&mut m, &["p"]);
    assert_eq!(
        m.notice,
        "a single value replaces a row's value — press P on the row"
    );
    assert_eq!(arr_len(&m.fs().doc, &sp(&["logging", "tags"])), 2);
}

#[test]
fn paste_comma_separated_objects_into_array() {
    // the exact form a multi-object selection copies: one line, top-level
    // commas between units
    let text = r#"{"src":"/c","dst":"/d"},{"src":"/e","dst":"/f"}"#;
    let mut m = paste_app(two_mount_doc(), text);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down", "p"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 4);
    assert_eq!(mount_src(&m.fs().doc, 2), "/c");
    assert_eq!(mount_src(&m.fs().doc, 3), "/e");
    assert_eq!(
        sget(&m.fs().doc, &[ks("mounts"), PathSeg::Idx(3), ks("dst")]),
        "/f"
    );

    // P above the cursor item
    let mut m = paste_app(two_mount_doc(), text);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down", "P"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(arr_len(&m.fs().doc, &sp(&["mounts"])), 4);
    assert_eq!(mount_src(&m.fs().doc, 1), "/c");
    assert_eq!(mount_src(&m.fs().doc, 2), "/e");
    assert_eq!(mount_src(&m.fs().doc, 3), "/b");

    // commas inside braces or brackets must not split
    let mut m = paste_app(
        two_mount_doc(),
        r#"{ "src": "/c" },{ "src": "/d", "k": [1, 2] }"#,
    );
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(mount_src(&m.fs().doc, 2), "/d");
    let k = m
        .fs()
        .doc
        .get(&[ks("mounts"), PathSeg::Idx(2), ks("k")])
        .map(|n| m.fs().doc.compact(n));
    assert_eq!(k.as_deref(), Some("[1,2]"));

    // works into YAML documents too (JSON units are format-neutral)
    let mut m = yaml_paste_app(two_mount_doc(), text);
    press(&mut m, &["down", "down", "down", "down", "enter"]);
    press(&mut m, &["down", "p"]);
    assert_eq!(m.notice, "pasted 2 items into $.mounts");
    assert_eq!(mount_src(&m.fs().doc, 2), "/c");
    assert_eq!(mount_src(&m.fs().doc, 3), "/e");
}

#[test]
fn selection_special_strings_round_trip() {
    // strings that would corrupt a comma-joined line copy compact-quoted
    let doc = dbuild(|db| {
        db.obj(vec![(
            "logging",
            db.obj(vec![(
                "tags",
                db.arr(vec![
                    db.s("a,b"),
                    db.s("hello world"),
                    db.s(""),
                    db.s("plain"),
                ]),
            )]),
        )])
    });
    let (mut m, clip) = clip_app(doc);
    enter_tags(&mut m);
    press(&mut m, &["v", "j", "j", "j", "y"]);
    assert_eq!(m.notice, "Copied 4 values");
    assert_eq!(
        clip.lock().as_deref(),
        Some(r#""a,b","hello world","",plain"#)
    );
    press(&mut m, &["p"]);
    assert_eq!(m.notice, "pasted 4 items into $.logging.tags");
    let tag_at =
        |m: &App, i: usize| sget(&m.fs().doc, &[ks("logging"), ks("tags"), PathSeg::Idx(i)]);
    assert_eq!(tag_at(&m, 4), "a,b");
    assert_eq!(tag_at(&m, 5), "hello world");
    assert_eq!(tag_at(&m, 6), "");
    assert_eq!(tag_at(&m, 7), "plain");
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
    press(&mut m, &["down", "p"]);
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
    press(&mut m, &["down"]);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.items[1]");
    assert_eq!(n_at_idx(&m.fs().doc, "items", 1), "99");
    assert_eq!(n_at_idx(&m.fs().doc, "items", 0), "1");

    // an action/placeholder row refuses a single value
    let schema = schema_build(|b| {
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            ..FormNode::default()
        })
    });
    let mut opts = test_opts();
    opts.paste = Some(Arc::new(|| Ok("123".to_string())));
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), opts);
    assert_eq!(row_at(&mut m).kind, RowKind::NoItems);
    press(&mut m, &["P"]);
    assert_eq!(
        m.notice,
        "a single value replaces a row's value — press P on the row"
    );
    assert_eq!(row_at(&mut m).kind, RowKind::NoItems);
}

#[test]
fn paste_plain_string_onto_string_row() {
    // y/Y copy a string without quotes; P must accept that text back in a
    // JSON document (it used to be a parse error everywhere but YAML/TOML).
    let mut m = paste_app(test_doc(), "hello world");
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.name");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "hello world");
    press(&mut m, &["u"]);
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));

    // a quoted string whose JSON spelling is invalid keeps its text
    let mut m = paste_app(test_doc(), r#""C:\Users""#);
    press(&mut m, &["P"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), r#"C:\Users"#);

    // a selection with string values takes the plain text too
    let mut m = paste_app(test_doc(), "tag");
    press(&mut m, &["m", "down", "m", "P"]);
    assert_eq!(m.notice, "pasted value into 2 keys");
    assert_eq!(str_at(&m.fs().doc, &["name"]), "tag");
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "tag");
}

#[test]
fn paste_plain_string_needs_string_target() {
    // P on a non-string value keeps the old parse error instead of turning
    // arbitrary text into a value.
    let mut m = paste_app(test_doc(), "hello");
    press(&mut m, &["down", "down", "enter", "P"]); // server/port row
    assert_eq!(m.notice, "clipboard is not JSON keys or items");
    assert!(nodes_equal_doc(&test_doc(), &m.fs().doc));
}

#[test]
fn edit_ctrl_v_pastes_clipboard() {
    let mut opts = test_opts();
    opts.paste = Some(Arc::new(|| Ok("pasted".to_string())));
    let mut m = app_with(test_schema(), test_doc(), opts);
    press(&mut m, &["enter"]);
    m.update(Msg::Ctrl('v'));
    match &m.overlay {
        Some(Overlay::Edit(e)) => assert_eq!(e.ti.value(), "webapppasted"),
        _ => panic!("edit overlay must stay open"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "webapppasted");
}

#[test]
fn bracketed_paste_fills_raw_textarea() {
    let mut opts = test_opts();
    opts.paste = Some(Arc::new(|| Ok("{\n  \"a\": 1\n}".to_string())));
    let mut m = app_with(test_schema(), test_doc(), opts);
    m.overlay = Some(Overlay::Text(TextState {
        path: None,
        fn_: None,
        ta: TextArea::new(),
        err: String::new(),
        verbatim_str: false,
    }));
    m.update(Msg::Ctrl('v'));
    match &m.overlay {
        Some(Overlay::Text(s)) => assert_eq!(s.ta.value(), "{\n  \"a\": 1\n}"),
        _ => panic!("text overlay must stay open"),
    }
    press(&mut m, &["ctrl+s"]);
    assert_eq!(m.fs().doc.get(&path(&["a"])).is_some(), true);
}

#[test]
fn bracketed_paste_filters_search() {
    let mut opts = test_opts();
    opts.paste = Some(Arc::new(|| Ok("server".to_string())));
    let mut m = app_with(test_schema(), test_doc(), opts);
    press(&mut m, &["/"]);
    m.update(Msg::Paste("server".to_string()));
    match &m.overlay {
        Some(Overlay::Search(s)) => {
            assert_eq!(s.ti.value(), "server");
            assert!(s.results.iter().all(|r| !r.path.is_empty()));
            assert!(!s.results.is_empty(), "search results for server");
        }
        _ => panic!("search overlay must stay open"),
    }
}

#[derive(Clone, Copy)]
enum RuleSetValue {
    Missing,
    Str(&'static str),
    Array(&'static str),
    Object,
}

fn oneof_rule_set_schema() -> Schema {
    schema_build(|b| {
        let s = b.id(scalar("rule_set", "Rule set", "string"));
        let arr_items = b.id(scalar("", "", "string"));
        let arr = b.id(FormNode {
            key: "rule_set".into(),
            title: "Rule set".into(),
            kind: Kind::Array,
            items: Some(arr_items),
            ..FormNode::default()
        });
        let one = b.id(FormNode {
            key: "rule_set".into(),
            title: "Rule set".into(),
            kind: Kind::OneOf,
            variants: vec![s, arr],
            ..FormNode::default()
        });
        let item = b.id(FormNode {
            key: "rules".into(),
            title: "Rule".into(),
            kind: Kind::Object,
            children: vec![one],
            ..FormNode::default()
        });
        let rules = b.id(FormNode {
            key: "rules".into(),
            title: "Rules".into(),
            kind: Kind::Array,
            items: Some(item),
            ..FormNode::default()
        });
        let dns = b.id(FormNode {
            key: "dns".into(),
            title: "DNS".into(),
            kind: Kind::Object,
            children: vec![rules],
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![dns],
            ..FormNode::default()
        })
    })
}

fn oneof_rule_set_path() -> Vec<PathSeg> {
    vec![ks("dns"), ks("rules"), PathSeg::Idx(4), ks("rule_set")]
}

fn oneof_rule_set_doc(value: RuleSetValue) -> Doc {
    dbuild(|db| {
        let items = (0..5)
            .map(|i| {
                if i != 4 {
                    return db.obj(vec![]);
                }
                match value {
                    RuleSetValue::Missing => db.obj(vec![]),
                    RuleSetValue::Str(s) => db.obj(vec![("rule_set", db.s(s))]),
                    RuleSetValue::Array(s) => db.obj(vec![("rule_set", db.arr(vec![db.s(s)]))]),
                    RuleSetValue::Object => {
                        db.obj(vec![("rule_set", db.obj(vec![("a", db.n("1"))]))])
                    }
                }
            })
            .collect();
        db.obj(vec![("dns", db.obj(vec![("rules", db.arr(items))]))])
    })
}

fn paste_into_rule_set(clip: &str, doc: Doc) -> App {
    let mut opts = test_opts();
    let t = clip.to_string();
    opts.paste = Some(Arc::new(move || Ok(t.clone())));
    let mut m = app_with(oneof_rule_set_schema(), doc, opts);
    m.jump_to_row(&oneof_rule_set_path());
    m
}

#[test]
fn paste_scalar_onto_oneof_string_row() {
    // sing-box style: rule_set is oneOf string / [string]. The row is a
    // oneOf drill row, which used to be refused as a single-value target.
    let target = oneof_rule_set_path();
    for (clip, value) in [
        ("this-is-a-new-string", RuleSetValue::Str("geosite-old")),
        (
            r#""this-is-a-new-string""#,
            RuleSetValue::Str("geosite-old"),
        ),
        ("this-is-a-new-string", RuleSetValue::Missing),
        // a string leaf is a valid target even while the current value is
        // the array variant
        ("this-is-a-new-string", RuleSetValue::Array("geosite-old")),
    ] {
        let mut m = paste_into_rule_set(clip, oneof_rule_set_doc(value));
        press(&mut m, &["P"]);
        assert_eq!(
            m.notice, "pasted value into $.dns.rules[4].rule_set",
            "clipboard {clip}"
        );
        assert_eq!(
            sget(&m.fs().doc, &target),
            "this-is-a-new-string",
            "clipboard {clip}"
        );
    }
}

#[test]
fn paste_keys_into_oneof_object_value() {
    // P targets the oneOf row's own container (the current variant's
    // object), not the enclosing rules[4] object.
    let mut m = paste_into_rule_set(r#"{"b": 2}"#, oneof_rule_set_doc(RuleSetValue::Object));
    press(&mut m, &["P"]);
    let target = oneof_rule_set_path();
    assert_eq!(m.notice, "pasted 1 keys into $.dns.rules[4].rule_set");
    assert_eq!(obj_keys(&m.fs().doc, &target), vec!["a", "b"]);
    assert_eq!(
        obj_keys(&m.fs().doc, &[ks("dns"), ks("rules"), PathSeg::Idx(4)]),
        vec!["rule_set"]
    );
}

#[test]
fn paste_items_into_oneof_array_value() {
    let mut m = paste_into_rule_set(r#"["b"]"#, oneof_rule_set_doc(RuleSetValue::Array("a")));
    press(&mut m, &["P"]);
    let target = oneof_rule_set_path();
    assert_eq!(m.notice, "pasted 1 items into $.dns.rules[4].rule_set");
    assert_eq!(arr_strs(&m.fs().doc, &target), vec!["a", "b"]);
    assert_eq!(
        obj_keys(&m.fs().doc, &[ks("dns"), ks("rules"), PathSeg::Idx(4)]),
        vec!["rule_set"]
    );
}

#[test]
fn paste_scalar_onto_oneof_picker_row() {
    // P inside the variant picker still targets the field value; the
    // string-leaf variant makes plain clipboard text valid there too.
    let mut m = paste_into_rule_set("picked", oneof_rule_set_doc(RuleSetValue::Array("a")));
    press(&mut m, &["t"]);
    assert_eq!(m.top().map(|st| st.kind), Some(MenuKind::OneOf));
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.dns.rules[4].rule_set");
    assert_eq!(sget(&m.fs().doc, &oneof_rule_set_path()), "picked");
}

#[test]
fn paste_plain_string_into_untyped_raw_field() {
    let schema = schema_build(|b| {
        let raw = b.id(FormNode {
            key: "x".into(),
            title: "X".into(),
            kind: Kind::Raw,
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            title: "$".into(),
            kind: Kind::Object,
            children: vec![raw],
            ..FormNode::default()
        })
    });
    let mut opts = test_opts();
    opts.paste = Some(Arc::new(|| Ok("plain-text".to_string())));
    let mut m = app_with(schema, dbuild(|db| db.obj(vec![])), opts);
    m.jump_to_row(&path(&["x"]));
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.x");
    assert_eq!(str_at(&m.fs().doc, &["x"]), "plain-text");
}

#[test]
fn paste_value_onto_root_scalar() {
    for clip in ["plain-root", r#""quoted-root""#] {
        let schema = schema_build(|b| b.id(scalar("$", "", "string")));
        let mut opts = test_opts();
        let t = clip.to_string();
        opts.paste = Some(Arc::new(move || Ok(t.clone())));
        let mut m = app_with(schema, dbuild(|db| db.s("old")), opts);
        press(&mut m, &["P"]);
        assert_eq!(m.notice, "pasted value into $", "clipboard {clip}");
        assert_eq!(sget(&m.fs().doc, &[]), clip.trim_matches('"'));
    }
}

#[test]
fn paste_value_onto_root_oneof_picker_row() {
    let schema = schema_build(|b| {
        let s = b.id(scalar("$", "", "string"));
        let arr_items = b.id(scalar("", "", "string"));
        let arr = b.id(FormNode {
            key: "$".into(),
            kind: Kind::Array,
            items: Some(arr_items),
            ..FormNode::default()
        });
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::OneOf,
            variants: vec![s, arr],
            ..FormNode::default()
        })
    });
    let mut opts = test_opts();
    opts.paste = Some(Arc::new(|| Ok("root-pick".to_string())));
    let mut m = app_with(schema, dbuild(|db| db.s("old")), opts);
    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $");
    assert_eq!(sget(&m.fs().doc, &[]), "root-pick");
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
    // undo lands on the first pasted key
    assert_eq!(row_at(&mut m).path, sp(&["name"]));

    press(&mut m, &["P"]);
    assert_eq!(m.notice, "pasted value into $.name");
    assert_eq!(num_at(&m.fs().doc, &["name"]), "123");
}

#[test]
fn clip_keep_tail_marks_cuts_and_keeps_room_for_the_tail() {
    assert_eq!(clip_keep_tail("short", 10, 2), "short");
    // 6 available cells minus the 2-cell tail: 3 body cells + the ellipsis.
    assert_eq!(clip_keep_tail("abcdefgh", 6, 2), "abc…");
    assert_eq!(
        display_width(&clip_keep_tail("abcdefgh", 6, 2)),
        4,
        "the body keeps the budget for the tail"
    );
    assert_eq!(clip_keep_tail("abc", 6, 0), "abc");
    // Wide runes count their cells, not their chars: the cut never splits
    // one, so the body may land a cell under the budget.
    let cut = clip_keep_tail("很长的中文值需要被截断标记", 8, 2);
    assert_eq!(display_width(&cut), 5);
    assert!(cut.ends_with('…'));
}
