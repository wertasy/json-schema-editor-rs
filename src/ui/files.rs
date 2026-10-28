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

use crate::doc::{Doc, PathSeg};
use crate::smodel::Schema;
use crate::ui::history::History;
use crate::ui::menu::{
    App, ConfirmState, FilePickState, FilterAction, IssuesState, Msg, Overlay, SearchState,
};
use crate::ui::rows::{SynthKey, menu_kind_for, seed_variant_cursor};
use crate::ui::util::wrap_cursor;
use crate::ui::{FileSession, Issue, MenuState};

/// diskStat is the coarse file identity the idle loop watches: modification
/// time plus length (the length guards filesystems with coarse timestamps).
pub type DiskStat = (std::time::SystemTime, u64);

/// diskStatOf stats one session file; None when it cannot be stat'ed
/// (missing file, empty path).
pub fn disk_stat_of(path: &str) -> Option<DiskStat> {
    let md = std::fs::metadata(path).ok()?;
    Some((md.modified().ok()?, md.len()))
}

/// rootStack seeds the fresh top-of-stack menu for a (schema, document) pair:
/// the root menu, opened on the matched variant when the root is a pinned
/// oneOf. Session init, schema switching and disk reloads all start here.
pub fn root_stack(schema: &Schema, doc: &Doc) -> Vec<MenuState> {
    let mut st = MenuState {
        kind: menu_kind_for(schema, schema.root),
        node: schema.root,
        path: Vec::new(),
        cursor: 0,
        offset: 0,
        variant_of: None,
        one_of: None,
        sel: HashSet::new(),
        visual: false,
        vanchor: 0,
        filter: Default::default(),
    };
    seed_variant_cursor(schema, &mut st, doc);
    vec![st]
}

/// fileState is the per-file half of the Go listModel: everything that
/// belongs to one document.
pub struct FileState {
    /// the form tree of this file (arena; Go: `model *smodel.FormNode`)
    pub schema: Schema,
    pub doc: crate::doc::Doc,
    /// Format state for a faithful save: comments/directives and the YAML
    /// alias count. Owned here so one file's state never leaks to another.
    pub meta: crate::doc::Meta,
    /// The file's format codec (save, raw editors, schema directive).
    pub codec: &'static dyn crate::doc::Codec,
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
    /// a document comment was edited since the last save; comments live in
    /// the format metadata, not the Doc baseline, so they need their own
    /// dirty bit (the quit prompt and title marker consult it).
    pub comments_dirty: bool,
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
    /// the file's disk stat recorded at load and refreshed after each save,
    /// reload or declined reload prompt; the idle loop re-stats the file and
    /// opens the reload confirm when it drifted. None while the file does
    /// not exist on disk (started empty) or cannot be stat'ed.
    pub disk_stat: Option<DiskStat>,
}

impl FileState {
    /// Overwrites the document's recorded schema location (S-key picker):
    /// clear first so a fresh pick replaces the old value, then record it in
    /// the format's own convention (`schema_member` when the schema declares
    /// a top-level "$schema").
    pub fn replace_schema_location(&mut self, loc: &str, schema_member: bool) {
        self.codec
            .clear_schema_location(&mut self.doc, &mut self.meta);
        self.codec
            .set_schema_location_with(&mut self.doc, &mut self.meta, loc, schema_member);
    }

