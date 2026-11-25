//! Navigation-stack rebuild.

use super::*;
use super::{Fx, dbuild};

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
