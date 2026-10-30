//! The menu TUI core: App state, the crossterm event loop, key dispatch and
//! rendering (Go menu.go). Populated by the core porting slice.
//!
//! Rust port of `internal/ui/menu.go`. The bubbletea `listModel` becomes
//! [`App`]; the 16 nullable overlay fields + `routeKey` priority chain become
//! a single `Option<Overlay>`. Per-file state lives in
//! [`FileState`](crate::ui::files::FileState); the active file is
//! `files[cur]`. View rendering keeps Go's string-builder approach: a tiny
//! `style` layer emits the same ANSI SGR sequences lipgloss produced.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use regex::Regex;

use crate::doc::jsondoc::path_string;
use crate::doc::{Doc, NodeId, PathSeg, Value};
use crate::smodel::{FormNode, FormNodeId};
use crate::ui::files::{disk_stat_of, new_app};
use crate::ui::nav::{Nav, NavGo};
use crate::ui::rows::{
    MergeView, MergedKey, RowsEnv, SynthKey, dynamic_variants, match_variant_list, menu_rows_for,
    parse_issue, pinned_one_of_menu, unknown_key_issues,
};
use crate::ui::widgets::{TextArea, TextInput};
use crate::ui::{FileSession, Issue, MenuCtx, MenuKind, MenuState, Options, Outcome, Row, RowKind};

pub use crate::ui::files::FileState;

// --- styles (Go lipgloss constants) ---------------------------------------------

/// Tiny style layer producing the ANSI SGR sequences lipgloss emitted. Each
/// render closes with ESC[0m like lipgloss.
pub mod style {
    fn render(codes: &str, s: &str) -> String {
        if codes.is_empty() {
            return s.to_string();
        }
        format!("\x1b[{}m{}\x1b[0m", codes, s)
    }
    pub fn bold(s: &str) -> String {
        render("1", s)
    }
    pub fn faint(s: &str) -> String {
        render("2", s)
    }
    pub fn fg3(s: &str) -> String {
        render("33", s)
    }
    pub fn fg1(s: &str) -> String {
        render("31", s)
    }
    pub fn fg9(s: &str) -> String {
        render("91", s)
    }
    pub fn help(s: &str) -> String {
        render("38;5;248", s)
    }
    pub fn desc(s: &str) -> String {
        render("38;5;141", s)
    }
    /// Document comments use a different color from schema help in the
    /// description panel.
    pub fn comment(s: &str) -> String {
        render("38;5;117", s)
    }
    pub fn anchor(s: &str) -> String {
        render("38;5;177", s)
    }
    pub fn accent(s: &str) -> String {
        render("38;5;39", s)
    }
    pub fn sel(s: &str) -> String {
        render("1;48;5;236", s)
    }
    pub fn val_str(s: &str) -> String {
        render("38;5;114", s)
    }
    pub fn val_num(s: &str) -> String {
        render("38;5;51", s)
    }
    pub fn val_bool(s: &str) -> String {
        render("38;5;212", s)
    }
    pub fn val_enum(s: &str) -> String {
        render("38;5;215", s)
    }
    pub fn val_default(s: &str) -> String {
        render("38;5;245", s)
    }
    pub fn diff(s: &str) -> String {
        render("1;38;5;204", s)
    }
    /// The selected completion candidate row renders in reverse video across
    /// the whole padded row rectangle.
    pub fn sug_sel(s: &str) -> String {
        render("7", s)
    }
    pub fn sug_dim(s: &str) -> String {
        render("2;48;5;236", s)
    }
}

pub const MENU_HELP: &str =
    "↑/↓ move · enter edit/open · esc back · ctrl+s/w save · / search · : command · ? help";
/// Collapsed description cap per source: the schema help and the document
/// comment each show at most this many wrapped lines; a cut block gets an
/// emphasized trailing `…`.
pub const DESC_BLOCK_PREVIEW_LINES: usize = 2;

/// DescSource selects the color of one description-panel line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescSource {
    Schema,
    Comment,
}

/// DescLine is one wrapped line of the focused row's description panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescLine {
    pub source: DescSource,
    pub text: String,
    /// the block was cut here: render an emphasized `…` after the text
    pub folded: bool,
}

/// maxMacroKeys caps a single recorded macro; reaching it stops recording
/// and keeps the keys captured so far.
pub const MAX_MACRO_KEYS: usize = 500;

/// helpColWidth is the keyboard-help overlay's fixed description column.
pub const HELP_COL_WIDTH: usize = 24;
/// The menu's title column adapts to the widest key, within these bounds.
pub const TITLE_COL_MIN: usize = 16;
pub const TITLE_COL_MAX: usize = 42;

/// helpEntry is one row of the "?" key reference.
pub struct HelpEntry {
    pub keys: &'static str,
    pub desc: &'static str,
}

/// helpEntries is the full key reference shown by the "?" overlay.
pub const HELP_ENTRIES: &[HelpEntry] = &[
    HelpEntry {
        keys: "up / down / j / k",
        desc: "move (wraps at both ends; ctrl+j/k in search and issues)",
    },
    HelpEntry {
        keys: "ctrl+f / pgdown, ctrl+b / pgup",
        desc: "page down / up (menus, list overlays, comment and help pages)",
    },
    HelpEntry {
        keys: "n / N",
        desc: "next / previous key with a non-null value",
    },
    HelpEntry {
        keys: "ctrl+j / ctrl+k",
        desc: "adaptive next / previous: changed keys from the last replace/undo/redo, then keys with validation issues, then search hits; falls back to the non-null walk",
    },
    HelpEntry {
        keys: "y / Y",
        desc: "copy the \"key\": value pair / the value (JSON, YAML, or TOML per document); array pages: both copy the item's value",
    },
    HelpEntry {
        keys: "x / X",
        desc: "cut the \"key\": value pair / the value — y/Y plus delete, batch on marked rows; array pages: both cut the item",
    },
    HelpEntry {
        keys: "m",
        desc: "mark key-value rows / array items: y copies, d deletes the marked ones (m toggles)",
    },
    HelpEntry {
        keys: "v",
        desc: "visual select: j/k (or arrows) and gg/G mark rows between start and cursor; v/esc stops",
    },
    HelpEntry {
        keys: "f / F",
        desc: "fast move: type a character to jump to the next/previous key starting with it",
    },
    HelpEntry {
        keys: "p / P",
        desc: "smart paste clipboard JSON (YAML/TOML docs: that syntax too) — p: this menu, P: the row's own container (a single value replaces its value); keys/items land below (p) / above (P) the cursor row",
    },
    HelpEntry {
        keys: "z",
        desc: "show/hide fields with no value (null/unset)",
    },
    HelpEntry {
        keys: "- / = / +",
        desc: "number: subtract / add 1 (missing schema-typed values seed -1/1; schema-less/array-item numbers use their runtime value)",
    },
    HelpEntry {
        keys: "[N] gg / G",
        desc: "jump to first / last row; N gg in array menus: item N (Home/End too); visual select: lands on the first/last key, marking the range",
    },
    HelpEntry {
        keys: "ctrl+o / ctrl+i (tab)",
        desc: "back / forward in the navigation history (submenus, jumps; esc pops count too)",
    },
    HelpEntry {
        keys: "s",
        desc: "switch file: open the file picker when several files are open",
    },
    HelpEntry {
        keys: "S",
        desc: "switch schema: search SchemaStore and rebuild this file's form (the document body is kept)",
    },
    HelpEntry {
        keys: "ga / gr",
        desc: "jump to the row's anchor / its other uses — references and << merges (esc returns)",
    },
    HelpEntry {
        keys: "enter / right / l",
        desc: "edit field · open submenu",
    },
    HelpEntry {
        keys: "tab / shift+tab",
        desc: "complete next / previous candidate (ctrl+n / ctrl+p, ctrl+j / ctrl+k)",
    },
    HelpEntry {
        keys: "left / h",
        desc: "back one level",
    },
    HelpEntry {
        keys: "esc / q",
        desc: "cancel / back — q aliases esc outside inputs; a pending g combo clears first, then a root selection",
    },
    HelpEntry {
        keys: "",
        desc: "at the root: quit, asking to save when the doc changed",
    },
    HelpEntry {
        keys: "space",
        desc: "toggle boolean / enum member",
    },
    HelpEntry {
        keys: "d / delete",
        desc: "delete the row/item (no confirmation; required merge keys refused)",
    },
    HelpEntry {
        keys: "a / A",
        desc: "add a schema-less key before / after the cursor",
    },
    HelpEntry {
        keys: "e",
        desc: "edit the row's value in $EDITOR (multiline strings always open here)",
    },
    HelpEntry {
        keys: "E",
        desc: "edit the focused row's value in $EDITOR (raw JSON/YAML/TOML; whole document when the cursor is not on a value row)",
    },
    HelpEntry {
        keys: "&",
        desc: "anchor: set/rename the field's YAML anchor (empty removes; unreferenced only)",
    },
    HelpEntry {
        keys: "*",
        desc: "reference: point the field at an anchor (tab completes; empty removes)",
    },
    HelpEntry {
        keys: "J / K",
        desc: "move the item under the cursor down / up",
    },
    HelpEntry {
        keys: "t",
        desc: "switch a oneOf value's variant type / open the type picker for untyped values",
    },
    HelpEntry {
        keys: "u / ctrl+r",
        desc: "undo / redo",
    },
    HelpEntry {
        keys: "ctrl+s / w",
        desc: "save (the bottom bar shows the write target)",
    },
    HelpEntry {
        keys: "W",
        desc: "write a copy to another path: the menu grows a \"Write to\" row prefilled with this file's path — edit and enter (esc cancels); tab completes directory entries",
    },
    HelpEntry {
        keys: "/",
        desc: "search below the current path",
    },
    HelpEntry {
        keys: "r",
        desc: "search & replace string values",
    },
    HelpEntry {
        keys: "alt+x",
        desc: "regex toggle (default on): match the query as a regular expression ($1 groups in replace)",
    },
    HelpEntry {
        keys: "@ / .",
        desc: "record / play a macro",
    },
    HelpEntry {
        keys: "c",
        desc: "description: expand/fold; too long for one page opens a scrollable page (↑/↓ or j/k scroll, c/q/esc close)",
    },
    HelpEntry {
        keys: "C",
        desc: "edit the document comment of the focused key (empty text removes it)",
    },
    HelpEntry {
        keys: "?",
        desc: "open/close this help",
    },
    HelpEntry {
        keys: "ctrl+c",
        desc: "quit without saving (exit 130)",
    },
    HelpEntry {
        keys: "<",
        desc: "merge keys: merge anchors (space/comma separated; tab completes; empty clears)",
    },
    HelpEntry {
        keys: "ctrl+v",
        desc: "paste the clipboard into the focused text input (terminal ctrl+shift+v / bracketed paste works too)",
    },
    HelpEntry {
        keys: "D",
        desc: "remove the current array item (from the item list or its form; asks y/n)",
    },
    HelpEntry {
        keys: "ctrl+e",
        desc: "expand: materialize the referenced value (cursor row or menu) into a copy",
    },
    HelpEntry {
        keys: ":",
        desc: "command line: w saves, wa saves all dirty files, wq saves and quits, waq saves all and quits (append ! to force), q quits, q! discards and quits; r !<cmd> inserts stdout into the focused value, a !<cmd> appends stdout lines to a string array, w !<cmd> pipes the document to stdin, !<cmd> only runs; tab completes builtins or (after !) PATH commands, bash completion and paths, % expands to the session file path; candidates appear automatically and filter as you type; ↑/↓ are history until a candidate is selected; history persists across sessions (cache dir); !!+enter repeats the last shell command; multi-line output stays an inline multi-line notice when it fits the blank rows above the help bar, else opens a scrollable page (j/k, PgUp/PgDn, q/esc)",
    },
];

