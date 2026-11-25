//! Change badges on array rows after inserts/deletes.

use super::*;

/// Inserting into the middle of an array flags only the inserted row: the
/// shifted followers carry no badge (LCS-aligned diff marks).
#[test]
fn pty_array_mid_insert_marks_only_the_new_row() {
    let dir = mktemp();
    let cfg = dir.path().join("mid.json");
    write_file(&cfg, r#"{"nums":[1,2,3]}"#);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(
        &[cfg_arg.as_str()],
        &[("JSE_CATALOG_URL", "http://127.0.0.1:1/catalog.json")],
        &cache,
    ) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["nums", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    pty.send(b"\r"); // drill into nums
    pty.wait_frame("items", off, &["1", "2", "3"]);
    pty.pause(Duration::from_millis(200));
    pty.send(b"a"); // insert after the first row
    pty.wait_frame("type picker", off, &["String"]);
    pty.pause(Duration::from_millis(200));
    pty.send(b"\r9\r"); // String, value 9, commit -> [1,9,2,3]
    pty.wait_frame("inserted", off, &["9"]);
    pty.pause(Duration::from_millis(200));
    let snap = pty.snapshot();
    let tail = &snap[snap.len().saturating_sub(1600)..];
    assert!(tail.contains("9 +"), "the inserted row is marked:\n{tail}");
    assert!(
        !tail.contains("2 +") && !tail.contains("3 +"),
        "shifted followers must not be marked:\n{tail}"
    );
    pty.send(b"\x13"); // ctrl+s
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03");
    pty.wait_exit(130);
    // the item was added through the String variant
    assert_eq!(read_json(&cfg)["nums"], json!([1, "9", 2, 3]));
}

/// Parent rows aggregate element-level changes on the real screen: adding a
/// field inside an array element badges the element row and the array
/// parent row with the green +, and modifying a field adds the orange ~.
#[test]
fn pty_array_parent_shows_element_change_badges() {
    let dir = mktemp();
    let schema = dir.path().join("agg.schema.json");
    let cfg = dir.path().join("agg.json");
    write_file(
        &schema,
        r#"{"type":"object","properties":{"servers":{"type":"array","items":{"type":"object","properties":{"port":{"type":"integer"}}}}}}"#,
    );
    write_file(&cfg, r#"{"servers":[{"port":1}]}"#);
    let schema_arg = arg(&schema);
    let cfg_arg = arg(&cfg);
    let cache = mktemp();
    let Some(mut pty) = pty_or_skip(&["-s", &schema_arg, &cfg_arg], &[], &cache) else {
        return;
    };
    let (_, off) = pty.wait_frame("root", 0, &["servers", "↑/↓ move"]);
    pty.pause(Duration::from_millis(200));

    pty.send(b"\r"); // servers items
    pty.pause(Duration::from_millis(200));
    pty.send(b"\r"); // element 0 menu
    pty.pause(Duration::from_millis(200));
    pty.send(b"a"); // add key
    pty.pause(Duration::from_millis(200));
    pty.send(b"host\r"); // name + create
    pty.pause(Duration::from_millis(200));
    pty.send(b"\r"); // String variant
    pty.pause(Duration::from_millis(200));
    pty.send(b"web\r"); // commit host = "web"
    pty.pause(Duration::from_millis(200));

    // walk out: the element row and the servers parent row carry the +
    pty.send(b"\x1b");
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b");
    pty.pause(Duration::from_millis(200));
    let (snap, off2) = pty.wait_frame("root with +", off, &["servers"]);
    pty.pause(Duration::from_millis(200));
    let tail = &snap[snap.len().saturating_sub(1600)..];
    assert!(
        tail.contains('+'),
        "the parent chain must show the addition:\n{tail}"
    );

    // modify port inside the element: the ~ joins the parent badge
    pty.send(b"\r"); // servers items
    pty.pause(Duration::from_millis(200));
    pty.send(b"\r"); // element 0 menu
    pty.pause(Duration::from_millis(200));
    pty.send(b"\r"); // edit port
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x7f9\r"); // clear the old digit, commit port = 9
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b"); // back to servers items
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x1b"); // back to root
    pty.wait_frame("root with + and ~", off2, &["+"]);
    pty.pause(Duration::from_millis(200));
    let snap = pty.snapshot();
    let tail = &snap[snap.len().saturating_sub(1600)..];
    assert!(
        tail.contains('~'),
        "the parent row must show the modification too:\n{tail}"
    );
    pty.send(b"\x13"); // ctrl+s
    pty.pause(Duration::from_millis(200));
    pty.send(b"\x03");
    pty.wait_exit(130);
    assert_eq!(read_json(&cfg)["servers"][0]["port"], json!(9));
    assert_eq!(read_json(&cfg)["servers"][0]["host"], json!("web"));
}
