//! Main list body: row rendering, search rows, inline input lines and the
//! focused-description blocks.

use super::*;

impl App {
    /// listView renders the plain menu screen (Go listView).
    pub(in crate::ui::keys) fn list_view(&mut self) -> String {
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
        let taken_overlay = self.overlay.take();
        if let Some(Overlay::Search(s)) = taken_overlay {
            let header = self.list_header_rows(&st);
            let out = self.render_search_body(b, header, &s);
            self.overlay = Some(Overlay::Search(s));
            return out;
        }
        self.overlay = taken_overlay;
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
    pub(in crate::ui::keys) fn list_view_search(&mut self, s: &SearchState) -> String {
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

    pub(in crate::ui::keys) fn render_search_body(
        &mut self,
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
    pub(in crate::ui::keys) fn write_search_rows(
        &mut self,
        b: &mut String,
        s: &SearchState,
        height: usize,
    ) {
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
            self.mouse_targets
                .push((b.matches('\n').count(), 1, MouseTarget::OverlayRow(i)));
            self.write_cut(b, &line);
        }
    }

    /// writeCut clips s to the terminal width line by line.
    pub(in crate::ui::keys) fn write_cut(&self, b: &mut String, s: &str) {
        for line in s.split('\n') {
            b.push_str(&clip_display(line, self.width.saturating_sub(1)));
            b.push('\n');
        }
    }

    pub(in crate::ui::keys) fn write_rows(
        &mut self,
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
                self.mouse_targets.push((
                    b.matches('\n').count(),
                    1,
                    MouseTarget::EditSug(p.first + k),
                ));
                self.write_cut(b, &format!("{prefix}{}", p.lines[k]));
                i += 1;
                continue;
            }
            let row = &rows[i];
            let line = self.row_line(row, i == cursor, title_width);
            // mouse support: record the screen lines this row occupies
            // (wrapped rows span more than one line)
            let start = b.matches('\n').count();
            let n = line.matches('\n').count() + 1;
            self.mouse_targets.push((start, n, MouseTarget::MenuRow(i)));
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
            // cursor instead of inserting new lines. The state is taken out
            // so the candidate lines can be recorded as click targets.
            if matches!(&self.overlay, Some(Overlay::Anc(_))) {
                let Some(Overlay::Anc(s)) = self.overlay.take() else {
                    unreachable!("checked above");
                };
                let lines = self.anchor_input_lines(&s);
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
                        &[],
                    )
                } else {
                    Vec::new()
                };
                let (sug_start, ..) = Self::suggestion_slice(&s.sugs, s.sug, max);
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
                    self.mouse_targets.push((
                        b.matches('\n').count(),
                        1,
                        MouseTarget::AncSug(sug_start + k),
                    ));
                    self.write_cut(b, &format!("{prefix}{rect}"));
                }
                i += sugs.len().min(end.saturating_sub(i));
                self.overlay = Some(Overlay::Anc(s));
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
                    self.mouse_targets.push((
                        b.matches('\n').count(),
                        1,
                        MouseTarget::EditSug(p.first + k),
                    ));
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
    pub(in crate::ui::keys) fn write_input_lines(&self, title_width: usize) -> Vec<String> {
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
    pub(in crate::ui::keys) fn toggle_desc(&mut self, r: &Row, rows: &[Row]) {
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
    pub(in crate::ui::keys) fn desc_page_capacity(&self) -> usize {
        let status = self.status_lines().len();
        let notice = self.notice_lines().len();
        let hidden = usize::from(self.hide_empty);
        self.height
            .saturating_sub(4 + status + notice + hidden)
            .max(1)
    }

    /// focused_sources returns the schema help and document comment the
    /// description panel should show, after applying `C`'s source mode.
    pub(in crate::ui::keys) fn focused_sources(
        &self,
        st: &MenuState,
        rows: &[Row],
    ) -> (String, String) {
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
    pub(in crate::ui::keys) fn desc_lines_for(
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
}