// --- key messages ------------------------------------------------------------

/// Msg is the TUI key event (Go tea.KeyMsg), reduced to the shapes the menu
/// handles. `key_str` reproduces bubbletea's `msg.String()` values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// rune keys; the flag marks alt-modified runes ("alt+x")
    Char(char, bool),
    Esc,
    Enter,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Ctrl(char),
    /// bracketed paste text
    Paste(String),
}

impl Msg {
    /// String() reproduces the Go bubbletea key names the dispatchers match
    /// on: "up", "ctrl+j", "alt+x", "tab", "shift+tab", runes as the string.
    pub fn key_str(&self) -> String {
        match self {
            Msg::Char(' ', false) => " ".to_string(),
            Msg::Char(c, false) => c.to_string(),
            Msg::Char(c, true) => format!("alt+{}", c),
            Msg::Esc => "esc".to_string(),
            Msg::Enter => "enter".to_string(),
            Msg::Tab => "tab".to_string(),
            Msg::BackTab => "shift+tab".to_string(),
            Msg::Backspace => "backspace".to_string(),
            Msg::Delete => "delete".to_string(),
            Msg::Up => "up".to_string(),
            Msg::Down => "down".to_string(),
            Msg::Left => "left".to_string(),
            Msg::Right => "right".to_string(),
            Msg::Home => "home".to_string(),
            Msg::End => "end".to_string(),
            Msg::PageUp => "pgup".to_string(),
            Msg::PageDown => "pgdown".to_string(),
            Msg::Ctrl(c) => format!("ctrl+{}", c),
            Msg::Paste(_) => String::new(),
        }
    }
}

/// vertical_delta classifies the keys that move a list cursor one row: the
/// menu's j/k aliases and the arrow keys. Terminal auto-repeat delivers a
/// stream of these; the event loop coalesces consecutive identical ones so a
/// long menu pays for one cursor step (and one repaint) per input burst
/// instead of one per queued repeat.
pub fn vertical_delta(msg: &Msg) -> Option<i64> {
    match msg {
        Msg::Up => Some(-1),
        Msg::Down => Some(1),
        Msg::Char('k', false) => Some(-1),
        Msg::Char('j', false) => Some(1),
        _ => None,
    }
}

/// page_delta classifies the keys that move a list cursor or page scroll by
/// one screenful: ctrl+f/PgDn forward, ctrl+b/PgUp backward. Like
/// vertical_delta it is side-effect free, so repeats can be coalesced.
pub fn page_delta(msg: &Msg) -> Option<i64> {
    match msg {
        Msg::PageDown | Msg::Ctrl('f') => Some(1),
        Msg::PageUp | Msg::Ctrl('b') => Some(-1),
        _ => None,
    }
}

// --- overlay states ----------------------------------------------------------

/// editState is an in-place scalar input on one field row.
pub struct EditState {
    pub fn_: Option<FormNodeId>,
    /// schema-less scalar edits carry a synthetic node (Go built one on the
    /// fly) matching the value's own JSON type
    pub synth: Option<FormNode>,
    pub path: Vec<PathSeg>,
    pub ti: TextInput,
    pub err: String,
    /// editing a not-yet-appended array item; esc writes nothing
    pub is_new: bool,
    /// existing same-key values in the document (fixed at open)
    pub cands: Vec<String>,
    /// input prefix the current suggestion list was built from
    pub sug_prefix: String,
    /// cands filtered by the current input prefix
    pub sugs: Vec<String>,
    /// tab cycle position (-1 = none)
    pub sug_idx: i64,
    /// display column of the caret when the current suggestion list was
    /// built; the anchor for `…tail` candidate rows (full rows anchor at
    /// the input text start)
    pub sug_col: usize,
}

/// addState is the key-name input overlay for a: add a schema-less
/// key-value member under the current object menu.
pub struct AddState {
    /// object the new member goes into
    pub path: Vec<PathSeg>,
    /// the menu's node; its Children are the known keys
    pub node: FormNodeId,
    /// member insert position captured from the cursor (a/A)
    pub idx: usize,
    pub ti: TextInput,
    pub err: String,
}

/// writeState is the write-to-path input for W.
pub struct WriteState {
    pub ti: TextInput,
    pub err: String,
    /// input prefix the current suggestion list was built from
    pub sug_prefix: String,
    pub sugs: Vec<String>,
    pub sug_idx: i64,
    /// display column of the caret when the candidate list was rebuilt;
    /// the anchor for `…tail` candidate rows (full rows anchor at the
    /// input text start)
    pub sug_col: usize,
}

