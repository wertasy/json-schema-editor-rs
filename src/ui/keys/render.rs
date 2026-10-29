use super::*;

/// ListFrame is the fixed chrome of the main list screen: the menu viewport
/// height and every line pinned above the help bar. `list_view` and the
/// command-output sizing share it, so an inline multi-line notice is chosen
/// exactly when it fits the blank rows between the menu and the bottom block.
struct ListFrame {
    rows_height: usize,
    bottom: usize,
    desc_lines: Vec<DescLine>,
    summary_lines: Vec<String>,
    status: Vec<String>,
    title_width: usize,
    write_lines: Vec<String>,
    write_sugs: Vec<String>,
    cmd_line: Option<String>,
    cmd_sugs: Vec<String>,
}

/// EditSugs is the inline autocomplete window for the edited cell. It sits
/// below the row when it fits, otherwise it replaces rows above the row so
/// candidates stay visible when the cursor is at the bottom of the screen.
struct EditSugs {
    cursor_idx: usize,
    left: usize,
    /// Some(first covered row) when the window is above the edited row.
    above_start: Option<usize>,
    lines: Vec<String>,
}

impl App {
    // --- rendering (Go menu.go View layer) ----------------------------------

    /// View renders the active surface (Go View).
    pub fn view(&mut self) -> String {
        // The search overlay renders inline and needs &mut self (its
        // breadcrumb may synthesize menu rows), so it is taken out before the
        // read-only dispatch below.
        if matches!(&self.overlay, Some(Overlay::Search(_))) {
            if let Some(Overlay::Search(s)) = self.overlay.take() {
                let out = self.list_view_search(&s);
                self.overlay = Some(Overlay::Search(s));
                return out;
            }
        }
        match &self.overlay {
            Some(Overlay::Confirm(c)) => return self.confirm_view(c),
            Some(Overlay::Issues(s)) => return self.issues_view(s),
            Some(Overlay::Errs(s)) => return self.errs_view(s),
            Some(Overlay::RefPick(s)) => return self.ref_pick_view(s),
            Some(Overlay::Text(s)) => return self.text_view(s),
            Some(Overlay::CommentEdit(s)) => return self.comment_edit_view(s),
            Some(Overlay::Comment(s)) => return self.comment_view(s),
            Some(Overlay::ShellOut(s)) => return self.shell_out_view(s),
            Some(Overlay::Help(s)) => return self.help_view(s.scroll),
            Some(Overlay::FilePick(s)) => return self.file_pick_view(s),
            _ => {}
        }
        self.list_view()
    }

    /// breadcrumb renders the navigation context as a document path.
    pub(super) fn breadcrumb(&mut self) -> String {
        let st = self.top().cloned();
        let rows = self.menu_rows_snapshot();
        self.breadcrumb_with_rows(st.as_ref(), &rows)
    }

    /// breadcrumb_with_rows is breadcrumb for a caller that already built the
    /// row list: the merge-key arrow needs the cursor row, and building all
    /// rows again doubled listView's per-frame cost on a wide menu.
    pub(super) fn breadcrumb_with_rows(&mut self, st: Option<&MenuState>, rows: &[Row]) -> String {
        let Some(st) = st else {
            return "$".to_string();
        };
        let mut crumb = crate::doc::jsondoc::path_string(&st.path);
        if let Some((_, true)) = anchor_role(self.doc(), &st.path) {
            if let Some(n) = self.doc().get(&st.path) {
                if let Some(holder) = anchor_holder_path(self.doc(), n) {
                    crumb += " → ";
                    crumb += &crate::doc::jsondoc::path_string(&holder);
                    return clip_left(&crumb, self.width);
                }
            }
        }
        if let Some(anchor) = Self::merge_crumb_row(st, rows) {
            crumb += " → ";
            crumb += &crate::doc::jsondoc::path_string(&anchor);
        }
        clip_left(&crumb, self.width)
    }

    /// mergeCrumbRow returns the anchor path the breadcrumb appends while the
    /// cursor rests on a merged key.
    pub(super) fn merge_crumb_row(st: &MenuState, rows: &[Row]) -> Option<Vec<PathSeg>> {
        if st.cursor >= rows.len() {
            return None;
        }
        let r = &rows[st.cursor];
        if r.merged.is_empty() || r.path.is_empty() {
            return None;
        }
        Some(r.path[..r.path.len() - 1].to_vec())
    }

    /// menuTitle is the title-line label of the current menu (Go menuTitle).
    pub(super) fn menu_title(&self, st: &MenuState) -> (String, String) {
        // The root screen always names the file, never the schema's title
        // ("Schema for rust-toolchain.toml", "Server Configuration", …).
        if st.path.is_empty() {
            let base = crate::paths::file_base(&self.opts().path);
            if !base.is_empty() && base != "." {
                return (base.to_string(), self.menu_summary(st));
            }
        }
        let node = &self.fs().schema.nodes[st.node as usize];
        if !node.title.is_empty() {
            return (node.title.clone(), self.menu_summary(st));
        }
        let field = st.one_of.or(st.variant_of);
        let mut title = String::new();
        if let Some(f) = field {
            let fnode = &self.fs().schema.nodes[f as usize];
            if !fnode.title.is_empty() {
                title = fnode.title.clone();
            } else if !fnode.key.is_empty() {
                title = fnode.key.clone();
            }
        }
        if title.is_empty() {
            title = doc_key_at(&st.path);
        }
        if title.is_empty() {
            // nothing to name it after
            let summary = if st.one_of.is_some() {
                variant_label(&self.fs().schema, st.node)
            } else {
                String::new()
            };
            return (display_title(&self.fs().schema, st.node), summary);
        }
        let summary = if st.kind == MenuKind::OneOf {
            self.variant_picker_summary(st)
        } else if st.one_of.is_some() {
            variant_label(&self.fs().schema, st.node)
        } else {
            variant_signature(&self.fs().schema, Some(st.node))
        };
        (title, summary)
    }

    /// menu_summary is the bottom line for a titled menu (schema title or the
    /// root filename).
    pub(super) fn menu_summary(&self, st: &MenuState) -> String {
        if st.kind == MenuKind::OneOf {
            self.variant_picker_summary(st)
        } else if st.one_of.is_some() {
            variant_label(&self.fs().schema, st.node)
        } else {
            String::new()
        }
    }

    /// variantPickerSummary is the bottom `variant:` line of a oneOf type
    /// picker: the matched variant's type name followed by its signature.
    /// Other menus keep the node's own signature.
    pub(super) fn variant_picker_summary(&self, st: &MenuState) -> String {
        let node = &self.fs().schema.nodes[st.node as usize];
        if st.kind != MenuKind::OneOf || node.kind != Kind::OneOf {
            return variant_signature(&self.fs().schema, Some(st.node));
        }
        let val = self.doc().get(&st.path);
        let idx = current_variant(&self.fs().schema, st.node, self.doc(), val);
        if idx < 0 {
            return variant_signature(&self.fs().schema, Some(st.node));
        }
        variant_label(&self.fs().schema, node.variants[idx as usize])
    }

    /// statusLines renders the session status notices.
    pub(super) fn status_lines(&self) -> Vec<String> {
        if self.fs().stack.len() != 1 {
            return Vec::new();
        }
        let mut combined = self.opts().status.clone();
        let startup = self.fs().startup.clone();
        if !startup.is_empty() {
            if !combined.is_empty() {
                combined = format!("{}\n{}", startup, combined);
            } else {
                combined = startup;
            }
        }
        if combined.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for line in combined.split('\n') {
            for w in wrap_text(line, self.width.saturating_sub(1)) {
                out.push(style::fg3(&w));
            }
        }
        out
    }

    /// helpLine renders the bottom hint row.
    pub(super) fn help_line(&self) -> String {
        if matches!(&self.overlay, Some(Overlay::Command(_))) {
            return self.rec_hint(
                ": command · w/wa/wq/waq save · q quit · !<cmd> shell · r/a/w !<cmd> · tab completes · ↑/↓ history when idle · esc",
            );
        }
        if let Some(f) = &self.fast_move {
            if f.backward {
                return self.rec_hint("Fast move: type a character to move up · esc cancels");
            }
            return self.rec_hint("Fast move: type a character to move down · esc cancels");
        }
        if let Some(st) = self.top() {
            if st.filter.active {
                let hints = if st.kind == MenuKind::EnumArray {
                    "type to filter · ↑/↓ or tab move · enter toggle · esc cancel search"
                } else {
                    "type to filter · ↑/↓ or tab move · enter select · esc cancel search"
                };
                return self.rec_hint(hints);
            }
            if st.visual {
                return self.rec_hint(
                    "Visual select: j/k, gg/G or arrows mark rows · y copy · d delete · v/esc stop",
                );
            }
            let n = st.sel.len();
            if n > 0 {
                let rows = if n == 1 { "row" } else { "rows" };
                return self.rec_hint(&format!(
                    "Marked {} {}: y copy · d delete · m unmark",
                    n, rows
                ));
            }
        }
        self.rec_hint(crate::ui::menu::MENU_HELP)
    }

    /// command_input_line renders the `:` prompt above the help bar; the
    /// caret is the text input's reverse-video cell.
    pub(super) fn command_input_line(&self) -> Option<String> {
        match &self.overlay {
            Some(Overlay::Command(s)) => {
                Some(search_input_line(":", &s.ti, self.width.saturating_sub(1)))
            }
            _ => None,
        }
    }

