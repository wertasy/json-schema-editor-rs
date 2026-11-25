//! View entry: composes one frame from the list viewport, status block and
//! help bar; ListFrame holds the shared menu-screen chrome geometry.

mod diff_badges;
mod input_lines;
mod list_view;
mod menu_view;
mod overlay_views;
mod suggestions;

use super::*;
use crate::ui::history::DiffKind;
use crate::ui::menu::{DIFF_ADDED, DIFF_MODIFIED, DIFF_REMOVED, MouseTarget};

/// ListFrame is the fixed chrome of the main list screen: the menu viewport
/// height and every line pinned above the help bar. `list_view` and the
/// command-output sizing share it, so an inline multi-line notice is chosen
/// exactly when it fits the blank rows between the menu and the bottom block.
pub(super) struct ListFrame {
    pub(super) rows_height: usize,
    pub(super) bottom: usize,
    pub(super) desc_lines: Vec<DescLine>,
    pub(super) summary_lines: Vec<String>,
    pub(super) status: Vec<String>,
    pub(super) title_width: usize,
    pub(super) write_lines: Vec<String>,
    pub(super) write_sugs: Vec<String>,
    pub(super) cmd_line: Option<String>,
    pub(super) cmd_sugs: Vec<String>,
}

/// EditSugs is the inline autocomplete window for the edited cell. It sits
/// below the row when it fits, otherwise it replaces rows above the row so
/// candidates stay visible when the cursor is at the bottom of the screen.

impl App {
    /// View renders the active surface (Go View).
    pub fn view(&mut self) -> String {
        // The click-target map belongs to one frame; a surface that records
        // nothing leaves it empty and mouse clicks do nothing.
        self.mouse_targets.clear();
        // Per-frame diff marks against the load/last-save baseline drive
        // the colored row badges. The classified diff walk visits every
        // location once per repaint — the same order as building the rows.
        // Each mark ORs its kind into the path and every ancestor, so a
        // parent row aggregates all change kinds inside its subtree.
        self.diff_agg.clear();
        let marks = match self.fs().baseline.as_ref() {
            Some(base) => crate::ui::history::diff_kinds(base, &self.fs().doc),
            None => Vec::new(),
        };
        // Removed array elements have no row of their own (the doc-length
        // generated menu drops them): each removal paints the row that now
        // occupies its position. Earlier deletions shift later positions up,
        // so the marks adjust by the deletions before them and consecutive
        // removals merge onto one row. The removal count aggregates onto the
        // container path and its ancestors, so the parent row can show
        // `- ×N` when several children were deleted and the removed rows
        // themselves are gone. Other kinds propagate to the path and its
        // ancestors only (container rows aggregate their subtree).
        let mut entries: Vec<(Vec<PathSeg>, u8)> = Vec::new();
        let mut added_paths: Vec<Vec<PathSeg>> = Vec::new();
        let mut removed_runs: std::collections::HashMap<Vec<PathSeg>, Vec<usize>> =
            Default::default();
        let mut removed_counts: std::collections::HashMap<Vec<PathSeg>, usize> = Default::default();
        {
            let doc = &self.fs().doc;
            for (path, kind) in &marks {
                let bit = match kind {
                    DiffKind::Added => {
                        added_paths.push(path.clone());
                        DIFF_ADDED
                    }
                    DiffKind::Removed => DIFF_REMOVED,
                    DiffKind::Modified => DIFF_MODIFIED,
                };
                match (kind, path.last()) {
                    (DiffKind::Removed, Some(PathSeg::Idx(_))) => {
                        let prefix = path[..path.len() - 1].to_vec();
                        let i = match path.last() {
                            Some(PathSeg::Idx(i)) => *i,
                            _ => 0,
                        };
                        removed_runs.entry(prefix.clone()).or_default().push(i);
                        for k in 0..=prefix.len() {
                            *removed_counts.entry(prefix[..k].to_vec()).or_insert(0) += 1;
                        }
                    }
                    _ => {
                        for k in 0..=path.len() {
                            entries.push((path[..k].to_vec(), bit));
                        }
                    }
                }
            }
            for (prefix, mut idxs) in removed_runs {
                idxs.sort_unstable();
                let len = doc.array_len(&prefix);
                let mut shift = 0usize;
                for i in idxs {
                    let pos = i.saturating_sub(shift);
                    shift += 1;
                    if pos < len {
                        let mut p = prefix.clone();
                        p.push(PathSeg::Idx(pos));
                        entries.push((p, DIFF_REMOVED));
                    }
                }
                for k in 0..=prefix.len() {
                    entries.push((prefix[..k].to_vec(), DIFF_REMOVED));
                }
            }
        }
        self.diff_added = added_paths;
        for (path, bit) in entries {
            *self.diff_agg.entry(path).or_insert(0) |= bit;
        }
        // The search overlay renders inline and needs &mut self (its
        // breadcrumb may synthesize menu rows), so it is taken out before
        // the dispatch below.
        if matches!(&self.overlay, Some(Overlay::Search(_))) {
            if let Some(Overlay::Search(s)) = self.overlay.take() {
                let out = self.list_view_search(&s);
                self.overlay = Some(Overlay::Search(s));
                return out;
            }
        }
        // Read-only overlays render against &self; the list overlays collect
        // their click targets into the frame-local map.
        let mut hits = Vec::new();
        enum Rendered {
            Done(String),
            List,
        }
        let rendered = match &self.overlay {
            Some(Overlay::Confirm(c)) => Rendered::Done(self.confirm_view(c)),
            Some(Overlay::Issues(s)) => Rendered::Done(self.issues_view(s, &mut hits)),
            Some(Overlay::Errs(s)) => Rendered::Done(self.errs_view(s, &mut hits)),
            Some(Overlay::RefPick(s)) => Rendered::Done(self.ref_pick_view(s, &mut hits)),
            Some(Overlay::Text(s)) => Rendered::Done(self.text_view(s)),
            Some(Overlay::CommentEdit(s)) => Rendered::Done(self.comment_edit_view(s)),
            Some(Overlay::Comment(s)) => Rendered::Done(self.comment_view(s)),
            Some(Overlay::ShellOut(s)) => Rendered::Done(self.shell_out_view(s)),
            Some(Overlay::Help(s)) => Rendered::Done(self.help_view(s.scroll)),
            Some(Overlay::FilePick(s)) => Rendered::Done(self.file_pick_view(s, &mut hits)),
            _ => Rendered::List,
        };
        self.mouse_targets.extend(hits);
        if let Rendered::Done(out) = rendered {
            return out;
        }
        self.list_view()
    }