/// CommandState is the `:` command input: one line at the bottom, with
/// up/down walking the executed command history. Completion candidates
/// replace the word before the cursor only; `sug_start..sug_end` records the
/// text the current cycle owns so repeated tab presses do not rescan it.
pub struct CommandState {
    pub ti: TextInput,
    /// index into App::cmd_history while browsing; None at the prompt
    pub hist_idx: Option<usize>,
    /// input saved before the first history recall
    pub draft: String,
    /// word being completed (shown in the candidate window)
    pub sug_prefix: String,
    /// tab-completion candidates for the word before the cursor
    pub sug: Vec<String>,
    /// cycle position in `sug`; None when no completion has been applied
    pub sug_idx: Option<usize>,
    /// line produced by the current completion cycle
    pub sug_line: String,
    /// byte range of the current line owned by the selected candidate
    pub sug_start: usize,
    pub sug_end: usize,
    /// display column of the completed word's start; the candidate window's
    /// left edge is anchored there (typing inside the word never moves it,
    /// a separator that starts a new word re-anchors)
    pub sug_col: usize,
}

/// Inline '/'-filter state shared by every list overlay.
#[derive(Clone, Debug, Default)]
pub struct ListFilter {
    pub ti: TextInput,
    /// the filter input has focus
    pub active: bool,
    /// list cursor to restore when the filter is cancelled
    pub restore: usize,
}

/// The outcome of one key in a focused list filter.
pub enum FilterAction {
    /// the filter consumed the key (motion or text edit)
    Handled,
    /// esc cancelled the filter and restored the cursor
    Cancel,
    /// enter: the caller acts on the current cursor
    Commit,
    /// not a filter key
    Passthrough,
}

impl ListFilter {
    /// Focuses the filter with the current list cursor as the restore point.
    pub fn open(&mut self, cursor: usize) {
        self.restore = cursor;
        self.ti = TextInput::default();
        self.active = true;
    }

    /// Consumes one key while the filter is focused: text edits the query,
    /// arrows/tab/page move `cursor` over the `n` matched rows, enter
    /// commits, esc cancels and restores the cursor against the `total`
    /// unfiltered rows.
    pub fn key(
        &mut self,
        msg: &Msg,
        cursor: &mut usize,
        n: usize,
        total: usize,
        page: usize,
    ) -> FilterAction {
        use crate::ui::util::{page_cursor, wrap_cursor};
        match msg {
            Msg::Esc => {
                self.active = false;
                self.ti = TextInput::default();
                *cursor = self.restore.min(total.saturating_sub(1));
                FilterAction::Cancel
            }
            Msg::Enter => FilterAction::Commit,
            Msg::Up | Msg::BackTab | Msg::Ctrl('k') => {
                *cursor = wrap_cursor(*cursor, -1, n);
                FilterAction::Handled
            }
            Msg::Down | Msg::Tab | Msg::Ctrl('j') => {
                *cursor = wrap_cursor(*cursor, 1, n);
                FilterAction::Handled
            }
            Msg::PageDown | Msg::Ctrl('f') => {
                *cursor = page_cursor(*cursor, 1, page, n);
                FilterAction::Handled
            }
            Msg::PageUp | Msg::Ctrl('b') => {
                *cursor = page_cursor(*cursor, -1, page, n);
                FilterAction::Handled
            }
            _ => {
                let Some(k) = crate::ui::keys::msg_key(msg) else {
                    return FilterAction::Passthrough;
                };
                let before = self.ti.value().to_string();
                self.ti.handle_key(k);
                if self.ti.value() != before {
                    *cursor = 0;
                }
                FilterAction::Handled
            }
        }
    }
}

/// helpState is the "?" keyboard-help page; `scroll` is its first visible row.
#[derive(Default)]
pub struct HelpState {
    pub scroll: usize,
}

/// textState is the built-in raw textarea overlay; its syntax follows
/// Options.Raw (JSON, YAML, or TOML). `path` is None for the whole-document
/// editor.
pub struct TextState {
    pub path: Option<Vec<PathSeg>>,
    pub fn_: Option<FormNodeId>,
    pub ta: TextArea,
    pub err: String,
    /// The buffer holds a string VALUE, not a document: it was prefilled with
    /// the plain text (no YAML/JSON encoding) and applies verbatim.
    pub verbatim_str: bool,
}

/// issuesState is the load-time validation issues overlay.
#[derive(Default, Clone)]
pub struct IssuesState {
    pub items: Vec<Issue>,
    pub cursor: usize,
    /// inline '/'-search over issue messages and paths
    pub filter: ListFilter,
}

/// refPickState is the gr picker.
pub struct RefPickState {
    pub name: String,
    pub paths: Vec<Vec<PathSeg>>,
    pub cursor: usize,
    /// inline '/'-search over reference paths and value summaries
    pub filter: ListFilter,
}

/// errsState is the save-time validation error overlay.
pub struct ErrsState {
    pub lines: Vec<String>,
    pub paths: Vec<Option<Vec<PathSeg>>>,
    pub cursor: usize,
    /// end the session after the --force save (vs plain ctrl+s)
    pub exit: bool,
    /// write-to path for a W save ("" = the session's own file)
    pub target: String,
    /// inline '/'-search over the error lines
    pub filter: ListFilter,
}

/// confirmState is a y/n question overlay. on_no is optional.
pub struct ConfirmState {
    pub title: String,
    /// optional footer override
    pub hint: String,
    pub on_yes: Option<Box<dyn FnOnce(&mut App)>>,
    pub on_no: Option<Box<dyn FnOnce(&mut App)>>,
}

/// searchState is the "/" filter mode, scoped to the subtree below the
/// current breadcrumb path.
pub struct SearchState {
    /// replace mode: show the replace input, results are previews
    pub replace: bool,
    /// true = match case-insensitively (default)
    pub fold_case: bool,
    pub ti: TextInput,
    pub rti: TextInput,
    /// which input has focus (Go bubbles Focus/Blur); the query input starts
    /// focused
    pub replace_focused: bool,
    pub all: Vec<Row>,
    pub results: Vec<Row>,
    pub cursor: usize,
    /// true = the query is a regular expression (alt+x toggles)
    pub regex: bool,
    pub re: Option<Regex>,
    /// regex compile error message; "" when ok
    pub re_err: String,
    /// A search jump parks the overlay instead of closing it: esc at the
    /// landing depth (return_depth) reopens it, and closing it always
    /// returns to the menu the search was opened from (origin).
    pub parked: bool,
    pub return_depth: usize,
    pub origin: Option<Vec<MenuState>>,
}

/// anchor input modes: & defines/renames the anchor on a value, * points the
/// value at an existing anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorMode {
    AnchorSet,
    RefSet,
}

/// anchorState is the name input behind & and *.
pub struct AnchorState {
    pub path: Vec<PathSeg>,
    pub mode: AnchorMode,
    /// the row's schema node, for the required check
    pub fn_: Option<FormNodeId>,
    pub ti: TextInput,
    /// reference mode: anchor names in document order
    pub cands: Vec<String>,
    /// input prefix the current suggestion list was built from
    pub sug_prefix: String,
    /// candidate list captured when a tab cycle starts
    pub sugs: Vec<String>,
    /// input value produced by the current completion selection
    pub sug_line: String,
    /// completion cursor, -1 when nothing selected
    pub sug: i64,
    /// display column of the caret when the candidate list was rebuilt;
    /// the anchor for `…tail` candidate rows (full rows anchor at the
    /// input text start)
    pub sug_col: usize,
    pub err: String,
}

/// mergeState is the input behind `<`: anchor names, space-separated; tab
/// completes the token being typed over the document's mapping anchors.
pub struct MergeState {
    pub path: Vec<PathSeg>,
    pub ti: TextInput,
    /// mapping anchor names in document order (fixed at open)
    pub cands: Vec<String>,
    pub sug: i64,
    pub err: String,
}

/// fastMoveState is f/F fast move: waiting for the character.
#[derive(Clone)]
pub struct FastMoveState {
    /// F: search upward
    pub backward: bool,
}

/// filePickState is the s-key file switcher overlay.
pub struct FilePickState {
    pub cursor: usize,
    /// inline '/'-search over file paths
    pub filter: ListFilter,
}

/// externalEdit is the target of a pending $EDITOR run. A non-negative
/// insert_idx splices a new member instead of Set.
#[derive(Clone)]
pub struct ExternalEdit {
    pub target: Vec<PathSeg>,
    pub insert_idx: i64,
    /// temp file the editor is working on
    pub path: String,
    /// whole-document editor (the root raw row) vs one value
    pub whole_doc: bool,
}

/// commentState is the dedicated page for a description that does not fit
/// the menu's inline description area. `scroll` is the first visible line.
pub struct CommentState {
    /// the field/row label shown in the page title
    pub title: String,
    /// the fully wrapped description lines, tagged with their source color
    pub lines: Vec<DescLine>,
    pub scroll: usize,
}

