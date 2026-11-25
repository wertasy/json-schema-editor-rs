//! Key handling for the menu TUI, plus its rendering (the Go menu_keys.go /
//! menu_copy.go / menu_find.go / menu_merge.go / menu_alias.go /
//! menu_paste.go / menu_select.go port, and menu.go's View layer).
//!
//! Every edit writes through to the document immediately; overlays are
//! handled by the [`App`] methods `dispatch_key` routes to. Each overlay
//! handler returns the overlay to keep (None closes it), matching menu.rs's
//! `self.overlay = match ov { … }` dispatch.

use std::collections::HashSet;

use regex::Regex;

use crate::doc::jsondoc::bump_number_literal;
use crate::doc::paste::{
    PasteMember, parse_paste, paste_empty_node, paste_has_key, plain_paste_doc,
};
use crate::doc::{CopyMode, Doc, Member, NodeData, NodeId, PathSeg, Value, append_idx, append_key};
use crate::icons::{ERR, WARN};
use crate::smodel::{FormNode, FormNodeId, Kind};
use crate::ui::cmd_history::MAX_CMD_HISTORY;
use crate::ui::command;
use crate::ui::command::{
    CommandState, ShellMode, ShellTarget, apply, apply_lines, cmd_complete, cmd_complete_step,
    cmd_completion_active, cmd_sug_refresh, compact_output, completion_candidate,
    enum_choice_error, enum_index, filter_completions, filter_enum_keywords, output_lines,
    output_text, resolve_array_target, resolve_target, run_capture, run_with_input, strip_query,
};
use crate::ui::menu::{
    AddState, AnchorMode, AnchorState, App, CommentEditState, CommentState, ConfirmAction,
    ConfirmState, DESC_BLOCK_PREVIEW_LINES, DescLine, DescSource, EditState, ErrsState,
    ExternalEdit, FastMoveState, FilePickState, FilterAction, HelpState, IssuesState, ListFilter,
    MergeState, Msg, Overlay, RefPickState, SearchState, ShellOutState, SynthKey, TextState,
    WriteState, anchor_defs, anchor_holder, anchor_holder_path, anchor_merge_use_paths,
    anchor_role, count_anchor_uses, cursor_ancestor, cursor_by_path, cursor_for_row, cursor_prefix,
    is_path_prefix, merge_ref_uses, msg_key, nodes_equal, nodes_equal_doc, page_delta,
    ref_would_cycle, remap_state_cursor, vertical_delta,
};
use crate::ui::rows::{
    any_type_menu, clip_runes, copy_row_key, current_variant, display_title, doc_key_at,
    dynamic_variants, filter_search, find_row_key, is_dot_query, is_null, is_null_opt,
    is_path_query, match_variant_list, matched_leaf_variant, paste_value_row_ok, path_pattern,
    raw_value_summary, rebuild_stack, replace_all, replace_hits, schema_accepts_string,
    schema_accepts_value, search_index, seed_variant_cursor, sel_key, selectable_row, summary,
    tag_of_node, value_text, variant_contains, variant_field_defs, variant_label,
    variant_signature, variant_value_summary,
};
use crate::ui::state::MAX_MACRO_KEYS;
use crate::ui::state::{
    buf_count, desc_line_view, err_indices, fold_desc_block, is_count_buf, is_digit_key,
    issue_indices, merge_filtered, merge_token_list, ref_words_above, same_state_eq,
    split_last_token, worst_issue_icon,
};
use crate::ui::style;
use crate::ui::util::{
    clip_display, clip_keep_tail, clip_left, clip_left_ellipsis, display_width, page_cursor,
    wrap_cursor, wrap_text,
};
use crate::ui::widgets::{
    Key, TextArea, TextInput, apply_scalar, cycle_text_sugs, input_avail, search_field_line,
    search_input_line, ta_view, ti_view, ti_window,
};
use crate::ui::{Issue, MenuKind, MenuState, Row, RowKind};

/// CMD_SUG_WINDOW caps the visible tab-completion candidates above the
/// command prompt.
pub const CMD_SUG_WINDOW: usize = 8;

/// Candidate overlays size to their content with a readable floor; the
/// terminal's available width is the only ceiling.
pub const SUG_MIN_WIDTH: usize = 24;
/// Candidates sharing at least this many leading characters hide the shared
/// prefix so their differing tails stay visible.
pub const SUG_STRIP_PREFIX_MIN: usize = 8;

// ---------------------------------------------------------------------------
// App key handling

impl App {
    // --- the key pipeline ---------------------------------------------------

    /// plain_motion reports a vertical cursor key in the plain menu. It
    /// cannot change the document, so dispatch_key skips the undo snapshot,
    /// the whole-document comparison and the variant/issue refresh that
    /// every other key pays. A 20k-row menu made that work dominate the
    /// per-repeat cost while a key was held.
    fn plain_motion(&self, msg: &Msg) -> bool {
        self.overlay.is_none()
            && self.list_mode()
            && (vertical_delta(msg).is_some() || page_delta(msg).is_some())
    }

    /// Update handles one key message: ctrl+c aborts, q aliases esc outside
    /// inputs, macros intercept, then dispatchKey.
    pub fn update(&mut self, msg: Msg) {
        if msg == Msg::Ctrl('c') {
            self.aborted = true;
            return;
        }
        // q is an esc alias in every non-typing context; inside a focused
        // text input it is an ordinary character.
        let msg = if let Msg::Char('q', false) = &msg {
            if !self.typing() { Msg::Esc } else { msg }
        } else {
            msg
        };
        self.notice.clear();
        // session info notices live until the first key press
        self.fs_mut().startup.clear();
        // ctrl+v (and terminals that report ctrl+shift+v as ctrl+v) reads the
        // clipboard and flows through the same paste path as bracketed paste.
        // Outside a text input it stays an unbound list key.
        let msg = if msg == Msg::Ctrl('v') && self.typing() {
            match self.read_clipboard_text() {
                Ok(text) => Msg::Paste(text),
                Err(e) => {
                    self.notice = format!("paste failed: {e}");
                    return;
                }
            }
        } else {
            msg
        };
        if self.macro_control(&msg) {
            return;
        }
        if self.recording {
            self.rec_buf.push(msg.clone());
            if self.rec_buf.len() >= MAX_MACRO_KEYS {
                self.macro_buf = std::mem::take(&mut self.rec_buf);
                self.recording = false;
                self.notice = format!(
                    "macro limit reached ({} keys) — recording stopped",
                    MAX_MACRO_KEYS
                );
            }
        }
        self.dispatch_key(msg);
    }

