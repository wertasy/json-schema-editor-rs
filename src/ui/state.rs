//! Shared UI state: the per-file document state and the app model, the
//! single overlay surface with its per-surface states, the reduced key
//! protocol, and the pure helpers the dispatchers and renderers share.
//! Definitions live here — one layer below `menu`, `keys` and `files` — so
//! those modules operate on common data without importing each other.

use std::collections::HashMap;

use regex::Regex;

use crate::doc::jsondoc::path_string;
use crate::doc::{Doc, NodeId, PathSeg, Value};
use crate::icons::{ERR, WARN};
use crate::smodel::{FormNode, FormNodeId, Schema};
use crate::ui::command::CommandState;
use crate::ui::history::History;
use crate::ui::nav::Nav;
use crate::ui::style;
use crate::ui::util::clip_display;
use crate::ui::widgets::{Key, TextArea, TextInput};
use crate::ui::{Issue, MenuCtx, MenuState, Row, RowKind};

// --- per-file state -----------------------------------------------------------

/// diskStat is the coarse file identity the idle loop watches: modification
/// time plus length (the length guards filesystems with coarse timestamps).
pub type DiskStat = (std::time::SystemTime, u64);

/// Identity of a synthesized schema node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SynthKey {
    /// map/array value schema unknown: raw-editing fallback for node
    Fallback(FormNodeId),
    /// union menu over node's (dynamic) variant list
    Merged(FormNodeId),
    /// variant's menu with discriminator keys unionized
    Pinned(FormNodeId, FormNodeId),
    /// arbitrary JSON type picker (unknown keys, untyped schema fields)
    AnyType,
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

/// fileState is the per-file half of the Go listModel: everything that
/// belongs to one document.
pub struct FileState {
    /// the form tree of this file (arena; Go: `model *smodel.FormNode`)
    pub schema: Schema,
    pub doc: crate::doc::Doc,
    /// Format state for a faithful save: comments/directives and the YAML
    /// alias count. Owned here so one file's state never leaks to another.
    pub meta: crate::doc::Meta,
    /// The file's format codec (save, raw editors, schema directive).
    pub codec: &'static dyn crate::doc::Codec,
    pub opts: crate::ui::Options,
    pub stack: Vec<MenuState>,
    /// at least one successful write this session
    pub saved: bool,
    /// the last successful write skipped validation errors (--force)
    pub with_errors: bool,
    /// memoized synthesized schema nodes (Go: `menuCache`); keyed by the
    /// rows::SynthKey identity that replaces Go's pointer memo
    pub menu_cache: HashMap<SynthKey, crate::smodel::FormNodeId>,
    /// undo/redo stacks (Go: `undoStack`/`redoStack`)
    pub history: History,
    /// loaded/last-saved snapshot for change marks
    pub baseline: Option<crate::doc::Doc>,
    /// a document comment was edited since the last save; comments live in
    /// the format metadata, not the Doc baseline, so they need their own
    /// dirty bit (the quit prompt and title marker consult it).
    pub comments_dirty: bool,
    /// live validation problems (refreshed after writes)
    pub issues_now: Vec<Issue>,
    /// validation error locations, marked on their rows
    pub err_paths: Vec<Vec<PathSeg>>,
    /// unknown-key warning locations (yellow marks)
    pub warn_paths: Vec<Vec<PathSeg>>,
    /// parked issues overlay: esc at the jump target returns here
    pub issues_parked: Option<IssuesState>,
    /// stack depth of the issue jump target
    pub issues_return: usize,
    /// stack captured when the issue jump parked the list
    pub issues_origin: Option<Vec<MenuState>>,
    /// stack captured when ga/gr jumped; None when inactive
    pub jump_origin: Option<Vec<MenuState>>,
    /// stack depth of the ga/gr landing
    pub jump_return: usize,
    /// parked search overlay state (jump landed, esc at return_depth reopens)
    pub search_parked: Option<SearchState>,
    /// adaptive ctrl+j/k ring: the changed paths from the last
    /// replace/undo/redo, auto-landing on the first entry; any unrelated
    /// document edit clears it
    pub change_jump: Option<Vec<Vec<PathSeg>>>,
    /// session info notices (aliases kept, schema source), dropped on the
    /// first key press
    pub startup: String,
    /// the file's disk stat recorded at load and refreshed after each save,
    /// reload or declined reload prompt; the idle loop re-stats the file and
    /// opens the reload confirm when it drifted. None while the file does
    /// not exist on disk (started empty) or cannot be stat'ed.
    pub disk_stat: Option<DiskStat>,
}

