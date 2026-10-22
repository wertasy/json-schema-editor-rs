//! Multi-file sessions: one editor process holds several config files
//! (`jse a.json b.json`). The active file's state is `files[cur]`; switching
//! is an index change, so Go's captureFileState/restoreFileState dance
//! collapses into `switch_file_state`.
//!
//! Rust port of `internal/ui/menu_files.go`. Per-file state includes the
//! menu stack, undo/redo history, the change baseline, the live issue list
//! and the parked jump/search/issue return points, so returning to a file
//! lands on the exact breadcrumb path and cursor it was left at. Overlays
//! live on [`App`](crate::ui::menu::App) and can never be open across a
//! switch (the `s` picker is reachable only in list mode), so they are not
//! captured.

use std::collections::{HashMap, HashSet};

use crate::doc::PathSeg;
use crate::smodel::Schema;
use crate::ui::history::History;
use crate::ui::menu::{App, IssuesState, SearchState};
use crate::ui::rows::{SynthKey, menu_kind_for, seed_variant_cursor};
use crate::ui::{FileSession, Issue, MenuState};

/// fileState is the per-file half of the Go listModel: everything that
/// belongs to one document.
pub struct FileState {
    /// the form tree of this file (arena; Go: `model *smodel.FormNode`)
    pub schema: Schema,
    /// the session's root form node (Go: the `model` pointer itself); the
    /// spawned root menu captures it, so later code reads the stack instead
    #[allow(dead_code)]
    pub root: crate::smodel::FormNodeId,
    pub doc: crate::doc::Doc,
    pub opts: crate::ui::Options,
    pub stack: Vec<MenuState>,
    /// at least one successful write this session
    pub saved: bool,
    /// the last successful write skipped validation errors (--force)
    pub with_errors: bool,
    /// memoized synthesized schema nodes (Go: `menuCache`); keyed by the
    /// rows::SynthKey identity that replaces Go's pointer memo
    pub menu_cache: HashMap<SynthKey, crate::smodel::FormNodeId>,
    /// undo/redo stacks (Go: `undoStack`/`redoStack`)
    pub history: History,
    /// loaded/last-saved snapshot for change marks
    pub baseline: Option<crate::doc::Doc>,
    /// live validation problems (refreshed after writes)
    pub issues_now: Vec<Issue>,
    /// validation error locations, marked on their rows
    pub err_paths: Vec<Vec<PathSeg>>,
    /// unknown-key warning locations (yellow marks)
    pub warn_paths: Vec<Vec<PathSeg>>,
    /// parked issues overlay: esc at the jump target returns here
    pub issues_parked: Option<IssuesState>,
    /// stack depth of the issue jump target
    pub issues_return: usize,
    /// stack captured when the issue jump parked the list
    pub issues_origin: Option<Vec<MenuState>>,
    /// stack captured when ga/gr jumped; None when inactive
    pub jump_origin: Option<Vec<MenuState>>,
    /// stack depth of the ga/gr landing
    pub jump_return: usize,
    /// parked search overlay state (jump landed, esc at return_depth reopens)
    pub search_parked: Option<SearchState>,
    /// session info notices (aliases kept, schema source), dropped on the
    /// first key press
    pub startup: String,
}

impl FileState {
    /// newFileState builds the per-file init for one session file: the
    /// seeded root menu, the change baseline and the startup notices.
    pub fn new(session: FileSession) -> FileState {
        let schema = session.schema;
        let root = session.root;
        let doc = session.doc;
        let mut stack = vec![MenuState {
            kind: menu_kind_for(&schema, root),
            node: root,
            path: Vec::new(),
            cursor: 0,
            offset: 0,
            variant_of: None,
            one_of: None,
            sel: HashSet::new(),
            visual: false,
            vanchor: 0,
            search_ti: Default::default(),
            searching: false,
            search_prefilled: false,
            search_applied: false,
        }];
        // oneOf root: open on the matched variant
        seed_variant_cursor(&schema, stack.last_mut().expect("root pushed"), &doc);
        // Clone keeps merge-key members structurally while sharing nothing
        // with the live document, so edits through alias references keep
        // counting as changes.
        let baseline = doc.clone();
        let startup = session.opts.startup_notice.clone();
        FileState {
            schema,
            root,
            doc,
            opts: session.opts,
            stack,
            saved: false,
            with_errors: false,
            menu_cache: HashMap::new(),
            history: History::default(),
            baseline: Some(baseline),
            issues_now: Vec::new(),
            err_paths: Vec::new(),
            warn_paths: Vec::new(),
            issues_parked: None,
            issues_return: 0,
            issues_origin: None,
            jump_origin: None,
            jump_return: 0,
            search_parked: None,
            startup,
        }
    }
}

/// newMultiFileModel builds the editor for one or more session files: every
/// file gets its per-file state (including its live issue list and row
/// marks), and the first becomes the active one.
pub fn new_app(files: Vec<FileSession>) -> App {
    let mut app = App::bare(files.into_iter().map(FileState::new).collect());
    let n = app.files.len();
    for i in 0..n {
        app.cur = i;
        let msgs = app.files[i].opts.load_errors.clone();
        let items = app.live_issues(msgs);
        app.set_issues(items);
    }
    app.cur = 0;
    app
}

