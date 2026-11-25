//! Search index, query matching, ranking, regex filter and replace-all.

use super::*;
use super::{
    Fx, TB, child_of, dbuild, fnode, must_schema, paths, root_state, scalar, st, tenum, test_schema,
};

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

#[test]
fn dot_query_supports_bracket_index_parts() {
    let mut fx = Fx::new();
    let rst = root_state(&fx.schema);
    let all = search_index(&mut fx.schema, &fx.doc, &rst);
    let ctx = fx.ctx();
    let join = |rows: &[Row]| {
        let mut v: Vec<String> = paths(rows);
        v.sort();
        v.join(",")
    };
    let subtree = "$.mounts[0],$.mounts[0].dst,$.mounts[0].src";

    // a bracket part spells the same steps as the dot part it replaces
    assert_eq!(
        join(&filter_search(&ctx, &all, ".mounts[0]", true, None)),
        subtree
    );
    assert_eq!(
        join(&filter_search(&ctx, &all, ".mounts.0", true, None)),
        subtree
    );
    // an index still being typed matches too
    assert_eq!(
        join(&filter_search(&ctx, &all, ".mounts[0", true, None)),
        subtree
    );

    // queries with an index bracket but no leading dot are dot queries too
    assert_eq!(join(&filter_search(&ctx, &all, "[0]", true, None)), subtree);
    assert_eq!(
        join(&filter_search(&ctx, &all, "mounts[0]", true, None)),
        subtree
    );
    assert_eq!(
        join(&filter_search(&ctx, &all, "mounts[0].src", true, None)),
        "$.mounts[0].src"
    );

    // regex mode keeps the tiered literal matcher on the path
    let re = Regex::new(&path_pattern(".mounts[0]")).unwrap();
    assert_eq!(
        join(&filter_search(&ctx, &all, ".mounts[0]", true, Some(&re))),
        subtree
    );

    // non-index brackets stay ordinary text queries
    assert!(!is_dot_query("a[x]"));
    assert!(!is_dot_query("[x]"));
    assert!(!is_dot_query("["));
    assert!(is_dot_query("[0]"));
    assert!(is_dot_query("mounts[0].src"));
    assert!(is_dot_query("mounts[1"));
    assert!(is_dot_query(".0"));
    assert!(!is_dot_query("plain"));
}