    /// dispatchKey routes one key to the active handler and runs the
    /// post-key refreshes (variant inference, live validation issues). Keys
    /// that change the document push the pre-key snapshot onto the undo
    /// stack, so one undo step reverts exactly one key's worth of changes.
    /// u/ctrl+r act only in list mode and bypass the snapshot wrap.
    pub fn dispatch_key(&mut self, msg: Msg) {
        // An expanded description is transient: any key except the c toggle
        // folds it first, then runs normally.
        if self.desc_expanded && msg.key_str() != "c" {
            self.desc_expanded = false;
        }
        // Bracketed paste (terminal paste, ctrl+shift+v) only edits focused
        // text inputs; list mode keeps ignoring it.
        if let Msg::Paste(text) = &msg {
            if self.typing() {
                self.paste_into_focused_input(text);
            }
            return;
        }
        if self.list_mode() && self.apply_history_key(&msg) {
            self.cmd_buf.clear(); // a history key breaks any pending g combo
            self.refresh_variant_menu();
            self.refresh_issues();
            return;
        }
        self.ring_keep = false;
        // S switches the schema, not a document edit: it records the location
        // and refreshes issues itself, so it must not pollute undo history.
        if self.list_mode() && msg.key_str() == "S" {
            self.route_key(msg);
            return;
        }
        if self.plain_motion(&msg) {
            self.route_key(msg);
            return;
        }
        let pre = self.doc().clone();
        let pre_file = self.cur;
        self.route_key(msg);
        // A file switch swaps the active document. That is navigation, not an
        // edit: pushing the leaving clone would pollute the restored file's
        // undo history.
        let edited = self.cur == pre_file
            && !nodes_equal(
                &pre,
                Some(pre.root),
                &self.fs().doc,
                Some(self.fs().doc.root),
            );
        if edited {
            self.files[pre_file].history.push_undo(pre);
        }
        if edited && !self.ring_keep {
            self.files[pre_file].change_jump = None;
        }
        self.refresh_variant_menu();
        if edited {
            self.refresh_issues();
        }
    }

    /// routeKey is the overlay dispatch: the innermost open surface consumes
    /// the key.
    fn route_key(&mut self, msg: Msg) {
        let Some(ov) = self.overlay.take() else {
            self.key_list(msg);
            return;
        };
        self.overlay = match ov {
            Overlay::Confirm(c) => self.key_confirm(msg, c),
            Overlay::Issues(s) => self.key_issues(msg, s),
            Overlay::Errs(s) => self.key_errs(msg, s),
            Overlay::RefPick(s) => self.key_ref_pick(msg, s),
            Overlay::Text(s) => self.key_text(msg, s),
            Overlay::Search(s) => self.key_search(msg, s),
            Overlay::Edit(s) => self.key_edit(msg, s),
            Overlay::Add(s) => self.key_add(msg, s),
            Overlay::Write(s) => self.key_write(msg, s),
            Overlay::Command(s) => self.key_command(msg, s),
            Overlay::ShellOut(s) => self.key_shell_out(msg, s),
            Overlay::Anc(s) => self.key_anchor(msg, s),
            Overlay::Mrg(s) => self.key_merge(msg, s),
            Overlay::Comment(s) => self.key_comment(msg, s),
            Overlay::CommentEdit(s) => self.key_comment_edit(msg, s),
            Overlay::Help(s) => self.key_help(&msg, s),
            Overlay::FilePick(s) => self.key_file_pick(&msg, s),
        };
    }

    /// applyHistoryKey handles u/ctrl+r; consumed reports whether the key
    /// was a history key. A successful undo/redo arms the change ring and
    /// lands the cursor on the first changed row; ctrl+j/k cycles the rest.
    pub fn apply_history_key(&mut self, msg: &Msg) -> bool {
        let key = msg.key_str();
        let f = self.fs_mut();
        let res = match key.as_str() {
            "u" => Some(f.history.undo(&mut f.doc)),
            "ctrl+r" => Some(f.history.redo(&mut f.doc)),
            _ => None,
        };
        match res {
            Some(mut res) => {
                self.notice = res.notice.to_string();
                if res.changed {
                    self.repair_stack();
                    if !res.paths.is_empty() {
                        let first = res.paths[0].clone();
                        // rotate the landed entry to the back: ctrl+j then
                        // walks the rest in order even when the landing left
                        // the cursor off any entry (rule 1 keeps an editor
                        // open; removals park on the container row), and
                        // ctrl+k re-lands it. replace-all/issues/search
                        // rings arm unchanged and always land exactly.
                        let mut ring = std::mem::take(&mut res.paths);
                        ring.remove(0);
                        ring.push(first.clone());
                        self.fs_mut().change_jump = Some(ring);
                        self.nav_forward();
                        self.land_after_history(&first);
                    }
                }
                true
            }
            None => false,
        }
    }

    // --- save flow ----------------------------------------------------------

    /// save handles ctrl+s: validate, then write the active file and stay in
    /// the session (an invalid document opens the errors overlay; --force
    /// writes anyway).
    pub fn save(&mut self) {
        self.validate_and_write(false, "");
    }

