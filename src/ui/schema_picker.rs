//! The schema picker: an interactive search over the SchemaStore catalog,
//! opened when no schema is configured and the file name does not uniquely
//! determine one. Two modes sharing one filter:
//!   - full-screen TUI (type to filter, up/down to move, enter to select,
//!     esc to abort)
//!   - accessible line protocol (search term, then a numbered list)
//!
//! Rust port of `internal/ui/schema_picker.go`.

use std::io::Write as _;

use crate::catalog::Entry;
use crate::ui::keys::{char_width, clip_display, display_width};
use crate::ui::widgets::{Key, TextInput};

/// PickOptions configures pick.
pub struct PickOptions {
    /// line protocol; false = full-screen TUI
    pub accessible: bool,
    /// config file name shown in the header
    pub filename: String,
    /// initial search term (the file name stem)
    pub query: String,
}

/// pick lets the user choose one catalog entry; None when the user aborts or
/// the input is exhausted.
pub fn pick(entries: &[Entry], opts: &PickOptions) -> Option<Entry> {
    if opts.accessible {
        pick_accessible(entries, opts)
    } else {
        pick_tui(entries.to_vec(), opts.filename.clone(), &opts.query)
    }
}

/// filterEntries returns the indexes of entries whose Name or Description
/// contains query (case-insensitive); an empty query selects everything.
pub fn filter_entries(entries: &[Entry], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            q.is_empty()
                || e.name.to_lowercase().contains(&q)
                || e.description.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

// --- full-screen TUI ---------------------------------------------------------

struct PickerModel {
    entries: Vec<Entry>,
    filename: String,
    ti: TextInput,
    filtered: Vec<usize>,
    cursor: usize,
    offset: usize,
    width: usize,
    height: usize,
    stale: bool, // the preset query is untouched: the first text key replaces it
}

/// style helpers mirroring the menu's palette (lipgloss ANSI).
fn s_title(s: &str) -> String {
    format!("\x1b[1m{s}\x1b[0m")
}
fn s_status(s: &str) -> String {
    format!("\x1b[33m{s}\x1b[0m")
}
fn s_help(s: &str) -> String {
    format!("\x1b[38;5;248m{s}\x1b[0m")
}
fn s_sel(s: &str) -> String {
    format!("\x1b[1m\x1b[48;5;236m{s}\x1b[0m")
}
fn s_crumb(s: &str) -> String {
    format!("\x1b[2m{s}\x1b[0m")
}

fn wrap_cursor(cursor: i64, delta: i64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as i64;
    (((cursor + delta) % len + len) % len) as usize
}

/// one_line folds control characters so one catalog field always renders on
/// one row (descriptions occasionally carry tabs or embedded newlines).
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// clip_cells truncates plain text to max terminal cells, marking the cut
/// with "…". CJK and emoji count two cells, so a rune-count limit would let
/// the row wrap and push the help line off screen.
fn clip_cells(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = clip_display(s, max.saturating_sub(1));
    out.push('…');
    out
}

/// input_line renders `search: <query>` with a block caret, scrolling the
/// query horizontally so the caret stays inside the terminal. A wide-query
/// row used to wrap and corrupt every row below it.
fn input_line(prompt: &str, value: &str, cursor: usize, width: usize) -> String {
    let avail = width.saturating_sub(display_width(prompt) + 2); // caret + margin
    let chars: Vec<char> = value.chars().collect();
    let cursor = cursor.min(chars.len());
    let cell_at = |n: usize| chars[..n].iter().map(|&c| char_width(c)).sum::<usize>();
    let cursor_cell = cell_at(cursor);
    // Drop leading characters until the caret fits in the visible window.
    let mut start = 0usize;
    let window = avail.max(1);
    while start < cursor && cursor_cell - cell_at(start) >= window {
        start += 1;
    }
    // Fill the rest of the row, never splitting a wide character.
    let mut end = start;
    let mut used = 0usize;
    while end < chars.len() {
        let w = char_width(chars[end]);
        if used + w > avail {
            break;
        }
        used += w;
        end += 1;
    }
    let before: String = chars[start..cursor].iter().collect();
    let mut out = String::from(prompt);
    out.push_str(&before);
    if cursor < end {
        out.push_str("\x1b[7m");
        out.push(chars[cursor]);
        out.push_str("\x1b[27m"); // keep the surrounding help color
        out.push_str(&chars[cursor + 1..end].iter().collect::<String>());
    } else {
        out.push_str("\x1b[7m \x1b[27m");
    }
    out
}

impl PickerModel {
    fn new(entries: Vec<Entry>, filename: String, query: &str) -> PickerModel {
        let mut ti = TextInput::new();
        ti.set_value(query);
        ti.cursor_end();
        let mut m = PickerModel {
            entries,
            filename,
            ti,
            filtered: Vec::new(),
            cursor: 0,
            offset: 0,
            width: 80,
            height: 24,
            stale: !query.is_empty(),
        };
        m.refilter();
        m
    }

    /// refilter reapplies the search term; the cursor restarts at the top,
    /// like the menu search.
    fn refilter(&mut self) {
        self.filtered = filter_entries(&self.entries, self.ti.value());
        self.cursor = 0;
        self.offset = 0;
    }

    fn view(&self) -> String {
        let mut b = String::new();
        let width = self.width.max(1);
        // Every row must fit one terminal line: a wrapped row shifts the
        // whole screen and the help line stops being the bottom row.
        b.push_str(&s_title(&clip_display(
            "Select a schema",
            width.saturating_sub(1),
        )));
        b.push('\n');
        let status = format!(
            "for {} — {} of {}",
            one_line(&self.filename),
            self.filtered.len(),
            self.entries.len()
        );
        b.push_str(&s_status(&clip_display(&status, width.saturating_sub(1))));
        b.push('\n');
        // The query row keeps the terminal's default foreground (white);
        // s_help is reserved for the bottom help row.
        b.push_str(&input_line(
            "search: ",
            self.ti.value(),
            self.ti.cursor(),
            width,
        ));
        b.push('\n');
        let height = self.height.saturating_sub(4).max(1); // title, status, search, help
        // Go's View has a value receiver: the window correction is local to
        // the rendered frame and does not persist (only Update persists).
        let (cursor, offset) = scroll_window(self.cursor, self.offset, self.filtered.len(), height);
        for i in offset..self.filtered.len().min(offset + height) {
            let e = &self.entries[self.filtered[i]];
            let prefix = if i == cursor { "> " } else { "  " };
            // prefix (2) + content stays inside width-1, so the row's newline
            // can never land in the terminal's wrap-pending last column.
            let avail = width.saturating_sub(3);
            let name = clip_cells(&one_line(&e.name), avail);
            let name_width = display_width(&name);
            let desc = if avail > name_width + 2 {
                clip_cells(&one_line(&e.description), avail - name_width - 2)
            } else {
                String::new()
            };
            let sep = if desc.is_empty() { "" } else { "  " };
            if i == cursor {
                b.push_str(&s_sel(&format!("{prefix}{name}{sep}{desc}")));
            } else {
                b.push_str(&format!("{prefix}{name}{sep}{}", s_crumb(&desc)));
            }
            b.push('\n');
        }
        let mut lines = b.matches('\n').count();
        while lines < self.height.saturating_sub(1) {
            b.push('\n');
            lines += 1;
        }
        b.push_str(&s_help(&clip_display(
            "type to search · ↑/↓ or tab/shift+tab move · enter select · esc cancel",
            width.saturating_sub(1),
        )));
        b
    }

    /// handle_repeat applies a run of identical keys from one input burst.
    /// Consecutive vertical moves are one cursor step; other keys replay so
    /// their per-press semantics (text edits, enter, esc) are unchanged.
    fn handle_repeat(&mut self, key: &PickerKey, n: usize) -> Option<Option<Entry>> {
        if n > 1 && matches!(key, PickerKey::Up | PickerKey::Down) {
            let delta = if matches!(key, PickerKey::Up) { -1 } else { 1 };
            self.cursor = wrap_cursor(self.cursor as i64, delta * n as i64, self.filtered.len());
            return None;
        }
        for _ in 0..n {
            if let Some(choice) = self.handle(key) {
                return Some(choice);
            }
        }
        None
    }

    fn handle(&mut self, key: &PickerKey) -> Option<Option<Entry>> {
        match key {
            PickerKey::Esc | PickerKey::CtrlC => return Some(None),
            // cursor moves and commit keys are handled before text edits
            PickerKey::Up => {
                self.cursor = wrap_cursor(self.cursor as i64, -1, self.filtered.len());
                return None;
            }
            PickerKey::Down => {
                self.cursor = wrap_cursor(self.cursor as i64, 1, self.filtered.len());
                return None;
            }
            PickerKey::Enter => {
                if !self.filtered.is_empty() {
                    return Some(Some(self.entries[self.filtered[self.cursor]].clone()));
                }
                return None;
            }
            PickerKey::Widget(k) => {
                let before = self.ti.value().to_string();
                if self.stale && modifies_text(key) {
                    // The preset stem behaves like selected text: typing or
                    // editing starts a fresh search over it instead of
                    // appending to it.
                    self.stale = false;
                    self.ti.set_value("");
                }
                self.ti.handle_key(*k);
                if before != self.ti.value() {
                    self.refilter();
                }
                None
            }
        }
    }
}

fn modifies_text(key: &PickerKey) -> bool {
    matches!(
        key,
        PickerKey::Widget(Key::Char(_))
            | PickerKey::Widget(Key::Backspace)
            | PickerKey::Widget(Key::Delete)
    )
}

/// The picker's key space: the widget keys plus navigation/commit keys the
/// widgets enum does not carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerKey {
    Widget(Key),
    Up,
    Down,
    Enter,
    Esc,
    CtrlC,
}

