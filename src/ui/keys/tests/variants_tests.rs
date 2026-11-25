//! Type and variant pickers: oneOf/anyOf inference, enum dropdown and
//! enum overlay search, map values.

use super::*;
use super::{
    TB, app_with, arr_len, assert_hint_bottom, body_val, child, dbuild, find_row, fnode, getv, has,
    infer_schema, ks, map_one_of_schema, mk_app, num_at, obj_keys, plain, press, row_at, scalar,
    schema_build, schema_from_json, search_state, set_cursor_path, sget, sp, str_at, tenum,
    test_doc, test_opts,
};

use super::*;

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
    assert!(m.top().unwrap().filter.active);
    assert_eq!(m.top().unwrap().filter.ti.value(), "");
    assert_eq!(m.top().unwrap().cursor, before);
    assert_eq!(m.top().unwrap().filter.restore, before);

    // "u" is a filter character, not undo
    press(&mut m, &["u"]);
    assert_eq!(m.top().unwrap().filter.ti.value(), "u");
    press(&mut m, &["x"]);
    assert_eq!(m.top().unwrap().filter.ti.value(), "ux");
    assert!(m.top().unwrap().filter.active);

    // no match keeps search mode alive so backspace can undo the query
    let rows = m.menu_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, RowKind::NoItems);
    assert!(rows[0].label.contains("no matching"));
    press(&mut m, &["backspace", "backspace"]);
    assert_eq!(m.top().unwrap().filter.ti.value(), "");
    press(&mut m, &["esc"]);
    assert!(!m.top().unwrap().filter.active);
    assert!(m.top().unwrap().filter.ti.value().is_empty());

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

#[test]
fn matching_variants() {
    let (schema, one_of, _) = infer_schema();
    let variants = schema.nodes[one_of as usize].variants.clone();
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
                &crate::ui::rows::match_variant_list(&schema, &variants, &Doc::new(), None),
            ),
            Some((doc, v)) => variant_titles(
                &schema,
                &crate::ui::rows::match_variant_list(&schema, &variants, doc, Some(*v)),
            ),
        };
        assert_eq!(got, want, "{name}");
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

    press(&mut m, &["enter"]);
    assert!(
        matches!(m.overlay, Some(Overlay::Edit(_))),
        "type picker should open"
    );
    if let Some(Overlay::Edit(e)) = &m.overlay {
        assert!(e.sugs.iter().any(|s| s == "trojan"), "union discriminator");
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
        Some(Overlay::Edit(e)) => assert!(e.sugs.iter().any(|s| s == "trojan")),
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
        Some(Overlay::Edit(e)) => {
            let labels: Vec<&str> = e.sugs.iter().map(String::as_str).collect();
            assert_eq!(labels, ["inline", "", "local", "remote"]);
            assert_eq!(e.sug_idx, 0, "unset type highlights the first option");
        }
        _ => panic!("type picker should open"),
    }
    press(&mut m, &["down", "down", "down"]); // remote
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
        Some(Overlay::Edit(e)) => {
            let labels: Vec<&str> = e.sugs.iter().map(String::as_str).collect();
            assert_eq!(labels, ["inline", "", "local", "remote"]);
            assert_eq!(e.sug_idx, 3, "current value remote is highlighted");
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
fn search_jump_to_enum_array_element() {
    // enum-array elements are document values; the jump lands in the toggle
    // menu on the row of that value
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "levels": {"type": "array", "items": {"type": "string", "enum": ["debug", "info", "warn"]}}
          }
        }"#,
    );
    let doc = dbuild(|db| db.obj(vec![("levels", db.arr(vec![db.s("debug"), db.s("info")]))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "i", "n", "f", "o"]);
    let want = vec![ks("levels"), PathSeg::Idx(1)];
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.path == want)
        .expect("levels[1] search row");
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::EnumArray);
    assert_eq!(m.top().unwrap().path, vec![ks("levels")]);
    let row = row_at(&mut m);
    assert_eq!(row.kind, RowKind::Toggle);
    assert_eq!(row.idx, 1, "cursor lands on the info toggle");
}

