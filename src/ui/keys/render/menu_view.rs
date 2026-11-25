//! Top chrome: breadcrumb/title column, menu summaries, status and help
//! lines, and the scrollable help page.

use super::*;

impl App {
    /// breadcrumb renders the navigation context as a document path.
    pub(in crate::ui::keys) fn breadcrumb(&mut self) -> String {
        let st = self.top().cloned();
        let rows = self.menu_rows_snapshot();
        self.breadcrumb_with_rows(st.as_ref(), &rows)
    }

    /// breadcrumb_with_rows is breadcrumb for a caller that already built the
    /// row list: the merge-key arrow needs the cursor row, and building all
    /// rows again doubled listView's per-frame cost on a wide menu.
    pub(in crate::ui::keys) fn breadcrumb_with_rows(
        &mut self,
        st: Option<&MenuState>,
        rows: &[Row],
    ) -> String {
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
    pub(in crate::ui::keys) fn merge_crumb_row(
        st: &MenuState,
        rows: &[Row],
    ) -> Option<Vec<PathSeg>> {
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
    pub(in crate::ui::keys) fn menu_title(&self, st: &MenuState) -> (String, String) {
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
    pub(in crate::ui::keys) fn menu_summary(&self, st: &MenuState) -> String {
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
    pub(in crate::ui::keys) fn variant_picker_summary(&self, st: &MenuState) -> String {
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
    pub(in crate::ui::keys) fn status_lines(&self) -> Vec<String> {
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
    pub(in crate::ui::keys) fn help_line(&self) -> String {
        if let Some(bar) = self.fetch_bar() {
            return bar;
        }
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
    pub(in crate::ui::keys) fn command_input_line(&self) -> Option<String> {
        match &self.overlay {
            Some(Overlay::Command(s)) => {
                Some(search_input_line(":", &s.ti, self.width.saturating_sub(1)))
            }
            _ => None,
        }
    }

    /// helpView renders the "?" overlay.
    pub(in crate::ui::keys) fn help_view(&self, scroll: usize) -> String {
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

    pub(in crate::ui::keys) fn help_col(&self, left: &str) -> String {
        let w = display_width(left);
        if w >= crate::ui::menu::HELP_COL_WIDTH {
            format!("{left} ")
        } else {
            format!("{left}{}", " ".repeat(crate::ui::menu::HELP_COL_WIDTH - w))
        }
    }

    /// recHint renders a hint row with a right-aligned tail (cmd buffer /
    /// recording marker).
    pub(in crate::ui::keys) fn rec_hint(&self, hints: &str) -> String {
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

    /// finish_with_hint pads the view with blank rows so the help row lands
    /// on the terminal's last line, then appends it without a trailing
    /// newline. Views must size their content to leave that row free (the
    /// windowed overlays do).
    pub(in crate::ui::keys) fn finish_with_hint(&self, mut b: String, hint: String) -> String {
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1) {
            b.push('\n');
            lines += 1;
        }
        b.push_str(&hint);
        b
    }
}
