//! Floating suggestion/completion overlays: geometry, active-run styling
//! and the candidate lines for input, command and write paths.

use super::*;

pub(super) struct EditSugs {
    pub(super) cursor_idx: usize,
    pub(super) left: usize,
    /// Some(first covered row) when the window is above the edited row.
    pub(super) above_start: Option<usize>,
    /// absolute index of the first candidate in `lines` (mouse click targets)
    pub(super) first: usize,
    pub(super) lines: Vec<String>,
}

/// styled_sug_row renders one completion row: unmatched runs carry the
/// row's base look, the fuzzy-matched runs add the bold red highlight.
fn styled_sug_row(text: &str, base: &str, hits: &[usize]) -> String {
    let hit: std::collections::HashSet<usize> = hits.iter().copied().collect();
    let mut out = String::with_capacity(text.len() + 16);
    let mut run = String::new();
    let mut run_hit = false;
    for (k, c) in text.chars().enumerate() {
        let h = hit.contains(&k);
        if !run.is_empty() && h != run_hit {
            out.push_str(&style::sug_run(base, run_hit, &run));
            run.clear();
        }
        run_hit = h;
        run.push(c);
    }
    if !run.is_empty() {
        out.push_str(&style::sug_run(base, run_hit, &run));
    }
    out
}

impl App {
    /// suggestion_slice returns the absolute candidate range a window of
    /// `max` rows shows, the selected absolute index within it and whether
    /// any row is selected.
    pub(super) fn suggestion_slice(
        sugs: &[String],
        idx: i64,
        max: usize,
    ) -> (usize, usize, usize, bool) {
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
    pub(super) fn sug_strip_active(sugs: &[String], idx: i64, query: &str, max: usize) -> bool {
        let (start, end, ..) = Self::suggestion_slice(sugs, idx, max);
        end - start > 1 && query.chars().count() >= SUG_STRIP_PREFIX_MIN
    }

    /// suggestion_rect_width is the rectangle width suggestion_overlay_lines
    /// draws for these candidates within `avail` cells (0 when nothing
    /// shows). Placement asks for it first so a caret-anchored window can
    /// slide left without its width changing.
    pub(in crate::ui::keys) fn suggestion_rect_width(
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
    /// row is reverse-video across the full rectangle, the rest are faint,
    /// and the fuzzy-matched text (`hl`, char indices per candidate) is
    /// bold red in every row.
    pub(in crate::ui::keys) fn suggestion_overlay_lines(
        sugs: &[String],
        idx: i64,
        query: &str,
        max: usize,
        left: usize,
        max_width: usize,
        hl: &[Vec<usize>],
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
        let shown_of = |raw: &str| -> String {
            match strip.then(|| strip_query(raw, query)).flatten() {
                Some(tail) if !tail.is_empty() => tail.to_string(),
                _ => raw.to_string(),
            }
        };
        let natural = sugs[start..end]
            .iter()
            .map(|raw| display_width(&shown_of(raw)))
            .max()
            .unwrap_or(0);
        let width = natural.max(SUG_MIN_WIDTH).min(avail);
        if width == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(max);
        for i in start..end {
            let on = selected && i == sel;
            let base = if on { "7" } else { "2;48;5;236" };
            let raw = &sugs[i];
            let mut shown = shown_of(raw);
            // stripping drops the typed prefix from the shown text: the
            // highlight indices shift with it and prefix-only hits (the
            // whole query is the prefix) disappear entirely
            let lead = raw.chars().count() - shown.chars().count();
            let mut hits: Vec<usize> = hl
                .get(i)
                .map(|v| v.iter().filter(|c| **c >= lead).map(|c| c - lead).collect())
                .unwrap_or_default();
            // keep the tail visible: when the shown text does not fit, cut
            // its head and shift the highlight with it
            if display_width(&shown) > width {
                let clipped = clip_left_ellipsis(&shown, width);
                let dropped = shown.chars().count() + 1 - clipped.chars().count();
                shown = clipped;
                hits = hits
                    .into_iter()
                    .filter(|c| *c >= dropped)
                    .map(|c| c - dropped)
                    .collect();
            }
            // Fill the row so the background paints a solid rectangle, not
            // just the glyphs of the candidate text: the padding joins the
            // plain text before styling, so an unhighlighted row stays one
            // SGR sequence.
            let pad = width.saturating_sub(display_width(&shown));
            if pad > 0 {
                shown.push_str(&" ".repeat(pad));
            }
            out.push(styled_sug_row(&shown, base, &hits));
        }
        out
    }

    /// overlay_prefix keeps the content left of an inline candidate window:
    /// the underlying row's first `left` cells, padded when it is shorter.
    pub(in crate::ui::keys) fn overlay_prefix(line: &str, left: usize) -> String {
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
    pub(in crate::ui::keys) fn indent_overlay(lines: Vec<String>, left: usize) -> Vec<String> {
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
    pub(in crate::ui::keys) fn command_suggestion_lines(&self) -> Vec<String> {
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
            &[],
        );
        Self::indent_overlay(lines, left)
    }

    /// writeSuggestionLines is the W write-path candidate window (above the
    /// input line: the help bar occupies the space below it).
    pub(in crate::ui::keys) fn write_suggestion_lines(&self, title_width: usize) -> Vec<String> {
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
            &[],
        );
        Self::indent_overlay(lines, left)
    }

    /// writeRows renders the visible slice of rows (scroll computed
    /// read-only around the persisted offset).
    /// edit_sugs_plan places the inline edit completion window. Below is
    /// preferred; when the window does not fit there and the rows above have
    /// more room, the same rectangle is drawn over them instead.
    pub(super) fn edit_sugs_plan(
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
        // tails next to the input position. The enum dropdown is exempt:
        // its input is a search keyword, not the options' prefix, so its
        // rows always show the full option text anchored at the text
        // start (an empty strip query never activates).
        let query: &str = if e
            .fn_
            .is_some_and(|id| !self.fs().schema.nodes[id as usize].enum_.is_empty())
        {
            ""
        } else {
            &e.sug_prefix
        };
        let title = self.title_col(&self.row_title_text(row), title_width);
        let text_left = 2 + display_width(&title);
        let (_, _, shift) = ti_window(&e.ti, self.edit_input_avail(&title));
        let line_right = self.width.saturating_sub(1);
        // When the rectangle would run past the right edge, slide it left so
        // every candidate stays on screen — the horizontal counterpart of
        // the rows-above fallback above.
        let rect = Self::suggestion_rect_width(&e.sugs, e.sug_idx, query, capacity, line_right);
        let base = if Self::sug_strip_active(&e.sugs, e.sug_idx, query, capacity) {
            e.sug_col
        } else {
            0
        };
        let left = (text_left + base.saturating_sub(shift)).min(line_right.saturating_sub(rect));
        let lines = Self::suggestion_overlay_lines(
            &e.sugs, e.sug_idx, query, capacity, left, line_right, &e.sug_hl,
        );
        if lines.is_empty() {
            return None;
        }
        let (first, ..) = Self::suggestion_slice(&e.sugs, e.sug_idx, capacity);
        Some(EditSugs {
            cursor_idx: cursor,
            left,
            above_start: use_above.then(|| cursor - lines.len()),
            first,
            lines,
        })
    }
}