    /// suggestion_slice returns the absolute candidate range a window of
    /// `max` rows shows, the selected absolute index within it and whether
    /// any row is selected.
    fn suggestion_slice(sugs: &[String], idx: i64, max: usize) -> (usize, usize, usize, bool) {
        let n = sugs.len();
        let selected = idx >= 0;
        let sel = if selected {
            (idx as usize).min(n - 1)
        } else {
            0
        };
        let max = max.min(n);
        let start = if selected {
            sel.saturating_sub(max.saturating_sub(1)).min(n - max)
        } else {
            0
        };
        (start, start + max, sel, selected)
    }

    /// sug_strip_active reports whether the visible candidate window renders
    /// the `…tail` form: a long typed prefix hidden behind the ellipsis and
    /// more than one visible row. Such rows anchor at the caret so the
    /// visible tails stay next to the input position; full rows anchor at
    /// the word/input start.
    fn sug_strip_active(sugs: &[String], idx: i64, query: &str, max: usize) -> bool {
        let (start, end, ..) = Self::suggestion_slice(sugs, idx, max);
        end - start > 1 && query.chars().count() >= SUG_STRIP_PREFIX_MIN
    }

    /// suggestion_rect_width is the rectangle width suggestion_overlay_lines
    /// draws for these candidates within `avail` cells (0 when nothing
    /// shows). Placement asks for it first so a caret-anchored window can
    /// slide left without its width changing.
    pub(super) fn suggestion_rect_width(
        sugs: &[String],
        idx: i64,
        query: &str,
        max: usize,
        avail: usize,
    ) -> usize {
        if sugs.is_empty() || max == 0 || avail == 0 {
            return 0;
        }
        let (start, end, ..) = Self::suggestion_slice(sugs, idx, max);
        let strip = end - start > 1 && query.chars().count() >= SUG_STRIP_PREFIX_MIN;
        let natural = sugs[start..end]
            .iter()
            .map(|raw| {
                let shown = match strip.then(|| strip_query(raw, query)).flatten() {
                    Some(tail) if !tail.is_empty() => tail.to_string(),
                    _ => raw.clone(),
                };
                display_width(&shown)
            })
            .max()
            .unwrap_or(0);
        natural.max(SUG_MIN_WIDTH).min(avail)
    }

    /// suggestion_overlay_lines renders one candidate window: the selected
    /// row is reverse-video across the full rectangle, the rest are faint.
    pub(super) fn suggestion_overlay_lines(
        sugs: &[String],
        idx: i64,
        query: &str,
        max: usize,
        left: usize,
        max_width: usize,
    ) -> Vec<String> {
        let avail = max_width.saturating_sub(left);
        if sugs.is_empty() || max == 0 || avail == 0 {
            return Vec::new();
        }
        let (start, end, sel, selected) = Self::suggestion_slice(sugs, idx, max);
        // Hide the already-typed leading text when it is long enough; the
        // differing tail is what the user still needs to choose between. No
        // `…` marks the hidden prefix — that marker belongs to
        // clip_left_ellipsis, which cuts only when the shown text does not
        // fit the width.
        let strip = end - start > 1 && query.chars().count() >= SUG_STRIP_PREFIX_MIN;
        let displays: Vec<String> = (start..end)
            .map(|i| {
                let raw = &sugs[i];
                match strip.then(|| strip_query(raw, query)).flatten() {
                    Some(tail) if !tail.is_empty() => tail.to_string(),
                    _ => raw.clone(),
                }
            })
            .collect();
        let natural = displays
            .iter()
            .map(|shown| display_width(shown))
            .max()
            .unwrap_or(0);
        let width = natural.max(SUG_MIN_WIDTH).min(avail);
        if width == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(max);
        for (j, shown) in displays.iter().enumerate() {
            let i = start + j;
            let on = selected && i == sel;
            let mut line = shown.clone();
            if display_width(&line) > width {
                line = clip_left_ellipsis(&line, width);
            }
            // Fill the row so the background paints a solid rectangle, not
            // just the glyphs of the candidate text.
            let pad = width.saturating_sub(display_width(&line));
            line.push_str(&" ".repeat(pad));
            out.push(if on {
                style::sug_sel(&line)
            } else {
                style::sug_dim(&line)
            });
        }
        out
    }

    /// overlay_prefix keeps the content left of an inline candidate window:
    /// the underlying row's first `left` cells, padded when it is shorter.
    pub(super) fn overlay_prefix(line: &str, left: usize) -> String {
        if left == 0 {
            return String::new();
        }
        let mut out = clip_display(line, left);
        let pad = left.saturating_sub(display_width(&out));
        out.push_str(&" ".repeat(pad));
        out
    }

    /// indent_overlay indents a bottom input's candidate rectangle. There is
    /// no underlying row content to preserve there.
    pub(super) fn indent_overlay(lines: Vec<String>, left: usize) -> Vec<String> {
        if left == 0 {
            return lines;
        }
        let indent = " ".repeat(left);
        lines
            .into_iter()
            .map(|line| format!("{indent}{line}"))
            .collect()
    }

    /// command_suggestion_lines renders the visible window of an active tab
    /// completion. The prompt is at the bottom, so its window sits above it.
    pub(super) fn command_suggestion_lines(&self) -> Vec<String> {
        let Some(Overlay::Command(s)) = &self.overlay else {
            return Vec::new();
        };
        if s.sug.is_empty() {
            return Vec::new();
        }
        let idx =
            if s.sug_idx.is_some() && s.ti.value() == s.sug_line && s.ti.cursor_byte() == s.sug_end
            {
                s.sug_idx.unwrap() as i64
            } else {
                -1
            };
        // Full candidate rows anchor at the completed word's start column
        // (sug_col); `…tail` rows anchor at the caret recorded when the
        // list was built, so the visible tails stay next to the input
        // position. The input viewport may have scrolled since (a long
        // command on a narrow terminal), so the anchor maps through the
        // visible window; when the rectangle would run past the right edge
        // it slides left so every candidate stays on screen.
        let text_left = display_width(":");
        let (_, _, shift) = ti_window(&s.ti, input_avail(self.width.saturating_sub(1), text_left));
        let line_right = self.width.saturating_sub(1);
        let rect =
            Self::suggestion_rect_width(&s.sug, idx, &s.sug_prefix, CMD_SUG_WINDOW, line_right);
        let value = s.ti.value();
        let base = if Self::sug_strip_active(&s.sug, idx, &s.sug_prefix, CMD_SUG_WINDOW) {
            display_width(&value[..s.sug_end.min(value.len())])
        } else {
            s.sug_col
        };
        let left = (text_left + base.saturating_sub(shift)).min(line_right.saturating_sub(rect));
        let lines = Self::suggestion_overlay_lines(
            &s.sug,
            idx,
            &s.sug_prefix,
            CMD_SUG_WINDOW,
            left,
            line_right,
        );
        Self::indent_overlay(lines, left)
    }

    /// writeSuggestionLines is the W write-path candidate window (above the
    /// input line: the help bar occupies the space below it).
    pub(super) fn write_suggestion_lines(&self, title_width: usize) -> Vec<String> {
        let Some(Overlay::Write(w)) = &self.overlay else {
            return Vec::new();
        };
        if w.sugs.is_empty() {
            return Vec::new();
        }
        let text_left = display_width(&self.title_col("Write to", title_width));
        let (_, _, shift) = ti_window(&w.ti, input_avail(self.width.saturating_sub(1), text_left));
        let line_right = self.width.saturating_sub(1);
        let rect = Self::suggestion_rect_width(
            &w.sugs,
            w.sug_idx,
            &w.sug_prefix,
            CMD_SUG_WINDOW,
            line_right,
        );
        // Full candidate rows anchor at the input text start; `…tail` rows
        // anchor at the caret recorded when the list was rebuilt.
        let base = if Self::sug_strip_active(&w.sugs, w.sug_idx, &w.sug_prefix, CMD_SUG_WINDOW) {
            w.sug_col
        } else {
            0
        };
        let left = (text_left + base.saturating_sub(shift)).min(line_right.saturating_sub(rect));
        let lines = Self::suggestion_overlay_lines(
            &w.sugs,
            w.sug_idx,
            &w.sug_prefix,
            CMD_SUG_WINDOW,
            left,
            line_right,
        );
        Self::indent_overlay(lines, left)
    }

