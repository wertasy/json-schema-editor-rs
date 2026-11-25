//! The GUI's model bridge: a headless terminal [`App`] driven through the
//! same key dispatcher the TUI uses, read back as plain view data.
//!
//! Every user gesture becomes a `Msg` (or a cursor move plus one), so
//! editing semantics — write-through applies, enum ordinals, oneOf variant
//! switches, issue refresh, undo snapshots — are the TUI's by construction.
//! The bridge translates `App` state through the model layer's computed view
//! APIs (`row_views`, `overlay_data`, header facts) into [`Snapshot`].

use std::cell::{OnceCell, RefCell};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use nsh_core::prelude::Signal;
use nsh_core::prelude::signal;

pub use crate::ui::rows::{RowOps, RowView};
use crate::ui::state::{App, FileState, Msg, OverlayData};
use crate::ui::{FileSession, Outcome};

pub use crate::ui::state::OverlayData as SheetView;

/// Opens a config file inside a running GUI session (the open screen).
/// Provided by the binary; the GUI itself never loads schemas.
pub type OpenFn = Arc<dyn Fn(&str) -> Result<FileSession, String> + Send + Sync>;

/// Facts the binary reports on stdout after the window closes (exit-code
/// mapping stays in main.rs, mirroring the terminal run). Shared with the
/// engine thread, so atomics and a mutex.
#[derive(Debug, Default)]
pub struct SharedOutcome {
    saved: AtomicBool,
    with_errors: AtomicBool,
    dirty: AtomicBool,
    saved_files: Mutex<Vec<String>>,
}

impl SharedOutcome {
    /// The outcome at window close. `dirty` is refreshed on every publish,
    /// so a close with unsaved changes never reports "unchanged".
    pub fn into_outcome(&self) -> Outcome {
        let saved = self.saved.load(Ordering::Relaxed);
        Outcome {
            saved,
            with_errors: self.with_errors.load(Ordering::Relaxed),
            aborted: false,
            unchanged: !saved && !self.dirty.load(Ordering::Relaxed),
            saved_files: self
                .saved_files
                .lock()
                .map(|v| v.clone())
                .unwrap_or_default(),
            err: None,
        }
    }
}

/// Header facts for the toolbar and breadcrumb.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Header {
    pub file: String,
    pub crumb: Vec<String>,
    pub dirty: bool,
    pub errs: usize,
    pub warns: usize,
    pub files: Vec<String>,
    pub cur: usize,
    pub hide_empty: bool,
    /// the top menu is an array: `a` inserts an item
    pub items_menu: bool,
    pub status: String,
    pub load_errors: usize,
}

/// Everything the views render, rebuilt after every dispatched key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub loaded: bool,
    pub header: Header,
    pub rows: Vec<RowView>,
    pub sheet: Option<SheetView>,
    pub notice: String,
}

/// The reactive view handles. Created on the engine thread inside the root
/// component (signals cannot exist off it) and attached to the bridge.
#[derive(Clone, Copy)]
pub struct Views {
    pub rows: Signal<Vec<RowView>>,
    pub header: Signal<Header>,
    pub sheet: Signal<Option<SheetView>>,
    pub notice: Signal<String>,
    pub loaded: Signal<bool>,
    /// open screen: typed path + error
    pub open_path: Signal<String>,
    pub open_err: Signal<String>,
}

impl std::fmt::Debug for Views {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Views").finish_non_exhaustive()
    }
}

impl Views {
    /// Creates the handles; must run on the engine thread.
    pub fn new() -> Views {
        Views {
            rows: signal(Vec::new()),
            header: signal(Header::default()),
            sheet: signal(None),
            notice: signal(String::new()),
            loaded: signal(false),
            open_path: signal(String::new()),
            open_err: signal(String::new()),
        }
    }
}

/// The bridge: owns the headless `App` and publishes snapshots into the
/// view signals.
pub struct Bridge {
    app: RefCell<App>,
    open: Option<OpenFn>,
    outcome: Arc<SharedOutcome>,
    views: OnceCell<Views>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge").finish_non_exhaustive()
    }
}