/// ShellOutState is the scrolling page for a bare `!<cmd>` command's
/// multi-line stdout. `lines` are already wrapped to the terminal width;
/// `scroll` is the first visible line.
pub struct ShellOutState {
    /// the executed command line, shown in the page title
    pub command: String,
    pub lines: Vec<String>,
    pub scroll: usize,
}

/// CommentEditState is the multiline editor for the document comment of one
/// key. ctrl+s applies it to the session metadata (the document body is not
/// touched); esc cancels.
pub struct CommentEditState {
    pub path: Vec<PathSeg>,
    /// the row label shown in the editor title
    pub title: String,
    pub ta: TextArea,
}

/// Overlay is the single open surface (Go's 16 nullable fields + routeKey
/// priority chain). Only one can be open at a time; the key router matches
/// on it.
pub enum Overlay {
    Confirm(ConfirmState),
    Issues(IssuesState),
    Errs(ErrsState),
    RefPick(RefPickState),
    Text(TextState),
    Search(SearchState),
    Edit(EditState),
    Add(AddState),
    Write(WriteState),
    Command(CommandState),
    ShellOut(ShellOutState),
    Anc(AnchorState),
    Mrg(MergeState),
    Comment(CommentState),
    CommentEdit(CommentEditState),
    Help(HelpState),
    FilePick(FilePickState),
}

impl Overlay {
    /// Whether a text input currently owns the keyboard (q/@/. are literal
    /// characters there, and history keys must not fire).
    pub fn typing(&self) -> bool {
        match self {
            Overlay::Edit(_)
            | Overlay::Text(_)
            | Overlay::CommentEdit(_)
            | Overlay::Search(_)
            | Overlay::Add(_)
            | Overlay::Write(_)
            | Overlay::Command(_)
            | Overlay::Anc(_)
            | Overlay::Mrg(_) => true,
            Overlay::Issues(s) => s.filter.active,
            Overlay::Errs(s) => s.filter.active,
            Overlay::RefPick(s) => s.filter.active,
            Overlay::FilePick(s) => s.filter.active,
            _ => false,
        }
    }
}

// --- the app ------------------------------------------------------------------

/// App is the menu TUI model (Go listModel) over one or more files.
pub struct App {
    pub files: Vec<FileState>,
    pub cur: usize,
    pub width: usize,
    pub height: usize,
    pub overlay: Option<Overlay>,
    /// f/F fast move: waiting for the character
    pub fast_move: Option<FastMoveState>,
    /// z filter: hide object keys with no document value
    pub hide_empty: bool,
    /// c: the focused row's description is expanded inline. Any other key
    /// collapses it before acting; descriptions too long for one page open
    /// Overlay::Comment instead.
    pub desc_expanded: bool,
    /// transient list-mode message, cleared on the next key
    pub notice: String,
    /// pending multi-key command (vi-style: shown bottom-right, never
    /// cleared by a timer)
    pub cmd_buf: Vec<String>,
    /// macro capture armed
    pub recording: bool,
    pub rec_buf: Vec<Msg>,
    /// last completed macro
    pub macro_buf: Vec<Msg>,
    /// navigation trail (ctrl+o / ctrl+i)
    pub nav: Nav,
    /// the session ended via ctrl+c/EOF abort
    pub aborted: bool,
    /// the session ended cleanly (root quit)
    pub exiting: bool,
    /// set while the replace-all key arms the change ring, so the
    /// dispatcher's edited-check does not clear what that key just set
    /// (undo/redo arm the ring before the edited-check and return early)
    pub ring_keep: bool,
    /// run $EDITOR for real (false in tests: the pending edit is stashed
    /// instead so `finish_external_value` can deliver the content)
    pub exec_editor: bool,
    /// pending $EDITOR target stashed for the event loop (or tests)
    pub pending_edit: Option<ExternalEdit>,
    /// `:` command lines executed, oldest first; seeded from and persisted
    /// to Options::cmd_history_file, so it spans sessions
    pub cmd_history: Vec<String>,
    /// built-in clipboard: the last text y/X copied this session, serving
    /// paste when no system clipboard tool exists or reading it fails (a
    /// jse extension, no Go counterpart)
    pub builtin_clipboard: Option<String>,
}

impl App {
    /// bare builds the shell other constructors fill in (files already
    /// converted to FileState). The `:` command history is seeded from the
    /// first file's Options::cmd_history_file, so up/down recall the
    /// previous sessions' commands.
    pub fn bare(files: Vec<FileState>) -> App {
        let cmd_history = files
            .first()
            .and_then(|f| f.opts.cmd_history_file.as_deref())
            .map(crate::ui::cmd_history::load)
            .unwrap_or_default();
        App {
            files,
            cur: 0,
            width: 80,
            height: 24,
            overlay: None,
            fast_move: None,
            hide_empty: false,
            desc_expanded: false,
            notice: String::new(),
            cmd_buf: Vec::new(),
            recording: false,
            rec_buf: Vec::new(),
            macro_buf: Vec::new(),
            nav: Nav::default(),
            aborted: false,
            exiting: false,
            ring_keep: false,
            exec_editor: false,
            pending_edit: None,
            cmd_history,
            builtin_clipboard: None,
        }
    }

    /// newListModel/newMultiFileModel entry (Go newMultiFileModel).
    pub fn new(files: Vec<FileSession>) -> App {
        new_app(files)
    }

    // --- state accessors ----------------------------------------------------

    /// top returns the current navigation state.
    pub fn top(&self) -> Option<&MenuState> {
        self.fs().stack.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut MenuState> {
        self.fs_mut().stack.last_mut()
    }

    pub fn doc(&self) -> &Doc {
        &self.fs().doc
    }

    pub fn opts(&self) -> &Options {
        &self.fs().opts
    }

    /// typing reports whether a text input has focus.
    pub fn typing(&self) -> bool {
        if self.overlay.as_ref().is_some_and(Overlay::typing) {
            return true;
        }
        if self.fast_move.is_some() {
            return true;
        }
        self.top().is_some_and(|st| st.filter.active)
    }

    /// listMode reports whether the plain menu key table owns the keyboard.
    pub fn list_mode(&self) -> bool {
        self.overlay.is_none() && !self.typing()
    }

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

    // --- rows ---------------------------------------------------------------

    /// menuRows builds the row list for the current (top) menu state.
    pub fn menu_rows(&mut self) -> Vec<Row> {
        let st = self.top().cloned();
        match st {
            Some(st) => self.menu_rows_state(&st),
            None => Vec::new(),
        }
    }

    /// menuRowsFor builds the row list for one menu state: the top one in
    /// menu_rows, any state for cursor bookkeeping during variant flips.
    pub fn menu_rows_state(&mut self, st: &MenuState) -> Vec<Row> {
        let hide_empty = self.hide_empty;
        let cur = self.cur;
        let f = &mut self.files[cur];
        let is_root = f.stack.first().map_or(false, |top| same_state(top, st));
        let FileState {
            schema,
            doc,
            menu_cache,
            issues_now,
            ..
        } = f;
        let lookup = MergeLookup { doc };
        let mut cache = std::mem::take(menu_cache);
        let mut env = RowsEnv {
            is_root,
            issues_now,
            merge: &lookup,
            synth: cache,
        };
        let rows = menu_rows_for(schema, doc, hide_empty, st, &mut env);
        cache = env.synth;
        *menu_cache = cache;
        rows
    }

    /// A MenuCtx for the pure search/summary helpers.
    pub fn ctx(&self) -> MenuCtx<'_> {
        let f = self.fs();
        MenuCtx {
            schema: &f.schema,
            doc: &f.doc,
        }
    }

    /// clampStateCursor on the top state.
    pub fn clamp_cursor(&mut self, rows: &[Row]) {
        let max = rows.len().saturating_sub(1);
        if let Some(st) = self.top_mut() {
            st.cursor = st.cursor.min(max);
        }
    }

    pub fn repair_stack(&mut self) {
        loop {
            let top = match self.top() {
                Some(t) => t,
                None => return,
            };
            if self.fs().stack.len() <= 1 {
                break;
            }
            let p = top.path.clone();
            if p.is_empty() || self.doc().get(&p).is_some() {
                break;
            }
            self.fs_mut().stack.pop();
        }
        let rows = self.menu_rows();
        self.clamp_cursor(&rows);
    }