#[test]
fn search_jump_to_schemaless_array_element() {
    // arrays without `items` (and nested arrays degraded to Raw) have no
    // per-element rows: the jump lands on the nearest editable row — the
    // array field itself — instead of leaving a stale cursor position
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "free": {"type": "array"},
            "matrix": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}}
          }
        }"#,
    );
    let doc = dbuild(|db| {
        db.obj(vec![
            ("free", db.arr(vec![db.s("alpha"), db.s("beta")])),
            ("matrix", db.arr(vec![db.arr(vec![db.n("1"), db.n("2")])])),
        ])
    });
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["/", "a", "l", "p", "h", "a"]);
    let want = vec![ks("free"), PathSeg::Idx(0)];
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.path == want)
        .expect("free[0] search row");
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(m.top().unwrap().kind, MenuKind::Object);
    assert_eq!(
        row_at(&mut m).path,
        vec![ks("free")],
        "cursor on the array field"
    );

    // nested array elements are indexed too; their jump also lands on the
    // field that owns the raw subtree
    press(&mut m, &["/", "2"]);
    let want = vec![ks("matrix"), PathSeg::Idx(0), PathSeg::Idx(1)];
    let idx = search_state(&m)
        .results
        .iter()
        .position(|r| r.path == want)
        .expect("matrix[0][1] search row");
    if let Some(Overlay::Search(s)) = &mut m.overlay {
        s.cursor = idx;
    }
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
    assert_eq!(
        row_at(&mut m).path,
        vec![ks("matrix")],
        "cursor on the nested array field"
    );
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
fn enum_variant_add_returns_to_parent_menu() {
    let schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "rules": {"type": "array", "items": {"oneOf": [
              {"type": "string", "title": "Name", "enum": ["a", "b"]},
              {"type": "object", "title": "Body",
               "properties": {"x": {"type": "string"}}}
            ]}}
          }
        }"#,
    );
    let mut m = app_with(
        schema,
        dbuild(|db| db.obj(vec![("rules", db.arr(vec![]))])),
        test_opts(),
    );
    press(&mut m, &["enter"]); // rules items menu
    assert_eq!(m.fs().stack.len(), 2);
    press(&mut m, &["enter"]); // < Add item > opens the variant picker
    assert_eq!(m.top().unwrap().kind, MenuKind::OneOf);
    assert_eq!(m.fs().stack.len(), 3);
    let rows = m.menu_rows();
    let pos = rows
        .iter()
        .position(|r| {
            r.variant.map_or(false, |v| {
                m.fs().schema.nodes[v as usize].kind == Kind::Enum
            })
        })
        .expect("enum variant row");
    m.top_mut().unwrap().cursor = pos;
    press(&mut m, &["enter"]); // choose the enum variant
    assert!(matches!(m.overlay, Some(Overlay::Edit(_))));
    press(&mut m, &["enter"]); // choose its first option
    assert!(m.overlay.is_none());
    assert_eq!(
        m.top().unwrap().kind,
        MenuKind::Items,
        "enum selection must return to the array menu"
    );
    assert_eq!(m.fs().stack.len(), 2);
    assert_eq!(sget(&m.fs().doc, &[ks("rules"), PathSeg::Idx(0)]), "a");

    // a boolean variant has no editor either: it must also leave the picker
    let bool_schema = schema_from_json(
        r#"{
          "type": "object",
          "properties": {
            "flags": {"type": "array", "items": {"oneOf": [
              {"type": "boolean", "title": "Flag"},
              {"type": "string", "title": "Name"}
            ]}}
          }
        }"#,
    );
    let mut bm = app_with(
        bool_schema,
        dbuild(|db| db.obj(vec![("flags", db.arr(vec![]))])),
        test_opts(),
    );
    press(&mut bm, &["enter", "enter"]); // items -> < Add item >
    assert_eq!(bm.top().unwrap().kind, MenuKind::OneOf);
    let pos = bm
        .menu_rows()
        .iter()
        .position(|r| {
            r.variant.map_or(false, |v| {
                bm.fs().schema.nodes[v as usize].kind == Kind::Boolean
            })
        })
        .expect("boolean variant row");
    bm.top_mut().unwrap().cursor = pos;
    press(&mut bm, &["enter"]);
    assert!(bm.overlay.is_none());
    assert_eq!(bm.top().unwrap().kind, MenuKind::Items);
    assert_eq!(bm.fs().stack.len(), 2);
    assert_eq!(
        bm.fs()
            .doc
            .value(bm.fs().doc.get(&[ks("flags"), PathSeg::Idx(0)]).unwrap()),
        &Value::Bool(true)
    );

    // same for a typed map's < Add key >
    let map_schema = schema_from_json(
        r#"{
          "type": "object",
          "additionalProperties": {"oneOf": [
            {"type": "string", "title": "Name", "enum": ["a", "b"]},
            {"type": "integer", "title": "Count"}
          ]}
        }"#,
    );
    let mut mm = app_with(map_schema, dbuild(|db| db.obj(vec![])), test_opts());
    press(&mut mm, &["a", "k", "enter"]);
    assert_eq!(mm.top().unwrap().kind, MenuKind::OneOf);
    let rows = mm.menu_rows();
    let pos = rows
        .iter()
        .position(|r| {
            r.variant.map_or(false, |v| {
                mm.fs().schema.nodes[v as usize].kind == Kind::Enum
            })
        })
        .expect("enum variant row");
    mm.top_mut().unwrap().cursor = pos;
    press(&mut mm, &["enter", "enter"]);
    assert!(mm.overlay.is_none());
    assert_eq!(mm.fs().stack.len(), 1, "back to the map menu");
    assert_eq!(str_at(&mm.fs().doc, &["k"]), "a");
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
    press(&mut m, &["down", "enter"]); // open the enum dropdown on Mode
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.sugs, vec!["http", "grpc", "tcp"], "all options listed");
            assert_eq!(e.sug_idx, 0, "the default value http is highlighted");
            assert_eq!(e.ti.value(), "http", "the input is prefilled");
        }
        _ => panic!("enum dropdown should open"),
    }
    press(&mut m, &["down", "enter"]); // pick grpc
    assert!(m.overlay.is_none(), "dropdown must close on enter");
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
}