    /// list_frame lays out the fixed chrome around the menu rows. A notice
    /// with `notice_rows` lines shrinks the viewport; the caller supplies 0
    /// when asking how many blank rows are available for one.
    pub(super) fn list_frame(
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

    /// list_header_rows counts the title, breadcrumb and any inline input
    /// rows above the menu.
    pub(super) fn list_header_rows(&self, st: &MenuState) -> usize {
        let mut header = 2;
        if let Some(Overlay::Mrg(s)) = &self.overlay {
            header += self.merge_input_lines(s).matches('\n').count() + 1;
        }
        if matches!(st.kind, MenuKind::OneOf | MenuKind::EnumArray) && st.filter.active {
            header += 1;
        }
        header
    }

    /// notice_lines renders the transient notice. A plain one-line notice is
    /// clipped like before; a message carrying newlines (command output) is
    /// wrapped into the rows above the bottom block.
    pub(in crate::ui::keys) fn notice_lines(&self) -> Vec<String> {
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
    pub(in crate::ui::keys) fn blank_rows_above_notice(&mut self) -> usize {
        let Some(st) = self.top().cloned() else {
            return 0;
        };
        let rows = self.menu_rows_snapshot();
        let (_, summary) = self.menu_title(&st);
        let frame = self.list_frame(&st, &rows, &summary, 0);
        let rendered = rows.len().min(frame.rows_height.max(1));
        frame.rows_height.saturating_sub(rendered)
    }

    /// menuRows for the current (top) state (read-only flows reuse the same
    /// warm cache path as menu_rows).
    pub(in crate::ui::keys) fn menu_rows_snapshot(&mut self) -> Vec<Row> {
        self.menu_rows()
    }
}