    /// openWriteTo arms the w/W input.
    pub fn open_write_to(&mut self) {
        let mut ti = TextInput::new();
        ti.set_value(self.opts().path.clone());
        ti.cursor_end();
        self.overlay = Some(Overlay::Write(WriteState {
            ti,
            err: String::new(),
            sug_prefix: String::new(),
            sugs: Vec::new(),
            sug_idx: -1,
            sug_col: 0,
        }));
        self.refresh_write_suggestions();
    }

    /// validateAndWrite validates the document like ctrl+s and then writes
    /// it to target: "" is the session's own file (opts.Save), any other
    /// path goes through opts.WriteTo (menu W).
    pub fn validate_and_write(&mut self, exit: bool, target: &str) {
        let validate = self.opts().validate.clone();
        if let Some(validate) = validate {
            let errs = validate(&self.doc());
            if !errs.is_empty() {
                self.open_errs_overlay(errs, exit, target);
                return;
            }
        }
        self.write_to(exit, false, target);
    }

    /// Rows available to a scrolling overlay list: the screen minus the
    /// title/footer chrome, an optional extra line and the filter input.
    pub(crate) fn overlay_page_size(&self, extra: usize, filtering: bool) -> usize {
        self.height
            .saturating_sub(2 + extra + usize::from(filtering))
            .max(1)
    }

    // --- input bursts -------------------------------------------------------

    /// update_repeat applies `n` identical keys collected from one terminal
    /// input burst. Consecutive menu cursor moves collapse into one step;
    /// everything else (typing, overlays, macro recording) replays the key so
    /// each press keeps its own semantics.
    pub fn update_repeat(&mut self, msg: Msg, n: usize) {
        if n <= 1 {
            self.update(msg);
            return;
        }
        let page = page_delta(&msg);
        let delta = vertical_delta(&msg);
        if page.is_none() && delta.is_none() {
            for _ in 0..n {
                self.update(msg.clone());
            }
            return;
        }
        // Recording, text inputs and overlays keep one event per press.
        if self.recording || !self.list_mode() || self.overlay.is_some() {
            for _ in 0..n {
                self.update(msg.clone());
            }
            return;
        }
        self.notice.clear();
        self.fs_mut().startup.clear();
        if let Some(page) = page {
            self.move_page(page * n as i64);
        } else if let Some(delta) = delta {
            self.move_plain(delta * n as i64);
        }
    }

    /// move_page moves the plain menu cursor one screenful, clamped at the
    /// first/last row. The visible row count is the same one listView uses.
    pub(super) fn move_page(&mut self, delta: i64) {
        let rows = self.menu_rows();
        self.move_page_rows(delta, &rows);
    }

    pub(super) fn move_page_rows(&mut self, delta: i64, rows: &[Row]) {
        self.clamp_cursor(rows);
        if rows.is_empty() {
            return;
        }
        let page = match self.top().cloned() {
            Some(st) => self.list_page_size(&st, rows),
            None => return,
        };
        let cur = self.top().map(|st| st.cursor).unwrap_or(0);
        let last = rows.len().saturating_sub(1);
        let next = (cur as i64 + delta * page as i64).clamp(0, last as i64) as usize;
        if let Some(st) = self.top_mut() {
            st.cursor = next;
        }
        self.cmd_buf.clear(); // a jump drops a pending count
        self.visual_sync(rows);
    }

    /// move_plain is the plain menu's vertical movement (key_list's j/k and
    /// arrow branch) for a whole run of repeats. The row list does not depend
    /// on the cursor, so one build serves the whole run.
    pub(super) fn move_plain(&mut self, delta: i64) {
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
        if rows.is_empty() {
            return; // key_list leaves a pending command untouched too
        }
        let n = rows.len();
        if let Some(st) = self.top_mut() {
            st.cursor = wrap_cursor(st.cursor, delta, n);
        }
        self.cmd_buf.clear(); // jump_keys drops a pending count on a plain move
        self.visual_sync(&rows);
    }

    /// list_page_size is the number of list rows on screen. It mirrors the
    /// height listView assigns to write_rows so a page move lands exactly one
    /// visible screen away.
    pub(super) fn list_page_size(&self, st: &MenuState, rows: &[Row]) -> usize {
        let (_title, summary) = self.menu_title(st);
        let desc_lines = self.desc_lines_for(st, rows, !self.desc_expanded).len();
        let mut summary_lines = if summary.is_empty() {
            0
        } else {
            wrap_text(
                &format!("variant: {}", summary),
                self.width.saturating_sub(1),
            )
            .len()
        };
        if st.kind == MenuKind::OneOf && st.filter.active {
            summary_lines += 1;
        }
        self.height
            .saturating_sub(
                2 + 1
                    + desc_lines
                    + summary_lines
                    + self.status_lines().len()
                    + self.notice_lines().len()
                    + usize::from(self.hide_empty),
            )
            .max(1)
    }

    // --- schema switching (S) ----------------------------------------------

    /// openSchemaPicker searches the SchemaStore catalog for the active file
    /// and switches the form model to the chosen schema. The catalog load
    /// runs on a worker thread so the help bar can render a progress bar
    /// while the network hangs; [`App::poll_jobs`] opens the picker when it
    /// finishes. The document body is untouched apart from the recorded
    /// schema location.
    pub(super) fn begin_schema_picker(&mut self) {
        if self.opts().accessible {
            return; // no key events in the line protocol
        }
        if self.opts().load_schema.is_none() {
            self.notice = "schema switching unavailable".to_string();
            return;
        }
        if self.pending_catalog.is_some() {
            return; // already loading
        }
        let loc = crate::catalog::loc();
        let spawned = std::thread::Builder::new()
            .name("jse-catalog-load".to_string())
            .spawn({
                let loc = loc.clone();
                move || crate::catalog::load(&loc)
            });
        match spawned {
            Ok(handle) => self.pending_catalog = Some(handle),
            Err(_) => {
                // no thread available: fall back to the blocking load
                let entries = match crate::catalog::load(&loc) {
                    Ok(entries) => entries,
                    Err(e) => {
                        self.notice = format!("schema catalog: {e}");
                        return;
                    }
                };
                self.pick_with_entries(entries);
            }
        }
    }