    // --- variant menus ------------------------------------------------------

    /// cachedMenu pins the merged union menu for a oneOf node.
    fn cached_menu_merged(&mut self, node: FormNodeId) -> FormNodeId {
        if let Some(&id) = self.fs().menu_cache.get(&SynthKey::Merged(node)) {
            return id;
        }
        let variants = dynamic_variants(&self.fs().schema, node).unwrap_or_default();
        let id = crate::ui::rows::merged_one_of_menu(&mut self.fs_mut().schema, node, &variants);
        self.fs_mut().menu_cache.insert(SynthKey::Merged(node), id);
        id
    }

    /// cachedMenu pins one variant's menu with unionized discriminators.
    fn cached_menu_pinned(&mut self, node: FormNodeId, variant: FormNodeId) -> FormNodeId {
        if let Some(&id) = self.fs().menu_cache.get(&SynthKey::Pinned(node, variant)) {
            return id;
        }
        let variants = dynamic_variants(&self.fs().schema, node)
            .unwrap_or_else(|| self.fs().schema.nodes[node as usize].variants.clone());
        let id = pinned_one_of_menu(&mut self.fs_mut().schema, &variants, variant);
        self.fs_mut()
            .menu_cache
            .insert(SynthKey::Pinned(node, variant), id);
        id
    }

    /// dynamicMenu builds the item menu of an all-object oneOf: the merged
    /// union menu while the value is unset or ambiguous, the matched
    /// variant's menu once exactly one candidate remains.
    pub fn dynamic_menu(&mut self, node: FormNodeId, path: Vec<PathSeg>) -> MenuState {
        let mut st = MenuState {
            kind: MenuKind::Object,
            node,
            path,
            cursor: 0,
            offset: 0,
            variant_of: Some(node),
            one_of: None,
            sel: std::collections::HashSet::new(),
            visual: false,
            vanchor: 0,
            filter: Default::default(),
        };
        let val = self.doc().get(&st.path);
        let cands = {
            let schema = &self.fs().schema;
            let variants = dynamic_variants(schema, node).unwrap_or_default();
            match_variant_list(schema, &variants, &self.doc(), val)
        };
        st.node = if cands.len() == 1 {
            let v = cands[0];
            self.cached_menu_pinned(node, v)
        } else {
            self.cached_menu_merged(node)
        };
        st
    }

    /// refreshVariantMenu re-runs variant inference on every dynamic item
    /// menu in the stack after each key press: the node flips between merged
    /// and pinned as the value disambiguates. The cursor follows the field
    /// it was on (matched by schema key), so flipping via the discriminator
    /// keeps it selected.
    pub fn refresh_variant_menu(&mut self) {
        for i in 0..self.fs().stack.len() {
            let (variant_of, path, node) = {
                let st = &self.fs().stack[i];
                match st.variant_of {
                    Some(v) => (v, st.path.clone(), st.node),
                    None => continue,
                }
            };
            let want = self.dynamic_menu(variant_of, path).node;
            if want == node {
                continue;
            }
            let key = self.cursor_key(i);
            self.fs_mut().stack[i].node = want;
            if !key.is_empty() {
                if let Some(pos) = self.row_index_by_key(i, &key) {
                    self.fs_mut().stack[i].cursor = pos;
                }
            }
        }
    }

    /// cursorKey returns the schema key of the row under the state's cursor.
    fn cursor_key(&mut self, i: usize) -> String {
        let st = self.fs().stack[i].clone();
        let rows = self.menu_rows_state(&st);
        let st = &self.fs().stack[i];
        if st.cursor >= rows.len() {
            return String::new();
        }
        rows[st.cursor]
            .fn_
            .map(|f| self.fs().schema.nodes[f as usize].key.clone())
            .unwrap_or_default()
    }

    /// rowIndexByKey finds the first row of the state backed by the given key.
    fn row_index_by_key(&mut self, i: usize, key: &str) -> Option<usize> {
        let st = self.fs().stack[i].clone();
        let rows = self.menu_rows_state(&st);
        rows.iter().position(|r| {
            r.fn_
                .map_or(false, |f| self.fs().schema.nodes[f as usize].key == key)
        })
    }

    // --- change tracking ----------------------------------------------------

    /// changed reports whether the value at path differs from the
    /// loaded/last-saved baseline; without a baseline nothing is marked.
    pub fn changed(&self, path: &[PathSeg]) -> bool {
        let f = self.fs();
        match &f.baseline {
            Some(base) => {
                let cur = f.doc.get(path);
                let basev = base.get(path);
                !nodes_equal_opt(&f.doc, cur, base, basev)
            }
            None => false,
        }
    }

    /// dirty reports whether anything in the document differs from the
    /// baseline.
    pub fn dirty(&self) -> bool {
        let f = self.fs();
        if f.comments_dirty {
            return true;
        }
        match &f.baseline {
            Some(base) => !nodes_equal(&f.doc, Some(f.doc.root), base, Some(base.root)),
            None => false,
        }
    }

    // --- issues -------------------------------------------------------------

    /// setIssues replaces the live validation problem list and its row marks.
    pub fn set_issues(&mut self, items: Vec<Issue>) {
        let mut err_paths = Vec::new();
        let mut warn_paths = Vec::new();
        for it in &items {
            let mut marks: Vec<Vec<PathSeg>> = it.marks.clone();
            if marks.is_empty() {
                if let Some(p) = &it.path {
                    marks = vec![p.clone()];
                }
            }
            for p in marks {
                if p.is_empty() && it.path.is_none() {
                    // document root: the status row carries the count
                    continue;
                }
                if p.is_empty() {
                    continue;
                }
                if it.warn {
                    warn_paths.push(p);
                } else {
                    err_paths.push(p);
                }
            }
        }
        let f = self.fs_mut();
        f.issues_now = items;
        f.err_paths = err_paths;
        f.warn_paths = warn_paths;
    }

    /// refreshIssues re-validates the document after a write so fixed
    /// problems leave the list, the counts and the row marks — and new
    /// problems appear immediately. Callers invoke it only when the document
    /// changed. Without a validator the load-time list stays as loaded.
    pub fn refresh_issues(&mut self) {
        let validate = match &self.opts().validate {
            Some(v) => v.clone(),
            None => return,
        };
        // The root menu gains or loses its issue status row with the problem
        // count; keep the cursor on the row it was on while the list shifts.
        let root = self.fs().stack.first().cloned();
        let before = root
            .clone()
            .map(|r| self.menu_rows_state(&r))
            .unwrap_or_default();
        let msgs = validate(&self.doc());
        let items = self.live_issues(msgs);
        self.set_issues(items);
        let Some(root) = root else { return };
        let after = self.menu_rows_state(&root);
        if after.len() != before.len() {
            remap_state_cursor(&mut self.files[self.cur].stack[0], &before, &after);
        }
    }

    /// liveIssues combines validator messages with the editor's unknown-key
    /// warnings for the live issue list. A key the validator already flags
    /// (e.g. additionalProperties) is not warned about twice.
    pub fn live_issues(&mut self, msgs: Vec<String>) -> Vec<Issue> {
        let mut items = issues_from(&msgs);
        let warnings = {
            let f = self.fs();
            unknown_key_issues(&f.schema, &f.doc)
        };

        // An additionalProperties message names keys in map iteration order,
        // and a failed oneOf branch also names fields that are valid in the
        // matched variant. Mark only the names that really are schema-unknown
        // keys in the edited value; when none are, mark the object itself.
        for it in items.iter_mut() {
            if it.warn || !it.msg.contains("additional properties ") {
                continue;
            }
            let Some((loc, _)) = crate::ui::rows::split_issue(&it.msg) else {
                continue;
            };
            let listed: std::collections::HashSet<String> =
                it.flagged.iter().map(|p| path_string(p)).collect();
            let mut actual: Vec<Vec<PathSeg>> = Vec::new();
            for w in &warnings {
                if let Some(wp) = &w.path {
                    if listed.contains(&path_string(wp)) {
                        actual.push(wp.clone());
                    }
                }
            }
            if actual.is_empty() {
                it.path = Some(loc.clone());
                it.flagged = vec![loc.clone()];
                it.marks = vec![loc];
            } else {
                it.path = Some(actual[0].clone());
                it.flagged = actual.clone();
                it.marks = actual;
            }
        }

        let mut flagged: std::collections::HashSet<String> = std::collections::HashSet::new();
        for it in &items {
            for p in &it.flagged {
                flagged.insert(path_string(p));
            }
            if let Some(p) = &it.path {
                flagged.insert(path_string(p));
            }
        }
        for w in warnings {
            if let Some(wp) = &w.path {
                if !flagged.contains(&path_string(wp)) {
                    items.push(w);
                }
            }
        }
        items
    }