/// cursor-anchored scrolling window (same offset rules as writeRows),
/// returning the corrected (cursor, offset).
fn scroll_window(cursor: usize, mut offset: usize, len: usize, height: usize) -> (usize, usize) {
    if offset > cursor {
        offset = cursor;
    }
    if cursor >= offset + height {
        offset = cursor + 1 - height;
    }
    let max_off = len.saturating_sub(height);
    if offset > max_off {
        offset = max_off;
    }
    (cursor, offset)
}

fn pick_tui(entries: Vec<Entry>, filename: String, query: &str) -> Option<Entry> {
    // own bubbletea-equivalent program: raw mode + alt screen, string frames
    use crossterm::cursor::{Hide, Show};
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };
    use std::io::Write as _;
    let _ = enable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), EnterAlternateScreen, Hide);
    let choice = (|| -> Option<Entry> {
        let mut m = PickerModel::new(entries, filename, query);
        // bubbletea delivers an initial WindowSizeMsg; crossterm does not, so
        // query the pty size once before the first frame.
        if let Ok((w, h)) = crossterm::terminal::size() {
            m.width = w as usize;
            m.height = h as usize;
        }
        loop {
            let mut out = std::io::stdout();
            let _ = out.write_all(b"\x1b[H\x1b[2J");
            let _ = out.write_all(frame(&m.view()).as_bytes());
            let _ = out.flush();
            // Block for one key, then drain the rest of the burst. A held
            // arrow key queues repeats faster than a long catalog repaints;
            // one frame per burst keeps the cursor from coasting after the
            // key is released (and collapses consecutive moves to one step).
            let mut keys: Vec<PickerKey> = Vec::new();
            loop {
                if crossterm::event::poll(Duration::from_millis(250)).unwrap_or(false) {
                    match crossterm::event::read() {
                        Ok(crossterm::event::Event::Key(k)) => {
                            if k.kind != crossterm::event::KeyEventKind::Release {
                                keys.push(to_key(&k));
                                break;
                            }
                        }
                        Ok(crossterm::event::Event::Resize(w, h)) => {
                            m.width = w as usize;
                            m.height = h as usize;
                        }
                        _ => {}
                    }
                }
            }
            while keys.len() < MAX_PICKER_BURST
                && crossterm::event::poll(Duration::ZERO).unwrap_or(false)
            {
                match crossterm::event::read() {
                    Ok(crossterm::event::Event::Key(k)) => {
                        if k.kind != crossterm::event::KeyEventKind::Release {
                            keys.push(to_key(&k));
                        }
                    }
                    Ok(crossterm::event::Event::Resize(w, h)) => {
                        m.width = w as usize;
                        m.height = h as usize;
                    }
                    _ => {}
                }
            }
            let mut i = 0;
            while i < keys.len() {
                let mut n = 1;
                if matches!(keys[i], PickerKey::Up | PickerKey::Down) {
                    while i + n < keys.len() && keys[i + n] == keys[i] {
                        n += 1;
                    }
                }
                if let Some(choice) = m.handle_repeat(&keys[i], n) {
                    return choice;
                }
                i += n;
            }
        }
    })();
    let _ = disable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen, Show);
    choice
}

