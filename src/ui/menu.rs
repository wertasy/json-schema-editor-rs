//! The menu TUI core: App construction, the crossterm event loop, mouse
//! support and the render entry (Go menu.go). Key dispatch and the save flow
//! live in [`keys`](crate::ui::keys); shared state in [`state`](crate::ui::state).
//!
//! Rust port of `internal/ui/menu.go`. The bubbletea `listModel` becomes
//! [`App`]; the 16 nullable overlay fields + `routeKey` priority chain become
//! a single `Option<Overlay>`. Per-file state lives in
//! [`FileState`](crate::ui::files::FileState); the active file is
//! `files[cur]`. View rendering keeps Go's string-builder approach: a tiny
//! `style` layer emits the same ANSI SGR sequences lipgloss produced.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;

use crate::ui::files::new_app;
pub use crate::ui::state::*;
pub use crate::ui::style;
use crate::ui::{FileSession, Outcome};

// --- key messages ------------------------------------------------------------

impl App {
    /// newListModel/newMultiFileModel entry (Go newMultiFileModel).
    pub fn new(files: Vec<FileSession>) -> App {
        new_app(files)
    }

    /// startSession arms a real terminal run: $EDITOR edits exec for real
    /// (tests keep the stash path), and the pty size becomes the viewport.
    pub fn start_session(&mut self, w: u16, h: u16) {
        self.exec_editor = true;
        self.set_viewport(w, h);
    }

    // --- mouse support ------------------------------------------------------

    /// scroll_to clamps a scroll offset moved by one wheel tick (+1 = up).
    fn scroll_to(cur: usize, delta: i64, max: usize) -> usize {
        (cur as i64 - delta).clamp(0, max as i64) as usize
    }

    /// mouse_target_at maps a screen line (0-based, terminal coordinates) to
    /// what was rendered there in the last frame, if anything.
    pub fn mouse_target_at(&self, line: usize) -> Option<MouseTarget> {
        self.mouse_targets
            .iter()
            .find(|(start, n, _)| line >= *start && line < start + n)
            .map(|(.., t)| *t)
    }

    /// mouse_click is the left button: on a menu row it moves the cursor
    /// there and presses Enter; on a dropdown candidate or an overlay list
    /// row it selects that entry (cursor + Enter). Clicks on lines with no
    /// target (header, help bar, blank space, scroll pages) do nothing.
    pub fn mouse_click(&mut self, line: usize) -> bool {
        let Some(target) = self.mouse_target_at(line) else {
            return false;
        };
        match target {
            MouseTarget::MenuRow(idx) => {
                // The inline surfaces that render over the list (scalar
                // edit, add-key, anchor input, merge input) share the row
                // map; input overlays parked in the bottom block (write-to,
                // `:` command) must not have a stray click commit them.
                if !matches!(
                    self.overlay,
                    None | Some(Overlay::Edit(_))
                        | Some(Overlay::Add(_))
                        | Some(Overlay::Anc(_))
                        | Some(Overlay::Mrg(_))
                ) {
                    return false;
                }
                if let Some(st) = self.top_mut() {
                    st.cursor = idx;
                }
                self.update(Msg::Enter);
                true
            }
            MouseTarget::EditSug(k) => {
                let Some(Overlay::Edit(mut e)) = self.overlay.take() else {
                    return false;
                };
                let Some(v) = e.sugs.get(k).cloned() else {
                    self.overlay = Some(Overlay::Edit(e));
                    return false;
                };
                e.ti.set_value(v);
                e.ti.cursor_end();
                e.sug_idx = k as i64;
                self.overlay = Some(Overlay::Edit(e));
                self.update(Msg::Enter);
                true
            }
            MouseTarget::AncSug(k) => {
                let Some(Overlay::Anc(mut s)) = self.overlay.take() else {
                    return false;
                };
                let Some(v) = s.sugs.get(k).cloned() else {
                    self.overlay = Some(Overlay::Anc(s));
                    return false;
                };
                s.sug = k as i64;
                s.ti.set_value(v.clone());
                s.ti.cursor_end();
                s.sug_line = v;
                self.overlay = Some(Overlay::Anc(s));
                self.update(Msg::Enter);
                true
            }
            MouseTarget::OverlayRow(k) => {
                eprintln!(
                    "DBG OverlayRow k={k} overlay={:?}",
                    std::mem::discriminant(&self.overlay)
                );
                let picked = match self.overlay.take() {
                    Some(Overlay::Issues(mut s)) => {
                        s.cursor = k;
                        self.overlay = Some(Overlay::Issues(s));
                        true
                    }
                    Some(Overlay::Errs(mut s)) => {
                        s.cursor = k;
                        self.overlay = Some(Overlay::Errs(s));
                        true
                    }
                    Some(Overlay::RefPick(mut s)) => {
                        s.cursor = k;
                        self.overlay = Some(Overlay::RefPick(s));
                        true
                    }
                    Some(Overlay::FilePick(mut s)) => {
                        s.cursor = k;
                        self.overlay = Some(Overlay::FilePick(s));
                        true
                    }
                    Some(Overlay::Search(mut s)) => {
                        s.cursor = k;
                        self.overlay = Some(Overlay::Search(s));
                        true
                    }
                    other => {
                        self.overlay = other;
                        false
                    }
                };
                if picked {
                    self.update(Msg::Enter);
                }
                picked
            }
        }
    }