// --- help text and description data -------------------------------------------

pub const MENU_HELP: &str =
    "↑/↓ move · enter edit/open · esc back · ctrl+s/w save · / search · : command · ? help";

/// DiffKind bit flags for the aggregated per-path diff marks
/// ([`App::diff_agg`]).
pub const DIFF_ADDED: u8 = 1;
pub const DIFF_REMOVED: u8 = 2;
pub const DIFF_MODIFIED: u8 = 4;
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
        desc: "adaptive next / previous: changed keys from the last replace/undo/redo, then keys with validation issues, then search hits, then keys changed since load/save",
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
        desc: "number: subtract / add 1 (missing schema-typed values seed from their numeric schema default, else -1/1; schema-less/array-item numbers use their runtime value)",
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
        desc: "edit the row's value in $EDITOR (strings open as their own text; multiline strings always open here)",
    },
    HelpEntry {
        keys: "E",
        desc: "edit the focused row's value in $EDITOR (raw JSON/YAML/TOML, strings as text; whole document when the cursor is not on a value row)",
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

// --- the key protocol ---------------------------------------------------------

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

/// Map a Msg to the reduced widget Key (Go feeds the bubbles widgets the
/// tea.KeyMsg directly; this covers the operations they exercise).
pub(crate) fn msg_key(msg: &Msg) -> Option<Key> {
    Some(match msg {
        Msg::Char(c, false) => Key::Char(*c),
        // shell-style word motion inside every text input
        Msg::Char('f', true) | Msg::Char('F', true) => Key::WordForward,
        Msg::Char('b', true) | Msg::Char('B', true) => Key::WordBackward,
        Msg::Enter => Key::Enter,
        Msg::Backspace => Key::Backspace,
        Msg::Delete => Key::Delete,
        Msg::Left => Key::Left,
        Msg::Right => Key::Right,
        Msg::Up => Key::Up,
        Msg::Down => Key::Down,
        Msg::Home => Key::Home,
        Msg::End => Key::End,
        Msg::Ctrl('a') => Key::CtrlA,
        Msg::Ctrl('e') => Key::CtrlE,
        Msg::Ctrl('k') => Key::CtrlK,
        Msg::Ctrl('u') => Key::CtrlU,
        Msg::Ctrl('w') => Key::CtrlW,
        Msg::Ctrl('y') => Key::CtrlY,
        _ => return None,
    })
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
    /// matched char indices per sugs entry (fuzzy highlight)
    pub sug_hl: Vec<Vec<usize>>,
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
                let Some(k) = msg_key(msg) else {
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
    pub on_yes: Option<ConfirmAction>,
    pub on_no: Option<ConfirmAction>,
}

/// ConfirmAction is what a y/n overlay runs on the answer. It carries the
/// data the answer acts on, not behavior: the confirm state must not know
/// the app model, so the key router (keys::overlays) interprets the variant
/// against `App`.
pub enum ConfirmAction {
    /// reload the file at this index from disk
    ReloadFile(usize),
    /// remove the value at `path`; `pop` also drops the items-menu level
    RemoveItem { path: Vec<PathSeg>, pop: bool },
    /// replace the value at `path` with `variant`'s editor; the router
    /// hands the fields to the replace-value handler
    ReplaceValue {
        variant: Option<FormNodeId>,
        path: Vec<PathSeg>,
        one_of: FormNodeId,
        old: Option<NodeId>,
        keep_slot: bool,
    },
    /// save all dirty files and exit the session
    SaveDirtyAndExit,
    /// discard the changes and exit as aborted
    Abort,
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
    /// the target was a string value: the temp file holds its unquoted text
    /// and the edited text applies verbatim (the raw textarea's rule)
    pub verbatim_str: bool,
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
    /// screen line → click target for the last rendered frame (mouse
    /// support): (first line, line count, target), rebuilt per repaint
    pub mouse_targets: Vec<(usize, usize, MouseTarget)>,
    /// diff classification per document path against the load/last-save
    /// baseline, rebuilt per repaint: a bitmask of DiffKind bits aggregated
    /// over the path and its whole subtree (the empty path aggregates the
    /// entire document for the root row)
    pub diff_agg: std::collections::HashMap<Vec<crate::doc::jsondoc::PathSeg>, u8>,
    /// paths marked as added (whole added subtrees): children carrying a
    /// value inside these inherit the `+` badge
    pub diff_added: Vec<Vec<crate::doc::jsondoc::PathSeg>>,
    /// in-flight SchemaStore catalog load (`S`): the event loop polls the
    /// handle and renders the help-bar progress until it finishes
    pub pending_catalog:
        Option<std::thread::JoinHandle<Result<Vec<crate::catalog::Entry>, String>>>,
    /// in-flight schema download after a picker selection: the entry plus
    /// the load job; the tail of [`App::apply_schema`] runs on completion
    pub pending_schema: Option<(
        crate::catalog::Entry,
        std::thread::JoinHandle<Result<crate::ui::LoadedSchema, String>>,
    )>,
}

/// What a click on a rendered screen line hits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseTarget {
    /// row index in the main menu list
    MenuRow(usize),
    /// candidate index of the inline scalar edit's dropdown
    EditSug(usize),
    /// candidate index of the anchor/reference input's dropdown
    AncSug(usize),
    /// cursor index of the open overlay's list (issues, errors, refs,
    /// files, search results)
    OverlayRow(usize),
}

/// human_bytes renders a byte count for the fetch progress bar
/// (`512 B`, `12.3 kB`, `4.2 MB`).
pub(crate) fn human_bytes(v: u64) -> String {
    const UNITS: [&str; 3] = ["B", "kB", "MB"];
    let mut f = v as f64;
    let mut unit = 0usize;
    while f >= 1024.0 && unit + 1 < UNITS.len() {
        f /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{v} B")
    } else {
        format!("{f:.1} {}", UNITS[unit])
    }
}

impl App {
    /// set_viewport adopts the terminal size. A pty can report a degenerate
    /// size (0×0 while tearing down); the floor keeps every row/pane
    /// computation a real divisor instead of an empty screen.
    pub fn set_viewport(&mut self, w: u16, h: u16) {
        self.width = (w as usize).max(1);
        self.height = (h as usize).max(1);
    }

    /// The active file's state (Go: the listModel fields themselves).
    pub fn fs(&self) -> &FileState {
        &self.files[self.cur]
    }

    pub fn fs_mut(&mut self) -> &mut FileState {
        &mut self.files[self.cur]
    }

    /// top returns the current navigation state.
    pub fn top(&self) -> Option<&MenuState> {
        self.fs().stack.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut MenuState> {
        self.fs_mut().stack.last_mut()
    }
    /// fetch_bar renders the bottom-bar progress line while a SchemaStore
    /// catalog / schema load is pending or an HTTP fetch is streaming:
    /// `[####      ] 12.3 kB / 45.6 kB — updating SchemaStore cache`. The
    /// total comes from Content-Length; without it the bar sweeps.
    pub fn fetch_bar(&self) -> Option<String> {
        let prog = crate::progress::progress();
        let pending = self.pending_catalog.is_some() || self.pending_schema.is_some();
        if prog.is_none() && !pending {
            return None;
        }
        let label = if self.pending_catalog.is_some() {
            "loading SchemaStore catalog".to_string()
        } else if self.pending_schema.is_some() {
            "loading schema".to_string()
        } else {
            prog.as_ref()
                .map(|p| p.label.clone())
                .unwrap_or_else(|| "fetching".to_string())
        };
        let (got, total) = prog.as_ref().map_or((0, None), |p| (p.got, p.total));
        let width = self.width.saturating_sub(2).min(24).max(8);
        let filled = match total.filter(|t| *t > 0) {
            Some(t) => ((got.min(t) as f64 / t as f64) * width as f64).round() as usize,
            None => {
                // indeterminate: a sweeping window driven by the wall clock
                let phase = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_millis() / 120)
                    .unwrap_or(0) as usize;
                let w = width / 4;
                (phase % width).min(width - w) + if (phase / width) % 2 == 0 { 0 } else { w }
            }
        };
        let mut bar = String::with_capacity(width + 32);
        bar.push('[');
        for k in 0..width {
            bar.push_str(&style::accent(if k < filled { "█" } else { "·" }));
        }
        bar.push(']');
        let bytes = match total.filter(|t| *t > 0) {
            Some(t) => format!("{} / {}", human_bytes(got), human_bytes(t)),
            None => human_bytes(got),
        };
        let line = format!("{bar} {bytes} — {label}");
        Some(crate::ui::util::clip_display(
            &line,
            self.width.saturating_sub(1),
        ))
    }

    /// diff_badge_of renders the colored diff badge for a row path: one
    /// glyph per change kind present in the row's subtree (`+` green added,
    /// `-` red removed, `~` orange modified). Children of an **added**
    /// subtree inherit the `+` once they carry a value; unassigned keys are
    /// suppressed (`drop_added`).
    pub(crate) fn diff_badge_of(&self, path: &[PathSeg], has_value: bool) -> Option<String> {
        let bits = self.diff_kinds_of(path, has_value)?;
        if bits == 0 {
            return None;
        }
        let mut out = String::new();
        if bits & DIFF_ADDED != 0 {
            out.push_str(&style::mark_add(" +"));
        }
        if bits & DIFF_REMOVED != 0 {
            out.push_str(&style::mark_del(" -"));
        }
        if bits & DIFF_MODIFIED != 0 {
            out.push_str(&style::mark_mod(" ~"));
        }
        Some(out)
    }

    /// diff_kinds_of returns the change-kind bitmask for a row path: the
    /// union of the kinds marked on the path itself and anywhere inside its
    /// subtree. There is no ancestor fallback — a changed sibling must not
    /// mark this row. Children of an **added** subtree inherit the added bit
    /// once they carry a value.
    fn diff_kinds_of(&self, path: &[PathSeg], has_value: bool) -> Option<u8> {
        let mut bits = *self.diff_agg.get(path).filter(|bits| **bits != 0)?;
        if bits & DIFF_ADDED == 0
            && has_value
            && self
                .diff_added
                .iter()
                .any(|added| path.starts_with(added) && path.len() > added.len())
        {
            bits |= DIFF_ADDED;
        }
        Some(bits)
    }

    /// A MenuCtx for the pure search/summary helpers.
    pub fn ctx(&self) -> MenuCtx<'_> {
        let f = self.fs();
        MenuCtx {
            schema: &f.schema,
            doc: &f.doc,
        }
    }
}