#[test]
fn menu_enum_overlay_search_filters_and_selects() {
    let schema = schema_from_json(
        r#"{"type":"object","properties":{
          "mode":{"type":"string","enum":["http","grpc","tcp"]}}}"#,
    );
    let doc = dbuild(|db| db.obj(vec![("mode", db.s("tcp"))]));
    let mut m = app_with(schema, doc, test_opts());
    press(&mut m, &["enter"]); // open the enum dropdown on Mode
    press(&mut m, &["G", "R"]); // keyword search is case-insensitive
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(
                e.ti.value(),
                "GR",
                "typing over the highlighted option starts a fresh search"
            );
            assert_eq!(e.sugs, vec!["grpc"], "only grpc matches");
            assert_eq!(e.sug_idx, 0, "the top match stays highlighted");
        }
        _ => panic!("enum dropdown should stay open while searching"),
    }
    let view = plain(&m.view());
    assert!(view.contains("grpc"), "matching option is listed:\n{view}");
    assert!(
        !view.contains("http"),
        "non-matching options are hidden:\n{view}"
    );
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none(), "enter applies the named option");
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
}

#[test]
fn menu_enum_overlay_search_no_match_stays() {
    let doc = test_doc();
    let mut m = mk_app(doc.clone(), test_opts());
    press(&mut m, &["down", "enter", "z", "z"]);
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.ti.value(), "zz");
            assert!(e.sugs.is_empty(), "no option matches zz");
        }
        _ => panic!("enum dropdown should stay open"),
    }
    press(&mut m, &["enter"]); // no match: must not write anything
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert!(e.err.contains("must be one of"), "err = {}", e.err);
            assert!(e.err.contains("grpc"), "err names the options: {}", e.err);
        }
        _ => panic!("invalid enum input keeps the editor open"),
    }
    assert!(!has(&m.fs().doc, &sp(&["mode"])), "no option was written");

    press(&mut m, &["backspace", "backspace"]); // query cleared: full list back
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.ti.value(), "");
            assert_eq!(e.sugs, vec!["http", "grpc", "tcp"]);
            assert_eq!(e.sug_idx, 0, "the first option is highlighted");
        }
        _ => panic!("backspace restores the option list"),
    }
    press(&mut m, &["down", "enter"]); // grpc from the restored list
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "grpc");
    assert!(m.overlay.is_none());
}