    // --- navigation trail ---------------------------------------------------

    pub fn nav_snapshot(&self) -> crate::ui::nav::NavMark {
        crate::ui::nav::nav_snapshot(&self.fs().stack, self.cur)
    }

    /// navForward records the location left behind by a forward relocation.
    pub fn nav_forward(&mut self) {
        let leaving = self.nav_snapshot();
        self.nav.forward(leaving);
    }

    /// navBackStep records the location a back motion leaves behind.
    pub fn nav_back_step(&mut self, leaving: crate::ui::nav::NavMark) {
        let landing = self.nav_snapshot();
        self.nav.back_step(leaving, &landing);
    }

    /// navGo moves one step along the navigation trail.
    pub fn nav_go(&mut self, back: bool) {
        let current = self.nav_snapshot();
        match self.nav.go(back, &current) {
            NavGo::Stay(notice) => self.notice = notice.to_string(),
            NavGo::Jump(mk) => self.nav_restore(mk),
        }
    }

    /// navRestore swaps in a frozen stack (nav.rs contract: switch file,
    /// drop parked returns, repair).
    pub fn nav_restore(&mut self, mk: crate::ui::nav::NavMark) {
        if self.files.len() > 1 && mk.file != self.cur {
            self.switch_file_state(mk.file); // plain swap: navGo already moved the trails
        }
        self.fs_mut().stack = mk.stack;
        let f = self.fs_mut();
        f.jump_origin = None;
        f.jump_return = 0;
        f.search_parked = None;
        f.issues_parked = None;
        self.repair_stack();
    }

    // --- the key pipeline ---------------------------------------------------

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
                        self.fs_mut().change_jump = Some(std::mem::take(&mut res.paths));
                        self.nav_forward();
                        self.jump_to_row(&first);
                    }
                }
                true
            }
            None => false,
        }
    }

    // --- save flow ----------------------------------------------------------

    /// save handles ctrl+s: write the document and stay in the session.
    pub fn save(&mut self) {
        self.persist(false);
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

    /// persist validates first; validation errors open the overlay (with
    /// the --force y/n path remembering whether the session should end
    /// afterwards).
    pub fn persist(&mut self, exit: bool) {
        self.validate_and_write(exit, "");
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

    /// openErrsOverlay shows validation errors (at most 15 lines, each
    /// jumpable when it parses to a path); exit arms the --force y path to
    /// end the session after the write.
    pub fn open_errs_overlay(&mut self, errs: Vec<String>, exit: bool, target: &str) {
        const MAX_SHOWN: usize = 15;
        let mut lines = Vec::new();
        let mut paths = Vec::new();
        for (i, e) in errs.iter().enumerate() {
            if i == MAX_SHOWN {
                lines.push(format!("… and {} more", errs.len() - MAX_SHOWN));
                paths.push(None);
                break;
            }
            lines.push(e.clone());
            paths.push(parse_issue(e));
        }
        self.overlay = Some(Overlay::Errs(ErrsState {
            lines,
            paths,
            cursor: 0,
            exit,
            target: target.to_string(),
            filter: Default::default(),
        }));
    }

    /// writeTo persists through opts.Save (target "") or opts.WriteTo (a W
    /// path). A failed write surfaces as a notice and the session continues;
    /// a successful session-file write refreshes the change-mark baseline.
    pub fn write_to(&mut self, exit: bool, forced: bool, target: &str) {
        if target.is_empty() {
            let save = match &self.opts().save {
                Some(s) => s.clone(),
                None => {
                    self.notice = "Write failed: no save target".to_string();
                    return;
                }
            };
            if let Err(e) = save(&self.doc(), &self.fs().meta) {
                self.notice = format!("Write failed: {}", e);
                return;
            }
            let f = self.fs_mut();
            f.saved = true;
            f.with_errors = forced;
            f.baseline = Some(f.doc.clone()); // markers clear once saved
            f.comments_dirty = false;
            // our own write is not an external change: re-record the stat
            f.disk_stat = disk_stat_of(&f.opts.path);
            if exit {
                self.exiting = true;
                return;
            }
            if !self.opts().path.is_empty() {
                self.notice = format!("Write to {}", self.opts().path);
                return;
            }
            self.notice = "Saved".to_string();
            return;
        }
        let write_to = match &self.opts().write_to {
            Some(w) => w.clone(),
            None => {
                self.notice = "Write failed: no write-to target".to_string();
                return;
            }
        };
        if let Err(e) = write_to(target, &self.doc(), &self.fs().meta) {
            self.notice = format!("Write failed: {}", e);
            return;
        }
        self.notice = format!("Wrote to {}", target);
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

/// same_state compares two menu states the way Go compared stack pointers:
/// position identity, not value equality of frozen snapshots.
pub fn same_state(a: &MenuState, b: &MenuState) -> bool {
    a.node == b.node
        && a.kind == b.kind
        && a.variant_of == b.variant_of
        && a.one_of == b.one_of
        && a.path == b.path
}

/// nodesEqual over one arena with optional roots (Go nil *Node handling).
pub fn nodes_equal(a: &Doc, ai: Option<NodeId>, b: &Doc, bi: Option<NodeId>) -> bool {
    nodes_equal_opt(a, ai, b, bi)
}

/// Cross-document comparison (pre-snapshot vs live doc live in different
/// arenas).
pub fn nodes_equal_opt(a: &Doc, ai: Option<NodeId>, b: &Doc, bi: Option<NodeId>) -> bool {
    match (ai, bi) {
        (None, None) => true,
        (Some(x), Some(y)) => nodes_equal_id(a, x, b, y),
        _ => false,
    }
}

/// Whole-document comparison (Go nodesEqual(m.doc, m.baseline)).
pub fn nodes_equal_doc(a: &Doc, b: &Doc) -> bool {
    nodes_equal_opt(a, Some(a.root), b, Some(b.root))
}

fn nodes_equal_id(a: &Doc, x: NodeId, b: &Doc, y: NodeId) -> bool {
    let (da, db) = (a.node(x), b.node(y));
    if da.anchor != db.anchor {
        return false;
    }
    if !da.merge.is_empty() || !db.merge.is_empty() {
        if da.merge.len() != db.merge.len() {
            return false;
        }
        for i in 0..da.merge.len() {
            if !nodes_equal_id(a, da.merge[i], b, db.merge[i]) {
                return false;
            }
        }
        return true;
    }
    match (&da.value, &db.value) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Str(s1), Value::Str(s2)) => s1 == s2,
        (Value::Str(_), _) | (_, Value::Str(_)) => false,
        (Value::Num(n1), Value::Num(n2)) => n1 == n2,
        (Value::Num(_), _) | (_, Value::Num(_)) => false,
        (Value::Bool(v1), Value::Bool(v2)) => v1 == v2,
        (Value::Bool(_), _) | (_, Value::Bool(_)) => false,
        (Value::Object(m1), Value::Object(m2)) => {
            if m1.len() != m2.len() {
                return false;
            }
            // Go built a map here too: a linear find per member made wide
            // objects quadratic (8 ms for a 2000-key object per keypress)
            let by: std::collections::HashMap<&str, NodeId> =
                m2.iter().map(|m| (m.key.as_str(), m.val)).collect();
            for mem in m1 {
                let Some(&bv) = by.get(mem.key.as_str()) else {
                    return false;
                };
                if !nodes_equal_id(a, mem.val, b, bv) {
                    return false;
                }
            }
            true
        }
        (Value::Object(_), _) | (_, Value::Object(_)) => false,
        (Value::Array(a1), Value::Array(a2)) => {
            a1.len() == a2.len()
                && a1
                    .iter()
                    .zip(a2.iter())
                    .all(|(&x1, &x2)| nodes_equal_id(a, x1, b, x2))
        }
    }
}

/// issuesFrom parses validator messages into jumpable issues.
pub fn issues_from(msgs: &[String]) -> Vec<Issue> {
    msgs.iter()
        .map(|s| Issue {
            msg: s.clone(),
            path: parse_issue(s),
            flagged: crate::ui::rows::issue_flag_paths(s),
            marks: Vec::new(),
            warn: false,
        })
        .collect()
}

/// remapStateCursor moves st.cursor onto the same row after a row-set
/// change. If that row was hidden, the next surviving row in the old order
/// wins.
pub fn remap_state_cursor(st: &mut MenuState, before: &[Row], after: &[Row]) {
    if st.cursor < before.len() {
        for i in st.cursor..before.len() {
            if let Some(j) = index_same_row(after, &before[i]) {
                st.cursor = j;
                return;
            }
        }
    }
    let max = after.len().saturating_sub(1);
    st.cursor = st.cursor.min(max);
}

/// indexSameRow finds the line same_menu_row identifies, None when gone.
fn index_same_row(rows: &[Row], want: &Row) -> Option<usize> {
    rows.iter().position(|r| same_menu_row(want, r))
}

/// sameMenuRow reports whether a and b are the same selectable line: kind,
/// document position, schema node and label.
pub fn same_menu_row(a: &Row, b: &Row) -> bool {
    a.kind == b.kind
        && a.idx == b.idx
        && a.path == b.path
        && a.fn_ == b.fn_
        && a.variant == b.variant
        && a.label == b.label
}

/// cursorPrefix marks the focused row, two cells wide.
pub fn cursor_prefix(on: bool) -> &'static str {
    if on { "> " } else { "  " }
}