// --- shared pure helpers ------------------------------------------------------

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

// --- pure helpers shared by menu/keys/files ---------------------------------

/// isDigitKey reports whether key is a single decimal digit.
pub(crate) fn is_digit_key(key: &str) -> bool {
    let b = key.as_bytes();
    b.len() == 1 && b[0] >= b'0' && b[0] <= b'9'
}

/// isCountBuf reports whether the pending buffer is digits only — a count
/// for N gg. A buffer holding the armed g (or ga/gr remnants) is not.
pub(crate) fn is_count_buf(buf: &[String]) -> bool {
    !buf.is_empty() && buf.iter().all(|k| is_digit_key(k))
}

/// bufCount folds the digit prefix of a pending buffer into its number.
pub(crate) fn buf_count(buf: &[String]) -> (usize, bool) {
    let mut n = 0usize;
    let mut digits = 0;
    for k in buf {
        if !is_digit_key(k) {
            break;
        }
        n = n * 10 + (k.as_bytes()[0] - b'0') as usize;
        digits += 1;
    }
    (n, digits > 0)
}

/// listSearchMatch reports whether text contains the query
/// (case-insensitive); an empty query matches everything.
fn list_search_match(q: &str, text: &str) -> bool {
    q.is_empty() || text.to_lowercase().contains(&q.to_lowercase())
}