impl Bridge {
    /// A bridge over a fresh headless app; `open` enables the open screen.
    pub fn new(
        files: Vec<FileSession>,
        outcome: Arc<SharedOutcome>,
        open: Option<OpenFn>,
    ) -> Bridge {
        Bridge {
            app: RefCell::new(App::new(files)),
            open,
            outcome,
            views: OnceCell::new(),
        }
    }

    /// Attaches the view handles created inside the root component.
    pub fn attach(&self, views: Views) {
        let _ = self.views.set(views);
    }

    /// The attached view handles.
    fn views(&self) -> Views {
        *self
            .views
            .get()
            .expect("views attached in the root component")
    }

    pub fn rows(&self) -> Signal<Vec<RowView>> {
        self.views().rows
    }

    pub fn header(&self) -> Signal<Header> {
        self.views().header
    }

    pub fn sheet(&self) -> Signal<Option<SheetView>> {
        self.views().sheet
    }

    pub fn notice(&self) -> Signal<String> {
        self.views().notice
    }

    pub fn loaded(&self) -> Signal<bool> {
        self.views().loaded
    }

    pub fn open_path(&self) -> Signal<String> {
        self.views().open_path
    }

    pub fn open_err(&self) -> Signal<String> {
        self.views().open_err
    }

    /// Viewport for the headless app: overlay page sizes read it; no
    /// rendering depends on it in the GUI.
    pub fn set_viewport(&self, w: u16, h: u16) {
        self.app.borrow_mut().set_viewport(w, h);
    }