    /// newFileState builds the per-file init for one session file: the
    /// seeded root menu, the change baseline and the startup notices.
    pub fn new(session: FileSession) -> FileState {
        let schema = session.schema;
        let doc = session.doc;
        let meta = session.meta;
        let stack = root_stack(&schema, &doc);
        // Clone keeps merge-key members structurally while sharing nothing
        // with the live document, so edits through alias references keep
        // counting as changes.
        let baseline = doc.clone();
        let startup = session.opts.startup_notice.clone();
        let disk_stat = disk_stat_of(&session.opts.path);
        FileState {
            schema,
            doc,
            meta,
            codec: session.codec,
            opts: session.opts,
            stack,
            saved: false,
            with_errors: false,
            menu_cache: HashMap::new(),
            history: History::default(),
            baseline: Some(baseline),
            comments_dirty: false,
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
            disk_stat,
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

/// What a multi-file save does after writing every dirty file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveMode {
    /// `wa`: stay in the session
    Stay,
    /// `wa!`: stay in the session, skip validation
    StayForced,
    /// `waq`: write and end the session
    Quit,
    /// `waq!`: write, skip validation, end the session
    QuitForced,
}

impl SaveMode {
    fn quits(self) -> bool {
        matches!(self, SaveMode::Quit | SaveMode::QuitForced)
    }

    fn forced(self) -> bool {
        matches!(self, SaveMode::StayForced | SaveMode::QuitForced)
    }
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

    /// pollDiskChange re-stats the active file and opens the reload confirm
    /// when its disk state drifted from the recorded one. The render loop
    /// calls this only on idle ticks (no input pending) and never while a
    /// surface is open: the confirm would steal its keys. The drifted stat is
    /// recorded immediately so the question does not re-fire every tick; n
    /// keeps the in-memory document until the next external change. A deleted
    /// file is ignored (the in-memory copy stays editable and saving
    /// recreates it). Returns true when the screen changed.
    pub fn poll_disk_change(&mut self) -> bool {
        if self.overlay.is_some() || self.exiting || self.aborted {
            return false;
        }
        let path = self.fs().opts.path.clone();
        if path.is_empty() {
            return false;
        }
        let Some(stat) = disk_stat_of(&path) else {
            return false;
        };
        if self.fs().disk_stat == Some(stat) {
            return false;
        }
        self.fs_mut().disk_stat = Some(stat);
        let idx = self.cur;
        self.overlay = Some(Overlay::Confirm(ConfirmState {
            title: format!("{} changed on disk. Reload? (y/n)", path),
            hint: "y/enter/space reload from disk · n/esc keep editing".to_string(),
            on_yes: Some(Box::new(move |m| m.reload_file(idx))),
            on_no: None,
        }));
        true
    }

    /// reloadFile replaces the in-memory document of file `idx` with the
    /// bytes on disk (yes arm of the reload confirm). The schema is kept: a
    /// schema-driven file re-seeds the root menu against the same form tree,
    /// a schema-less one re-derives the runtime root (the shape may have
    /// changed). The baseline resets to the reloaded document so the change
    /// marks clear; undo history is untouched — the dispatcher snapshots the
    /// pre-reload document, so `u` reverts the reload like any other change.
    /// A failed read or parse keeps the current document and reports it.
    pub fn reload_file(&mut self, idx: usize) {
        if idx >= self.files.len() {
            return;
        }
        if idx != self.cur {
            self.switch_file_state(idx);
        }
        let path = self.fs().opts.path.clone();
        let codec = self.fs().codec;
        let label = codec.label();
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                self.notice = format!("Reload failed: {}", e);
                return;
            }
        };
        let parsed = match codec.parse(&String::from_utf8_lossy(&data)) {
            Ok(p) => p,
            Err(e) => {
                self.notice = format!("Reload failed: invalid {}: {}", label, e);
                return;
            }
        };
        let schema_less = self.fs().schema.schema_less;
        {
            let f = self.fs_mut();
            f.doc = parsed.doc;
            f.meta = parsed.meta;
            f.baseline = Some(f.doc.clone());
            f.comments_dirty = false;
            f.issues_parked = None;
            f.issues_return = 0;
            f.issues_origin = None;
            f.search_parked = None;
            f.jump_origin = None;
            f.jump_return = 0;
            f.disk_stat = disk_stat_of(&f.opts.path);
            if !schema_less {
                f.stack = root_stack(&f.schema, &f.doc);
            }
        }
        if schema_less {
            self.rebuild_schema_less_root();
        }
        if self.opts().validate.is_none() {
            let items = self.live_issues(Vec::new());
            self.set_issues(items);
        }
        self.notice = format!("Reloaded {}", path);
    }