/// issueIndices lists the issues matching the overlay's search query.
pub(crate) fn issue_indices(s: &IssuesState) -> impl Iterator<Item = usize> + '_ {
    let q = s.filter.ti.value();
    s.items
        .iter()
        .enumerate()
        .filter(move |(_, it)| {
            list_search_match(q, &it.msg)
                || it.path.as_ref().map_or(false, |p| {
                    list_search_match(q, &crate::doc::jsondoc::path_string(p))
                })
        })
        .map(|(i, _)| i)
}

/// errIndices lists the save-error lines matching the search query.
pub(crate) fn err_indices(s: &ErrsState) -> impl Iterator<Item = usize> + '_ {
    let q = s.filter.ti.value();
    s.lines
        .iter()
        .enumerate()
        .filter(move |(_, line)| list_search_match(q, line))
        .map(|(i, _)| i)
}

/// refWordsAbove reports whether every word appears as a whole member key in
/// the NUL-wrapped ancestor path.
pub(crate) fn ref_words_above(words: &[&str], ancestors: &str) -> bool {
    words
        .iter()
        .all(|w| ancestors.contains(&format!("\0{w}\0")))
}

/// isMergeSeparator reports whether r separates merge-key tokens: spaces and
/// commas (both the "a b" and "[*a, *b]" reading work).
fn is_merge_separator(r: char) -> bool {
    r == ' ' || r == ',' || r == '\t'
}