    /// mouse_back is the right button: the same as esc (go back / close the
    /// active surface).
    pub fn mouse_back(&mut self) {
        self.update(Msg::Esc);
    }

    /// mouse_hover moves the plain-list cursor to the hovered row (mouse
    /// moved over the menu). Active only with no overlay open and no live
    /// selection: editing/overlays own the pointer, and a hover must not
    /// extend a visual select. Returns whether the screen changed.
    pub fn mouse_hover(&mut self, line: usize) -> bool {
        if self.overlay.is_some() {
            return false;
        }
        let Some(MouseTarget::MenuRow(idx)) = self.mouse_target_at(line) else {
            return false;
        };
        let Some(st) = self.top_mut() else {
            return false;
        };
        if st.cursor == idx || st.visual {
            return false;
        }
        st.cursor = idx;
        true
    }

    /// overlay_wheel scrolls the full-screen scrollable pages (help,
    /// comment, shell output) by delta (+1 = wheel up). Other surfaces
    /// ignore the wheel; returns whether the screen changed.
    pub fn overlay_wheel(&mut self, delta: i64) -> bool {
        let page_lines = |len: usize, height: usize| len.saturating_sub(height.max(1));
        let moved = match self.overlay.take() {
            Some(Overlay::Help(mut s)) => {
                let max = page_lines(HELP_ENTRIES.len(), self.height.saturating_sub(3));
                s.scroll = Self::scroll_to(s.scroll, delta, max);
                self.overlay = Some(Overlay::Help(s));
                true
            }
            Some(Overlay::Comment(mut s)) => {
                let max = page_lines(s.lines.len(), self.height.saturating_sub(2));
                s.scroll = Self::scroll_to(s.scroll, delta, max);
                self.overlay = Some(Overlay::Comment(s));
                true
            }
            Some(Overlay::ShellOut(mut s)) => {
                let max = page_lines(s.lines.len(), self.height.saturating_sub(2));
                s.scroll = Self::scroll_to(s.scroll, delta, max);
                self.overlay = Some(Overlay::ShellOut(s));
                true
            }
            other => {
                self.overlay = other;
                false
            }
        };
        moved
    }

    /// The outcome of a finished session (Go: saveReport + flags).
    pub fn outcome(&self) -> Outcome {
        let (saved, forced, paths) = self.save_report();
        Outcome {
            saved,
            with_errors: forced,
            aborted: self.aborted,
            unchanged: self.exiting && !saved,
            saved_files: paths,
            err: None,
        }
    }
}

// --- terminal plumbing -------------------------------------------------------

fn key_event_to_msg(key: KeyEvent) -> Option<Msg> {
    // Press and terminal auto-repeat both act; only the release is dropped.
    // Some terminals report repeats as KeyEventKind::Repeat (Windows, kitty
    // keyboard protocol), others as plain presses.
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                Some(Msg::Ctrl(c.to_ascii_lowercase()))
            } else {
                Some(Msg::Char(c, alt))
            }
        }
        KeyCode::Esc => Some(Msg::Esc),
        KeyCode::Enter => Some(Msg::Enter),
        KeyCode::Tab => Some(Msg::Tab),
        KeyCode::BackTab => Some(Msg::BackTab),
        KeyCode::Backspace => Some(Msg::Backspace),
        KeyCode::Delete => Some(Msg::Delete),
        KeyCode::Up => Some(Msg::Up),
        KeyCode::Down => Some(Msg::Down),
        KeyCode::Left => Some(Msg::Left),
        KeyCode::Right => Some(Msg::Right),
        KeyCode::Home => Some(Msg::Home),
        KeyCode::End => Some(Msg::End),
        KeyCode::PageUp => Some(Msg::PageUp),
        KeyCode::PageDown => Some(Msg::PageDown),
        _ => None,
    }
}