/// frame converts the view to terminal output for raw mode.
///
/// Crossterm's raw mode disables output post-processing (OPOST), so a bare
/// `\n` only moves the cursor down and keeps its column: each row started
/// where the previous one ended ("staircase" indentation). Write `\r\n`
/// explicitly. The view already fills exactly the screen height and ends
/// without a newline, so nothing scrolls.
fn frame(view: &str) -> String {
    view.replace('\n', "\r\n")
}

/// Map a crossterm key event to the widgets Key enum (picker subset).
fn to_key(k: &crossterm::event::KeyEvent) -> PickerKey {
    use crossterm::event::{KeyCode, KeyModifiers};
    match k.code {
        KeyCode::Char('c') if k.modifiers == KeyModifiers::CONTROL => PickerKey::CtrlC,
        // Go's picker treats ctrl+j/ctrl+k as down/up (the menu's bindings);
        // without these the letters land in the search input.
        KeyCode::Char('j') if k.modifiers == KeyModifiers::CONTROL => PickerKey::Down,
        KeyCode::Char('k') if k.modifiers == KeyModifiers::CONTROL => PickerKey::Up,
        // shell-style word motion (alt+f/alt+b) in the query input
        KeyCode::Char('f') if k.modifiers == KeyModifiers::ALT => {
            PickerKey::Widget(Key::WordForward)
        }
        KeyCode::Char('b') if k.modifiers == KeyModifiers::ALT => {
            PickerKey::Widget(Key::WordBackward)
        }
        KeyCode::Char(c) => PickerKey::Widget(Key::Char(c)),
        KeyCode::Enter => PickerKey::Enter,
        KeyCode::Backspace => PickerKey::Widget(Key::Backspace),
        KeyCode::Delete => PickerKey::Widget(Key::Delete),
        KeyCode::Left => PickerKey::Widget(Key::Left),
        KeyCode::Right => PickerKey::Widget(Key::Right),
        KeyCode::Up => PickerKey::Up,
        KeyCode::Down => PickerKey::Down,
        KeyCode::Home => PickerKey::Widget(Key::Home),
        KeyCode::End => PickerKey::Widget(Key::End),
        KeyCode::Esc => PickerKey::Esc,
        KeyCode::Tab => PickerKey::Down, // tab moves down (Go parity)
        KeyCode::BackTab => PickerKey::Up, // shift+tab moves up
        _ => PickerKey::Esc,
    }
}

