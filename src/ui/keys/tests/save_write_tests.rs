//! Save flow, write-to targets and validation gating.

use super::*;
use super::{counting_opts, mk_app, opts_validate, press, sp, test_doc, test_opts};

use super::*;
use parking_lot::Mutex;

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
        fopts.save = Some(Arc::new(move |_, _| {
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
}

fn write_opts(path: &str, save: Arc<AtomicUsize>, paths: Arc<Mutex<Vec<String>>>) -> Options {
    let mut opts = test_opts();
    opts.path = path.to_string();
    opts.save = Some(Arc::new(move |_, _| {
        save.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    opts.write_to = Some(Arc::new(move |p: &str, _: &Doc, _: &Meta| {
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
    opts.write_to = Some(Arc::new(move |path: &str, _: &Doc, _: &Meta| {
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
    opts.write_to = Some(Arc::new(|_: &str, _: &Doc, _: &Meta| {
        Err("disk full".to_string())
    }));
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
        m.notice.contains("Write failed: disk full"),
        "notice = {}",
        m.notice
    );
}

#[test]
fn save_error_is_reported_as_write_failure() {
    let mut opts = test_opts();
    opts.path = "/origin/f.json".into();
    opts.save = Some(Arc::new(|_: &Doc, _: &Meta| Err("disk full".to_string())));
    let mut m = mk_app(test_doc(), opts);
    press(&mut m, &["ctrl+s"]);
    assert!(
        m.notice.contains("Write failed: disk full"),
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
    opts.write_to = Some(Arc::new(move |path: &str, _: &Doc, _: &Meta| {
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