#[test]
fn menu_enum_overlay_takes_q_literally() {
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
    press(&mut m, &["enter"]); // open the enum dropdown on the only field
    press(&mut m, &["q"]); // q searches; there is no separate search mode
    match &m.overlay {
        Some(Overlay::Edit(e)) => {
            assert_eq!(e.ti.value(), "q");
            assert_eq!(e.sugs, vec!["quiet"]);
        }
        _ => panic!("enum dropdown should stay open"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["mode"]), "quiet");
}

#[test]
fn enum_dropdown_sits_below_the_input() {
    let doc = test_doc();
    let mut m = mk_app(doc, test_opts());
    m.width = 80;
    m.height = 12;
    press(&mut m, &["down", "enter", "g", "r"]);
    let view = plain(&m.view());
    let lines: Vec<&str> = view.lines().collect();
    // the edited Mode row carries the keyword being typed; the matching
    // option is listed right below it
    let input = lines
        .iter()
        .position(|l| l.contains("gr"))
        .expect("input row");
    let option = lines
        .iter()
        .position(|l| l.contains("grpc"))
        .expect("matched option");
    assert!(input < option, "dropdown must sit below the input:\n{view}");
    assert_hint_bottom(&mut m, "ctrl+s/w save");
}

#[test]
fn enum_dropdown_anchors_at_the_input_start() {
    // The enum input is a search keyword, not the options' prefix: the
    // window always anchors at the input text start with full option
    // rows, even when a long prefilled value or keyword would trigger
    // the `…tail` caret anchoring used by prefix completions.
    let schema = schema_build(|b| {
        let mut mode = fnode("mode", "Mode", Kind::Enum);
        mode.enum_ = tenum(b, &["production-alpha", "production-beta", "staging"]);
        let mode = b.id(mode);
        b.id(FormNode {
            key: "$".into(),
            kind: Kind::Object,
            children: vec![mode],
            ..FormNode::default()
        })
    });
    let doc = dbuild(|db| db.obj(vec![("mode", db.s("production-alpha"))]));
    let mut m = app_with(schema, doc, test_opts());
    m.width = 80;
    m.height = 12;
    press(&mut m, &["enter"]); // enum edit opens prefilled + highlighted
    let rows = m.menu_rows();
    let tw = m.menu_title_col_width(&rows);
    let input_left = 2 + display_width(&m.title_col("Mode", tw));
    let anchor = |m: &mut App| {
        let view = m.view();
        let line = view
            .lines()
            .find(|l| l.contains("\x1b[7") && plain(l).trim().starts_with("production"))
            .expect("highlighted option row")
            .to_string();
        let start = line.find("\x1b[7").expect("reverse video");
        (display_width(&line[..start]), view)
    };

    // on open the query is the prefilled current value (16 chars)
    let (left, view) = anchor(&mut m);
    assert_eq!(
        left, input_left,
        "dropdown must anchor at the input text start on open:\n{view}"
    );

    // retyping a long keyword keeps the anchor there (select-all-on-type
    // restarts the keyword search) and rows keep the full option text
    press(&mut m, &["p", "r", "o", "d", "u", "c", "t", "i", "o", "n"]);
    let (left, view) = anchor(&mut m);
    assert_eq!(
        left, input_left,
        "long keyword must not re-anchor the dropdown at the caret:\n{view}"
    );
    assert!(
        plain(&view).contains("production-beta"),
        "options must keep their full text, not `…tail` rows:\n{view}"
    );
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