// --- accessible line protocol --------------------------------------------------

/// pickAccessible drives the picker as line prompts over stdin/stdout:
///
///   Select a schema for <Filename>
///   ? Schema search [<Query>]: <empty line = keep the current query>
///     1) <Name> — <description>
///     …  (at most 15 rows; a longer list ends with "… N more — refine the
///         search", an empty result prints "  no matches" and returns to the
///         search prompt)
///   ? Number (0 = new search, enter = abort): <row number>
///
/// An invalid or out-of-range number re-issues the number prompt; an empty
/// line there aborts. EOF at any prompt aborts, so an exhausted script
/// cannot spin.
pub fn pick_accessible(entries: &[Entry], opts: &PickOptions) -> Option<Entry> {
    let stdin = std::io::stdin();
    let mut reader = crate::ui::lineio::LineReader::new(stdin.lock());
    let mut out = std::io::stdout();
    let mut query = opts.query.clone();
    let _ = writeln!(out, "Select a schema for {}", opts.filename);
    'search: loop {
        let _ = write!(out, "? Schema search [{query}]: ");
        let _ = out.flush();
        let line = reader.read_line()?;
        let q = line.trim();
        if !q.is_empty() {
            query = q.to_string();
        }
        let idx = filter_entries(entries, &query);
        if idx.is_empty() {
            let _ = writeln!(out, "  no matches");
            continue 'search;
        }
        let shown = idx.len().min(15);
        for i in 0..shown {
            let e = &entries[idx[i]];
            if !e.description.is_empty() {
                let _ = writeln!(out, "  {}) {} — {}", i + 1, e.name, e.description);
            } else {
                let _ = writeln!(out, "  {}) {}", i + 1, e.name);
            }
        }
        let more = idx.len() - shown;
        if more > 0 {
            let _ = writeln!(out, "  … {more} more — refine the search");
        }
        loop {
            let _ = write!(out, "? Number (0 = new search, enter = abort): ");
            let _ = out.flush();
            let line = reader.read_line()?;
            if line.trim().is_empty() {
                // enter = abort
                return None;
            }
            match line.trim().parse::<usize>() {
                Ok(0) => continue 'search, // new search
                Ok(n) if n <= idx.len() => return Some(entries[idx[n - 1]].clone()),
                _ => {
                    let _ = writeln!(out, "! invalid number"); // re-issue the prompt
                }
            }
        }
    }
}

use std::time::Duration;