    /// pick_with_entries opens the schema picker for `entries` and applies
    /// the chosen schema (its download itself runs as a pending job).
    fn pick_with_entries(&mut self, entries: Vec<crate::catalog::Entry>) {
        let base = crate::paths::file_base(&self.opts().path);
        let chosen = crate::ui::schema_picker::pick(
            &entries,
            &crate::ui::schema_picker::PickOptions {
                accessible: false,
                filename: base.to_string(),
                query: crate::paths::search_stem(base),
            },
        );
        // The picker owns raw mode + the alternate screen while it runs; the
        // menu session must get them back or later keys sit in line-buffered
        // input and the next frame has nowhere to draw.
        let _ = crossterm::terminal::enable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::event::EnableBracketedPaste,
            crossterm::cursor::Hide
        );
        let Some(chosen) = chosen else {
            self.notice = "schema unchanged".to_string();
            return;
        };
        self.apply_schema(&chosen);
    }

    /// poll_jobs completes finished catalog / schema load jobs: the picker
    /// opens on the loaded entries, the schema tail applies on the downloaded
    /// schema. Returns whether the screen changed.
    pub fn poll_jobs(&mut self) -> bool {
        let mut changed = false;
        let catalog_done = matches!(&self.pending_catalog, Some(h) if h.is_finished());
        if catalog_done {
            let handle = self.pending_catalog.take().expect("checked finished");
            match handle.join() {
                Ok(Ok(entries)) => {
                    self.pick_with_entries(entries);
                    changed = true;
                }
                Ok(Err(e)) => self.notice = format!("schema catalog: {e}"),
                Err(_) => self.notice = "schema catalog: load thread failed".to_string(),
            }
        }
        let schema_done = matches!(&self.pending_schema, Some((_, h)) if h.is_finished());
        if schema_done {
            let (chosen, handle) = self.pending_schema.take().expect("checked finished");
            match handle.join() {
                Ok(Ok(loaded)) => {
                    self.finish_schema(&chosen, loaded);
                    changed = true;
                }
                Ok(Err(e)) => self.notice = format!("load schema: {e}"),
                Err(_) => self.notice = "load schema: load thread failed".to_string(),
            }
        }
        changed
    }

    /// applySchema installs a chosen catalog schema. The schema download
    /// and compile run on a worker thread so the help bar can render a
    /// progress bar; [`App::finish_schema`] applies the result when the job
    /// completes.
    pub(super) fn apply_schema(&mut self, chosen: &crate::catalog::Entry) {
        let Some(load) = self.opts().load_schema.clone() else {
            self.notice = "schema switching unavailable".to_string();
            return;
        };
        let loc = chosen.url.clone();
        let spawned = std::thread::Builder::new()
            .name("jse-schema-load".to_string())
            .spawn({
                let loc = loc.clone();
                let load = load.clone();
                move || load(&loc)
            });
        match spawned {
            Ok(handle) => self.pending_schema = Some((chosen.clone(), handle)),
            Err(_) => {
                // no thread available: fall back to the blocking load
                match load(&chosen.url.clone()) {
                    Ok(loaded) => self.finish_schema(chosen, loaded),
                    Err(e) => self.notice = format!("load schema: {e}"),
                }
            }
        }
    }

    /// finishSchema applies a downloaded schema: normalize it, rebuild the
    /// validator, overwrite the recorded schema location, and reset the
    /// menu stack. The live document becomes the input to the new form.
    fn finish_schema(&mut self, chosen: &crate::catalog::Entry, loaded: crate::ui::LoadedSchema) {
        let loc = chosen.url.clone();
        let crate::ui::LoadedSchema {
            schema: model,
            validate,
            status,
            schema_member,
            ..
        } = loaded;
        self.fs_mut().replace_schema_location(&loc, schema_member);

        let f = self.fs_mut();
        f.schema = model;
        f.menu_cache.clear();
        f.opts.validate = validate;
        f.opts.status = status;
        f.issues_parked = None;
        f.issues_return = 0;
        f.issues_origin = None;
        f.search_parked = None;
        f.change_jump = None;
        f.jump_origin = None;
        f.jump_return = 0;
        f.stack = crate::ui::files::root_stack(&f.schema, &f.doc);

        self.nav = Default::default();
        self.hide_empty = false;
        self.cmd_buf.clear();
        self.fast_move = None;
        self.notice = format!("schema from SchemaStore: {}", chosen.name);
        if self.opts().validate.is_some() {
            self.refresh_issues();
        } else {
            let items = self.live_issues(Vec::new());
            self.set_issues(items);
        }
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
    }

    // --- list mode ---------------------------------------------------------

    /// keyList is the plain menu's key table (Go keyList).
    pub fn key_list(&mut self, msg: Msg) {
        let st_kind = self.top().map(|st| st.kind);
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
        if self.menu_search_key(&msg) {
            return;
        }
        if rows.is_empty() {
            return;
        }
        if self.fast_move.is_some() && self.fast_move_key(&msg) {
            return;
        }
        self.fast_move = None; // any other key cancels fast move and works as usual
        let cursor = self.top().map(|st| st.cursor).unwrap_or(0);
        let r = rows[cursor].clone();
        if self.g_anchor_keys(&msg, &r) {
            return;
        }
        let cur = self.top().map(|st| st.cursor).unwrap_or(0);
        let mut moved_cursor = cur;
        if self.jump_keys(&msg, &mut moved_cursor, rows.len()) {
            let moved = moved_cursor != cur;
            if let Some(st) = self.top_mut() {
                st.cursor = moved_cursor;
            }
            let visual = self.top().map(|st| st.visual).unwrap_or(false);
            if visual && moved {
                self.visual_jump(&rows, cur);
            }
            return;
        }
        match msg.key_str().as_str() {
            "up" | "k" => {
                if let Some(st) = self.top_mut() {
                    st.cursor = wrap_cursor(st.cursor, -1, rows.len());
                }
                // A cursor move leaves the row set unchanged: reuse it.
                self.visual_sync(&rows);
            }
            "down" | "j" => {
                if let Some(st) = self.top_mut() {
                    st.cursor = wrap_cursor(st.cursor, 1, rows.len());
                }
                self.visual_sync(&rows);
            }
            "pgdown" | "ctrl+f" => self.move_page_rows(1, &rows),
            "pgup" | "ctrl+b" => self.move_page_rows(-1, &rows),
            "ctrl+j" => self.adaptive_jump(&rows, 1),
            "ctrl+k" => self.adaptive_jump(&rows, -1),
            "n" => self.move_to_value(&rows, 1),
            "N" => self.move_to_value(&rows, -1),
            // y/p/P/x/X/d act on the selection: a live visual select ends
            // first, then the operation runs on the marks
            "p" => {
                self.end_visual_for_op();
                self.paste_clipboard(&r, false);
            }
            "P" => {
                self.end_visual_for_op();
                self.paste_clipboard(&r, true);
            }
            "y" => {
                self.end_visual_for_op();
                self.copy_row_or_selection(&r);
            }
            "Y" => self.copy_row(&r, false),
            "x" => {
                self.end_visual_for_op();
                if st_kind == Some(MenuKind::Items) {
                    self.cut_row_or_selection(&r, false);
                } else {
                    self.cut_row_or_selection(&r, true);
                }
            }
            "X" => {
                self.end_visual_for_op();
                self.cut_row_or_selection(&r, false);
            }
            "m" => self.toggle_select(&r),
            "v" => {
                let notice;
                {
                    let Some(st) = self.top_mut() else { return };
                    st.visual = !st.visual;
                    if st.visual {
                        st.vanchor = st.cursor;
                        notice = "visual select on — j/k/gg/G mark rows, v stops".to_string();
                    } else {
                        notice = "visual select off".to_string();
                    }
                }
                let rows = self.menu_rows();
                self.visual_sync(&rows);
                self.notice = notice;
            }
            "f" => self.fast_move = Some(FastMoveState { backward: false }),
            "F" => self.fast_move = Some(FastMoveState { backward: true }),
            "z" => self.toggle_empty_keys(),
            "e" => self.edit_value_external_row(&r),
            "E" => self.edit_raw_current(&r),
            "ctrl+e" => {
                if !r.merged.is_empty() {
                    self.expand_merged(&r);
                } else if let Some(path) = self.expand_target(&r) {
                    self.expand_alias(&path);
                }
            }
            "&" => self.open_anchor(&r, AnchorMode::AnchorSet),
            "*" => self.open_anchor(&r, AnchorMode::RefSet),
            "<" => self.open_merge(),
            "enter" | "right" | "l" => {
                let is_enum_cycle = (msg.key_str() == "right" || msg.key_str() == "l")
                    && r.kind == RowKind::Field
                    && r.fn_.map_or(false, |f| {
                        self.fs().schema.nodes[f as usize].kind == Kind::Enum
                    });
                if is_enum_cycle {
                    self.cycle_enum(&r, 1);
                } else {
                    self.activate(r);
                }
            }
            "left" | "h" => {
                let is_enum_cycle = r.kind == RowKind::Field
                    && r.fn_.map_or(false, |f| {
                        self.fs().schema.nodes[f as usize].kind == Kind::Enum
                    });
                if is_enum_cycle {
                    self.cycle_enum(&r, -1);
                } else if self.fs().stack.len() <= 1 {
                    // the root menu has no parent: only esc/q leave it
                } else {
                    self.back();
                }
            }
            "esc" => {
                // esc/q cancels a live multi-selection at any menu depth
                // before it navigates: marks and visual select both count.
                let (visual, selected) = self
                    .top()
                    .map_or((false, false), |st| (st.visual, !st.sel.is_empty()));
                if visual || selected {
                    if let Some(st) = self.top_mut() {
                        st.visual = false;
                        st.sel.clear();
                    }
                    self.notice = if visual {
                        "visual select off"
                    } else {
                        "selection cleared"
                    }
                    .to_string();
                } else {
                    self.back();
                }
            }
            "ctrl+o" => self.nav_go(true),
            "tab" | "ctrl+i" => self.nav_go(false),
            " " => self.space_key(r),
            "d" | "delete" => {
                self.end_visual_for_op();
                self.delete_row(&r);
            }
            "D" => self.remove_item_shortcut(&r),
            "-" => self.adjust_number(&r, -1),
            "=" | "+" => self.adjust_number(&r, 1),
            "J" => self.move_item(&r, 1),
            "K" => self.move_item(&r, -1),
            "t" => {
                let variant_key = match r.fn_ {
                    Some(f) if self.fs().schema.nodes[f as usize].kind == Kind::OneOf => Some(f),
                    _ => None,
                };
                if st_kind != Some(MenuKind::OneOf) {
                    if let Some(f) = variant_key {
                        // t on a variant key opens its picker without
                        // entering the current scalar/object editor
                        self.push(f, r.path.clone());
                    } else if let Some(one_of) = self.top().and_then(|st| st.one_of) {
                        let path = self.top().map(|st| st.path.clone()).unwrap_or_default();
                        self.fs_mut().stack.pop();
                        self.push(one_of, path);
                    } else {
                        match r.fn_ {
                            Some(f) => {
                                let n = &self.fs().schema.nodes[f as usize];
                                if n.kind == Kind::Raw && !n.null_only && !n.raw_editor {
                                    self.open_any_type(r.path.clone());
                                }
                            }
                            None if r.kind == RowKind::Unknown => {
                                self.open_any_type(r.path.clone());
                            }
                            None => {}
                        }
                    }
                }
            }
            "a" => {
                if st_kind == Some(MenuKind::Items) {
                    self.insert_item(&r, true);
                } else {
                    self.add_key(&r, false);
                }
            }
            "A" => {
                if st_kind == Some(MenuKind::Items) {
                    self.insert_item(&r, false);
                } else {
                    self.add_key(&r, true);
                }
            }
            "c" => self.toggle_desc(&r, &rows),
            "C" => self.edit_comment_row(&r),
            "?" => self.overlay = Some(Overlay::Help(HelpState::default())),
            "/" => {
                if matches!(st_kind, Some(MenuKind::OneOf) | Some(MenuKind::EnumArray)) {
                    self.start_menu_search();
                } else {
                    self.open_search(false);
                }
            }
            "r" => self.open_search(true),
            "s" => {
                if self.files.len() > 1 {
                    self.overlay = Some(Overlay::FilePick(FilePickState {
                        cursor: self.cur,
                        filter: ListFilter {
                            restore: self.cur,
                            ..Default::default()
                        },
                    }));
                } else {
                    self.notice = "only one file is open".to_string();
                }
            }
            "S" => self.begin_schema_picker(),
            "ctrl+s" | "w" => self.save(),
            "W" => self.open_write_to(),
            ":" => self.open_command(),
            _ => {}
        }
    }

    /// toggleEmptyKeys flips the z filter for every stacked menu, remapping
    /// each saved cursor onto the same row (Go toggleEmptyKeys).
    pub fn toggle_empty_keys(&mut self) {
        let stack: Vec<MenuState> = self.fs().stack.clone();
        let mut levels: Vec<(MenuState, Vec<Row>)> = Vec::with_capacity(stack.len());
        for mut st in stack {
            let rows = self.menu_rows_state(&st);
            let max = rows.len().saturating_sub(1);
            st.cursor = st.cursor.min(max);
            levels.push((st, rows));
        }
        self.hide_empty = !self.hide_empty;
        for (mut st, before) in levels {
            let after = self.menu_rows_state(&st);
            remap_state_cursor(&mut st, &before, &after);
            // write the remapped cursor back onto the stack entry
            let idx = self
                .fs()
                .stack
                .iter()
                .position(|s| same_state_eq(s, &st))
                .unwrap_or(usize::MAX);
            if idx != usize::MAX {
                self.files[self.cur].stack[idx].cursor = st.cursor;
            }
        }
        if self.hide_empty {
            self.notice = "empty fields hidden · z shows them".to_string();
        } else {
            self.notice = "empty fields shown".to_string();
        }
    }

    /// moveToValue moves the cursor to the next/previous row with a
    /// non-null value (rows.rs move_to_value over the top state).
    pub(super) fn move_to_value(&mut self, rows: &[Row], delta: i64) {
        let Some(mut st) = self.top().cloned() else {
            return;
        };
        crate::ui::rows::move_to_value(&self.ctx(), &mut st, rows, delta);
        if let Some(t) = self.top_mut() {
            t.cursor = st.cursor;
        }
    }

    /// activate performs the enter action for a row (Go activate).
    pub(super) fn activate(&mut self, r: Row) {
        match r.kind {
            RowKind::Field => {
                let f = r.fn_.expect("field row without schema node");
                self.open_field(f, r.path.clone(), false);
            }
            RowKind::Drill => {
                let f = r.fn_.expect("drill row without schema node");
                self.push_child_menu(f, r.path.clone());
            }
            RowKind::Add => self.insert_item(&r, true),
            RowKind::Variant => self.enter_variant(&r),
            RowKind::Toggle => self.toggle_enum_member(&r),
            RowKind::Unknown => self.activate_unknown(r.path.clone()),
            RowKind::Expand => self.expand_alias(&r.path),
            RowKind::NoItems => {
                if !matches!(
                    self.top().map(|st| st.kind),
                    Some(MenuKind::OneOf) | Some(MenuKind::EnumArray)
                ) {
                    self.add_key(&r, false);
                }
            }
            RowKind::Issues => self.open_issues(),
        }
    }

    /// space toggles booleans/enum members, else activates (Go space).
    pub(super) fn space_key(&mut self, r: Row) {
        match r.kind {
            RowKind::Toggle => self.toggle_enum_member(&r),
            RowKind::Field => {
                let kind = r
                    .fn_
                    .map(|f| self.fs().schema.nodes[f as usize].kind)
                    .unwrap_or_default();
                if kind == Kind::Boolean {
                    let f = r.fn_.expect("field row without schema node");
                    self.toggle_bool(f, &r.path);
                } else {
                    self.activate(r);
                }
            }
            RowKind::Variant => {
                let is_bool = r
                    .variant
                    .map(|v| self.fs().schema.nodes[v as usize].kind == Kind::Boolean)
                    .unwrap_or(false);
                if is_bool {
                    let v = r.variant.expect("variant row without node");
                    self.toggle_bool(v, &r.path);
                } else {
                    self.activate(r);
                }
            }
            RowKind::Unknown => {
                if self.unknown_bool(&r.path) {
                    self.toggle_unknown_bool(&r.path);
                } else {
                    self.activate(r);
                }
            }
            _ => self.activate(r),
        }
    }

    /// back pops one menu level; on the root screen it ends the session
    /// directly when the document is unchanged and otherwise asks whether
    /// to save first (Go back).
    pub fn back(&mut self) {
        // parked search: esc at the landing depth reopens it
        let search_back = self
            .fs()
            .search_parked
            .as_ref()
            .map_or(false, |s| self.fs().stack.len() == s.return_depth);
        if search_back {
            self.search_return_to();
            return;
        }
        // parked issues: esc at the landing depth returns to the list
        if self.fs().issues_parked.is_some() && self.fs().stack.len() == self.fs().issues_return {
            self.issues_return_to();
            return;
        }
        // ga/gr return: esc at the landing depth restores the captured stack
        if self.fs().jump_origin.is_some() && self.fs().stack.len() == self.fs().jump_return {
            let leaving = self.nav_snapshot();
            let origin = self.fs_mut().jump_origin.take().unwrap_or_default();
            self.fs_mut().jump_return = 0;
            self.fs_mut().stack = origin;
            self.nav_back_step(leaving);
            return;
        }
        if self.fs().stack.len() > 1 {
            let leaving = self.nav_snapshot();
            self.fs_mut().stack.pop();
            if let Some(st) = self.top_mut() {
                st.visual = false; // navigation ends a visual select
            }
            self.nav_back_step(leaving);
            return;
        }
        let dirty = self.dirty_files();
        if dirty.is_empty() {
            self.exiting = true;
            return;
        }
        let title = if dirty.len() > 1 {
            format!("Save changes in {} files? (y/n)", dirty.len())
        } else {
            "Save changes? (y/n)".to_string()
        };
        self.overlay = Some(Overlay::Confirm(ConfirmState {
            title,
            hint: "y/enter/space save & exit · n discard & exit · esc stay".to_string(),
            on_yes: Some(ConfirmAction::SaveDirtyAndExit),
            on_no: Some(ConfirmAction::Abort),
        }));
    }

    /// push drills into fn's menu (Go push).
    pub fn push(&mut self, fn_: FormNodeId, path: Vec<PathSeg>) {
        self.nav_forward();
        if let Some(top) = self.fs_mut().stack.last_mut() {
            top.visual = false;
        }
        let kind = crate::ui::rows::menu_kind_for(&self.fs().schema, fn_);
        let mut st = MenuState {
            kind,
            node: fn_,
            path,
            cursor: 0,
            offset: 0,
            variant_of: None,
            one_of: None,
            sel: HashSet::new(),
            visual: false,
            vanchor: 0,
            filter: Default::default(),
        };
        {
            let f = self.fs_mut();
            seed_variant_cursor(&f.schema, &mut st, &f.doc);
        }
        self.fs_mut().stack.push(st);
    }

    /// pushChildMenu drills into a child node (Go pushChildMenu).
    pub(super) fn push_child_menu(&mut self, fn_: FormNodeId, path: Vec<PathSeg>) {
        let kind = self.fs().schema.nodes[fn_ as usize].kind;
        if kind == Kind::OneOf && dynamic_variants(&self.fs().schema, fn_).is_some() {
            self.nav_forward();
            let st = self.dynamic_menu(fn_, path);
            self.fs_mut().stack.push(st);
            return;
        }
        if kind == Kind::OneOf {
            let v = self.doc().get(&path);
            if !is_null_opt(self.doc(), v) {
                let idx = current_variant(&self.fs().schema, fn_, self.doc(), v);
                if idx >= 0 {
                    let variant = self.fs().schema.nodes[fn_ as usize].variants[idx as usize];
                    self.enter_variant_editor(variant, path, Some(fn_));
                    return;
                }
            }
        }
        self.push(fn_, path);
    }

    // --- jump keys ---------------------------------------------------------

    /// jumpKeys applies the gg/G/Home/End jumps shared by the menu list and
    /// the pure-list overlays (Go jumpKeys).
    pub fn jump_keys(&mut self, msg: &Msg, cursor: &mut usize, n: usize) -> bool {
        let key = msg.key_str();
        if !self.cmd_buf.is_empty() {
            // a g-family combo or count is pending
            if is_digit_key(&key) && is_count_buf(&self.cmd_buf) {
                self.cmd_buf.push(key);
                return true;
            }
            if key == "g" && is_count_buf(&self.cmd_buf) {
                self.cmd_buf.push(key);
                return true;
            }
            let (count, has_count) = buf_count(&self.cmd_buf);
            self.cmd_buf.clear();
            match key.as_str() {
                "g" => {
                    if self.list_mode() {
                        self.nav_forward();
                    }
                    if has_count && self.jump_item(cursor, count) {
                        return true;
                    }
                    *cursor = 0;
                    return true;
                }
                "esc" => return true, // esc cancels the pending combo only
                _ => return false,
            }
        }
        if is_digit_key(&key) && self.count_armed() {
            self.cmd_buf.push(key);
            return true;
        }
        match key.as_str() {
            "g" => {
                self.cmd_buf.push(key);
                true
            }
            "G" | "end" => {
                if self.list_mode() {
                    self.nav_forward();
                }
                *cursor = n.saturating_sub(1);
                true
            }
            "home" => {
                if self.list_mode() {
                    self.nav_forward();
                }
                *cursor = 0;
                true
            }
            _ => false,
        }
    }

    /// countArmed reports whether a digit may start a count: the menu list
    /// of an array.
    pub(super) fn count_armed(&self) -> bool {
        self.list_mode() && self.top().map(|st| st.kind) == Some(MenuKind::Items)
    }

    /// jumpItem moves the cursor to the item row with the given array index,
    /// clamped to the last item (Go jumpItem).
    pub(super) fn jump_item(&mut self, cursor: &mut usize, idx: usize) -> bool {
        let rows = self.menu_rows();
        let mut last: Option<usize> = None;
        for (i, r) in rows.iter().enumerate() {
            if r.idx < 0 || !matches!(r.kind, RowKind::Field | RowKind::Drill) {
                continue;
            }
            if r.idx == idx as i64 {
                *cursor = i;
                return true;
            }
            last = Some(i);
        }
        if let Some(l) = last {
            *cursor = l; // past-the-end counts clamp to the last item
            return true;
        }
        false // empty array: only < Add item >
    }

    // --- fast move (f/F) ---------------------------------------------------

    /// fastMoveKey handles a key while fast move waits for a character.
    pub fn fast_move_key(&mut self, msg: &Msg) -> bool {
        match msg {
            Msg::Esc => {
                self.fast_move = None;
                self.notice = "fast move cancelled".to_string();
                true
            }
            Msg::Char(ch, false) => {
                let backward = self.fast_move.as_ref().map(|f| f.backward).unwrap_or(false);
                self.fast_move = None;
                self.fast_move_to(*ch, backward);
                true
            }
            _ => false,
        }
    }

    /// fastMoveTo moves the cursor to the nearest row after (or, backward,
    /// before) the cursor whose key starts with ch (Go findMove).
    pub(super) fn fast_move_to(&mut self, ch: char, backward: bool) {
        let Some(st) = self.top() else { return };
        let cursor = st.cursor;
        let rows = self.menu_rows();
        if rows.is_empty() {
            return;
        }
        let want = ch.to_string();
        for i in 1..=rows.len() {
            let mut idx = cursor as i64;
            if backward {
                idx -= i as i64;
            } else {
                idx += i as i64;
            }
            idx %= rows.len() as i64;
            if idx < 0 {
                idx += rows.len() as i64;
            }
            let idx = idx as usize;
            if let Some(key) = find_row_key(&self.fs().schema, &rows[idx]) {
                if key.starts_with(&want) {
                    if let Some(st) = self.top_mut() {
                        st.cursor = idx;
                    }
                    return;
                }
            }
        }
        self.notice = format!("no key starts with {:?}", ch.to_string());
    }

    // --- selection ---------------------------------------------------------

    /// toggleSelect flips the cursor row's membership in the selection.
    pub(super) fn toggle_select(&mut self, r: &Row) {
        if !selectable_row(r) {
            self.notice = "m marks key-value or item rows".to_string();
            return;
        }
        let Some(st) = self.top_mut() else { return };
        let k = sel_key(&r.path);
        if !st.sel.insert(k) {
            // was already present: remove
            let k = sel_key(&r.path);
            st.sel.remove(&k);
        }
    }

    /// endVisualForOp ends a live visual select before a selection-consuming
    /// operation runs: the marks stay until the operation itself consumes or
    /// clears them, and j/k afterwards navigate plainly again.
    pub(super) fn end_visual_for_op(&mut self) {
        if let Some(st) = self.top_mut() {
            st.visual = false;
        }
    }

    /// clearStaleMarks drops a live selection after a paste moved rows: the
    /// marks are index paths, so an insert would silently re-target them.
    pub(super) fn clear_stale_marks(&mut self) {
        if let Some(st) = self.top_mut() {
            st.sel.clear();
        }
    }

    /// visualSync rebuilds the selection while visual select is on.
    pub(super) fn visual_sync(&mut self, rows: &[Row]) {
        let Some(st) = self.top_mut() else { return };
        if !st.visual {
            return;
        }
        let (mut lo, mut hi) = (st.vanchor, st.cursor);
        if lo > hi {
            (lo, hi) = (hi, lo);
        }
        let mut sel = HashSet::new();
        for i in lo..=hi.min(rows.len().saturating_sub(1)) {
            if selectable_row(&rows[i]) {
                sel.insert(sel_key(&rows[i].path));
            }
        }
        st.sel = sel;
    }

    /// visualJump lands a list jump while visual select is on and extends
    /// the marked range (Go visualJump).
    pub(super) fn visual_jump(&mut self, rows: &[Row], from: usize) {
        let Some(st) = self.top_mut() else { return };
        if st.cursor < rows.len() && !selectable_row(&rows[st.cursor]) {
            let mut dest: Option<usize> = None;
            let mut i = st.cursor as i64 - 1;
            while i >= 0 && dest.is_none() {
                if selectable_row(&rows[i as usize]) {
                    dest = Some(i as usize);
                }
                i -= 1;
            }
            let mut i = st.cursor + 1;
            while i < rows.len() && dest.is_none() {
                if selectable_row(&rows[i]) {
                    dest = Some(i);
                }
                i += 1;
            }
            if let Some(d) = dest {
                st.cursor = d;
            }
        }
        let (lo, hi) = (
            st.vanchor.min(from).min(st.cursor),
            st.vanchor.max(from).max(st.cursor),
        );
        if st.cursor == lo {
            st.vanchor = hi;
        } else {
            st.vanchor = lo;
        }
        self.visual_sync(rows);
    }

    /// rowSelected reports whether the row is in the selection.
    pub(super) fn row_selected(&self, r: &Row) -> bool {
        match self.top() {
            Some(st) => !r.path.is_empty() && st.sel.contains(&sel_key(&r.path)),
            None => false,
        }
    }
}

mod clipboard;
mod edit;
mod overlays;
mod render;
mod search;

/// Severity marker for the issues status row.
enum Sev {
    Err,
    Warn,
}

impl Sev {
    fn render_str(&self, s: &str) -> String {
        match self {
            Sev::Err => style::fg1(s),
            Sev::Warn => style::fg3(s),
        }
    }
}

/// Value-summary style marker (Go valueStyle → style choice).
enum ValStyle {
    Default,
    Dim,
    Enum,
    Bool,
    Num,
    Str,
    Obj,
}

impl ValStyle {
    fn render_str(&self, s: &str) -> String {
        match self {
            ValStyle::Default => style::val_default(s),
            ValStyle::Dim => style::faint(s),
            ValStyle::Enum => style::val_enum(s),
            ValStyle::Bool => style::val_bool(s),
            ValStyle::Num => style::val_num(s),
            ValStyle::Str => style::val_str(s),
            ValStyle::Obj => style::bold(s),
        }
    }
}

#[cfg(test)]
mod tests;
