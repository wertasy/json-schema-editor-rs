//! Schema switching, disk reload prompts, multi-file sessions, help.

use super::*;
use super::{
    app_with, assert_hint_bottom, child, dbuild, find_row, help_scroll, ks, mk_app, mstate, press,
    schema_from_json, session, set_cursor_path, sp, str_at, test_doc, test_opts, test_schema,
};

use super::*;

// --- fetch progress bar -----------------------------------------------------

/// The help bar renders a progress bar while a catalog load is pending and
/// a download streams.
#[test]
fn help_bar_shows_fetch_progress() {
    let mut m = mk_app(test_doc(), test_opts());
    m.width = 80;
    let entry = crate::catalog::Entry {
        name: "Slow".into(),
        description: String::new(),
        url: "https://example.invalid/slow.schema.json".into(),
        file_match: Vec::new(),
    };
    m.pending_schema = Some((
        entry,
        std::thread::spawn(|| -> Result<crate::ui::LoadedSchema, String> {
            std::thread::sleep(std::time::Duration::from_millis(150));
            Ok(crate::ui::LoadedSchema {
                schema: crate::smodel::untyped(&crate::doc::Doc::new()),
                validate: None,
                status: String::new(),
                schema_member: false,
            })
        }),
    ));
    crate::progress::progress_begin("https://schemas.schemastore.org/catalog.json");
    crate::progress::progress_total(4096);
    crate::progress::progress_tick(1234);
    let bar = m.help_line();
    assert!(bar.contains('█'), "bar body: {bar}");
    assert!(bar.contains("loading schema"), "label: {bar}");
    assert!(bar.contains("1.2 kB"), "downloaded bytes: {bar}");
    assert!(bar.contains("4.0 kB"), "total bytes: {bar}");
    crate::progress::progress_end();
    // once the job completed and the fetch is over, poll takes it and the
    // hints come back
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if m.pending_schema
            .as_ref()
            .is_none_or(|(_, h)| h.is_finished())
        {
            break;
        }
    }
    m.poll_jobs();
    assert!(!m.help_line().contains('█'));
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
        filter: Default::default(),
    }));
    assert_hint_bottom(&mut m, "enter jump");

    m.overlay = Some(Overlay::Errs(ErrsState {
        lines: vec!["boom".into()],
        paths: vec![Some(sp(&["server"]))],
        cursor: 0,
        exit: false,
        target: String::new(),
        filter: Default::default(),
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
    ta.set_viewport(40, 3);
    m.overlay = Some(Overlay::Text(TextState {
        path: None,
        fn_: None,
        ta,
        err: String::new(),
        verbatim_str: false,
    }));
    assert_hint_bottom(&mut m, "ctrl+s apply");

    m.overlay = None;
    m.overlay = Some(Overlay::FilePick(FilePickState {
        cursor: 0,
        filter: Default::default(),
    }));
    assert_hint_bottom(&mut m, "enter switch");
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
    assert!(matches!(m.overlay, Some(Overlay::Help(_))));
    assert!(
        !m.view()
            .contains("move the item under the cursor down / up")
    );

    press(&mut m, &["pgdown"]);
    assert_eq!(help_scroll(&m), 21, "one full help screen");
    assert!(
        m.view()
            .contains("move the item under the cursor down / up")
    );

    press(&mut m, &["pgup"]);
    assert_eq!(help_scroll(&m), 0);
    press(&mut m, &["q"]);
    assert!(m.overlay.is_none(), "q must close the help page");
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
fn repair_stack_pops_levels_whose_path_disappeared() {
    let schema = test_schema();
    let doc = test_doc();
    let mut app = app_with(schema, doc, test_opts());
    let server = child(&app.fs().schema, app.fs().schema.root, "server");
    app.push(server, sp(&["server"]));
    assert_eq!(app.fs().stack.len(), 2);
    // the container the stack is inside disappears from the document
    app.fs_mut().doc.remove(&[ks("server")]);
    app.repair_stack();
    assert_eq!(app.fs().stack.len(), 1);
    assert!(app.fs().stack[0].path.is_empty());
}

#[test]
fn whole_document_raw_edit_adopts_format_meta() {
    // A whole-document raw edit replaces the body; the format metadata must
    // be re-parsed with it or the comments in the new text are lost.
    let mut m = mk_app(test_doc(), test_opts());
    let mut ta = TextArea::new();
    ta.set_value("{\n  \"name\": \"x\" // keep me\n}\n");
    m.overlay = Some(Overlay::Text(TextState {
        path: None,
        fn_: None,
        ta,
        err: String::new(),
        verbatim_str: false,
    }));
    press(&mut m, &["ctrl+s"]);
    assert!(m.overlay.is_none(), "{:?}", m.notice);
    assert_eq!(
        m.fs().meta.comment_text(&sp(&["name"])).as_deref(),
        Some("keep me")
    );
}

// --- disk change detection ---------------------------------------------------

fn file_opts(path: &std::path::Path) -> Options {
    let mut opts = test_opts();
    opts.path = path.to_string_lossy().into_owned();
    opts
}

fn name_doc(v: &str) -> Doc {
    dbuild(|db| db.obj(vec![("name", db.s(v))]))
}

#[test]
fn disk_change_prompts_and_yes_reloads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cfg.json");
    std::fs::write(&path, r#"{"name":"old"}"#).expect("seed file");
    let doc = name_doc("old");
    let mut m = app_with(crate::smodel::untyped(&doc), doc, file_opts(&path));

    // unchanged: no prompt, no overlay
    assert!(!m.poll_disk_change());
    assert!(m.overlay.is_none());

    // external change opens the y/n confirm
    std::fs::write(&path, r#"{"name":"fresh-on-disk","extra":true}"#).expect("rewrite");
    assert!(m.poll_disk_change());
    assert!(
        matches!(&m.overlay, Some(Overlay::Confirm(c))
            if c.title.contains("changed on disk") && c.title.contains("Reload")),
        "overlay = {:?}",
        m.overlay.is_some()
    );

    press(&mut m, &["y"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "fresh-on-disk");
    assert!(!m.dirty(), "reload resets the change baseline");
    assert_eq!(m.fs().stack.len(), 1, "stack resets to the root");
    assert!(m.notice.contains("Reloaded"), "{}", m.notice);
    // the drifted stat was recorded: the question does not re-fire
    assert!(!m.poll_disk_change());
    assert!(m.overlay.is_none());
}

#[test]
fn disk_change_decline_keeps_doc_until_next_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cfg.json");
    std::fs::write(&path, r#"{"name":"old"}"#).expect("seed file");
    let doc = name_doc("old");
    let mut m = app_with(crate::smodel::untyped(&doc), doc, file_opts(&path));

    std::fs::write(&path, r#"{"name":"on-disk","extra":1}"#).expect("rewrite");
    assert!(m.poll_disk_change());
    press(&mut m, &["n"]);
    assert_eq!(
        str_at(&m.fs().doc, &["name"]),
        "old",
        "decline keeps the doc"
    );
    assert!(!m.poll_disk_change(), "decline suppresses the re-prompt");

    // the next external change asks again; esc declines the same way
    std::fs::write(&path, r#"{"name":"again","extra":22}"#).expect("rewrite");
    assert!(m.poll_disk_change());
    press(&mut m, &["esc"]);
    assert!(m.overlay.is_none());
    assert_eq!(str_at(&m.fs().doc, &["name"]), "old");
}

#[test]
fn disk_change_only_watches_the_active_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a = dir.path().join("a.json");
    let b = dir.path().join("b.json");
    std::fs::write(&a, r#"{"name":"a"}"#).expect("seed a");
    std::fs::write(&b, r#"{"name":"b"}"#).expect("seed b");
    let opts_a = file_opts(&a);
    let doc_a = name_doc("a");
    let mut opts_b = test_opts();
    opts_b.path = b.to_string_lossy().into_owned();
    let mut m = App::new(vec![
        session(test_schema(), doc_a, opts_a),
        session(test_schema(), name_doc("b"), opts_b),
    ]);
    // the inactive file changed: no prompt while a.json is active
    std::fs::write(&b, r#"{"name":"b2","extra":1}"#).expect("rewrite b");
    assert!(!m.poll_disk_change());
    // switching makes it active and the drift shows
    m.switch_file_state(1);
    assert!(m.poll_disk_change());
    press(&mut m, &["y"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "b2");
}

#[test]
fn disk_reload_schema_less_follows_new_root_shape() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cfg.json");
    std::fs::write(&path, r#"{"name":"old"}"#).expect("seed file");
    let doc = name_doc("old");
    let mut m = app_with(crate::smodel::untyped(&doc), doc, file_opts(&path));
    assert_eq!(m.top().expect("root").kind, MenuKind::Object);

    // the root became an array: the schema-less menu follows it
    std::fs::write(&path, "[1,2]").expect("rewrite");
    assert!(m.poll_disk_change());
    press(&mut m, &["y"]);
    assert_eq!(m.top().expect("root").kind, MenuKind::Items);
    assert!(m.fs().doc.get(&path_idx(1)).is_some());
    assert!(!m.dirty());
}

#[test]
fn disk_reload_schema_keeps_model_and_resets_stack() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cfg.json");
    std::fs::write(&path, r#"{"name":"old"}"#).expect("seed file");
    let doc = dbuild(|db| db.obj(vec![("name", db.s("old"))]));
    let mut m = app_with(test_schema(), doc, file_opts(&path));
    // drill in, so the reload has a stack to reset
    app_push_name(&mut m);
    assert_eq!(m.fs().stack.len(), 2);
    assert!(!m.fs().schema.schema_less);

    std::fs::write(&path, r#"{"name":"from-disk"}"#).expect("rewrite");
    assert!(m.poll_disk_change());
    press(&mut m, &["y"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "from-disk");
    assert_eq!(m.fs().stack.len(), 1, "stack resets to the root");
    assert!(!m.fs().schema.schema_less, "the schema model is kept");
    assert!(!m.dirty());
}

#[test]
fn disk_reload_invalid_content_keeps_document() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cfg.json");
    std::fs::write(&path, r#"{"name":"old"}"#).expect("seed file");
    let doc = name_doc("old");
    let mut m = app_with(crate::smodel::untyped(&doc), doc, file_opts(&path));

    std::fs::write(&path, "not json {").expect("rewrite");
    assert!(m.poll_disk_change());
    press(&mut m, &["y"]);
    assert_eq!(
        str_at(&m.fs().doc, &["name"]),
        "old",
        "bad parse keeps the doc"
    );
    assert!(
        m.notice.starts_with("Reload failed"),
        "notice = {}",
        m.notice
    );
    assert!(
        m.overlay.is_none(),
        "the failed reload still closes the ask"
    );
}

#[test]
fn saving_refreshes_disk_stat_without_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cfg.json");
    std::fs::write(&path, r#"{"name":"old"}"#).expect("seed file");
    let doc = name_doc("old");
    let mut opts = file_opts(&path);
    let sink = path;
    opts.save = Some(Arc::new(move |_doc: &Doc, _meta: &crate::doc::Meta| {
        std::fs::write(&sink, r#"{"name":"written-by-save"}"#).map_err(|e| e.to_string())
    }));
    let mut m = mk_app(doc, opts);

    press(&mut m, &["ctrl+s"]);
    assert!(m.notice.contains("Write to"), "{}", m.notice);
    // our own write is not an external change: no reload question
    assert!(!m.poll_disk_change());
}

#[test]
fn missing_then_created_file_prompts_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("late.json");
    let doc = name_doc("in-memory");
    let mut m = app_with(crate::smodel::untyped(&doc), doc, file_opts(&path));

    // the file does not exist: nothing to compare, no prompt
    assert!(!m.poll_disk_change());
    // it appears on disk: that is a drift, reload replaces the empty doc
    std::fs::write(&path, r#"{"name":"created"}"#).expect("create");
    assert!(m.poll_disk_change());
    press(&mut m, &["y"]);
    assert_eq!(str_at(&m.fs().doc, &["name"]), "created");
}

/// app_push_name drills into the name member (push + path) like enter would.
fn app_push_name(m: &mut App) {
    let root = m.fs().schema.root;
    let name = child(&m.fs().schema, root, "name");
    m.push(name, sp(&["name"]));
}

/// path_idx builds a single-index path segment path.
fn path_idx(i: usize) -> Vec<PathSeg> {
    vec![PathSeg::Idx(i)]
}

// --- feature guard tests (audit additions) ---------------------------------

/// json_dump renders the live document for before/after comparisons.
fn json_dump(doc: &Doc) -> String {
    crate::doc::codec_for("")
        .write(doc, &crate::doc::Meta::default())
        .unwrap_or_default()
}

/// Restores JSE_CATALOG_URL even when an assertion panics mid-test.
struct CatalogUrlGuard(Option<String>);

impl CatalogUrlGuard {
    fn set(url: &str) -> Self {
        let saved = std::env::var("JSE_CATALOG_URL").ok();
        // SAFETY: test-only env manipulation, serialized by ENV_LOCK
        unsafe {
            std::env::set_var("JSE_CATALOG_URL", url);
        }
        CatalogUrlGuard(saved)
    }
}

#[test]
fn s_hints_only_one_file() {
    let mut m = mk_app(test_doc(), test_opts());
    press(&mut m, &["s"]);
    assert_eq!(m.notice, "only one file is open");
    assert!(m.overlay.is_none(), "no file picker opened");
}

#[test]
fn schema_compile_failure_disables_validation_but_opens() {
    // smodel is lenient about a non-array "required", while the jsonschema
    // compiler rejects it: the session must still open, minus validation.
    let dir = tempfile::tempdir().expect("tempdir");
    let schema = dir.path().join("schema.json");
    std::fs::write(
        &schema,
        br#"{"type":"object","properties":{"a":{"type":"string"}},"required":"not-an-array"}"#,
    )
    .expect("schema write");
    let config = dir.path().join("config.json");
    std::fs::write(&config, br#"{"a": "x"}"#).expect("config write");

    let sess = crate::session::load_session(
        config.to_str().expect("config path"),
        schema.to_str().expect("schema path"),
        false,
        false,
        false,
    )
    .expect("a validator compile failure must not fail the load");
    assert!(
        sess.opts.validate.is_none(),
        "compile failure disables save-time validation"
    );
    assert!(
        sess.opts
            .status
            .contains("schema compile failed, save-time validation disabled"),
        "status = {}",
        sess.opts.status
    );
    assert!(!sess.schema.schema_less, "the form tree still applies");
    let _ = child(&sess.schema, sess.root, "a"); // panics unless mapped

    let mut m = App::new(vec![sess]);
    assert!(find_row(&mut m, |r, _| r.path == sp(&["a"])).is_some());
    press(&mut m, &["enter"]); // editing keeps working
    match &mut m.overlay {
        Some(Overlay::Edit(e)) => e.ti.set_value("y"),
        _ => panic!("edit overlay missing"),
    }
    press(&mut m, &["enter"]);
    assert_eq!(str_at(&m.fs().doc, &["a"]), "y");
}

#[test]
fn s_catalog_failure_keeps_current_schema() {
    // the lock serializes with catalog::tests' JSE_CATALOG_URL mutations
    let _g = crate::catalog::tests::ENV_LOCK.lock();
    let _guard = CatalogUrlGuard::set("http://127.0.0.1:1/jse-guard-missing-catalog.json");

    let mut opts = test_opts();
    opts.load_schema = Some(Arc::new(
        |_: &str| -> Result<crate::ui::LoadedSchema, String> {
            Err("picker must not reach schema load".to_string())
        },
    ));
    let mut m = mk_app(test_doc(), opts);
    let root_before = m.fs().schema.root;
    let doc_before = json_dump(&m.fs().doc);

    press(&mut m, &["S"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while m.pending_catalog.is_some() && std::time::Instant::now() < deadline {
        m.poll_jobs();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    m.poll_jobs();
    assert!(m.pending_catalog.is_none(), "catalog job finished");
    assert!(
        m.notice.starts_with("schema catalog:"),
        "notice = {}",
        m.notice
    );
    assert_eq!(m.fs().schema.root, root_before, "schema kept");
    assert_eq!(json_dump(&m.fs().doc), doc_before, "document kept");
    assert!(m.overlay.is_none(), "no picker opened");
    assert_eq!(m.fs().stack.len(), 1, "navigation untouched");
}
