//! Inline input-line renderers: the scalar edit line, anchor/merge
//! key inputs, and the shared suggestion-window plan/geometry helpers
//! every input surface draws with.

use super::*;
impl App {
    /// edit_input_avail is the inline edit input's viewport width: the
    /// clipped line minus the "> " marker and the title column, keeping the
    /// caret cell and one margin column visible. edit_sugs_plan uses the
    /// same budget so the candidate anchor matches the rendered window.
    pub(in crate::ui::keys) fn edit_input_avail(&self, left: &str) -> usize {
        input_avail(
            self.width.saturating_sub(1),
            display_width(cursor_prefix(true)) + display_width(left),
        )
    }

    /// editLines renders the in-place input line plus error. Long values
    /// scroll horizontally so the caret stays visible on a narrow terminal.
    pub(in crate::ui::keys) fn edit_lines(&self, e: &EditState, left: &str) -> String {
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
    pub(in crate::ui::keys) fn anchor_input_lines(&self, s: &AnchorState) -> String {
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
    pub(in crate::ui::keys) fn merge_input_lines(&self, s: &MergeState) -> String {
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
}
