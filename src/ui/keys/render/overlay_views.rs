//! Overlay surfaces: confirm, issues, ref picker, save errors, comment
//! pages, shell output, text view, file picker, plus their key handlers.

use super::*;

impl App {
    pub(in crate::ui::keys) fn confirm_view(&self, c: &ConfirmState) -> String {
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

    pub(in crate::ui::keys) fn issues_view(
        &self,
        s: &IssuesState,
        hits: &mut Vec<(usize, usize, MouseTarget)>,
    ) -> String {
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
            mark_target(&b, hits, MouseTarget::OverlayRow(off + i));
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

    pub(in crate::ui::keys) fn ref_pick_view(
        &self,
        s: &RefPickState,
        hits: &mut Vec<(usize, usize, MouseTarget)>,
    ) -> String {
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
            mark_target(&b, hits, MouseTarget::OverlayRow(off + i));
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

    pub(in crate::ui::keys) fn errs_view(
        &self,
        s: &ErrsState,
        hits: &mut Vec<(usize, usize, MouseTarget)>,
    ) -> String {
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
            mark_target(&b, hits, MouseTarget::OverlayRow(off + i));
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

    pub(in crate::ui::keys) fn comment_view(&self, s: &CommentState) -> String {
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
    pub(in crate::ui::keys) fn shell_out_view(&self, s: &ShellOutState) -> String {
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
    pub(in crate::ui::keys) fn comment_edit_view(&self, s: &CommentEditState) -> String {
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

    pub(in crate::ui::keys) fn text_view(&self, s: &TextState) -> String {
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

    pub(in crate::ui::keys) fn file_pick_view(
        &self,
        pick: &FilePickState,
        hits: &mut Vec<(usize, usize, MouseTarget)>,
    ) -> String {
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
            mark_target(&b, hits, MouseTarget::OverlayRow(off + i));
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
    pub(in crate::ui::keys) fn edit_comment_row(&mut self, r: &Row) {
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
    pub(in crate::ui::keys) fn open_comment_edit(
        &mut self,
        path: &[PathSeg],
        title: &str,
        text: &str,
    ) {
        self.overlay = Some(self.comment_edit_overlay(path, title, text));
    }

    /// comment_edit_overlay builds the comment editor state (the overlay
    /// router assigns the returned surface).
    pub(in crate::ui::keys) fn comment_edit_overlay(
        &self,
        path: &[PathSeg],
        title: &str,
        text: &str,
    ) -> Overlay {
        let mut ta = TextArea::new();
        ta.set_viewport_within(self.width as u16, self.height as u16);
        ta.set_value(text);
        Overlay::CommentEdit(CommentEditState {
            path: path.to_vec(),
            title: title.to_string(),
            ta,
        })
    }

    /// doc_comment_at reads the session's document comment for one path.
    pub(in crate::ui::keys) fn doc_comment_at(&self, path: &[PathSeg]) -> Option<String> {
        self.fs().meta.comment_text(path)
    }

    /// row_comment_text returns the document comment of a member row (None
    /// for synthetic rows such as add/save/raw and for paths without one).
    pub(in crate::ui::keys) fn row_comment_text(&self, r: &Row) -> Option<String> {
        if !matches!(
            r.kind,
            RowKind::Field | RowKind::Drill | RowKind::Unknown | RowKind::Toggle | RowKind::Variant
        ) || self.doc().get(&r.path).is_none()
        {
            return None;
        }
        self.doc_comment_at(&r.path)
    }

    /// ref_indices lists the reference paths matching the picker query.
    pub(in crate::ui::keys) fn ref_indices<'a>(
        &'a self,
        s: &'a RefPickState,
    ) -> impl Iterator<Item = usize> + 'a {
        let q = s.filter.ti.value().to_lowercase();
        s.paths
            .iter()
            .enumerate()
            .filter(move |(_, p)| q.is_empty() || self.ref_label(p).to_lowercase().contains(&q))
            .map(|(i, _)| i)
    }

    /// ref_label renders one reference row: path plus value summary.
    pub(in crate::ui::keys) fn ref_label(&self, p: &[PathSeg]) -> String {
        format!(
            "{}  {}",
            crate::doc::jsondoc::path_string(p),
            raw_value_summary(&self.fs().schema, self.doc(), p)
        )
    }
}

/// mark_target records one rendered list line as a mouse click target.
fn mark_target(b: &str, hits: &mut Vec<(usize, usize, MouseTarget)>, t: MouseTarget) {
    let start = b.matches('\n').count();
    hits.push((start, 1, t));
}