/// isPathPrefix reports whether prefix is a prefix of path.
pub fn is_path_prefix(prefix: &[PathSeg], path: &[PathSeg]) -> bool {
    prefix.len() <= path.len() && prefix.iter().zip(path.iter()).all(|(a, b)| a == b)
}

/// cursorByPath finds the row for path: an exact match first, else the
/// deepest row whose path is a prefix of it.
pub fn cursor_by_path(rows: &[Row], path: &[PathSeg]) -> usize {
    for (i, r) in rows.iter().enumerate() {
        if !r.path.is_empty() && r.path == path {
            return i;
        }
    }
    let (mut best, mut bi) = (0usize, 0usize);
    for (i, r) in rows.iter().enumerate() {
        if !r.path.is_empty() && is_path_prefix(&r.path, path) && r.path.len() > best {
            best = r.path.len();
            bi = i;
        }
    }
    bi
}

/// cursorAncestor finds the deepest row whose path is a strict prefix of
/// path: the nearest editable ancestor of a value the menu cannot show as a
/// row. Used by search jumps into schema-less array items (raw `items`, no
/// `items` at all), where neither the item nor its subpaths have rows of
/// their own.
pub fn cursor_ancestor(rows: &[Row], path: &[PathSeg]) -> Option<usize> {
    let mut best = 0usize;
    let mut bi: Option<usize> = None;
    for (i, r) in rows.iter().enumerate() {
        if r.path.is_empty()
            || r.path.len() >= path.len()
            || !is_path_prefix(&r.path, path)
            || (bi.is_some() && r.path.len() <= best)
        {
            continue;
        }
        best = r.path.len();
        bi = Some(i);
    }
    bi
}

/// cursorForRow finds the row matching a search hit (same schema node at the
/// same path, or the same variant); None when the target is a container
/// whose menu we entered.
pub fn cursor_for_row(rows: &[Row], res: &Row) -> Option<usize> {
    if res.kind != RowKind::Variant {
        for (i, r) in rows.iter().enumerate() {
            if r.kind != RowKind::Variant && r.fn_ == res.fn_ && r.path == res.path {
                return Some(i);
            }
        }
        // Pinned/merged oneOf menus synthesize union nodes for keys the
        // variants define differently (const discriminators). The search hit
        // carries the original variant's node, so fall back to the row at
        // the same path rather than leaving the cursor on row 0.
        for (i, r) in rows.iter().enumerate() {
            if r.kind == res.kind && !r.path.is_empty() && r.path == res.path {
                return Some(i);
            }
        }
        // Schema-less rows carry no schema node, while the menu row for an
        // array item carries the runtime (any-type) node: the path alone
        // identifies the value.
        if res.kind == RowKind::Unknown {
            for (i, r) in rows.iter().enumerate() {
                if !r.path.is_empty() && r.path == res.path {
                    return Some(i);
                }
            }
        }
    }
    for (i, r) in rows.iter().enumerate() {
        if r.variant.is_some() && r.variant == res.variant {
            return Some(i);
        }
    }
    None
}

// --- merge/alias lookups feeding rows.rs (Go menu_merge.go / menu_alias.go) -----

/// MergeLookup implements [`MergeView`] over the document's anchor state —
/// Go hung these on the listModel; they are pure document walks here.
pub struct MergeLookup<'a> {
    pub doc: &'a Doc,
}

impl MergeView for MergeLookup<'_> {
    fn alias_at(&self, path: &[PathSeg]) -> Option<String> {
        // Go's listModel.aliasAt returns ok only for reference occurrences
        // (menu_alias.go:64-67): the menu offers < Expand > on a reference,
        // not on the anchor carrier itself.
        alias_at(self.doc, path)
            .filter(|(_, is_ref)| *is_ref)
            .map(|(name, _)| name)
    }

    fn merged_keys(&self, st: &MenuState) -> Vec<MergedKey> {
        merged_keys(self.doc, st)
    }
}

/// anchorRole classifies the node at path: the anchor carrier ("&name") or a
/// reference to one ("*name"). None when nothing there carries anchor
/// identity. Returns (name, is_reference).
pub fn anchor_role(doc: &Doc, path: &[PathSeg]) -> Option<(String, bool)> {
    if path.is_empty() {
        return None;
    }
    let n = doc.get(path)?;
    let data = doc.node(n);
    if data.anchor.is_empty() {
        return None;
    }
    let holder = anchor_holder_path(doc, n);
    let is_ref = holder.as_deref() != Some(path);
    Some((data.anchor.clone(), is_ref))
}

/// aliasAt reports the anchor name when the node at path is a reference.
pub fn alias_at(doc: &Doc, path: &[PathSeg]) -> Option<(String, bool)> {
    let (name, is_ref) = anchor_role(doc, path)?;
    Some((name, is_ref))
}

/// anchorHolderPath returns the first path in document order whose node is n
/// (NodeId identity) — the location Write tags with the anchor.
pub fn anchor_holder_path(doc: &Doc, n: NodeId) -> Option<Vec<PathSeg>> {
    fn walk(doc: &Doc, cur: NodeId, path: &[PathSeg], n: NodeId) -> Option<Vec<PathSeg>> {
        if cur == n {
            return Some(path.to_vec());
        }
        match &doc.value(cur) {
            Value::Object(mems) => {
                for mem in mems {
                    let mut p = path.to_vec();
                    p.push(PathSeg::key(mem.key.clone()));
                    if let Some(found) = walk(doc, mem.val, &p, n) {
                        return Some(found);
                    }
                }
                None
            }
            Value::Array(items) => {
                for (i, &it) in items.iter().enumerate() {
                    let mut p = path.to_vec();
                    p.push(PathSeg::Idx(i));
                    if let Some(found) = walk(doc, it, &p, n) {
                        return Some(found);
                    }
                }
                None
            }
            _ => None,
        }
    }
    walk(doc, doc.root, &[], n)
}

/// anchorDef is one anchor in document order: its name and carrier node.
pub struct AnchorDef {
    pub name: String,
    pub node: NodeId,
}

/// anchorDefs lists the document's anchors: every distinct anchor name with
/// the first node carrying it, in document order.
pub fn anchor_defs(doc: &Doc) -> Vec<AnchorDef> {
    let mut defs: Vec<AnchorDef> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    fn walk(
        doc: &Doc,
        cur: NodeId,
        defs: &mut Vec<AnchorDef>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        let data = doc.node(cur);
        if !data.anchor.is_empty() && seen.insert(data.anchor.clone()) {
            defs.push(AnchorDef {
                name: data.anchor.clone(),
                node: cur,
            });
        }
        match &data.value {
            Value::Object(mems) => {
                for mem in mems {
                    walk(doc, mem.val, defs, seen);
                }
            }
            Value::Array(items) => {
                for &it in items {
                    walk(doc, it, defs, seen);
                }
            }
            _ => {}
        }
    }
    walk(doc, doc.root, &mut defs, &mut seen);
    defs
}

/// anchorHolder returns the first node carrying the anchor name, if any.
pub fn anchor_holder<'a>(doc: &'a Doc, name: &str) -> Option<NodeId> {
    anchor_defs(doc)
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| d.node)
}

