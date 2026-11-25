//! YAML anchors, aliases, merge keys and reference completion.

use super::*;
use super::{
    TB, app_with, bget, find_row, fnode, getv, has, ks, mk_app, num_at, path_marked, press,
    required_merge_model, row_at, scalar, schema_build, set_cursor, set_cursor_path, sget, sp,
    tenum, test_doc, test_opts, yaml_doc,
};

use super::*;

#[test]
fn reference_list_search() {
    let mut m = mk_app(test_doc(), test_opts());
    m.width = 80;
    m.height = 12;
    m.overlay = Some(Overlay::RefPick(RefPickState {
        name: "x".into(),
        paths: vec![sp(&["name"]), sp(&["server", "host"])],
        cursor: 0,
        filter: Default::default(),
    }));

    press(&mut m, &["/", "h", "o", "s", "t"]);
    let view = m.view();
    assert!(view.contains("$.server.host"), "{view}");
    assert!(!view.contains("$.name"), "{view}");
    press(&mut m, &["enter"]);
    assert!(m.overlay.is_none());
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
    press(&mut m, &["ctrl+e"]);
    assert!(
        has(&m.fs().doc, &sp(&["jobs", "build2", "env"])),
        "ctrl+e must materialize an own env member"
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
    press(&mut m, &["ctrl+e"]);
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

#[test]
fn merge_ctrl_j_k_cycle_anchor_candidates() {
    // ctrl+j applies the first merge candidate just like tab while
    // candidates exist (the window is open).
    let mut m = completion_model("sobj: &n3\n  k: v\nsobj2: &n4\n  m: w\n");
    let rows = m.menu_rows();
    let i = rows
        .iter()
        .position(|r| r.fn_.is_some() && m.fs().schema.nodes[r.fn_.unwrap() as usize].key == "sobj")
        .expect("sobj row");
    m.top_mut().unwrap().cursor = i;
    press(&mut m, &["enter", "<"]);
    let cands = match &m.overlay {
        Some(Overlay::Mrg(s)) => merge_filtered(s),
        _ => panic!("merge input did not open"),
    };
    assert_eq!(cands, vec!["n4".to_string()]);
    press(&mut m, &["ctrl+j"]);
    assert_eq!(
        mrg_value(&m),
        "n4 ",
        "ctrl+j must complete the mapping anchor"
    );
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
fn alias_expand_shortcut_ctrl_e() {
    let mut m = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    drill_copy(&mut m);
    press(&mut m, &["ctrl+e"]);
    let cp = getv(&m.fs().doc, &sp(&["copy"])).unwrap();
    assert_eq!(m.fs().doc.node(cp).anchor, "");
    assert!(alias_at(&m.fs().doc, &sp(&["copy"])).is_none());

    // ctrl+e on a reference row in the parent menu expands that row
    let mut m2 = alias_model("base: &x\n  p: 1\ncopy: *x\n");
    press(&mut m2, &["down"]);
    press(&mut m2, &["ctrl+e"]);
    let cp = getv(&m2.fs().doc, &sp(&["copy"])).unwrap();
    assert_eq!(m2.fs().doc.node(cp).anchor, "");
    let b = getv(&m2.fs().doc, &sp(&["base"])).unwrap();
    assert_eq!(m2.fs().doc.node(b).anchor, "x");

    // without a reference under the cursor ctrl+e does nothing
    let mut m3 = alias_model("base: &x\n  p: 1\n");
    press(&mut m3, &["ctrl+e"]);
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
fn reference_ctrl_j_k_cycle_anchor_candidates() {
    // ctrl+j/ctrl+k cycle reference candidates like tab/shift+tab; the
    // first press starts the cycle from the empty input.
    let mut m = completion_model("sname: &alpha one\nsname2: &beta two\n");
    open_reference_on(&mut m, "target");
    assert_eq!(anc_cands(&m).join(","), "alpha,beta");
    let val = |m: &App| match &m.overlay {
        Some(Overlay::Anc(s)) => s.ti.value().to_string(),
        _ => panic!("reference input is not open"),
    };
    press(&mut m, &["ctrl+j"]);
    assert_eq!(val(&m), "alpha");
    press(&mut m, &["ctrl+j"]);
    assert_eq!(val(&m), "beta");
    press(&mut m, &["ctrl+k"]);
    assert_eq!(val(&m), "alpha");
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
    // the change ring points at the restored reference; the undo keeps the
    // editor menu open (the user sits inside it), so esc returns to the
    // reference row and the expand affordance lives one drill below it
    assert_eq!(m.fs().change_jump, Some(vec![vec![ks("copy")]]));
    press(&mut m, &["esc"]);
    assert_eq!(row_at(&mut m).path, sp(&["copy"]));
    press(&mut m, &["enter"]);
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

#[test]
fn anchor_completion_keeps_the_typed_name() {
    let mut m =
        completion_model("sname: &alpha one\nsnum: &beta 2\nextra1: x\nextra2: y\nextra3: z\n");
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "extra1"
    });
    press(&mut m, &["*"]);
    match &m.overlay {
        Some(Overlay::Anc(s)) => assert_eq!(s.sugs.len(), 2, "sugs = {:?}", s.sugs),
        _ => panic!("reference input did not open"),
    }
    // "beta" is fully typed: the identical candidate stays offered.
    press(&mut m, &["b", "e", "t", "a"]);
    match &m.overlay {
        Some(Overlay::Anc(s)) => assert_eq!(s.sugs, vec!["beta".to_string()]),
        _ => panic!(),
    }
}

#[test]
fn anchor_candidate_overlay_uses_reverse_video_and_arrow_selection() {
    let mut m =
        completion_model("sname: &alpha one\nsnum: &beta 2\nextra1: x\nextra2: y\nextra3: z\n");
    set_cursor(&mut m, |r| {
        r.kind == RowKind::Unknown && r.label == "extra1"
    });
    press(&mut m, &["*"]);
    let view = m.view();
    assert!(
        view.contains("\x1b[2;48;5;236malpha"),
        "anchor suggestions must appear on open: {view}"
    );
    assert!(
        !view.contains("\x1b[7malpha"),
        "the anchor popup must not preselect a candidate: {view}"
    );
    press(&mut m, &["tab"]);
    let view = m.view();
    assert!(
        view.contains("\x1b[7malpha"),
        "anchor candidate overlay must use reverse video: {view}"
    );
    assert!(
        view.contains("\x1b[2;48;5;236mbeta"),
        "the other anchor candidate must be faint: {view}"
    );
    let line = view
        .lines()
        .find(|l| l.contains("\x1b[7malpha"))
        .expect("selected anchor candidate line");
    let start = line.find("\x1b[7m").expect("candidate background");
    assert_eq!(
        display_width(&line[..start]),
        display_width("  reference: "),
        "anchor candidates align with the reference input text"
    );
    press(&mut m, &["down"]);
    match &m.overlay {
        Some(Overlay::Anc(s)) => assert_eq!(s.ti.value(), "beta"),
        _ => panic!("anchor overlay closed"),
    }
    let view = m.view();
    assert!(
        view.contains("\x1b[7mbeta"),
        "arrow selection must move the accent marker: {view}"
    );
}
