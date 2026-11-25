//! Fixture tests for the jsondoc arena. They live in a sibling module, not
//! inside jsondoc.rs, because the fixtures parse through
//! `jsoncdoc::parse_strict` and jsondoc itself must not use jsoncdoc — the
//! scanner dependency has to stay one-directional (jsoncdoc → jsondoc).

use crate::doc::jsoncdoc::parse_strict;
use crate::doc::jsondoc::Doc;

fn doc(s: &str) -> Doc {
    parse_strict(s).expect("test document must parse")
}

#[cfg(test)]
mod tests {
    use super::{doc, parse_strict};
    use crate::doc::jsondoc::PathSeg::{Idx, Key};
    use crate::doc::jsondoc::*;
    use serde_json::json;

    const ROUNDTRIP_SRC: &str = r#"{
  "name": "alpha",
  "port": 8080,
  "ratio": 1.0,
  "big": 1e3,
  "enabled": true,
  "nothing": null,
  "nested": {
    "a/b": "slash key",
    "m~n": "tilde key",
    "名字": "unicode"
  },
  "list": [
    "x",
    42,
    {
      "inner": true
    },
    []
  ],
  "empty_obj": {},
  "empty_arr": []
}"#;

    fn set_path(d: &mut Doc, path: Vec<PathSeg>, v: NodeId) {
        d.set(&path, v).expect("Set must succeed");
    }

    #[test]
    fn parse_write_roundtrip() {
        let n = doc(ROUNDTRIP_SRC);
        let out = n.write();
        assert_eq!(out, ROUNDTRIP_SRC.to_string() + "\n");
        // idempotent
        let n2 = doc(&out);
        assert_eq!(n2.write(), out);
        // numeric literals preserved exactly
        let got = n.get(&[Key("ratio".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("1.0".into()));
        let got = n.get(&[Key("big".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("1e3".into()));
        // key order preserved
        let Value::Object(members) = n.value(n.root) else {
            panic!()
        };
        let keys: Vec<&str> = members.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(
            keys.join(","),
            "name,port,ratio,big,enabled,nothing,nested,list,empty_obj,empty_arr"
        );
    }

    #[test]
    fn compact() {
        let n = doc(ROUNDTRIP_SRC);
        assert_eq!(
            n.compact(n.root),
            r#"{"name":"alpha","port":8080,"ratio":1.0,"big":1e3,"enabled":true,"nothing":null,"nested":{"a/b":"slash key","m~n":"tilde key","名字":"unicode"},"list":["x",42,{"inner":true},[]],"empty_obj":{},"empty_arr":[]}"#
        );
        let sub = n.get(&[Key("nested".into())]).unwrap();
        assert_eq!(
            n.compact(sub),
            r#"{"a/b":"slash key","m~n":"tilde key","名字":"unicode"}"#
        );
    }

    #[test]
    fn get_set_remove_append() {
        let mut n = doc(r#"{"a": {"b": 1}}"#);
        assert!(
            n.get(&[Key("a".into()), Key("b".into()), Key("c".into())])
                .is_none()
        );
        assert!(n.get(&[Key("a".into()), Key("z".into())]).is_none());

        // Set auto-creates intermediates: key → object, index → array.
        let deep = n.new_str("deep");
        set_path(
            &mut n,
            vec![Key("x".into()), Key("y".into()), Idx(0), Key("z".into())],
            deep,
        );
        let got = n
            .get(&[Key("x".into()), Key("y".into()), Idx(0), Key("z".into())])
            .unwrap();
        assert_eq!(n.value(got), &Value::Str("deep".into()));

        // Existing key keeps position on replace.
        let seven = n.new_num("7");
        set_path(&mut n, vec![Key("a".into()), Key("b".into())], seven);
        let Value::Object(members) = n.value(n.root) else {
            panic!()
        };
        assert_eq!(members[0].key, "a");
        let got = n.get(&[Key("a".into()), Key("b".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("7".into()));

        // Append creates and grows arrays.
        let one = n.new_str("one");
        let two = n.new_str("two");
        n.append(&[Key("list".into())], one).unwrap();
        n.append(&[Key("list".into())], two).unwrap();
        let arr = n.get(&[Key("list".into())]).unwrap();
        assert_eq!(
            match n.value(arr) {
                Value::Array(a) => a.len(),
                _ => panic!(),
            },
            2
        );

        // Remove middle array element and object key.
        assert!(n.remove(&[Key("list".into()), Idx(0)]));
        let arr = n.get(&[Key("list".into())]).unwrap();
        match n.value(arr) {
            Value::Array(a) => {
                assert_eq!(a.len(), 1);
                assert_eq!(n.value(a[0]), &Value::Str("two".into()));
            }
            _ => panic!(),
        }
        assert!(n.remove(&[Key("a".into()), Key("b".into())]));
        assert!(n.get(&[Key("a".into()), Key("b".into())]).is_none());
        assert!(!n.remove(&[Key("a".into()), Key("b".into())]));
    }

    fn summary_arr(d: &Doc, id: NodeId) -> String {
        let Value::Array(items) = d.value(id) else {
            panic!()
        };
        items
            .iter()
            .map(|&v| match d.value(v) {
                Value::Str(s) => s.clone(),
                Value::Num(l) => l.clone(),
                _ => "?".to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn member_key_list(d: &Doc, id: NodeId) -> String {
        let Value::Object(members) = d.value(id) else {
            panic!()
        };
        members
            .iter()
            .map(|m| m.key.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn insert_and_insert_member() {
        let mut n = doc(r#"{"list": ["a", "b", "c"]}"#);
        // splice mid-array shifts the tail
        let x = n.new_str("x");
        n.insert(&[Key("list".into())], 1, x).unwrap();
        let arr = n.get(&[Key("list".into())]).unwrap();
        assert_eq!(summary_arr(&n, arr), "a x b c");
        // idx == len appends; out-of-range clamps
        let end = n.new_str("end");
        let clamped = n.new_str("clamped");
        n.insert(&[Key("list".into())], 4, end).unwrap();
        n.insert(&[Key("list".into())], 99, clamped).unwrap();
        let arr = n.get(&[Key("list".into())]).unwrap();
        assert_eq!(summary_arr(&n, arr), "a x b c end clamped");
        // auto-creates the array, like Append
        let one = n.new_num("1");
        n.insert(&[Key("fresh".into())], 0, one).unwrap();
        let arr = n.get(&[Key("fresh".into())]).unwrap();
        match n.value(arr) {
            Value::Array(a) => {
                assert_eq!(a.len(), 1);
                assert!(matches!(n.value(a[0]), Value::Num(_)));
            }
            _ => panic!(),
        }

        // member splice at idx; existing key keeps position, value replaced
        let root = n.new_str("root");
        n.insert_member(&[], "top", 0, root).unwrap();
        let got = n.get(&[Key("top".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Str("root".into()));
        let zz = n.new_num("1");
        let mm = n.new_num("2");
        n.insert_member(&[], "zz", 99, zz).unwrap();
        n.insert_member(&[], "mm", 0, mm).unwrap();
        // doc so far: top, list, fresh — zz appends, mm splices in at 0
        assert_eq!(member_key_list(&n, n.root), "mm top list fresh zz");
        let nine = n.new_num("9");
        n.insert_member(&[], "top", 0, nine).unwrap();
        let got = n.get(&[Key("top".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("9".into()));
        assert_eq!(member_key_list(&n, n.root), "mm top list fresh zz");
        let k = n.new_num("1");
        assert!(n.insert_member(&[Key("list".into())], "k", 0, k).is_err());

        // a target the document never created builds its parent chain, like Set
        let mut fresh = Doc::new();
        let bash = fresh.new_str("bash");
        fresh
            .insert_member(
                &[Key("defaults".into()), Key("run".into())],
                "shell",
                0,
                bash,
            )
            .unwrap();
        let got = fresh
            .get(&[
                Key("defaults".into()),
                Key("run".into()),
                Key("shell".into()),
            ])
            .unwrap();
        assert_eq!(fresh.value(got), &Value::Str("bash".into()));
        // null ancestors and missing array elements resolve to containers too
        let one = fresh.new_num("1");
        fresh
            .insert_member(&[Key("items".into()), Idx(0)], "k", 0, one)
            .unwrap();
        let got = fresh
            .get(&[Key("items".into()), Idx(0), Key("k".into())])
            .unwrap();
        assert_eq!(fresh.value(got), &Value::Num("1".into()));
        // a real scalar in the chain is still a structural conflict
        let mut conflict = Doc::new();
        let one = conflict.new_num("1");
        let a = conflict.new_object();
        if let Value::Object(m) = &mut conflict.nodes[a as usize].value {
            m.push(Member {
                key: "a".into(),
                val: one,
            });
        }
        conflict.set(&[], a).unwrap();
        let k = conflict.new_num("1");
        assert!(
            conflict
                .insert_member(&[Key("a".into()), Key("b".into())], "k", 0, k)
                .is_err()
        );
    }

    #[test]
    fn move_test() {
        let mut n = doc(r#"{"list": ["a", "b", "c", "d"], "obj": {}}"#);
        let list = |n: &Doc| {
            let arr = n.get(&[Key("list".into())]).unwrap();
            summary_arr(n, arr)
        };
        assert!(n.move_(&[Key("list".into())], 1, 2));
        assert_eq!(list(&n), "a c b d");
        assert!(n.move_(&[Key("list".into())], 3, 0));
        assert_eq!(list(&n), "d a c b");
        assert!(n.move_(&[Key("list".into())], 0, 3));
        assert_eq!(list(&n), "a c b d");
        assert!(n.move_(&[Key("list".into())], 2, 2));
        assert_eq!(list(&n), "a c b d");
        // out of range: fail without changing the list (Go also tests -1,
        // which the usize API cannot express)
        assert!(!n.move_(&[Key("list".into())], 4, 0));
        assert!(!n.move_(&[Key("list".into())], 0, 4));
        assert_eq!(list(&n), "a c b d");
        // empty object and missing path: false, no panic
        assert!(!n.move_(&[Key("obj".into())], 0, 0));
        assert!(!n.move_(&[Key("nope".into())], 0, 0));

        // nested array paths
        let mut n2 = doc(r#"{"outer": {"list": ["x", "y", "z"]}}"#);
        assert!(n2.move_(&[Key("outer".into()), Key("list".into())], 0, 2));
        let inner = n2.get(&[Key("outer".into()), Key("list".into())]).unwrap();
        assert_eq!(summary_arr(&n2, inner), "y z x");
    }

    #[test]
    fn set_panics() {
        let mut n = doc(r#"{"s": "text"}"#);
        let one = n.new_num("1");
        assert!(n.set(&[Key("s".into()), Key("k".into())], one).is_err());
    }

    #[test]
    fn set_out_of_range_panics() {
        let mut n = doc(r#"{"a": [1]}"#);
        let x = n.new_str("x");
        assert!(n.set(&[Key("a".into()), Idx(5)], x).is_err());
    }

    #[test]
    fn set_replaces_null_with_container() {
        // JSON null carries no value: the first write through it replaces it
        // with the container the path demands.
        let mut n = doc(r#"{"map": {"Host": null}, "obj": null}"#);
        let x = n.new_str("x");
        n.set(&[Key("map".into()), Key("Host".into()), Idx(0)], x)
            .unwrap();
        let got = n
            .get(&[Key("map".into()), Key("Host".into()), Idx(0)])
            .unwrap();
        assert_eq!(n.value(got), &Value::Str("x".into()));
        let arr = n.get(&[Key("map".into()), Key("Host".into())]).unwrap();
        match n.value(arr) {
            Value::Array(a) => assert_eq!(a.len(), 1),
            _ => panic!("Host should be a 1-element array"),
        }

        let seven = n.new_num("7");
        n.set(&[Key("obj".into()), Key("sub".into())], seven)
            .unwrap();
        let obj = n.get(&[Key("obj".into())]).unwrap();
        assert!(matches!(n.value(obj), Value::Object(_)));
    }

    #[test]
    fn to_any() {
        let n = doc(r#"{"i": 3, "f": 1.5, "s": "x", "b": false, "n": null, "a": [1, "y"]}"#);
        let v = n.to_value(n.root);
        assert_eq!(v["i"], json!(3));
        assert_eq!(v["f"], json!(1.5));
        assert_eq!(v["s"], json!("x"));
        assert_eq!(v["a"], json!([1, "y"]));
    }

    #[test]
    fn clone_test() {
        let n = doc(r#"{"b":1.0,"a":[true,null,"x"]}"#);
        let want = n.write();
        let mut c = n.clone();
        assert_eq!(c.write(), want);
        // Mutating the clone must not touch the original.
        let y = c.new_str("y");
        c.set(&[Key("b".into())], y).unwrap();
        assert_eq!(n.write(), want);
        let got = n.get(&[Key("b".into())]).unwrap();
        assert_eq!(n.value(got), &Value::Num("1.0".into()));
    }

    #[test]
    fn summary() {
        let cases = [
            (r#"{"a": "hello"}"#, "hello"),
            (r#"{"a": 8080}"#, "8080"),
            (r#"{"a": true}"#, "true"),
            (r#"{"a": null}"#, "null"),
            (r#"{"a": {"title": "T", "x": 1}}"#, "T"),
            (r#"{"a": {"x": 1}}"#, "{…}"),
            (r#"{"a": [1, 2, 3]}"#, "[3 items]"),
        ];
        for (src, want) in cases {
            let n = doc(src);
            let got = n.get(&[Key("a".into())]).unwrap();
            assert_eq!(n.summary(got), want, "Summary({src})");
        }
    }

    #[test]
    fn path_string_and_parse() {
        let cases: Vec<(Vec<PathSeg>, &str)> = vec![
            (vec![], "$"),
            (vec![PathSeg::key("server")], "$.server"),
            (
                vec![PathSeg::key("server"), PathSeg::key("port")],
                "$.server.port",
            ),
            (
                vec![PathSeg::key("mounts"), Idx(0), PathSeg::key("src")],
                "$.mounts[0].src",
            ),
            (vec![Idx(0), PathSeg::key("x")], "$[0].x"),
            (vec![PathSeg::key("a/b"), PathSeg::key("m~n")], "$.a/b.m~n"),
            (vec![PathSeg::key("a[b]")], "$.a[b]"),
        ];
        for (path, s) in cases {
            assert_eq!(path_string(&path), s, "PathString({s:?})");
            let got = parse_path(s).unwrap_or_else(|| panic!("ParsePath({s}) failed"));
            assert_eq!(path_string(&got), s, "ParsePath({s}) roundtrip");
        }
        for s in ["", "server", "$.", "$..a", "$[0", "$[x]", "$[0]x", "$.a."] {
            assert!(parse_path(s).is_none(), "ParsePath({s:?}) should fail");
        }
    }

    #[test]
    fn parse_errors() {
        for src in [r#"{"a":}"#, "[1,", "{} {}"] {
            assert!(parse_strict(src).is_err(), "Parse({src:?}) should fail");
        }
    }

    /// sharedDoc builds {"base":&x {"p":1},"copy":<same node>}: the shape
    /// yamldoc.Parse produces for "base: &x {p: 1}\ncopy: *x".
    fn shared_doc() -> Doc {
        let mut n = doc(r#"{"base":{"p":1}}"#);
        let base = n.get(&[Key("base".into())]).unwrap();
        n.node_mut(base).anchor = "x".to_string();
        n.set(&[Key("copy".into())], base).unwrap();
        n
    }

    #[test]
    fn clone_preserves_alias_sharing() {
        let n = shared_doc();
        let want = n.write();
        let mut c = n.clone();
        assert_eq!(c.write(), want);
        let cb = c.get(&[Key("base".into())]).unwrap();
        let cc = c.get(&[Key("copy".into())]).unwrap();
        assert_eq!(cb, cc, "Clone broke alias sharing");
        // Writing through the occurrence updates the anchor and vice versa;
        // the source tree stays untouched.
        let two = c.new_num("2");
        c.set(&[Key("copy".into()), Key("p".into())], two).unwrap();
        let got = c.get(&[Key("base".into()), Key("p".into())]).unwrap();
        assert_eq!(
            c.value(got),
            &Value::Num("2".into()),
            "shared write did not reach the anchor"
        );
        assert_eq!(n.write(), want, "original mutated");
    }

    #[test]
    fn set_writes_through_anchored_node() {
        let mut n = shared_doc();
        let base = n.get(&[Key("base".into())]).unwrap();
        let copy_n = n.get(&[Key("copy".into())]).unwrap();
        assert_eq!(base, copy_n, "fixture is not shared");
        let replaced = n.new_str("replaced");
        n.set(&[Key("copy".into())], replaced).unwrap();
        assert_eq!(n.value(base), &Value::Str("replaced".into()));
        assert_eq!(n.node(base).anchor, "x", "anchor lost");
    }

    #[test]
    fn detach_breaks_alias_sharing() {
        let mut n = shared_doc();
        let cp = n.clone_detached(n.get(&[Key("copy".into())]).unwrap());
        assert!(n.detach(&[Key("copy".into())], cp));
        let base = n.get(&[Key("base".into())]).unwrap();
        assert_ne!(base, cp, "Detach did not break the reference");
        assert!(
            n.node(cp).anchor.is_empty(),
            "CloneDetached kept the anchor"
        );
        let nine = n.new_num("9");
        n.set(&[Key("base".into()), Key("p".into())], nine).unwrap();
        let got = n.get(&[Key("copy".into()), Key("p".into())]).unwrap();
        assert_eq!(
            n.value(got),
            &Value::Num("1".into()),
            "detached copy followed the anchor"
        );
        assert!(!n.detach(&[Key("missing".into())], cp));
    }

    /// mergeDoc builds {"base":&b1 {env,out}, "b2":&b2 {env2, extra}, "child":
    /// {<<: [*b1,*b2], own}} with the merge spec pointing at the anchor nodes.
    fn merge_doc() -> Doc {
        let mut d = Doc::new();
        let base = d.new_object();
        d.node_mut(base).anchor = "b1".into();
        let e1 = d.new_str("e1");
        let o2 = d.new_str("o2");
        if let Value::Object(m) = &mut d.nodes[base as usize].value {
            m.push(Member {
                key: "env".into(),
                val: e1,
            });
            m.push(Member {
                key: "out".into(),
                val: o2,
            });
        }
        let b2 = d.new_object();
        d.node_mut(b2).anchor = "b2".into();
        let e2 = d.new_str("e2");
        let x = d.new_str("x");
        if let Value::Object(m) = &mut d.nodes[b2 as usize].value {
            m.push(Member {
                key: "env".into(),
                val: e2,
            });
            m.push(Member {
                key: "extra".into(),
                val: x,
            });
        }
        let merge_val = d.push(NodeData {
            value: Value::Null,
            anchor: String::new(),
            merge: vec![base, b2],
            raw: String::new(),
        });
        let own = d.new_str("yes");
        let child = d.new_object();
        if let Value::Object(m) = &mut d.nodes[child as usize].value {
            m.push(Member {
                key: "<<".into(),
                val: merge_val,
            });
            m.push(Member {
                key: "own".into(),
                val: own,
            });
        }
        let root = d.new_object();
        for (k, v) in [("base", base), ("b2", b2), ("child", child)] {
            if let Value::Object(m) = &mut d.nodes[root as usize].value {
                m.push(Member {
                    key: k.into(),
                    val: v,
                });
            }
        }
        d.set(&[], root).unwrap();
        d
    }

    #[test]
    fn merge_refs_and_resolution() {
        let n = merge_doc();
        let child = n.get(&[Key("child".into())]).unwrap();
        let refs = n.merge_refs(child);
        assert_eq!(refs.len(), 2);
        assert_eq!(n.node(refs[0]).anchor, "b1");
        assert_eq!(n.node(refs[1]).anchor, "b2");
        // ToAny applies YAML precedence: own wins, earlier anchor wins
        let got = n.to_value(n.root);
        let child_map = &got["child"];
        assert_eq!(child_map["own"], json!("yes"));
        assert_eq!(child_map["env"], json!("e1"));
        assert_eq!(child_map["out"], json!("o2"));
        assert_eq!(child_map["extra"], json!("x"));
        assert_eq!(child_map.as_object().unwrap().len(), 4);
    }

    #[test]
    fn merge_json_output_resolves() {
        let n = merge_doc();
        // pretty JSON and Compact both resolve merges inline
        assert_eq!(
            n.compact(n.root),
            r#"{"base":{"env":"e1","out":"o2"},"b2":{"env":"e2","extra":"x"},"child":{"own":"yes","env":"e1","out":"o2","extra":"x"}}"#
        );
    }

    #[test]
    fn merge_clone_keeps_spec_and_sharing() {
        let n = merge_doc();
        let mut cp = n.clone();
        let child = cp.get(&[Key("child".into())]).unwrap();
        let base = cp.get(&[Key("base".into())]).unwrap();
        let refs = cp.merge_refs(child);
        assert_eq!(refs.len(), 2);
        assert_eq!(
            refs[0], base,
            "cloned merge must point at the cloned anchor"
        );
        // a write through the cloned anchor is what the merged view resolves
        let changed = cp.new_str("changed");
        cp.set(&[Key("base".into()), Key("env".into())], changed)
            .unwrap();
        let got = cp.to_value(cp.root)["child"]["env"].clone();
        assert_eq!(
            got,
            json!("changed"),
            "merged view did not follow the anchor"
        );
    }

    #[test]
    fn merge_clone_detached_flattens() {
        let mut n = merge_doc();
        let child = n.get(&[Key("child".into())]).unwrap();
        let detached = n.clone_detached(child);
        assert!(
            n.merge_refs(detached).is_empty(),
            "detached copy must carry no merge refs"
        );
        let keys: Vec<&str> = match n.value(detached) {
            Value::Object(m) => m.iter().map(|m| m.key.as_str()).collect(),
            _ => panic!(),
        };
        assert_eq!(keys.join(","), "own,env,out,extra");
        // edits to the detached copy never reach the anchor: replace env in
        // the detached copy (Set's plain replace — the old value is not
        // anchored)
        let mine = n.new_str("mine");
        n.overwrite_member(detached, "env", mine);
        let base = n.get(&[Key("base".into())]).unwrap();
        let base_env = match n.value(base) {
            Value::Object(m) => m.iter().find(|m| m.key == "env").unwrap().val,
            _ => panic!(),
        };
        assert_eq!(
            n.value(base_env),
            &Value::Str("e1".into()),
            "anchor changed through the detached copy"
        );
    }

    #[test]
    fn float_text_matches_ecmascript() {
        // `String(x)` in JS: the spelling JS-based LSP/format tooling emits.
        for (f, want) in [
            (1.5, "1.5"),
            (80.0, "80"),
            (-0.5, "-0.5"),
            (0.1, "0.1"),
            (100000.0, "100000"),
            (1_000_000.0, "1000000"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
        ] {
            assert_eq!(number_text(f), want, "{f}");
        }
    }
}

#[cfg(test)]
mod perf_guard {
    /// Regression: line_at used to rescan the whole prefix per call, making
    /// a ~480 KB schema parse quadratic (~2 s). Generate the fixture instead
    /// of committing a real-world schema just for this timing check.
    #[test]
    fn large_schema_parses_quickly() {
        let mut text = String::from("{\"type\":\"object\",\"properties\":{\n");
        for i in 0..8000 {
            if i > 0 {
                text.push_str(",\n");
            }
            text.push_str(&format!(
                "  \"key{i:05}\": {{\"type\": \"string\", \"title\": \"Field {i}\"}}"
            ));
        }
        text.push_str("\n}}");
        assert!(
            text.len() > 400_000,
            "generated fixture is {} bytes",
            text.len()
        );
        let t = std::time::Instant::now();
        let doc = super::doc(&text);
        let elapsed = t.elapsed();
        assert!(!doc.nodes.is_empty());
        // the old O(n^2) scan took ~2 s here; 500 ms leaves a 20x margin
        assert!(elapsed.as_millis() < 500, "parse took {elapsed:?}");
    }
}