/// countAnchorUses counts how many document locations hold the node: the
/// carrier plus every reference to it.
pub fn count_anchor_uses(doc: &Doc, n: NodeId) -> usize {
    fn walk(doc: &Doc, cur: NodeId, n: NodeId, count: &mut usize) {
        if cur == n {
            // shared node: its subtree is visited once per reference
            *count += 1;
            return;
        }
        match &doc.value(cur) {
            Value::Object(mems) => {
                for mem in mems {
                    walk(doc, mem.val, n, count);
                }
            }
            Value::Array(items) => {
                for &it in items {
                    walk(doc, it, n, count);
                }
            }
            _ => {}
        }
    }
    let mut count = 0;
    walk(doc, doc.root, n, &mut count);
    count
}

/// refWouldCycle reports whether pointing path at target would nest the
/// anchor inside itself: the target must not be one of the value's
/// ancestors (the value being replaced itself may become the target — that
/// is fine).
pub fn ref_would_cycle(doc: &Doc, path: &[PathSeg], target: NodeId) -> bool {
    for k in 0..=path.len() {
        if let Some(n) = doc.get(&path[..k]) {
            if n == target {
                return true;
            }
        }
    }
    false
}

/// mergeRefUses counts the merge-key members anywhere in the document
/// (anchor subtrees included) whose merge list references n.
pub fn merge_ref_uses(doc: &Doc, n: NodeId) -> usize {
    fn walk(
        doc: &Doc,
        cur: NodeId,
        n: NodeId,
        on_path: &mut std::collections::HashSet<NodeId>,
        count: &mut usize,
    ) {
        if !on_path.insert(cur) {
            return;
        }
        for &r in &doc.node(cur).merge {
            if r == n {
                *count += 1;
            }
        }
        match &doc.value(cur) {
            Value::Object(mems) => {
                for mem in mems {
                    walk(doc, mem.val, n, on_path, count);
                }
            }
            Value::Array(items) => {
                for &it in items {
                    walk(doc, it, n, on_path, count);
                }
            }
            _ => {}
        }
        on_path.remove(&cur);
    }
    let mut count = 0;
    let mut on_path = std::collections::HashSet::new();
    walk(doc, doc.root, n, &mut on_path, &mut count);
    count
}

/// mergeSourceFor resolves one merge reference into (name, holder path,
/// node). Unreachable references (their anchor was deleted) drop out.
fn merge_source_for(doc: &Doc, r: NodeId) -> Option<(String, Vec<PathSeg>, NodeId)> {
    let data = doc.node(r);
    if data.anchor.is_empty() || !matches!(doc.value(r), Value::Object(_)) {
        return None;
    }
    let holder = anchor_holder_path(doc, r)?;
    Some((data.anchor.clone(), holder, r))
}

/// mergedKeys lists the merged keys of the mapping at st.path in merge order
/// with YAML precedence applied: keys the mapping defines itself, and keys
/// an earlier anchor already provided, shadow the later ones.
pub fn merged_keys(doc: &Doc, st: &MenuState) -> Vec<MergedKey> {
    let Some(n) = doc.get(&st.path) else {
        return Vec::new();
    };
    let Some(Value::Object(mems)) = doc.value_opt(n) else {
        return Vec::new();
    };
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for mem in mems {
        if doc.node(mem.val).merge.is_empty() {
            seen.insert(mem.key.clone());
        }
    }
    let mut out: Vec<MergedKey> = Vec::new();
    for r in doc.merge_refs(n) {
        let Some((_, holder, node)) = merge_source_for(doc, r) else {
            continue;
        };
        // the anchor's resolved members: its own keys plus whatever its own
        // "<<" chain provides, with the anchor's own keys winning
        for mem in doc.resolved_members(node) {
            if seen.contains(&mem.key) {
                continue;
            }
            seen.insert(mem.key.clone());
            // inherited keys live in the mapping that actually defines them:
            // the badge, the breadcrumb and edits follow that source
            let mut busy = std::collections::HashSet::new();
            let (def_holder, def_node) =
                merge_defining_source(doc, (holder.clone(), node), &mem.key, &mut busy)
                    .unwrap_or((holder.clone(), node));
            let mut path = def_holder;
            path.push(PathSeg::key(mem.key.clone()));
            // Go's mergedKey src is the defining source, so the badge names
            // the anchor that actually holds the value (menu_merge.go:89).
            out.push(MergedKey {
                key: mem.key,
                path,
                src_name: doc.node(def_node).anchor.clone(),
            });
        }
    }
    out
}

type MergeSource = (Vec<PathSeg>, NodeId);

/// mergeDefiningSource finds the mapping that defines key for the merge
/// source src: the mapping itself when it defines it, else the first mapping
/// its own merge keys pull the key from (YAML precedence: own keys win,
/// earlier anchors before later ones).
fn merge_defining_source(
    doc: &Doc,
    src: MergeSource,
    key: &str,
    busy: &mut std::collections::HashSet<NodeId>,
) -> Option<MergeSource> {
    let (path, node) = src;
    if !busy.insert(node) {
        return None;
    }
    let result = (|| {
        if let Some(Value::Object(mems)) = doc.value_opt(node) {
            for mem in mems {
                if doc.node(mem.val).merge.is_empty() && mem.key == key {
                    return Some((path, node));
                }
            }
        }
        for r in doc.merge_refs(node) {
            if let Some(sub) = merge_source_for(doc, r) {
                if let Some(got) = merge_defining_source(doc, (sub.1, r), key, busy) {
                    return Some(got);
                }
            }
        }
        None
    })();
    busy.remove(&node);
    result
}

/// anchorMergeUsePaths lists every document location that uses the anchor
/// node n, in document order: the paths holding it directly (the carrier
/// and each alias occurrence) plus the paths of the mappings whose merge
/// keys reference it.
pub fn anchor_merge_use_paths(doc: &Doc, n: NodeId) -> Vec<Vec<PathSeg>> {
    let mut paths: Vec<Vec<PathSeg>> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut on_path: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    fn add(
        paths: &mut Vec<Vec<PathSeg>>,
        seen: &mut std::collections::HashSet<String>,
        p: &[PathSeg],
    ) {
        if seen.insert(path_string(p)) {
            paths.push(p.to_vec());
        }
    }
    fn walk(
        doc: &Doc,
        cur: NodeId,
        path: &[PathSeg],
        n: NodeId,
        on_path: &mut std::collections::HashSet<NodeId>,
        paths: &mut Vec<Vec<PathSeg>>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        if !on_path.insert(cur) {
            return;
        }
        if cur == n {
            // the shared node itself: its subtree belongs to the anchor's
            // own definition, not to this occurrence
            add(paths, seen, path);
            on_path.remove(&cur);
            return;
        }
        if doc.merge_refs(cur).iter().any(|&r| r == n) {
            add(paths, seen, path);
        }
        match &doc.value(cur) {
            Value::Object(mems) => {
                for mem in mems {
                    let mut p = path.to_vec();
                    p.push(PathSeg::key(mem.key.clone()));
                    walk(doc, mem.val, &p, n, on_path, paths, seen);
                }
            }
            Value::Array(items) => {
                for (i, &it) in items.iter().enumerate() {
                    let mut p = path.to_vec();
                    p.push(PathSeg::Idx(i));
                    walk(doc, it, &p, n, on_path, paths, seen);
                }
            }
            _ => {}
        }
        on_path.remove(&cur);
    }
    walk(doc, doc.root, &[], n, &mut on_path, &mut paths, &mut seen);
    paths
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
    app.exec_editor = true;
    let mut stdout = std::io::stdout();
    let _ = crossterm::terminal::enable_raw_mode();
    // bubbletea delivers an initial WindowSizeMsg; crossterm does not, so the
    // first frame must query the pty size explicitly (otherwise it renders at
    // the 80x24 default).
    if let Ok((w, h)) = crossterm::terminal::size() {
        app.width = w as usize;
        app.height = h as usize;
    }
    let _ = execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableBracketedPaste,
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
            // disk state. An external change opens the reload confirm; an
            // unchanged file costs one stat per tick and no repaint.
            redraw = app.poll_disk_change();
        }
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
                    app.width = *w as usize;
                    app.height = *h as usize;
                    redraw = true;
                    i += 1;
                }
                Event::Paste(s) => {
                    app.update(Msg::Paste(s.clone()));
                    app.run_pending_editor();
                    redraw = true;
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