/// run_menu drives the whole interactive TUI session: crossterm raw mode +
/// alternate screen, the render/poll loop, and the exit outcome.
pub fn run_menu(files: Vec<FileSession>) -> Outcome {
    let mut app = App::new(files);
    let mut stdout = std::io::stdout();
    let _ = crossterm::terminal::enable_raw_mode();
    // bubbletea delivers an initial WindowSizeMsg; crossterm does not, so the
    // first frame must query the pty size explicitly (otherwise it renders at
    // the 80x24 default). start_session also arms $EDITOR: an interactive run
    // execs external edits for real, while tests build via App::new and keep
    // them stashed.
    let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
    app.start_session(w, h);
    let _ = execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableMouseCapture,
        crossterm::cursor::Hide
    );
    // render on events only: the old loop repainted every poll timeout, so
    // an idle session burned CPU building every menu row 4x a second
    render(&mut stdout, &mut app);
    loop {
        // Block for the first event, then drain everything the terminal has
        // already queued before repainting. A held j/k delivers key repeats
        // faster than a long menu can repaint; processing one event per frame
        // left those repeats piled up in the tty buffer, so the cursor kept
        // moving for seconds after the key was released. One burst = one
        // coalesced move + one frame.
        let events = read_event_burst();
        let mut redraw = false;
        if events.is_empty() {
            // Idle tick (the 250ms poll timeout): watch the active file's
            // disk state. An external change opens the reload confirm; a
            // live fetch progress keeps the help-bar bar animating. An
            // unchanged file costs one stat per tick and no repaint.
            redraw = app.poll_disk_change() || crate::progress::progress().is_some();
        }
        redraw |= app.poll_jobs();
        let mut i = 0;
        while i < events.len() {
            match &events[i] {
                Event::Key(k) => {
                    let Some(msg) = key_event_to_msg(*k) else {
                        i += 1;
                        continue;
                    };
                    // Consecutive identical motion keys collapse into a
                    // single move: replaying every repeat rebuilt the whole
                    // row list and repainted the whole menu.
                    let mut n = 1;
                    if vertical_delta(&msg).is_some() || page_delta(&msg).is_some() {
                        while i + n < events.len() {
                            match &events[i + n] {
                                Event::Key(k2) if key_event_to_msg(*k2).as_ref() == Some(&msg) => {
                                    n += 1;
                                }
                                _ => break,
                            }
                        }
                    }
                    app.update_repeat(msg, n);
                    app.run_pending_editor();
                    redraw = true;
                    i += n;
                }
                Event::Resize(w, h) => {
                    app.set_viewport(*w, *h);
                    redraw = true;
                    i += 1;
                }
                Event::Paste(s) => {
                    app.update(Msg::Paste(s.clone()));
                    app.run_pending_editor();
                    redraw = true;
                    i += 1;
                }
                Event::Mouse(m) => {
                    // Some sequences (press/release pairs) encode one wheel
                    // tick twice; consecutive identical scroll events in one
                    // burst are one tick, like collapsed key repeats.
                    let scroll = match m.kind {
                        MouseEventKind::ScrollUp => Some(1),
                        MouseEventKind::ScrollDown => Some(-1),
                        _ => None,
                    };
                    if scroll.is_some()
                        && i > 0
                        && matches!(&events[i - 1], Event::Mouse(m2) if m2.kind == m.kind)
                    {
                        i += 1;
                        continue;
                    }
                    match m.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            redraw = app.mouse_click(m.row as usize);
                        }
                        MouseEventKind::Down(MouseButton::Right) => {
                            app.mouse_back();
                            redraw = true;
                        }
                        MouseEventKind::ScrollUp => {
                            redraw = app.overlay_wheel(1) || app.edit_wheel(1);
                        }
                        MouseEventKind::ScrollDown => {
                            redraw = app.overlay_wheel(-1) || app.edit_wheel(-1);
                        }
                        MouseEventKind::Moved => redraw = app.mouse_hover(m.row as usize),
                        _ => {}
                    }
                    if redraw {
                        app.run_pending_editor();
                    }
                    i += 1;
                }
                _ => i += 1,
            }
            if app.aborted || app.exiting {
                break;
            }
        }
        if redraw {
            render(&mut stdout, &mut app);
        }
        if app.aborted || app.exiting {
            break;
        }
    }
    let _ = execute!(
        stdout,
        crossterm::cursor::Show,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    );
    let _ = crossterm::terminal::disable_raw_mode();
    app.outcome()
}

/// Upper bound on one drained burst so a flood on stdin cannot starve
/// rendering indefinitely; held-key repeats arrive far below this.
const MAX_EVENT_BURST: usize = 4096;

/// read_event_burst blocks for the next event, then collects every event
/// already buffered by the terminal (or crossterm) without blocking again.
fn read_event_burst() -> Vec<Event> {
    let mut events = Vec::new();
    if !crossterm::event::poll(std::time::Duration::from_millis(250)).unwrap_or(false) {
        return events;
    }
    while events.len() < MAX_EVENT_BURST {
        match crossterm::event::read() {
            Ok(e) => events.push(e),
            Err(_) => break,
        }
        if !crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
            break;
        }
    }
    events
}