/// splitLastToken splits the input into everything up to the final token and
/// that token, keeping the separator before it in rest.
pub(crate) fn split_last_token(v: &str) -> (String, String) {
    if let Some(i) = v.rfind(is_merge_separator) {
        return (v[..=i].to_string(), v[i + 1..].to_string());
    }
    (String::new(), v.to_string())
}

/// mergeTokenList splits an input fragment into bare anchor names: tokens
/// separated by spaces or commas, an optional "*" prefix stripped, empty
/// tokens dropped.
pub(crate) fn merge_token_list(v: &str) -> Vec<String> {
    v.split(is_merge_separator)
        .map(|t| t.trim().trim_start_matches('*').to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// mergeFiltered lists the candidates for the token being typed.
pub(crate) fn merge_filtered(s: &MergeState) -> Vec<String> {
    let (rest, last) = split_last_token(s.ti.value());
    let used: std::collections::HashSet<String> = merge_token_list(&rest).into_iter().collect();
    let last = last.trim_start_matches('*').to_lowercase();
    s.cands
        .iter()
        .filter(|c| !used.contains(*c) && (last.is_empty() || c.to_lowercase().starts_with(&last)))
        .cloned()
        .collect()
}

/// fold_desc_block caps one collapsed source block at
/// [`DESC_BLOCK_PREVIEW_LINES`] lines, leaving room for the trailing `…`.
pub(crate) fn fold_desc_block(mut lines: Vec<String>, width: usize) -> Vec<String> {
    if lines.len() <= DESC_BLOCK_PREVIEW_LINES {
        return lines;
    }
    lines.truncate(DESC_BLOCK_PREVIEW_LINES);
    if let Some(last) = lines.last_mut() {
        // one column for the space and one for the `…`
        *last = clip_display(last, width.saturating_sub(2));
    }
    lines
}

/// desc_line_view styles one description line; a folded block ends with an
/// emphasized `…`.
pub(crate) fn desc_line_view(line: &DescLine) -> String {
    let body = match line.source {
        DescSource::Schema => style::desc(&line.text),
        DescSource::Comment => style::comment(&line.text),
    };
    if line.folded {
        format!("{body} {}", style::accent("…"))
    } else {
        body
    }
}

/// worstIssueIcon picks the severity glyph for a set of issues: any real
/// validation error outranks the warnings.
pub(crate) fn worst_issue_icon(items: &[Issue]) -> &'static str {
    if items.iter().any(|it| !it.warn) {
        ERR
    } else {
        WARN
    }
}

/// same_state by value (menu.rs same_state needs two MenuStates).
pub(crate) fn same_state_eq(a: &MenuState, b: &MenuState) -> bool {
    a.node == b.node
        && a.kind == b.kind
        && a.variant_of == b.variant_of
        && a.one_of == b.one_of
        && a.path == b.path
}