    /// finish_with_hint pads the view with blank rows so the help row lands
    /// on the terminal's last line, then appends it without a trailing
    /// newline. Views must size their content to leave that row free (the
    /// windowed overlays do).
    pub(super) fn finish_with_hint(&self, mut b: String, hint: String) -> String {
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1) {
            b.push('\n');
            lines += 1;
        }
        b.push_str(&hint);
        b
    }

    /// recHint renders a hint row with a right-aligned tail (cmd buffer /
    /// recording marker).
    pub(super) fn rec_hint(&self, hints: &str) -> String {
        let limit = self.width.saturating_sub(1);
        let mut parts: Vec<String> = Vec::new();
        if !self.cmd_buf.is_empty() {
            parts.push(style::fg3(&self.cmd_buf.join("")));
        }
        if self.recording {
            parts.push(style::fg1(&format!("● REC {}", self.rec_buf.len())));
        }
        if parts.is_empty() {
            return style::help(&clip_display(hints, limit));
        }
        let right = parts.join(" ");
        let w = display_width(&right);
        if self.width < w + 8 {
            return style::help(&clip_display(hints, limit));
        }
        let left = clip_display(hints, limit.saturating_sub(w + 1));
        let mut pad = limit.saturating_sub(display_width(&left)).saturating_sub(w);
        if pad < 1 {
            pad = 1;
        }
        format!("{}{}{}", style::help(&left), " ".repeat(pad), right)
    }

    /// helpView renders the "?" overlay.
    pub(super) fn help_view(&self, scroll: usize) -> String {
        let mut b = String::new();
        b.push_str(&style::bold("Keyboard help"));
        b.push_str("\n\n");
        let entries = crate::ui::menu::HELP_ENTRIES;
        let page = self.height.saturating_sub(3).max(1);
        let max_scroll = entries.len().saturating_sub(page);
        let off = scroll.min(max_scroll);
        for e in entries.iter().skip(off).take(page) {
            let line = if self.width - 1 < crate::ui::menu::HELP_COL_WIDTH + 12 {
                // narrow terminal: do not waste the row on padding
                if e.keys.is_empty() {
                    e.desc.to_string()
                } else {
                    format!("{} {}", e.keys, e.desc)
                }
            } else {
                format!("{}{}", self.help_col(e.keys), e.desc)
            };
            b.push_str(&clip_display(
                &style::help(&line),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1) {
            b.push('\n');
            lines += 1;
        }
        b.push_str(&self.rec_hint(if max_scroll > 0 {
            "j/k or pgup/pgdn scroll · esc/? close"
        } else {
            "esc/? close"
        }));
        b
    }

    pub(super) fn help_col(&self, left: &str) -> String {
        let w = display_width(left);
        if w >= crate::ui::menu::HELP_COL_WIDTH {
            format!("{left} ")
        } else {
            format!("{left}{}", " ".repeat(crate::ui::menu::HELP_COL_WIDTH - w))
        }
    }

    /// list_header_rows counts the title, breadcrumb and any inline input
    /// rows above the menu.
    fn list_header_rows(&self, st: &MenuState) -> usize {
        let mut header = 2;
        if let Some(Overlay::Mrg(s)) = &self.overlay {
            header += self.merge_input_lines(s).matches('\n').count() + 1;
        }
        if matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray) && st.filter.active {
            header += 1;
        }
        header
    }

    /// list_frame lays out the fixed chrome around the menu rows. A notice
    /// with `notice_rows` lines shrinks the viewport; the caller supplies 0
    /// when asking how many blank rows are available for one.
    fn list_frame(
        &self,
        st: &MenuState,
        rows: &[Row],
        summary: &str,
        notice_rows: usize,
    ) -> ListFrame {
        let desc_lines = self.desc_lines_for(st, rows, !self.desc_expanded);
        let summary_lines = if summary.is_empty() {
            Vec::new()
        } else {
            wrap_text(
                &format!("variant: {}", summary),
                self.width.saturating_sub(1),
            )
        };
        let status = self.status_lines();
        let title_width = self.menu_title_col_width(rows);
        let write_lines = self.write_input_lines(title_width);
        let write_sugs = self.write_suggestion_lines(title_width);
        let cmd_line = self.command_input_line();
        let cmd_sugs = self.command_suggestion_lines();
        let bottom = desc_lines.len()
            + summary_lines.len()
            + status.len()
            + notice_rows
            + write_sugs.len()
            + write_lines.len()
            + cmd_sugs.len()
            + usize::from(cmd_line.is_some());
        let rows_height = self
            .height
            .saturating_sub(self.list_header_rows(st) + 1 + bottom + usize::from(self.hide_empty));
        ListFrame {
            rows_height,
            bottom,
            desc_lines,
            summary_lines,
            status,
            title_width,
            write_lines,
            write_sugs,
            cmd_line,
            cmd_sugs,
        }
    }

    /// notice_lines renders the transient notice. A plain one-line notice is
    /// clipped like before; a message carrying newlines (command output) is
    /// wrapped into the rows above the bottom block.
    pub(super) fn notice_lines(&self) -> Vec<String> {
        if self.notice.is_empty() {
            return Vec::new();
        }
        if !self.notice.contains('\n') {
            return vec![style::fg1(&clip_runes(&self.notice, self.width))];
        }
        wrap_text(&self.notice, self.width.saturating_sub(1))
            .into_iter()
            .map(|line| style::fg1(&line))
            .collect()
    }

    /// blank_rows_above_notice is how many rows are free between the last
    /// menu row and the pinned bottom block. Command output up to this many
    /// wrapped lines stays inline instead of opening the output page.
    pub(super) fn blank_rows_above_notice(&mut self) -> usize {
        let Some(st) = self.top().cloned() else {
            return 0;
        };
        let rows = self.menu_rows_snapshot();
        let (_, summary) = self.menu_title(&st);
        let frame = self.list_frame(&st, &rows, &summary, 0);
        let rendered = rows.len().min(frame.rows_height.max(1));
        frame.rows_height.saturating_sub(rendered)
    }

    /// listView renders the plain menu screen (Go listView).
    pub(super) fn list_view(&mut self) -> String {
        let mut b = String::new();
        let Some(st) = self.top().cloned() else {
            return b;
        };
        let (title_text, summary) = self.menu_title(&st);
        let mut title = style::bold(&title_text);
        // the menu of an anchored value names the anchor on the title line
        if let Some((name, is_ref)) = anchor_role(self.doc(), &st.path) {
            if is_ref {
                title += &format!(": {}", style::anchor(&format!("*{name}")));
            } else {
                title += &format!(": {}", style::anchor(&format!("&{name}")));
            }
        }
        // a mapping with merge keys names them on the title
        let merges = self.merge_names(&st.path);
        if !merges.is_empty() {
            title += &format!(" {}", style::anchor(&merges));
        }
        // The change dot is a status marker: when the terminal is too
        // narrow for the whole title the body is cut (marked with …) so
        // the dot stays visible; the multi-file position label degrades
        // first.
        let tail = if self.dirty() {
            format!(" {}", style::diff("●"))
        } else {
            String::new()
        };
        if self.files.len() > 1 {
            let base = crate::paths::file_base(&self.opts().path);
            // The root title is already the file name; repeat only the
            // position, not the name.
            let label = if base == title_text {
                format!("[{}/{}]", self.cur + 1, self.files.len())
            } else {
                format!("[{}/{} {}]", self.cur + 1, self.files.len(), base)
            };
            title += &format!(" {}", style::faint(&label));
        }
        b.push_str(&format!(
            "{}{tail}",
            clip_keep_tail(&title, self.width.saturating_sub(1), display_width(&tail))
        ));
        b.push('\n');
        let rows = self.menu_rows_snapshot();
        b.push_str(&clip_display(
            &style::faint(&clip_runes(
                &self.breadcrumb_with_rows(Some(&st), &rows),
                self.width,
            )),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        // the "<" merge-keys input edits the mapping itself: header row
        if let Some(Overlay::Mrg(s)) = &self.overlay {
            for line in self.merge_input_lines(s).split('\n') {
                b.push_str(&clip_display(line, self.width.saturating_sub(1)));
                b.push('\n');
            }
        }
        if let Some(Overlay::Search(s)) = &self.overlay {
            return self.render_search_body(b, self.list_header_rows(&st), s);
        }
        // Inline picker searches sit above their list, like every other
        // search prompt.
        if matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray) && st.filter.active {
            // search rows use the terminal's default foreground (white);
            // only the help row below the views is dim gray
            b.push_str(&clip_display(
                &search_input_line("search: ", &st.filter.ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let notice_lines = self.notice_lines();
        let frame = self.list_frame(&st, &rows, &summary, notice_lines.len());
        let h = frame.rows_height;
        let bottom = frame.bottom;
        let desc_lines = frame.desc_lines;
        let summary_lines = frame.summary_lines;
        let status = frame.status;
        let title_width = frame.title_width;
        let write_lines = frame.write_lines;
        let write_sugs = frame.write_sugs;
        let cmd_line = frame.cmd_line;
        let cmd_sugs = frame.cmd_sugs;
        self.write_rows(&mut b, &st, &rows, h, title_width);
        if self.hide_empty {
            b.push_str(&clip_display(
                &format!("  {}", style::faint("[empty hidden]")),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        // pin the bottom block above the hint bar
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1).saturating_sub(bottom) {
            b.push('\n');
            lines += 1;
        }
        for line in &desc_lines {
            b.push_str(&clip_display(
                &desc_line_view(line),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        for line in &summary_lines {
            b.push_str(&clip_display(
                &style::faint(line),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        for line in &status {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        for line in &notice_lines {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        for line in &write_sugs {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        for line in &write_lines {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        for line in &cmd_sugs {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if let Some(line) = &cmd_line {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        b.push_str(&self.help_line());
        b
    }

    /// The search branch of listView (search overlay is open and renders
    /// inline in the list frame).
    pub(super) fn list_view_search(&mut self, s: &SearchState) -> String {
        let mut b = String::new();
        let Some(st) = self.top().cloned() else {
            return b;
        };
        let (title_text, _summary) = self.menu_title(&st);
        let mut title = style::bold(&title_text);
        if let Some((name, is_ref)) = anchor_role(self.doc(), &st.path) {
            if is_ref {
                title += &format!(": {}", style::anchor(&format!("*{name}")));
            } else {
                title += &format!(": {}", style::anchor(&format!("&{name}")));
            }
        }
        let merges = self.merge_names(&st.path);
        if !merges.is_empty() {
            title += &format!(" {}", style::anchor(&merges));
        }
        let tail = if self.dirty() {
            format!(" {}", style::diff("●"))
        } else {
            String::new()
        };
        b.push_str(&format!(
            "{}{tail}",
            clip_keep_tail(&title, self.width.saturating_sub(1), display_width(&tail))
        ));
        b.push('\n');
        b.push_str(&clip_display(
            &style::faint(&clip_runes(&self.breadcrumb(), self.width)),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        self.render_search_body(b, 2, s)
    }

    pub(super) fn render_search_body(
        &self,
        mut b: String,
        header: usize,
        s: &SearchState,
    ) -> String {
        let mut inputs = 1usize;
        if s.replace {
            inputs += 1;
        }
        let status = self.status_lines();
        let search_focused = !s.replace || !s.replace_focused;
        let line_width = self.width.saturating_sub(1);
        b.push_str(&clip_display(
            &search_field_line("search", &s.ti, search_focused, line_width),
            line_width,
        ));
        b.push('\n');
        if s.replace {
            b.push_str(&clip_display(
                &search_field_line("replace", &s.rti, s.replace_focused, line_width),
                line_width,
            ));
            b.push('\n');
        }
        let mut h = self
            .height
            .saturating_sub(header + inputs + status.len() + 1);
        if h < 1 {
            h = 1;
        }
        self.write_search_rows(&mut b, s, h);
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1).saturating_sub(status.len()) {
            b.push('\n');
            lines += 1;
        }
        let case_txt = if s.fold_case {
            "case-insensitive"
        } else {
            "case-sensitive"
        };
        if !s.re_err.is_empty() {
            b.push_str(&clip_display(
                &style::fg1(&clip_runes(
                    &format!("invalid regex: {} · esc close", s.re_err),
                    self.width,
                )),
                self.width.saturating_sub(1),
            ));
            return b;
        }
        let mode_txt = if s.regex { "regex" } else { "literal" };
        for line in &status {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        let help = if s.replace {
            format!(
                "↑/↓ ctrl+j/k · pgup/dn · enter jump/replace · ctrl+r all · tab focus · alt+i {} · alt+x {} · esc",
                case_txt, mode_txt
            )
        } else {
            format!(
                "↑/↓ ctrl+j/k · pgup/pgdn page · enter jump · tab replace · alt+i {} · alt+x {} · esc close",
                case_txt, mode_txt
            )
        };
        b.push_str(&clip_display(
            &style::help(&help),
            self.width.saturating_sub(1),
        ));
        b
    }

    /// writeSearchRows renders the visible slice of search results.
    pub(super) fn write_search_rows(&self, b: &mut String, s: &SearchState, height: usize) {
        let height = height.max(1);
        let mut cursor = s.cursor;
        if cursor >= s.results.len() {
            cursor = s.results.len().saturating_sub(1);
        }
        let mut off = cursor as i64 - height as i64 + 1;
        if off < 0 {
            off = 0;
        }
        let off = off as usize;
        for i in off..s.results.len().min(off + height) {
            let mut line = format!("{}{}", cursor_prefix(i == cursor), s.results[i].label);
            if i == cursor {
                line = style::sel(&line);
            }
            self.write_cut(b, &line);
        }
    }

    /// writeCut clips s to the terminal width line by line.
    pub(super) fn write_cut(&self, b: &mut String, s: &str) {
        for line in s.split('\n') {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
    }

    /// writeRows renders the visible slice of rows (scroll computed
    /// read-only around the persisted offset).
    /// edit_sugs_plan places the inline edit completion window. Below is
    /// preferred; when the window does not fit there and the rows above have
    /// more room, the same rectangle is drawn over them instead.
    fn edit_sugs_plan(
        &self,
        rows: &[Row],
        offset: usize,
        end: usize,
        cursor: usize,
        height: usize,
        title_width: usize,
    ) -> Option<EditSugs> {
        let Some(Overlay::Edit(e)) = &self.overlay else {
            return None;
        };
        if e.sugs.is_empty() || cursor < offset || cursor >= end {
            return None;
        }
        let row = &rows[cursor];
        if !self.inline_edit_row(row, e) {
            return None;
        }
        let rel = cursor - offset;
        let extra_lines = self.row_line(row, true, title_width).matches('\n').count();
        let below = height.saturating_sub(rel + 1 + extra_lines);
        let above = rel;
        let needed = e.sugs.len().min(CMD_SUG_WINDOW);
        let use_above = below < needed && above > below;
        let capacity = if use_above { above } else { below }.min(CMD_SUG_WINDOW);
        // Full candidate rows anchor at the input text start: the edited
        // value is the prefix being matched, and the rectangle must stay
        // put while the user types. `…tail` rows anchor at the caret
        // recorded when the list was built instead, keeping the visible
        // tails next to the input position.
        let title = self.title_col(&self.row_title_text(row), title_width);
        let text_left = 2 + display_width(&title);
        let (_, _, shift) = ti_window(&e.ti, self.edit_input_avail(&title));
        let line_right = self.width.saturating_sub(1);
        // When the rectangle would run past the right edge, slide it left so
        // every candidate stays on screen — the horizontal counterpart of
        // the rows-above fallback above.
        let rect =
            Self::suggestion_rect_width(&e.sugs, e.sug_idx, &e.sug_prefix, capacity, line_right);
        let base = if Self::sug_strip_active(&e.sugs, e.sug_idx, &e.sug_prefix, capacity) {
            e.sug_col
        } else {
            0
        };
        let left = (text_left + base.saturating_sub(shift)).min(line_right.saturating_sub(rect));
        let lines = Self::suggestion_overlay_lines(
            &e.sugs,
            e.sug_idx,
            &e.sug_prefix,
            capacity,
            left,
            line_right,
        );
        if lines.is_empty() {
            return None;
        }
        Some(EditSugs {
            cursor_idx: cursor,
            left,
            above_start: use_above.then(|| cursor - lines.len()),
            lines,
        })
    }

    pub(super) fn write_rows(
        &self,
        b: &mut String,
        st: &MenuState,
        rows: &[Row],
        height: usize,
        title_width: usize,
    ) {
        let height = height.max(1);
        let cursor = st.cursor.min(rows.len().saturating_sub(1));
        let mut offset = st.offset;
        if offset > cursor {
            offset = cursor;
        }
        if cursor >= offset + height {
            offset = cursor + 1 - height;
        }
        let max_off = rows.len().saturating_sub(height);
        if offset > max_off {
            offset = max_off;
        }
        let end = rows.len().min(offset + height);
        let edit_sugs = self.edit_sugs_plan(rows, offset, end, cursor, height, title_width);
        let mut i = offset;
        while i < end {
            // A window that did not fit below the edited row covers the rows
            // above it instead; keep their left-hand content visible.
            if let Some(p) = &edit_sugs
                && let Some(start) = p.above_start
                && i >= start
                && i < p.cursor_idx
            {
                let k = i - start;
                let prefix =
                    Self::overlay_prefix(&self.row_line(&rows[i], false, title_width), p.left);
                self.write_cut(b, &format!("{prefix}{}", p.lines[k]));
                i += 1;
                continue;
            }
            let row = &rows[i];
            let line = self.row_line(row, i == cursor, title_width);
            self.write_cut(b, &line);
            let is_cursor = i == cursor;
            let extra_lines = line.matches('\n').count();
            i += 1;
            if !is_cursor {
                continue;
            }
            let slots_below =
                height.saturating_sub(cursor.saturating_sub(offset) + 1 + extra_lines);
            // The & / * input and its candidates overlay the rows below the
            // cursor instead of inserting new lines.
            if let Some(Overlay::Anc(s)) = &self.overlay {
                let lines = self.anchor_input_lines(s);
                let input_lines = lines.matches('\n').count() + 1;
                self.write_cut(b, &lines);
                i += input_lines.min(end.saturating_sub(i));
                let max = slots_below.saturating_sub(input_lines).min(CMD_SUG_WINDOW);
                let prompt = if s.mode == AnchorMode::RefSet {
                    "  reference: "
                } else {
                    "  anchor: "
                };
                let left = display_width(prompt);
                let sugs = if s.mode == AnchorMode::RefSet && !s.sugs.is_empty() {
                    // Full candidate rows anchor at the input text start;
                    // `…tail` rows anchor at the recorded caret.
                    let base = if Self::sug_strip_active(&s.sugs, s.sug, &s.sug_prefix, max) {
                        s.sug_col
                    } else {
                        0
                    };
                    Self::suggestion_overlay_lines(
                        &s.sugs,
                        s.sug,
                        &s.sug_prefix,
                        max,
                        left + base,
                        self.width.saturating_sub(1),
                    )
                } else {
                    Vec::new()
                };
                let base = i;
                for (k, rect) in sugs.iter().enumerate() {
                    let prefix = if base + k < end {
                        Self::overlay_prefix(
                            &self.row_line(&rows[base + k], false, title_width),
                            left,
                        )
                    } else {
                        " ".repeat(left)
                    };
                    self.write_cut(b, &format!("{prefix}{rect}"));
                }
                i += sugs.len().min(end.saturating_sub(i));
            }
            // Inline scalar edit: the candidate overlay covers the rows
            // below the edited row (and any free row slots), so the menu
            // never shifts. When it was placed above instead, nothing goes
            // below.
            if let Some(p) = &edit_sugs
                && p.cursor_idx + 1 == i
                && p.above_start.is_none()
            {
                let base = i;
                for (k, rect) in p.lines.iter().enumerate() {
                    let prefix = if base + k < end {
                        Self::overlay_prefix(
                            &self.row_line(&rows[base + k], false, title_width),
                            p.left,
                        )
                    } else {
                        " ".repeat(p.left)
                    };
                    self.write_cut(b, &format!("{prefix}{rect}"));
                }
                i += p.lines.len().min(end.saturating_sub(i));
            }
        }
        // in-place input for a brand-new array item on its own row
        if let Some(Overlay::Edit(e)) = &self.overlay {
            if e.is_new && !self.has_inline_edit_row(rows, e) {
                let line =
                    self.edit_lines(e, &self.title_col(&self.new_item_title(st), title_width));
                self.write_cut(b, &line);
            }
        }
        // key-name input for a schema-less member (a)
        if let Some(Overlay::Add(a)) = &self.overlay {
            let title = self.title_col("New key", title_width);
            let avail = input_avail(
                self.width.saturating_sub(1),
                display_width(cursor_prefix(true)) + display_width(&title),
            );
            let line = format!(
                "{}{}{}",
                cursor_prefix(true),
                title,
                ti_view(&a.ti, avail).1
            );
            self.write_cut(b, &line);
            if !a.err.is_empty() {
                self.write_cut(b, &style::fg1(&format!("  {}", a.err)));
            }
        }
    }

    /// write_input_lines renders the W write-to-path input in the bottom
    /// block (above the help bar, so it never scrolls with the menu rows).
    /// The line has no "> " cursor marker: it is the only focused input
    /// there.
    pub(super) fn write_input_lines(&self, title_width: usize) -> Vec<String> {
        let Some(Overlay::Write(w)) = &self.overlay else {
            return Vec::new();
        };
        let mut lines = vec![format!(
            "{}{}",
            self.title_col("Write to", title_width),
            ti_view(
                &w.ti,
                input_avail(
                    self.width.saturating_sub(1),
                    display_width(&self.title_col("Write to", title_width)),
                )
            )
            .1
        )];
        if !w.err.is_empty() {
            lines.push(style::fg1(&format!("  {}", w.err)));
        }
        lines
    }

    /// toggle_desc implements c: expand the focused description inline, or
    /// open the dedicated scrolling page when it cannot fit one screen.
    pub(super) fn toggle_desc(&mut self, r: &Row, rows: &[Row]) {
        let Some(st) = self.top().cloned() else {
            return;
        };
        let lines = self.desc_lines_for(&st, rows, false);
        if lines.is_empty() {
            return;
        }
        if lines.len() > self.desc_page_capacity() {
            self.desc_expanded = false;
            let title = if !r.label.is_empty() {
                r.label.clone()
            } else if let Some(f) = r.fn_ {
                self.fs().schema.nodes[f as usize].title.clone()
            } else {
                String::new()
            };
            self.overlay = Some(Overlay::Comment(CommentState {
                title,
                lines,
                scroll: 0,
            }));
        } else {
            self.desc_expanded = !self.desc_expanded;
        }
    }

    /// desc_page_capacity is how many wrapped description lines fit while
    /// keeping the title, breadcrumb, one menu row and the help bar.
    pub(super) fn desc_page_capacity(&self) -> usize {
        let status = self.status_lines().len();
        let notice = self.notice_lines().len();
        let hidden = usize::from(self.hide_empty);
        self.height
            .saturating_sub(4 + status + notice + hidden)
            .max(1)
    }

    /// keyComment drives the dedicated long-comment page.
    pub fn key_comment(&mut self, msg: Msg, mut s: CommentState) -> Option<Overlay> {
        let page = self.height.saturating_sub(2).max(1);
        let max_scroll = s.lines.len().saturating_sub(page);
        match msg.key_str().as_str() {
            "up" | "k" => s.scroll = s.scroll.saturating_sub(1),
            "down" | "j" => s.scroll = (s.scroll + 1).min(max_scroll),
            "pgdown" | "ctrl+f" => s.scroll = (s.scroll + page).min(max_scroll),
            "pgup" | "ctrl+b" => s.scroll = s.scroll.saturating_sub(page),
            "c" | "esc" => return None,
            _ => {}
        }
        Some(Overlay::Comment(s))
    }

    /// key_shell_out drives the scrolling page for a bare command's
    /// multi-line output: j/k and the arrows scroll one line, PgUp/PgDn and
    /// ctrl+b/ctrl+f page, q/esc closes.
    pub fn key_shell_out(&mut self, msg: Msg, mut s: ShellOutState) -> Option<Overlay> {
        let page = self.height.saturating_sub(2).max(1);
        let max_scroll = s.lines.len().saturating_sub(page);
        match msg.key_str().as_str() {
            "up" | "k" => s.scroll = s.scroll.saturating_sub(1),
            "down" | "j" => s.scroll = (s.scroll + 1).min(max_scroll),
            "pgdown" | "ctrl+f" => s.scroll = (s.scroll + page).min(max_scroll),
            "pgup" | "ctrl+b" => s.scroll = s.scroll.saturating_sub(page),
            "q" | "esc" => return None,
            _ => {}
        }
        Some(Overlay::ShellOut(s))
    }

    /// key_comment_edit drives the multiline document-comment editor.
    /// ctrl+s writes the comment into the session metadata (a later save emits it);
    /// the document body is unchanged.
    pub fn key_comment_edit(&mut self, msg: Msg, mut s: CommentEditState) -> Option<Overlay> {
        match msg.key_str().as_str() {
            "ctrl+s" => {
                let text = s.ta.value();
                if self.doc_comment_at(&s.path).unwrap_or_default() == text {
                    return None; // no-op: keep the stored formatting/markers
                }
                let prefix = self.fs().codec.comment_prefix();
                self.fs_mut().meta.set_comment_text(&s.path, &text, prefix);
                self.fs_mut().comments_dirty = true;
                return None;
            }
            "esc" => return None,
            _ => {}
        }
        if let Some(k) = msg_key(&msg) {
            s.ta.handle_key(k);
        }
        Some(Overlay::CommentEdit(s))
    }

    /// edit_comment_row implements C: open the multiline editor for the
    /// focused key's document comment. A key without a comment starts with an
    /// empty buffer, so the same key adds one; applying empty text removes it.
    pub(super) fn edit_comment_row(&mut self, r: &Row) {
        if !matches!(
            r.kind,
            RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Toggle | RowKind::Variant
        ) {
            return;
        }
        let path = r.path.clone();
        if self.doc().get(&path).is_none() {
            self.notice = "comment: key has no value in the document".to_string();
            return;
        }
        let text = self.doc_comment_at(&path).unwrap_or_default();
        let title = if !r.label.is_empty() {
            r.label.clone()
        } else if let Some(f) = r.fn_ {
            self.fs().schema.nodes[f as usize].title.clone()
        } else {
            String::new()
        };
        self.open_comment_edit(&path, &title, &text);
    }

    /// open_comment_edit arms the multiline comment editor.
    pub(super) fn open_comment_edit(&mut self, path: &[PathSeg], title: &str, text: &str) {
        self.overlay = Some(self.comment_edit_overlay(path, title, text));
    }

    /// comment_edit_overlay builds the comment editor state (the overlay
    /// router assigns the returned surface).
    pub(super) fn comment_edit_overlay(
        &self,
        path: &[PathSeg],
        title: &str,
        text: &str,
    ) -> Overlay {
        let mut ta = TextArea::new();
        let w = self.width.saturating_sub(4).clamp(20, 100);
        let h = self.height.saturating_sub(6).clamp(3, 24);
        ta.set_width(w as u16);
        ta.set_height(h as u16);
        ta.set_value(text);
        Overlay::CommentEdit(CommentEditState {
            path: path.to_vec(),
            title: title.to_string(),
            ta,
        })
    }

    /// doc_comment_at reads the session's document comment for one path.
    pub(super) fn doc_comment_at(&self, path: &[PathSeg]) -> Option<String> {
        self.fs().meta.comment_text(path)
    }

    /// row_comment_text returns the document comment of a member row (None
    /// for synthetic rows such as add/save/raw and for paths without one).
    pub(super) fn row_comment_text(&self, r: &Row) -> Option<String> {
        if !matches!(
            r.kind,
            RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Toggle | RowKind::Variant
        ) || self.doc().get(&r.path).is_none()
        {
            return None;
        }
        self.doc_comment_at(&r.path)
    }

    /// focused_sources returns the schema help and document comment the
    /// description panel should show, after applying `C`'s source mode.
    pub(super) fn focused_sources(&self, st: &MenuState, rows: &[Row]) -> (String, String) {
        if rows.is_empty() || st.cursor >= rows.len() {
            return (String::new(), String::new());
        }
        let r = &rows[st.cursor];
        let mut schema = String::new();
        if let Some(f) = r.fn_ {
            schema = self.fs().schema.nodes[f as usize].desc.clone();
        }
        if schema.is_empty() {
            if let Some(v) = r.variant {
                schema = self.fs().schema.nodes[v as usize].desc.clone();
            }
        }
        let comment = self.row_comment_text(r).unwrap_or_default();
        (schema, comment)
    }

    /// desc_lines_for wraps the focused sources into colored lines. When
    /// `folded`, each source independently keeps at most
    /// [`DESC_BLOCK_PREVIEW_LINES`] lines and ends with an emphasized `…`.
    pub(super) fn desc_lines_for(
        &self,
        st: &MenuState,
        rows: &[Row],
        folded: bool,
    ) -> Vec<DescLine> {
        let (schema, comment) = self.focused_sources(st, rows);
        let width = self.width.saturating_sub(1);
        let mut out = Vec::new();
        for (source, raw) in [(DescSource::Schema, schema), (DescSource::Comment, comment)] {
            if raw.trim().is_empty() {
                continue;
            }
            let mut lines = wrap_text(&raw, width);
            let cut = folded && lines.len() > DESC_BLOCK_PREVIEW_LINES;
            if cut {
                lines = fold_desc_block(lines, width);
            }
            let last = lines.len().saturating_sub(1);
            out.extend(lines.into_iter().enumerate().map(|(i, text)| DescLine {
                source,
                text,
                folded: cut && i == last,
            }));
        }
        out
    }

    /// edit_input_avail is the inline edit input's viewport width: the
    /// clipped line minus the "> " marker and the title column, keeping the
    /// caret cell and one margin column visible. edit_sugs_plan uses the
    /// same budget so the candidate anchor matches the rendered window.
    pub(super) fn edit_input_avail(&self, left: &str) -> usize {
        input_avail(
            self.width.saturating_sub(1),
            display_width(cursor_prefix(true)) + display_width(left),
        )
    }

    /// editLines renders the in-place input line plus error. Long values
    /// scroll horizontally so the caret stays visible on a narrow terminal.
    pub(super) fn edit_lines(&self, e: &EditState, left: &str) -> String {
        let mut line = format!(
            "{}{}{}",
            cursor_prefix(true),
            left,
            ti_view(&e.ti, self.edit_input_avail(left)).1
        );
        if !e.err.is_empty() {
            line += &format!("\n{}", style::fg1(&format!("  {}", e.err)));
        }
        line
    }

    /// anchorInputLines renders the & / * name input under the cursor row.
    pub(super) fn anchor_input_lines(&self, s: &AnchorState) -> String {
        let prompt = if s.mode == AnchorMode::RefSet {
            "  reference: "
        } else {
            "  anchor: "
        };
        let mut lines = vec![format!(
            "{}{}",
            style::help(prompt),
            ti_view(
                &s.ti,
                input_avail(self.width.saturating_sub(1), display_width(prompt))
            )
            .1
        )];
        if !s.err.is_empty() {
            lines.push(style::fg1(&format!("  {}", s.err)));
        }
        lines.join("\n")
    }

    /// mergeInputLines renders the merge-keys input under the title.
    pub(super) fn merge_input_lines(&self, s: &MergeState) -> String {
        let mut lines = vec![format!(
            "{}{}",
            style::help("merge keys: "),
            ti_view(
                &s.ti,
                input_avail(self.width.saturating_sub(1), display_width("merge keys: "))
            )
            .1
        )];
        if !merge_filtered(s).is_empty() {
            let cands = merge_filtered(s);
            let parts: Vec<String> = cands
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    if i as i64 == s.sug {
                        style::accent(c)
                    } else {
                        c.clone()
                    }
                })
                .collect();
            lines.push(format!("{}{}", style::help("tab: "), parts.join(" · ")));
        }
        if !s.err.is_empty() {
            lines.push(style::fg1(&s.err));
        }
        lines.join("\n")
    }

    /// menuTitleColWidth returns the adaptive title column width.
    pub(super) fn menu_title_col_width(&self, rows: &[Row]) -> usize {
        let mut width = crate::ui::menu::TITLE_COL_MIN;
        for r in rows {
            let n = match r.kind {
                RowKind::Field | RowKind::Drill => {
                    if r.fn_.is_none() {
                        continue;
                    }
                    let mut w = display_width(&self.row_title_text(r));
                    if self.field_required(r) {
                        w += 1; // required marker
                    }
                    w
                }
                RowKind::Unknown => display_width(&r.label),
                RowKind::Variant => {
                    let title = r
                        .variant
                        .map(|v| display_title(&self.fs().schema, v))
                        .unwrap_or_default();
                    display_width("○ ") + display_width(&title)
                }
                _ => continue,
            };
            if n > width {
                width = n;
            }
        }
        width.min(crate::ui::menu::TITLE_COL_MAX)
    }

    /// titleCol pads the left column to width cells.
    pub(super) fn title_col(&self, left: &str, width: usize) -> String {
        let w = display_width(left);
        if w < width {
            format!("{left}{}", " ".repeat(width - w + 1))
        } else {
            format!("{left} ")
        }
    }

    /// rowTitleText is the left-column label of a field or drill row.
    pub(super) fn row_title_text(&self, r: &Row) -> String {
        if !r.label.is_empty() {
            return r.label.clone();
        }
        match r.fn_ {
            Some(f) => self.fs().schema.nodes[f as usize].title.clone(),
            None => String::new(),
        }
    }

    /// newItemTitle labels the standalone input of the append-only flow.
    pub(super) fn new_item_title(&self, st: &MenuState) -> String {
        let mut key = self.fs().schema.nodes[st.node as usize].key.clone();
        if let Some(PathSeg::Key(s)) = st.path.last() {
            key = s.clone();
        }
        format!("{}[{}]", key, self.doc().array_len(&st.path))
    }

    /// inlineEditRow reports whether the active editor renders on r.
    pub(super) fn inline_edit_row(&self, r: &Row, e: &EditState) -> bool {
        if r.path != e.path {
            return false;
        }
        if r.kind == RowKind::Unknown {
            return true;
        }
        match (r.kind, r.fn_, e.fn_) {
            (RowKind::Field, Some(_), Some(_)) if r.fn_ == e.fn_ => true,
            (RowKind::Field, Some(f), _) | (RowKind::Drill, Some(f), _) => {
                self.fs().schema.nodes[f as usize].kind == Kind::OneOf
                    && e.synth.is_none()
                    && self.variant_contains_edit(f, e)
            }
            _ => false,
        }
    }

    pub(super) fn variant_contains_edit(&self, one_of: FormNodeId, e: &EditState) -> bool {
        let Some(target) = e.fn_ else { return false };
        variant_contains(&self.fs().schema, one_of, target)
    }

    pub(super) fn has_inline_edit_row(&self, rows: &[Row], e: &EditState) -> bool {
        rows.iter().any(|r| self.inline_edit_row(r, e))
    }

    /// rowLine renders one menu line (Go rowLine).
    /// drill_marker reports whether the row shows the "--->" drill marker.
    /// A oneOf field whose current variant resolved to a filled-in leaf
    /// value has nothing to drill into; the marker is kept only for mapping
    /// variants (objects/maps/arrays) and undetermined variants, where
    /// entering opens a real submenu.
    pub(super) fn drill_marker(&self, r: &Row) -> bool {
        if r.kind == RowKind::Issues {
            return true;
        }
        if r.kind != RowKind::Drill {
            return false;
        }
        let Some(f) = r.fn_ else { return true };
        let schema = &self.fs().schema;
        if schema.nodes[f as usize].kind != Kind::OneOf {
            return true;
        }
        let doc = self.doc();
        let Some(v) = doc.get(&r.path) else {
            return true;
        };
        if is_null_val(doc, v) {
            return true;
        }
        let Some(leaf) = matched_leaf_variant(schema, f, doc, Some(v)) else {
            return true;
        };
        matches!(
            schema.nodes[leaf as usize].kind,
            Kind::Object | Kind::Map | Kind::Array
        )
    }

    pub(super) fn row_line(&self, r: &Row, cursor: bool, title_width: usize) -> String {
        let st = self.top();
        // in-place input replaces the edited row
        if let Some(Overlay::Edit(e)) = &self.overlay {
            if self.inline_edit_row(r, e) {
                return self.edit_lines(e, &self.title_col(&self.row_title_text(r), title_width));
            }
        }
        let sel = self.row_selected(r);
        let visual = st.map(|s| s.visual).unwrap_or(false);
        let left;
        let mut right = String::new();
        match r.kind {
            RowKind::Variant => {
                let v = self.doc().get(&r.path);
                let cur = st.and_then(|s| {
                    let cur = current_variant(&self.fs().schema, s.node, self.doc(), v);
                    if cur < 0 {
                        None
                    } else {
                        Some(self.fs().schema.nodes[s.node as usize].variants[cur as usize])
                    }
                });
                let mut mark = "○";
                let mut mark_styled = style::faint(mark);
                if cur == r.variant {
                    mark = "●";
                    mark_styled = style::accent(mark);
                }
                let mut l = mark_styled;
                if let Some(vn) = r.variant {
                    // untitled variants stand in their structural signature,
                    // matching the any-type picker's name column instead of
                    // leaving the title cell blank
                    let title = display_title(&self.fs().schema, vn);
                    if !title.is_empty() {
                        l += &format!(" {title}");
                    }
                }
                left = l;
                // a titled variant keeps the signature in the summary
                // column; the untitled case already shows it as the title
                right = match r.variant {
                    Some(vn) if !self.fs().schema.nodes[vn as usize].title.is_empty() => {
                        style::faint(&variant_signature(&self.fs().schema, r.variant))
                    }
                    _ => String::new(),
                };
                if mark == "●" {
                    if let Some(v) = self.doc().get(&r.path) {
                        if !is_null_val(self.doc(), v) {
                            if let Some(vn) = r.variant {
                                let s = variant_value_summary(
                                    &self.fs().schema,
                                    vn,
                                    self.doc(),
                                    Some(v),
                                );
                                if !s.is_empty() {
                                    right += &format!(" {}", style::val_enum(&s));
                                }
                            }
                        }
                    }
                }
            }
            RowKind::Toggle => {
                let on = self.enum_member_on(r);
                let (box_, box_style) = if on {
                    ("[x]", style::accent("[x]"))
                } else {
                    ("[ ]", style::faint("[ ]"))
                };
                let label = r
                    .fn_
                    .and_then(|f| self.fs().schema.nodes[f as usize].items)
                    .and_then(|it| {
                        self.fs().schema.nodes[it as usize]
                            .enum_
                            .get(r.idx.max(0) as usize)
                    })
                    .map(|o| o.display.clone())
                    .unwrap_or_default();
                left = format!("{} {}", box_style, label);
                let _ = box_;
            }
            RowKind::Add | RowKind::Expand => {
                left = style::accent(&r.label);
            }
            RowKind::NoItems => {
                // An empty object menu's "< Add Key >" is an action row and
                // carries the accent color; the oneOf/enum-array placeholders
                // are informational and stay faint.
                let placeholder = matches!(
                    st.map(|s| s.kind),
                    Some(MenuKind::OneOf) | Some(MenuKind::EnumArray)
                );
                left = if placeholder {
                    style::faint(&r.label)
                } else {
                    style::accent(&r.label)
                };
            }
            RowKind::Unknown => {
                let title = r.label.clone();
                left = if sel && cursor {
                    style::bold(&style::fg9(&title))
                } else if sel {
                    style::fg9(&title)
                } else {
                    style::fg3(&title)
                };
                let v = self.doc().get(&r.path);
                right = format!(
                    "{}{}{}",
                    self.merge_badge(r),
                    self.anchor_badge(&r.path),
                    self.raw_value_styled(
                        v,
                        &raw_value_summary(&self.fs().schema, self.doc(), &r.path)
                    )
                );
            }
            RowKind::Issues => {
                left = self.issues_severity().render_str(&r.label);
            }
            _ => {
                // rowField / rowDrill
                let mut title = self.row_title_text(r);
                if self.field_required(r) {
                    title += &style::fg1("*");
                }
                left = if sel && cursor {
                    style::bold(&style::fg9(&title))
                } else if sel {
                    style::fg9(&title)
                } else {
                    title
                };
                let summ = match r.fn_ {
                    Some(f) => summary(&self.ctx(), f, &r.path),
                    None => String::new(),
                };
                right = format!(
                    "{}{}{}",
                    self.merge_badge(r),
                    self.anchor_badge(&r.path),
                    (self.value_style(r)).render_str(&summ)
                );
            }
        }
        let content = match r.kind {
            RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Variant => {
                format!("{}{}", self.title_col(&left, title_width), right)
            }
            _ => format!("{left}{right}"),
        };
        let mut prefix = cursor_prefix(cursor).to_string();
        if sel && (!cursor || visual) {
            prefix = style::fg9("> "); // selected rows carry a red > marker
        }
        let mut body = format!("{prefix}{content}");
        if r.kind == RowKind::Unknown && !r.in_schema && !self.fs().schema.schema_less {
            body += &format!(" {}", style::faint("(not in schema)"));
        }
        if self.drill_marker(r) {
            body += &format!(" {}", style::accent("--->"));
        }
        // The change dot and the severity glyph are status markers: on a
        // narrow terminal the body is cut (marked with …) so they stay
        // visible at the end of the row.
        let mut tail = String::new();
        if matches!(r.kind, RowKind::Field | RowKind::Drill | RowKind::Unknown) {
            if self.has_err(&r.path) {
                tail = format!(" {}", style::fg1(ERR));
            } else if (r.kind == RowKind::Unknown && !r.in_schema && !self.fs().schema.schema_less)
                || self.has_warn(&r.path)
            {
                tail = format!(" {}", style::fg3(WARN));
            }
            if self.changed(&r.path) {
                tail += &format!(" {}", style::diff("●"));
            }
        }
        let line = format!(
            "{}{tail}",
            clip_keep_tail(&body, self.width.saturating_sub(1), display_width(&tail))
        );
        if cursor {
            return style::sel(&line);
        }
        line
    }

    /// rawValueStyle colors a schema-less value by its JSON type.
    pub(super) fn raw_value_styled(&self, v: Option<NodeId>, s: &str) -> String {
        let styled = match v.map(|n| self.doc().value(n)) {
            None => style::faint(s),
            Some(Value::Null) => style::faint(s),
            Some(Value::Str(_)) => style::val_str(s),
            Some(Value::Num(_)) => style::val_num(s),
            Some(Value::Bool(_)) => style::val_bool(s),
            Some(_) => style::bold(s),
        };
        styled
    }

    pub(super) fn has_err(&self, path: &[PathSeg]) -> bool {
        self.fs().err_paths.iter().any(|p| is_path_prefix(path, p))
    }

    pub(super) fn has_warn(&self, path: &[PathSeg]) -> bool {
        self.fs().warn_paths.iter().any(|p| is_path_prefix(path, p))
    }

    pub(super) fn issues_severity(&self) -> Sev {
        if self.fs().issues_now.iter().any(|it| !it.warn) {
            Sev::Err
        } else {
            Sev::Warn
        }
    }

    pub(super) fn severity(&self, it: &Issue) -> String {
        if it.warn {
            style::fg3(&it.msg)
        } else {
            style::fg1(&it.msg)
        }
    }

    /// mergeNames renders the anchor list a mapping merges for its title.
    pub(super) fn merge_names(&self, path: &[PathSeg]) -> String {
        let Some(n) = self.doc().get(path) else {
            return String::new();
        };
        let mut parts: Vec<String> = Vec::new();
        for r in self.doc().merge_refs(n) {
            let anchor = self.doc().node(r).anchor.clone();
            if !anchor.is_empty() {
                parts.push(format!("*{anchor}"));
            }
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("<<: {}", parts.join(" "))
    }

    /// mergeBadge renders the "*anchor" provenance marker.
    pub(super) fn merge_badge(&self, r: &Row) -> String {
        if r.merged.is_empty() {
            return String::new();
        }
        format!("{} ", style::anchor(&format!("*{}", r.merged)))
    }

    /// anchorBadge renders the "&name" / "*name" marker.
    pub(super) fn anchor_badge(&self, path: &[PathSeg]) -> String {
        match anchor_role(self.doc(), path) {
            Some((name, true)) => format!("{} ", style::anchor(&format!("*{name}"))),
            Some((name, false)) => format!("{} ", style::anchor(&format!("&{name}"))),
            None => String::new(),
        }
    }

    /// fieldRequired reports whether the row's field is required in the
    /// current context (Go fieldRequired).
    pub(super) fn field_required(&self, r: &Row) -> bool {
        let Some(f) = r.fn_ else { return false };
        let mut required = self.fs().schema.nodes[f as usize].required;
        let Some(st) = self.top() else {
            return required;
        };
        let Some(variant_of) = st.variant_of else {
            return required;
        };
        let variants = dynamic_variants(&self.fs().schema, variant_of).unwrap_or_default();
        if variants.is_empty() {
            return required;
        }
        let key = self.fs().schema.nodes[f as usize].key.clone();
        let (_, defs) = variant_field_defs(&self.fs().schema, &variants);
        let Some(ds) = defs.get(&key) else {
            return required;
        };
        if ds.is_empty() {
            return required;
        }
        let val = self.doc().get(&st.path);
        if match_variant_list(&self.fs().schema, &variants, self.doc(), val).len() != 1 {
            // merged view: required only when every defining variant requires it
            required = true;
            for d in ds {
                if !self.fs().schema.nodes[*d as usize].required {
                    required = false;
                    break;
                }
            }
        }
        required
    }

    /// showsSchemaDefault reports whether the row previews fn.Default.
    pub(super) fn shows_schema_default(&self, f: Option<FormNodeId>, path: &[PathSeg]) -> bool {
        let Some(f) = f else { return false };
        let node = &self.fs().schema.nodes[f as usize];
        let Some(d) = node.default_ else { return false };
        if is_null_val(&self.fs().schema.doc, d) {
            return false;
        }
        is_null_opt(self.doc(), self.doc().get(path))
    }

    /// valueStyle picks the value-summary color.
    pub(super) fn value_style(&self, r: &Row) -> ValStyle {
        if self.shows_schema_default(r.fn_, &r.path) {
            return ValStyle::Default;
        }
        let Some(f) = r.fn_ else { return ValStyle::Dim };
        if is_null_opt(self.doc(), self.doc().get(&r.path)) {
            return ValStyle::Dim;
        }
        let mut eff = f;
        if self.fs().schema.nodes[f as usize].kind == Kind::OneOf {
            let v = self.doc().get(&r.path);
            if let Some(leaf) = matched_leaf_variant_pub(&self.fs().schema, f, self.doc(), v) {
                eff = leaf;
            }
        }
        match self.fs().schema.nodes[eff as usize].kind {
            Kind::Enum => ValStyle::Enum,
            Kind::Boolean => ValStyle::Bool,
            Kind::Object | Kind::Array | Kind::Map | Kind::OneOf => ValStyle::Obj,
            _ => match self.doc().get(&r.path).map(|n| self.doc().value(n)) {
                Some(Value::Num(_)) => ValStyle::Num,
                Some(Value::Bool(_)) => ValStyle::Bool,
                _ => ValStyle::Str,
            },
        }
    }

    /// enumMemberOn reports whether toggle row r's option is in the doc.
    pub(super) fn enum_member_on(&self, r: &Row) -> bool {
        let Some(cur) = self.doc().get(&r.path) else {
            return false;
        };
        let Value::Array(items) = self.doc().value(cur) else {
            return false;
        };
        let Some(f) = r.fn_ else { return false };
        let Some(it) = self.fs().schema.nodes[f as usize].items else {
            return false;
        };
        let Some(opt) = self.fs().schema.nodes[it as usize]
            .enum_
            .get(r.idx.max(0) as usize)
        else {
            return false;
        };
        items
            .iter()
            .any(|&i| self.doc().encode_value(i) == opt.encoded)
    }

    // --- overlay views -----------------------------------------------------

    pub(super) fn confirm_view(&self, c: &ConfirmState) -> String {
        let hint = if c.hint.is_empty() {
            "y/enter/space yes · n/esc no"
        } else {
            &c.hint
        };
        let b = format!(
            "{}\n",
            clip_display(&style::bold(&c.title), self.width.saturating_sub(1))
        );
        self.finish_with_hint(
            b,
            clip_display(&style::help(hint), self.width.saturating_sub(1)),
        )
    }

    pub(super) fn issues_view(&self, s: &IssuesState) -> String {
        let mut b = String::new();
        b.push_str(&clip_display(
            &style::bold(&format!(
                "{} Validation issues ({})",
                worst_issue_icon(&s.items),
                s.items.len()
            )),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        if s.items.is_empty() {
            b.push_str(&style::faint("( no issues )"));
            b.push('\n');
            return self.finish_with_hint(b, self.rec_hint("esc close"));
        }
        if s.filter.active {
            b.push_str(&clip_display(
                &search_input_line("search: ", &s.filter.ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let notice_lines = self.notice_lines();
        let height = self
            .height
            .saturating_sub(2 + usize::from(s.filter.active) + notice_lines.len())
            .max(1);
        let total = issue_indices(s).count();
        let cursor = s.cursor.min(total.saturating_sub(1));
        let off = (cursor as i64 - height as i64 + 1).max(0) as usize;
        for (i, idx) in issue_indices(s).skip(off).take(height).enumerate() {
            let it = &s.items[idx];
            let mut line = format!("  {}", self.severity(it));
            if off + i == cursor {
                line = style::sel(&format!("> {}", it.msg));
            }
            b.push_str(&clip_display(&line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if total == 0 {
            b.push_str(&style::faint("(no matches)"));
            b.push('\n');
        }
        for line in &notice_lines {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        let hint = if s.filter.active {
            "enter jump · ↑/↓ or tab move · esc cancel search"
        } else {
            "↑/↓ or ctrl+j/k move · pgup/pgdn page · enter jump · / search · esc close"
        };
        self.finish_with_hint(b, self.rec_hint(hint))
    }

    pub(super) fn ref_pick_view(&self, s: &RefPickState) -> String {
        let mut b = String::new();
        b.push_str(&clip_display(
            &style::bold(&format!("References to &{} ({})", s.name, s.paths.len())),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        if s.filter.active {
            b.push_str(&clip_display(
                &search_input_line("search: ", &s.filter.ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let height = self
            .height
            .saturating_sub(2 + usize::from(s.filter.active))
            .max(1);
        let total = self.ref_indices(s).count();
        let cursor = s.cursor.min(total.saturating_sub(1));
        let off = (cursor as i64 - height as i64 + 1).max(0) as usize;
        for (i, idx) in self.ref_indices(s).skip(off).take(height).enumerate() {
            let label = self.ref_label(&s.paths[idx]);
            let line = if off + i == cursor {
                style::sel(&format!("> {label}"))
            } else {
                format!("  {label}")
            };
            b.push_str(&clip_display(&line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if total == 0 {
            b.push_str(&style::faint("(no matches)"));
            b.push('\n');
        }
        let hint = if s.filter.active {
            "enter jump · ↑/↓ or tab move · esc cancel search"
        } else {
            "↑/↓ or ctrl+j/k move · pgup/pgdn page · enter jump · / search · esc close"
        };
        self.finish_with_hint(b, self.rec_hint(hint))
    }

    pub(super) fn errs_view(&self, s: &ErrsState) -> String {
        let mut b = String::new();
        b.push_str(&clip_display(
            &style::bold(&format!("{} Cannot save: validation errors", ERR)),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        if s.filter.active {
            b.push_str(&clip_display(
                &search_input_line("search: ", &s.filter.ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let height = self
            .height
            .saturating_sub(2 + usize::from(s.filter.active))
            .max(1);
        let total = err_indices(s).count();
        let cursor = s.cursor.min(total.saturating_sub(1));
        let off = (cursor as i64 - height as i64 + 1).max(0) as usize;
        for (i, idx) in err_indices(s).skip(off).take(height).enumerate() {
            let line = if off + i == cursor {
                style::sel(&format!("> {}", s.lines[idx]))
            } else {
                format!("  {}", s.lines[idx])
            };
            b.push_str(&clip_display(&line, self.width.saturating_sub(1)));
            b.push('\n');
        }
        if total == 0 {
            b.push_str(&style::faint("(no matches)"));
            b.push('\n');
        }
        let hint = if s.filter.active {
            "enter jump · ↑/↓ or tab move · esc cancel search"
        } else if self.opts().force {
            "pgup/pgdn page · enter jump · / search · y save anyway · n back"
        } else {
            "pgup/pgdn page · enter jump · / search · fix fields and save again, or rerun with --force"
        };
        self.finish_with_hint(
            b,
            clip_display(&style::help(hint), self.width.saturating_sub(1)),
        )
    }

    pub(super) fn comment_view(&self, s: &CommentState) -> String {
        let mut b = String::new();
        let title = if s.title.is_empty() {
            "Comment".to_string()
        } else {
            format!("Comment: {}", s.title)
        };
        b.push_str(&clip_display(
            &style::bold(&title),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        let page = self.height.saturating_sub(2).max(1);
        let max_scroll = s.lines.len().saturating_sub(page);
        let off = s.scroll.min(max_scroll);
        for line in s.lines.iter().skip(off).take(page) {
            b.push_str(&clip_display(
                &desc_line_view(line),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1) {
            b.push('\n');
            lines += 1;
        }
        b.push_str(&self.rec_hint("j/k · PgUp/PgDn page · c/q/esc close"));
        b
    }

    /// shell_out_view renders the scrolling page for a bare command's
    /// multi-line stdout. The title and output block sit on the bottom of the
    /// screen, directly above the help row, so short output stays where shell
    /// output normally appears instead of floating at the top.
    pub(super) fn shell_out_view(&self, s: &ShellOutState) -> String {
        let width = self.width.saturating_sub(1);
        let title = if s.command.is_empty() {
            "Command output".to_string()
        } else {
            format!("! {}", s.command)
        };
        let page = self.height.saturating_sub(2).max(1);
        let max_scroll = s.lines.len().saturating_sub(page);
        let off = s.scroll.min(max_scroll);
        let shown: Vec<&String> = s.lines.iter().skip(off).take(page).collect();
        // Blank rows above the block; a full page fills the area exactly.
        let block = 1 + shown.len();
        let above = self.height.saturating_sub(1);
        let mut b = "\n".repeat(above.saturating_sub(block));
        b.push_str(&clip_display(&style::bold(&title), width));
        b.push('\n');
        for line in shown {
            b.push_str(&clip_display(line, width));
            b.push('\n');
        }
        b.push_str(&self.rec_hint("j/k · PgUp/PgDn page · q/esc close"));
        b
    }

    /// comment_edit_view renders the multiline document-comment editor.
    pub(super) fn comment_edit_view(&self, s: &CommentEditState) -> String {
        let title = if s.title.is_empty() {
            "Edit comment".to_string()
        } else {
            format!("Edit comment: {}", s.title)
        };
        let mut b = format!("{}\n", style::bold(&title));
        b.push_str(&ta_view(&s.ta));
        b.push('\n');
        self.finish_with_hint(b, style::help("ctrl+s apply · esc cancel"))
    }

    pub(super) fn text_view(&self, s: &TextState) -> String {
        let mut title = format!("Edit raw {}", self.fs().codec.label());
        if let Some(f) = s.fn_ {
            let t = self.fs().schema.nodes[f as usize].title.clone();
            if !t.is_empty() {
                title = t;
            }
        }
        let mut b = format!("{}\n", style::bold(&title));
        b.push_str(&ta_view(&s.ta));
        b.push('\n');
        self.finish_with_hint(b, style::help("ctrl+s apply · esc cancel"))
    }

    pub(super) fn file_pick_view(&self, pick: &FilePickState) -> String {
        let mut b = String::new();
        b.push_str(&clip_display(
            &style::bold(&format!("Files ({})", self.files.len())),
            self.width.saturating_sub(1),
        ));
        b.push('\n');
        if pick.filter.active {
            b.push_str(&clip_display(
                &search_input_line("search: ", &pick.filter.ti, self.width.saturating_sub(1)),
                self.width.saturating_sub(1),
            ));
            b.push('\n');
        }
        let height = self
            .height
            .saturating_sub(2 + usize::from(pick.filter.active))
            .max(1);
        let total = self.file_pick_indices(pick).count();
        let cursor = pick.cursor.min(total.saturating_sub(1));
        let off = (cursor as i64 - height as i64 + 1).max(0) as usize;
        for (i, idx) in self
            .file_pick_indices(pick)
            .skip(off)
            .take(height)
            .enumerate()
        {
            let tail = if self.tab_dirty(idx) {
                format!(" {}", style::diff("●"))
            } else {
                String::new()
            };
            let body = format!(
                "{}{}",
                cursor_prefix(off + i == cursor),
                self.files[idx].opts.path.clone()
            );
            let line = format!(
                "{}{tail}",
                clip_keep_tail(&body, self.width.saturating_sub(1), display_width(&tail))
            );
            let line = if off + i == cursor {
                style::sel(&line)
            } else {
                line
            };
            b.push_str(&line);
            b.push('\n');
        }
        if total == 0 {
            b.push_str(&style::faint("(no matches)"));
            b.push('\n');
        }
        let hint = if pick.filter.active {
            "↑/↓ or tab move · enter switch · esc cancel search"
        } else {
            "↑/↓ or ctrl+j/k move · pgup/pgdn page · enter switch · / search · esc close"
        };
        self.finish_with_hint(b, self.rec_hint(hint))
    }
}
