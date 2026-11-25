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

use crate::doc::jsondoc::path_string;
use crate::doc::{Doc, PathSeg};
use crate::smodel::{FormNodeId, Schema};
use crate::ui::history::History;
use crate::ui::menu::{
    App, ConfirmAction, ConfirmState, ErrsState, FilePickState, Overlay, nodes_equal,
    remap_state_cursor,
};
use crate::ui::nav::{Nav, NavGo};
use crate::ui::rows::{
    dynamic_variants, match_variant_list, menu_kind_for, parse_issue, pinned_one_of_menu,
    seed_variant_cursor, unknown_key_issues,
};
use crate::ui::state::{DiskStat, FileState, SynthKey};
use crate::ui::{FileSession, Issue, MenuKind, MenuState, Options, Row};

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
            change_jump: None,
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
    app.seed_file_issues();
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

/// issuesFrom parses validator messages into jumpable issues.
pub fn issues_from(msgs: &[String]) -> Vec<Issue> {
    msgs.iter()
        .map(|s| Issue {
            msg: s.clone(),
            path: parse_issue(s),
            flagged: crate::ui::rows::issue_flag_paths(s),
            marks: Vec::new(),
            warn: false,
        })
        .collect()
}

impl App {
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

    /// seedFileIssues computes every file's live issue list up front, so the
    /// row marks are right the moment a file becomes active (new_app visits
    /// each file by switching to it, then lands back on the first).
    pub(crate) fn seed_file_issues(&mut self) {
        let n = self.files.len();
        for i in 0..n {
            self.cur = i;
            let msgs = self.files[i].opts.load_errors.clone();
            let items = self.live_issues(msgs);
            self.set_issues(items);
        }
        self.cur = 0;
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
            on_yes: Some(ConfirmAction::ReloadFile(idx)),
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
            f.change_jump = None;
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
    /// Whether the active file has unsaved changes.
    pub fn active_dirty(&self) -> bool {
        self.dirty_files().contains(&self.cur)
    }

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

    /// bare builds the shell other constructors fill in (files already
    /// converted to FileState). The `:` command history is seeded from the
    /// first file's Options::cmd_history_file, so up/down recall the
    /// previous sessions' commands.
    pub fn bare(files: Vec<FileState>) -> App {
        let cmd_history = files
            .first()
            .and_then(|f| f.opts.cmd_history_file.as_deref())
            .map(crate::ui::cmd_history::load)
            .unwrap_or_default();
        App {
            files,
            cur: 0,
            width: 80,
            height: 24,
            overlay: None,
            fast_move: None,
            hide_empty: false,
            desc_expanded: false,
            notice: String::new(),
            cmd_buf: Vec::new(),
            recording: false,
            rec_buf: Vec::new(),
            macro_buf: Vec::new(),
            nav: Nav::default(),
            aborted: false,
            exiting: false,
            ring_keep: false,
            exec_editor: false,
            pending_edit: None,
            cmd_history,
            builtin_clipboard: None,
            mouse_targets: Vec::new(),
            diff_agg: std::collections::HashMap::new(),
            diff_added: Vec::new(),
            pending_catalog: None,
            pending_schema: None,
        }
    }

    pub fn doc(&self) -> &Doc {
        &self.fs().doc
    }

    pub fn opts(&self) -> &Options {
        &self.fs().opts
    }

    /// typing reports whether a text input has focus.
    pub fn typing(&self) -> bool {
        if self.overlay.as_ref().is_some_and(Overlay::typing) {
            return true;
        }
        if self.fast_move.is_some() {
            return true;
        }
        self.top().is_some_and(|st| st.filter.active)
    }

    /// listMode reports whether the plain menu key table owns the keyboard.
    pub fn list_mode(&self) -> bool {
        self.overlay.is_none() && !self.typing()
    }

    /// dirty reports whether anything in the document differs from the
    /// baseline.
    pub fn dirty(&self) -> bool {
        let f = self.fs();
        if f.comments_dirty {
            return true;
        }
        match &f.baseline {
            Some(base) => !nodes_equal(&f.doc, Some(f.doc.root), base, Some(base.root)),
            None => false,
        }
    }

    /// setIssues replaces the live validation problem list and its row marks.
    pub fn set_issues(&mut self, items: Vec<Issue>) {
        let mut err_paths = Vec::new();
        let mut warn_paths = Vec::new();
        for it in &items {
            let mut marks: Vec<Vec<PathSeg>> = it.marks.clone();
            if marks.is_empty() {
                if let Some(p) = &it.path {
                    marks = vec![p.clone()];
                }
            }
            for p in marks {
                if p.is_empty() && it.path.is_none() {
                    // document root: the status row carries the count
                    continue;
                }
                if p.is_empty() {
                    continue;
                }
                if it.warn {
                    warn_paths.push(p);
                } else {
                    err_paths.push(p);
                }
            }
        }
        let f = self.fs_mut();
        f.issues_now = items;
        f.err_paths = err_paths;
        f.warn_paths = warn_paths;
    }

    /// liveIssues combines validator messages with the editor's unknown-key
    /// warnings for the live issue list. A key the validator already flags
    /// (e.g. additionalProperties) is not warned about twice.
    pub fn live_issues(&mut self, msgs: Vec<String>) -> Vec<Issue> {
        let mut items = issues_from(&msgs);
        let warnings = {
            let f = self.fs();
            unknown_key_issues(&f.schema, &f.doc)
        };

        // An additionalProperties message names keys in map iteration order,
        // and a failed oneOf branch also names fields that are valid in the
        // matched variant. Mark only the names that really are schema-unknown
        // keys in the edited value; when none are, mark the object itself.
        for it in items.iter_mut() {
            if it.warn || !it.msg.contains("additional properties ") {
                continue;
            }
            let Some((loc, _)) = crate::ui::rows::split_issue(&it.msg) else {
                continue;
            };
            let listed: std::collections::HashSet<String> =
                it.flagged.iter().map(|p| path_string(p)).collect();
            let mut actual: Vec<Vec<PathSeg>> = Vec::new();
            for w in &warnings {
                if let Some(wp) = &w.path {
                    if listed.contains(&path_string(wp)) {
                        actual.push(wp.clone());
                    }
                }
            }
            if actual.is_empty() {
                it.path = Some(loc.clone());
                it.flagged = vec![loc.clone()];
                it.marks = vec![loc];
            } else {
                it.path = Some(actual[0].clone());
                it.flagged = actual.clone();
                it.marks = actual;
            }
        }

        let mut flagged: std::collections::HashSet<String> = std::collections::HashSet::new();
        for it in &items {
            for p in &it.flagged {
                flagged.insert(path_string(p));
            }
            if let Some(p) = &it.path {
                flagged.insert(path_string(p));
            }
        }
        for w in warnings {
            if let Some(wp) = &w.path {
                if !flagged.contains(&path_string(wp)) {
                    items.push(w);
                }
            }
        }
        items
    }

    /// refreshIssues re-validates the document after a write so fixed
    /// problems leave the list, the counts and the row marks — and new
    /// problems appear immediately. Callers invoke it only when the document
    /// changed. Without a validator the load-time list stays as loaded.
    pub fn refresh_issues(&mut self) {
        let validate = match &self.opts().validate {
            Some(v) => v.clone(),
            None => return,
        };
        // The root menu gains or loses its issue status row with the problem
        // count; keep the cursor on the row it was on while the list shifts.
        let root = self.fs().stack.first().cloned();
        let before = root
            .clone()
            .map(|r| self.menu_rows_state(&r))
            .unwrap_or_default();
        let msgs = validate(&self.doc());
        let items = self.live_issues(msgs);
        self.set_issues(items);
        let Some(root) = root else { return };
        let after = self.menu_rows_state(&root);
        if after.len() != before.len() {
            remap_state_cursor(&mut self.files[self.cur].stack[0], &before, &after);
        }
    }

    pub fn repair_stack(&mut self) {
        loop {
            let top = match self.top() {
                Some(t) => t,
                None => return,
            };
            if self.fs().stack.len() <= 1 {
                break;
            }
            let p = top.path.clone();
            if p.is_empty() || self.doc().get(&p).is_some() {
                break;
            }
            self.fs_mut().stack.pop();
        }
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
    }

    /// clampStateCursor on the top state.
    pub fn clamp_cursor(&mut self, rows: &[Row]) {
        let max = rows.len().saturating_sub(1);
        if let Some(st) = self.top_mut() {
            st.cursor = st.cursor.min(max);
        }
    }

    pub fn nav_snapshot(&self) -> crate::ui::nav::NavMark {
        crate::ui::nav::nav_snapshot(&self.fs().stack, self.cur)
    }

    /// navForward records the location left behind by a forward relocation.
    pub fn nav_forward(&mut self) {
        let leaving = self.nav_snapshot();
        self.nav.forward(leaving);
    }

    /// navBackStep records the location a back motion leaves behind.
    pub fn nav_back_step(&mut self, leaving: crate::ui::nav::NavMark) {
        let landing = self.nav_snapshot();
        self.nav.back_step(leaving, &landing);
    }

    /// writeTo persists through opts.Save (target "") or opts.WriteTo (a W
    /// path). A failed write surfaces as a notice and the session continues;
    /// a successful session-file write refreshes the change-mark baseline.
    pub fn write_to(&mut self, exit: bool, forced: bool, target: &str) {
        if target.is_empty() {
            let save = match &self.opts().save {
                Some(s) => s.clone(),
                None => {
                    self.notice = "Write failed: no save target".to_string();
                    return;
                }
            };
            if let Err(e) = save(&self.doc(), &self.fs().meta) {
                self.notice = format!("Write failed: {}", e);
                return;
            }
            let f = self.fs_mut();
            f.saved = true;
            f.with_errors = forced;
            f.baseline = Some(f.doc.clone()); // markers clear once saved
            f.comments_dirty = false;
            // our own write is not an external change: re-record the stat
            f.disk_stat = disk_stat_of(&f.opts.path);
            if exit {
                self.exiting = true;
                return;
            }
            if !self.opts().path.is_empty() {
                self.notice = format!("Write to {}", self.opts().path);
                return;
            }
            self.notice = "Saved".to_string();
            return;
        }
        let write_to = match &self.opts().write_to {
            Some(w) => w.clone(),
            None => {
                self.notice = "Write failed: no write-to target".to_string();
                return;
            }
        };
        if let Err(e) = write_to(target, &self.doc(), &self.fs().meta) {
            self.notice = format!("Write failed: {}", e);
            return;
        }
        self.notice = format!("Wrote to {}", target);
    }

    /// rebuildSchemaLessRoot re-derives the runtime root after a whole-document
    /// raw edit swapped the root type (object ↔ array ↔ scalar): the
    /// schema-less model is a snapshot of the loaded shape, so a stale root
    /// would offer the wrong menu. Synth nodes and the stack are rebuilt
    /// against the new model.
    pub(crate) fn rebuild_schema_less_root(&mut self) {
        let f = self.fs_mut();
        f.schema = crate::smodel::untyped(&f.doc);
        let root = f.schema.root;
        let kind = crate::ui::rows::menu_kind_for(&f.schema, root);
        f.menu_cache.clear();
        f.stack.truncate(1);
        if let Some(st) = f.stack.first_mut() {
            st.kind = kind;
            st.node = root;
            st.path.clear();
            st.cursor = 0;
            st.offset = 0;
            st.variant_of = None;
            st.one_of = None;
            st.sel.clear();
            st.visual = false;
            st.vanchor = 0;
            st.filter.ti = Default::default();
            st.filter.active = false;
            st.filter.restore = 0;
        }
    }

    /// openErrsOverlay shows validation errors (at most 15 lines, each
    /// jumpable when it parses to a path); exit arms the --force y path to
    /// end the session after the write.
    pub fn open_errs_overlay(&mut self, errs: Vec<String>, exit: bool, target: &str) {
        const MAX_SHOWN: usize = 15;
        let mut lines = Vec::new();
        let mut paths = Vec::new();
        for (i, e) in errs.iter().enumerate() {
            if i == MAX_SHOWN {
                lines.push(format!("… and {} more", errs.len() - MAX_SHOWN));
                paths.push(None);
                break;
            }
            lines.push(e.clone());
            paths.push(parse_issue(e));
        }
        self.overlay = Some(Overlay::Errs(ErrsState {
            lines,
            paths,
            cursor: 0,
            exit,
            target: target.to_string(),
            filter: Default::default(),
        }));
    }

    /// navGo moves one step along the navigation trail.
    pub fn nav_go(&mut self, back: bool) {
        let current = self.nav_snapshot();
        match self.nav.go(back, &current) {
            NavGo::Stay(notice) => self.notice = notice.to_string(),
            NavGo::Jump(mk) => self.nav_restore(mk),
        }
    }

    /// navRestore swaps in a frozen stack (nav.rs contract: switch file,
    /// drop parked returns, repair).
    pub fn nav_restore(&mut self, mk: crate::ui::nav::NavMark) {
        if self.files.len() > 1 && mk.file != self.cur {
            self.switch_file_state(mk.file); // plain swap: navGo already moved the trails
        }
        self.fs_mut().stack = mk.stack;
        let f = self.fs_mut();
        f.jump_origin = None;
        f.jump_return = 0;
        f.search_parked = None;
        f.issues_parked = None;
        self.repair_stack();
    }

    /// dynamicMenu builds the item menu of an all-object oneOf: the merged
    /// union menu while the value is unset or ambiguous, the matched
    /// variant's menu once exactly one candidate remains.
    pub fn dynamic_menu(&mut self, node: FormNodeId, path: Vec<PathSeg>) -> MenuState {
        let mut st = MenuState {
            kind: MenuKind::Object,
            node,
            path,
            cursor: 0,
            offset: 0,
            variant_of: Some(node),
            one_of: None,
            sel: std::collections::HashSet::new(),
            visual: false,
            vanchor: 0,
            filter: Default::default(),
        };
        let val = self.doc().get(&st.path);
        let cands = {
            let schema = &self.fs().schema;
            let variants = dynamic_variants(schema, node).unwrap_or_default();
            match_variant_list(schema, &variants, &self.doc(), val)
        };
        st.node = if cands.len() == 1 {
            let v = cands[0];
            self.cached_menu_pinned(node, v)
        } else {
            self.cached_menu_merged(node)
        };
        st
    }

    /// refreshVariantMenu re-runs variant inference on every dynamic item
    /// menu in the stack after each key press: the node flips between merged
    /// and pinned as the value disambiguates. The cursor follows the field
    /// it was on (matched by schema key), so flipping via the discriminator
    /// keeps it selected.
    pub fn refresh_variant_menu(&mut self) {
        for i in 0..self.fs().stack.len() {
            let (variant_of, path, node) = {
                let st = &self.fs().stack[i];
                match st.variant_of {
                    Some(v) => (v, st.path.clone(), st.node),
                    None => continue,
                }
            };
            let want = self.dynamic_menu(variant_of, path).node;
            if want == node {
                continue;
            }
            let key = self.cursor_key(i);
            self.fs_mut().stack[i].node = want;
            if !key.is_empty() {
                if let Some(pos) = self.row_index_by_key(i, &key) {
                    self.fs_mut().stack[i].cursor = pos;
                }
            }
        }
    }

    /// cachedMenu pins the merged union menu for a oneOf node.
    fn cached_menu_merged(&mut self, node: FormNodeId) -> FormNodeId {
        if let Some(&id) = self.fs().menu_cache.get(&SynthKey::Merged(node)) {
            return id;
        }
        let variants = dynamic_variants(&self.fs().schema, node).unwrap_or_default();
        let id = crate::ui::rows::merged_one_of_menu(&mut self.fs_mut().schema, node, &variants);
        self.fs_mut().menu_cache.insert(SynthKey::Merged(node), id);
        id
    }

    /// cachedMenu pins one variant's menu with unionized discriminators.
    fn cached_menu_pinned(&mut self, node: FormNodeId, variant: FormNodeId) -> FormNodeId {
        if let Some(&id) = self.fs().menu_cache.get(&SynthKey::Pinned(node, variant)) {
            return id;
        }
        let variants = dynamic_variants(&self.fs().schema, node)
            .unwrap_or_else(|| self.fs().schema.nodes[node as usize].variants.clone());
        let id = pinned_one_of_menu(&mut self.fs_mut().schema, &variants, variant);
        self.fs_mut()
            .menu_cache
            .insert(SynthKey::Pinned(node, variant), id);
        id
    }

    /// cursorKey returns the schema key of the row under the state's cursor.
    fn cursor_key(&mut self, i: usize) -> String {
        let st = self.fs().stack[i].clone();
        let rows = self.menu_rows_state(&st);
        let st = &self.fs().stack[i];
        if st.cursor >= rows.len() {
            return String::new();
        }
        rows[st.cursor]
            .fn_
            .map(|f| self.fs().schema.nodes[f as usize].key.clone())
            .unwrap_or_default()
    }

    /// rowIndexByKey finds the first row of the state backed by the given key.
    fn row_index_by_key(&mut self, i: usize, key: &str) -> Option<usize> {
        let st = self.fs().stack[i].clone();
        let rows = self.menu_rows_state(&st);
        rows.iter().position(|r| {
            r.fn_
                .map_or(false, |f| self.fs().schema.nodes[f as usize].key == key)
        })
    }
}