/// Cap one drained picker burst so a flood on stdin cannot starve rendering.
const MAX_PICKER_BURST: usize = 1024;

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        vec![
            Entry {
                name: "Package".into(),
                description: "package.json schema".into(),
                url: "https://a".into(),
                file_match: vec![],
            },
            Entry {
                name: "Tsconfig".into(),
                description: "TypeScript compiler configuration".into(),
                url: "https://b".into(),
                file_match: vec![],
            },
        ]
    }

    #[test]
    fn filter_matches_name_or_description_case_insensitive() {
        let e = entries();
        assert_eq!(filter_entries(&e, ""), vec![0, 1]);
        assert_eq!(filter_entries(&e, "pack"), vec![0]);
        assert_eq!(filter_entries(&e, "SCHEMA"), vec![0]);
        assert_eq!(filter_entries(&e, "compiler"), vec![1]);
        assert!(filter_entries(&e, "zzz").is_empty());
    }

    #[test]
    fn stale_query_replaces_on_first_text_key() {
        let mut m = PickerModel::new(entries(), "x.json".into(), "pack");
        assert!(m.stale);
        assert_eq!(m.filtered.len(), 1);
        m.handle(&PickerKey::Widget(Key::Char('t')));
        assert!(!m.stale);
        assert_eq!(m.ti.value(), "t");
        // cursor keys do not clear the staleness
        let mut m2 = PickerModel::new(entries(), "x.json".into(), "pack");
        m2.handle(&PickerKey::Up);
        assert!(m2.stale, "cursor keys do not clear the staleness");
    }

    #[test]
    fn enter_without_results_does_not_choose() {
        let mut m = PickerModel::new(entries(), "x.json".into(), "");
        m.ti.set_value("zzz");
        m.refilter();
        assert!(m.handle(&PickerKey::Enter).is_none());
    }

    #[test]
    fn frame_uses_crlf_for_raw_mode() {
        // A lone LF under raw mode keeps the cursor column, producing the
        // staircase layout seen on a real terminal.
        assert_eq!(frame("a\nb\nc"), "a\r\nb\r\nc");
        assert!(!frame("a\nb").ends_with('\n'));
    }

    #[test]
    fn view_fills_height_without_trailing_newline() {
        let mut m = PickerModel::new(entries(), "x.json".into(), "");
        m.width = 40;
        m.height = 10;
        let v = m.view();
        assert!(v.contains("Select a schema"), "{v:?}");
        assert!(
            !v.ends_with('\n'),
            "a trailing newline scrolls the screen: {v:?}"
        );
        assert_eq!(v.matches('\n').count(), 9, "help must be the last row");
        assert!(
            !v.split('\n').nth(2).unwrap().contains("\x1b[38;5;248m"),
            "the search row must use the default white foreground"
        );
        for line in v.split('\n') {
            assert!(
                display_width(line) <= 40,
                "{line:?} is wider than the terminal"
            );
        }
    }

    #[test]
    fn view_clips_wide_characters_to_one_row() {
        let entries = vec![Entry {
            name: "中文配置项非常非常长的名称测试".into(),
            description:
                "这是一个用于测试排版的中文描述，包含很多宽字符，应该被正确截断而不是换行破坏布局。"
                    .into(),
            url: "https://a".into(),
            file_match: vec![],
        }];
        let mut m = PickerModel::new(entries, "配置文件.json".into(), "");
        m.width = 40;
        m.height = 12;
        let v = m.view();
        for line in v.split('\n') {
            assert!(
                display_width(line) <= 40,
                "wide row wraps past the terminal: {line:?} ({} cells)",
                display_width(line)
            );
        }
    }

    #[test]
    fn search_line_scrolls_to_keep_caret_visible() {
        let mut m = PickerModel::new(entries(), "x.json".into(), "");
        m.width = 30;
        m.ti.set_value("这是一个非常长的搜索词用来测试水平滚动abcdef");
        let v = m.view();
        let line = v.split('\n').nth(2).expect("search row");
        assert!(display_width(line) <= 30, "{line:?} wraps");
        assert!(line.contains("f"), "the tail must stay visible: {line:?}");
    }

    #[test]
    fn scroll_window_rules() {
        assert_eq!(scroll_window(0, 0, 3, 10), (0, 0));
        assert_eq!(scroll_window(5, 0, 10, 3), (5, 3));
        assert_eq!(scroll_window(2, 5, 10, 3), (2, 2));
        assert_eq!(scroll_window(9, 0, 10, 3), (9, 7));
    }
}