impl App {
    /// The active file's state (Go: the listModel fields themselves).
    pub fn fs(&self) -> &FileState {
        &self.files[self.cur]
    }

    pub fn fs_mut(&mut self) -> &mut FileState {
        &mut self.files[self.cur]
    }

    /// switchFileState swaps the active file: an index change plus a
    /// defensive stack repair (documents never change while inactive, so
    /// the repair is a no-op).
    pub fn switch_file_state(&mut self, i: usize) {
        if i >= self.files.len() {
            return;
        }
        self.cur = i;
        self.repair_stack();
    }

    /// switchToFile is the picker's enter: a jump in navigation terms — the
    /// leaving location becomes a ctrl+o back step and the forward branch
    /// dies, like a drill-in.
    pub fn switch_to_file(&mut self, i: usize) {
        if i == self.cur || i >= self.files.len() {
            return;
        }
        self.nav_forward();
        self.switch_file_state(i);
    }

    /// keyFilePick drives the s-key file switcher: enter switches to the
    /// highlighted file, esc closes without moving.
    pub fn key_file_pick(&mut self, msg: &crate::ui::menu::Msg) {
        let n = self.files.len();
        let mut cursor = self.file_pick.as_ref().map_or(0, |p| p.cursor);
        if self.jump_keys(msg, &mut cursor, n) {
            if let Some(p) = self.file_pick.as_mut() {
                p.cursor = cursor;
            }
            return;
        }
        match msg.key_str().as_str() {
            "up" | "k" | "ctrl+k" => {
                cursor = wrap_cursor(cursor, -1, n);
            }
            "down" | "j" | "ctrl+j" => {
                cursor = wrap_cursor(cursor, 1, n);
            }
            "pgdown" | "ctrl+f" => {
                let page = self.height.saturating_sub(2).max(1);
                cursor = (cursor + page).min(n.saturating_sub(1));
            }
            "pgup" | "ctrl+b" => {
                let page = self.height.saturating_sub(2).max(1);
                cursor = cursor.saturating_sub(page);
            }
            "enter" => {
                self.file_pick = None;
                self.switch_to_file(cursor);
                return;
            }
            "esc" => {
                self.file_pick = None;
                return;
            }
            _ => return,
        }
        if let Some(p) = self.file_pick.as_mut() {
            p.cursor = cursor;
        }
    }

    /// tabDirty reports whether the tab at index i has unsaved changes: the
    /// active one through the live flags, an inactive one through its
    /// document vs. its baseline.
    pub fn tab_dirty(&self, i: usize) -> bool {
        if i == self.cur {
            return self.dirty();
        }
        let ft = &self.files[i];
        match &ft.baseline {
            Some(base) => !crate::ui::menu::nodes_equal_doc(&ft.doc, base),
            None => false,
        }
    }

    /// dirtyFiles lists the tabs with unsaved changes, the active file
    /// first, the rest in file order. A single-file session reports only
    /// itself.
    pub fn dirty_files(&self) -> Vec<usize> {
        if self.files.len() <= 1 {
            if self.dirty() {
                return vec![self.cur];
            }
            return Vec::new();
        }
        let mut out: Vec<usize> = Vec::new();
        for i in 0..self.files.len() {
            if self.tab_dirty(i) {
                out.push(i);
            }
        }
        // the active tab leads the write order
        if let Some(j) = out.iter().position(|&i| i == self.cur) {
            if j != 0 {
                out.swap(0, j);
            }
        }
        out
    }

    /// saveDirtyFilesAndExit writes every dirty file (active first) and ends
    /// the session. The active file always writes — the "Save and exit" row
    /// keeps its contract of writing through even a clean document
    /// (rewritten identically); inactive tabs write only when dirty.
    /// Validation errors open the errs overlay with exit armed (y
    /// --force-saves and quits; n/esc cancels the exit); a failed write
    /// leaves the session open with the notice.
    pub fn save_dirty_files_and_exit(&mut self) {
        let mut order = self.dirty_files();
        if order.is_empty() || order[0] != self.cur {
            order.insert(0, self.cur);
        }
        for i in order {
            if i != self.cur {
                self.switch_file_state(i); // plain swap: the session is ending
            }
            let validate = self.fs().opts.validate.clone();
            if let Some(validate) = validate {
                let errs = validate(&self.fs().doc);
                if !errs.is_empty() {
                    self.open_errs_overlay(errs, true, "");
                    return;
                }
            }
            self.write_to(false, false, "");
            if self.dirty() {
                return; // the write failed; the notice explains
            }
        }
        self.exiting = true;
    }

    /// saveReport aggregates per-file write results (paths in file order).
    pub fn save_report(&self) -> (bool, bool, Vec<String>) {
        let mut saved = false;
        let mut forced = false;
        let mut paths: Vec<String> = Vec::new();
        for ft in &self.files {
            let (s, w, p) = (ft.saved, ft.with_errors, ft.opts.path.clone());
            if s {
                saved = true;
                if w {
                    forced = true;
                }
                paths.push(p);
            }
        }
        (saved, forced, paths)
    }
}

/// wrapCursor advances cur by delta over n items, wrapping at both ends (a
/// non-positive list stays at 0).
pub fn wrap_cursor(cur: usize, delta: i64, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let n = n as i64;
    let mut c = (cur as i64 + delta) % n;
    if c < 0 {
        c += n;
    }
    c as usize
}