/// render paints one frame: MoveTo(0,0), every view line + clear-to-EOL,
/// then clear the remaining lines.
fn render(w: &mut impl std::io::Write, app: &mut App) {
    let view = app.view();
    write_frame(w, &view, app.height);
}

/// write_frame paints at most `height` lines: a view that outgrows the
/// terminal (raw editor on a large file) must not flood it line by line.
fn write_frame(w: &mut impl std::io::Write, view: &str, height: usize) {
    // One buffered write instead of an execute! (and flush) per line: a
    // repaint of a full-screen menu used to cost 2 syscalls per row, which
    // is pure latency when key repeats are waiting behind the frame.
    let mut out = String::with_capacity(view.len() + height * 8);
    out.push_str("\x1b[1;1H"); // MoveTo(0,0)
    let mut lines = 0;
    for line in view.split('\n') {
        if lines >= height {
            break;
        }
        out.push_str(line);
        out.push_str("\x1b[K"); // Clear(UntilNewLine)
        if lines + 1 < height {
            out.push_str("\r\n");
        }
        lines += 1;
    }
    out.push_str("\x1b[J"); // Clear(FromCursorDown)
    let _ = w.write_all(out.as_bytes());
    let _ = w.flush();
}

// --- clipboard shell-outs (Go atotto/clipboard fallback) ---------------------

/// Which platform-specific clipboard helpers exist.
fn tool_in_path(tool: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(tool))
        .any(|p| p.is_file())
}

fn feed_tool(tool: &str, args: &[&str], text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(tool)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} exited with {}", tool, status))
    }
}

fn read_tool(tool: &str, args: &[&str]) -> Result<String, String> {
    use std::process::Command;
    let out = Command::new(tool)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("{} exited with {}", tool, out.status));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// write_clipboard is the Go `clipboard.WriteAll` fallback used when
/// opts.Copy is unset: shell out to the first present helper (Linux
/// wl-copy → xclip → xsel; macOS pbcopy; Windows clip).
pub fn write_clipboard(text: &str) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        if tool_in_path("pbcopy") {
            return feed_tool("pbcopy", &[], text);
        }
    } else if cfg!(target_os = "windows") {
        if tool_in_path("clip") {
            return feed_tool("clip", &[], text);
        }
    } else {
        if tool_in_path("wl-copy") {
            return feed_tool("wl-copy", &[], text);
        }
        if tool_in_path("xclip") {
            return feed_tool("xclip", &["-selection", "clipboard"], text);
        }
        if tool_in_path("xsel") {
            return feed_tool("xsel", &["--clipboard", "--input"], text);
        }
    }
    Err("clipboard: no clipboard tool found".to_string())
}

/// read_clipboard is the Go `clipboard.ReadAll` fallback (opts.Paste
/// unset). Windows has no paste helper in the atotto set.
pub fn read_clipboard() -> Result<String, String> {
    if cfg!(target_os = "macos") {
        if tool_in_path("pbpaste") {
            return read_tool("pbpaste", &[]);
        }
    } else if cfg!(target_os = "windows") {
        return Err("clipboard: cannot read without paste support".to_string());
    } else {
        if tool_in_path("wl-paste") {
            return read_tool("wl-paste", &["--no-newline"]);
        }
        if tool_in_path("xclip") {
            return read_tool("xclip", &["-selection", "clipboard", "-o"]);
        }
        if tool_in_path("xsel") {
            return read_tool("xsel", &["--clipboard", "--output"]);
        }
    }
    Err("clipboard: no clipboard tool found".to_string())
}

#[cfg(test)]
mod frame_tests {
    use super::{key_event_to_msg, write_frame};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    #[test]
    fn auto_repeat_acts_and_release_is_ignored() {
        let repeat =
            KeyEvent::new_with_kind(KeyCode::Char('j'), KeyModifiers::NONE, KeyEventKind::Repeat);
        assert_eq!(
            key_event_to_msg(repeat),
            Some(super::Msg::Char('j', false)),
            "held-key repeat must move the cursor"
        );
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(key_event_to_msg(release), None, "release must not move");
    }

    #[test]
    fn write_frame_stops_at_the_terminal_height() {
        let view = (0..50).map(|i| format!("l{i}\r\n")).collect::<String>();
        let mut out: Vec<u8> = Vec::new();
        write_frame(&mut out, &view, 4);
        let text = String::from_utf8_lossy(&out);
        for i in 0..4 {
            assert!(text.contains(&format!("l{i}")), "line {i} missing");
        }
        assert!(
            !text.contains("l4"),
            "lines past the height must not be painted"
        );
    }
}