    /// keyFilePick drives the s-key file switcher: enter switches to the
    /// highlighted file, esc closes without moving.
    pub fn key_file_pick(&mut self, msg: &Msg, mut s: FilePickState) -> Option<Overlay> {
        if s.filter.active {
            return self.key_file_pick_search(msg, s);
        }
        let n = self.files.len();
        if self.jump_keys(msg, &mut s.cursor, n) {
            return Some(Overlay::FilePick(s));
        }
        match msg.key_str().as_str() {
            "/" => s.filter.open(s.cursor),
            "up" | "k" | "ctrl+k" => {
                s.cursor = wrap_cursor(s.cursor, -1, n);
            }
            "down" | "j" | "ctrl+j" => {
                s.cursor = wrap_cursor(s.cursor, 1, n);
            }
            "pgdown" | "ctrl+f" => {
                let page = self.overlay_page_size(0, false);
                s.cursor = (s.cursor + page).min(n.saturating_sub(1));
            }
            "pgup" | "ctrl+b" => {
                let page = self.overlay_page_size(0, false);
                s.cursor = s.cursor.saturating_sub(page);
            }
            "enter" => {
                self.switch_to_file(s.cursor);
                return None;
            }
            "esc" => return None,
            _ => {}
        }
        Some(Overlay::FilePick(s))
    }

    /// keyFilePickSearch consumes keys while the file-picker filter is
    /// active.
    fn key_file_pick_search(&mut self, msg: &Msg, mut s: FilePickState) -> Option<Overlay> {
        let n = self.file_pick_indices(&s).count();
        let total = self.files.len();
        let page = self.overlay_page_size(0, true);
        if matches!(
            s.filter.key(msg, &mut s.cursor, n, total, page),
            FilterAction::Commit
        ) {
            let idx = self.file_pick_indices(&s).nth(s.cursor);
            if let Some(i) = idx {
                self.switch_to_file(i);
                return None;
            }
        }
        Some(Overlay::FilePick(s))
    }

    /// filePickIndices lists the files matching the picker query.
    pub(crate) fn file_pick_indices(&self, s: &FilePickState) -> impl Iterator<Item = usize> + '_ {
        let q = s.filter.ti.value().to_lowercase();
        self.files
            .iter()
            .enumerate()
            .filter(move |(_, f)| q.is_empty() || f.opts.path.to_lowercase().contains(&q))
            .map(|(i, _)| i)
    }

    /// tabDirty reports whether the tab at index i has unsaved changes: the
    /// active one through the live flags, an inactive one through its
    /// document vs. its baseline.
    pub fn tab_dirty(&self, i: usize) -> bool {
        if i == self.cur {
            return self.dirty();
        }
        let ft = &self.files[i];
        if ft.comments_dirty {
            return true;
        }
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
    /// the session (root exit). See [`App::save_dirty_files`].
    pub fn save_dirty_files_and_exit(&mut self) {
        self.save_dirty_files(SaveMode::Quit);
    }

    /// save_dirty_files writes every dirty file, active first, then either
    /// returns to the file that was active (`w`/`wa`) or ends the session
    /// (`wq`/`waq`, root exit). `Force` modes skip save-time validation.
    /// The active file always writes at exit (rewritten identically); a
    /// non-quitting save with nothing dirty only reports it.
    /// Validation errors open the errs overlay with exit armed as requested
    /// (y --force-saves; n/esc cancels); a failed write leaves the session
    /// open with the notice.
    pub fn save_dirty_files(&mut self, mode: SaveMode) {
        let exit = mode.quits();
        let mut order = self.dirty_files();
        if exit && (order.is_empty() || order[0] != self.cur) {
            order.insert(0, self.cur);
        }
        if order.is_empty() {
            self.notice = "No changes to save".to_string();
            return;
        }
        let count = order.len();
        let origin = self.cur;
        for i in order {
            if i != self.cur {
                self.switch_file_state(i);
            }
            if !mode.forced() {
                let validate = self.fs().opts.validate.clone();
                if let Some(validate) = validate {
                    let errs = validate(&self.fs().doc);
                    if !errs.is_empty() {
                        // leave the failing file active: the error list's
                        // paths and jumps belong to it
                        self.open_errs_overlay(errs, exit, "");
                        return;
                    }
                }
            }
            self.write_to(false, mode.forced(), "");
            if self.dirty() {
                return; // the write failed; the notice explains
            }
        }
        if self.cur != origin {
            self.switch_file_state(origin);
        }
        if exit {
            self.exiting = true;
        } else {
            let noun = if count == 1 { "file" } else { "files" };
            self.notice = format!("Saved {count} {noun}");
        }
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
