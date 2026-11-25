//! Menu row construction (root order, titles, item menus, oneOf).

use super::*;
use super::{Fx, child_of, dbuild, kinds, must_schema, root_state, rows_for, st};

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