    /// Rebuilds all view signals from the current app state.
    pub fn publish(&self) {
        {
            let app = self.app.borrow();
            let dirty = !app.file_paths().is_empty() && !app.dirty_files().is_empty();
            self.outcome.dirty.store(
                dirty || self.outcome.saved.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        }
        let snap = self.snapshot();
        let v = self.views();
        v.loaded.set(snap.loaded);
        v.header.set(snap.header);
        v.rows.set(snap.rows);
        v.sheet.set(snap.sheet);
        v.notice.set(snap.notice);
    }

    /// Reads the whole view state in one borrow.
    fn snapshot(&self) -> Snapshot {
        let mut app = self.app.borrow_mut();
        if app.file_paths().is_empty() {
            return Snapshot::default();
        }
        let rows = app.row_views();
        let (errs, warns) = app.issue_counts();
        let items_menu = app
            .top()
            .is_some_and(|st| st.kind == crate::ui::MenuKind::Items);
        let header = Header {
            file: app.active_path(),
            crumb: app.menu_crumb(),
            dirty: app.active_dirty(),
            errs,
            warns,
            files: app.file_paths(),
            cur: app.cur_index(),
            hide_empty: app.hide_empty,
            items_menu,
            status: app.active_status(),
            load_errors: app.active_load_errors(),
        };
        let sheet = app.overlay_data();
        let notice = app.notice.clone();
        Snapshot {
            loaded: true,
            header,
            rows,
            sheet,
            notice,
        }
    }

    // --- gestures -----------------------------------------------------------

    /// Dispatches one key at the current cursor.
    fn key(&self, msg: Msg) {
        self.app.borrow_mut().update(msg);
    }

    /// Presses row `idx`: cursor there, then Enter (the TUI's mouse click).
    pub fn activate(&self, idx: usize) {
        if let Some(st) = self.app.borrow_mut().top_mut() {
            st.cursor = idx;
        }
        self.key(Msg::Enter);
        self.publish();
    }

    /// Bool rows and enum-array toggles flip in place (the TUI's space).
    pub fn toggle(&self, idx: usize) {
        if let Some(st) = self.app.borrow_mut().top_mut() {
            st.cursor = idx;
        }
        self.key(Msg::Char(' ', false));
        self.publish();
    }

    /// Deletes the row's member/item (the TUI's `d`).
    pub fn delete_row(&self, idx: usize) {
        if let Some(st) = self.app.borrow_mut().top_mut() {
            st.cursor = idx;
        }
        self.key(Msg::Char('d', false));
        self.publish();
    }

    /// Inserts an array item before/after the row (`A` / `a`).
    pub fn insert_item(&self, idx: usize, after: bool) {
        if let Some(st) = self.app.borrow_mut().top_mut() {
            st.cursor = idx;
        }
        self.key(Msg::Char(if after { 'a' } else { 'A' }, false));
        self.publish();
    }

    /// Adds a member key on object menus (`a`).
    pub fn add_key(&self) {
        self.key(Msg::Char('a', false));
        self.publish();
    }

    /// Pops one level / closes the open sheet (`esc`).
    pub fn back(&self) {
        self.key(Msg::Esc);
        self.publish();
    }

    /// Saves the active file (ctrl+s).
    pub fn save(&self) {
        self.key(Msg::Ctrl('s'));
        self.record_saves();
        self.publish();
    }

    /// Hide-empty filter (`z`).
    pub fn hide_empty(&self) {
        self.key(Msg::Char('z', false));
        self.publish();
    }

    /// The file switcher (`s`).
    pub fn open_files(&self) {
        self.key(Msg::Char('s', false));
        self.publish();
    }

    /// The issue list.
    pub fn open_issues(&self) {
        self.app.borrow_mut().open_issues();
        self.publish();
    }

    /// Presses row `k` of the open list sheet (issues / errors / files).
    pub fn sheet_pick_row(&self, k: usize) {
        self.app.borrow_mut().overlay_cursor_to(k);
        self.key(Msg::Enter);
        self.publish();
    }

    /// Applies the sheet (enter).
    pub fn sheet_apply(&self) {
        self.key(Msg::Enter);
        self.record_saves();
        self.publish();
    }

    /// Closes the sheet without applying (esc).
    pub fn sheet_cancel(&self) {
        self.key(Msg::Esc);
        self.publish();
    }

    /// Replaces the sheet's text buffer (scalar / key-name / raw input).
    pub fn sheet_set_text(&self, s: &str) {
        self.app.borrow_mut().overlay_set_text(s);
        self.publish();
    }

    /// Picks enum option `k` of the open edit sheet: buffer + enter, the
    /// TUI's dropdown pick path.
    pub fn sheet_pick_option(&self, k: usize) {
        self.app.borrow_mut().overlay_pick_option(k);
        self.key(Msg::Enter);
        self.publish();
    }

    /// Answers the confirm sheet: y/n/esc.
    pub fn confirm(&self, yes: bool) {
        self.key(Msg::Char(if yes { 'y' } else { 'n' }, false));
        self.record_saves();
        self.publish();
    }

    /// Force-writes past the save-time errors (`--force` sessions).
    pub fn write_force(&self) {
        self.key(Msg::Char('y', false));
        self.record_saves();
        self.publish();
    }

    /// Opens a config file inside the session (the open screen).
    pub fn open_file(&self, path: &str) {
        let Some(open) = &self.open else {
            return;
        };
        match open(path) {
            Ok(sess) => {
                self.app.borrow_mut().add_file(FileState::new(sess));
                self.views().open_path.set(String::new());
                self.views().open_err.set(String::new());
            }
            Err(e) => self.views().open_err.set(e),
        }
        self.publish();
    }

    fn record_saves(&self) {
        let app = self.app.borrow();
        let mut files = match self.outcome.saved_files.lock() {
            Ok(v) => v,
            Err(e) => e.into_inner(),
        };
        for (path, with_errors) in app.written_files() {
            if !files.contains(&path) {
                files.push(path);
                self.outcome.saved.store(true, Ordering::Relaxed);
                self.outcome.with_errors.store(
                    self.outcome.with_errors.load(Ordering::Relaxed) | with_errors,
                    Ordering::Relaxed,
                );
            }
        }
    }
}
