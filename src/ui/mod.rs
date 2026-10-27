//! Terminal UI: menu engine, overlays, accessible line protocol.
//!
//! Rust port of `internal/ui` — the menuconfig-style TUI over the smodel
//! form tree. Editing writes through to the document immediately; there is
//! no per-screen form submit.
//!
//! Structural mapping from Go: the bubbletea `listModel` becomes [`App`],
//! and the 16 nullable overlay fields + `routeKey` priority chain become a
//! single `Option<Overlay>` (only one surface can be open at a time — every
//! Go open-path closes the previous one first). Per-file state lives in
//! [`FileState`]; the active file is `files[cur]`, so Go's
//! captureFileState/restoreFileState dance collapses into an index switch.
//!
//! All node references are arena ids: `*smodel.FormNode` → `FormNodeId`
//! (resolved against the file's `Schema`), `*jsondoc.Node` → `Doc` owned by
//! the file state with `NodeId`s into it.

pub mod accessible;
pub mod files;
pub mod history;
pub mod keys;
pub mod lineio;
pub mod menu;
pub mod nav;
pub mod rows;
pub mod schema_picker;
pub mod shell;
pub mod util;
pub mod widgets;

use std::sync::Arc;

use crate::doc::{Doc, PathSeg};
use crate::smodel::{FormNodeId, Schema};

/// result is the internal outcome of an accessible screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Res {
    /// done with this screen, go back
    Pop,
    /// user saved; unwind everything
    Saved,
    /// aborted on the root screen; exit 130
    Abort,
}

/// The validator closure a compiled schema installs: one message per
/// validation error, empty when the document is valid.
pub type ValidateFn = Arc<dyn Fn(&Doc) -> Vec<String> + Send + Sync>;

/// A schema compiled for one file: the form tree, its validator and the
/// status lines the load produced. The CLI and the S-key picker both go
/// through this shape so the UI never compiles schemas itself.
pub struct LoadedSchema {
    pub schema: Schema,
    pub root: FormNodeId,
    /// None when compilation failed or validation is disabled.
    pub validate: Option<ValidateFn>,
    /// Non-fatal status lines (offline cache, compile failure).
    pub status: String,
    /// The schema declares a top-level "$schema" property.
    pub schema_member: bool,
}

/// Options configures a Run. Only genuine environment I/O stays behind a
/// closure; document state (doc, meta, codec) belongs to [`FileState`].
#[derive(Clone)]
pub struct Options {
    pub accessible: bool,
    pub force: bool,
    /// Validate checks a config document; None disables save-time validation.
    pub validate: Option<ValidateFn>,
    /// Save writes the document to its destination with the session's
    /// current meta. Every save action goes through it (TUI ctrl+s, the
    /// Save-and-exit row, the accessible save); None makes saving impossible.
    pub save: Option<Arc<dyn Fn(&Doc, &crate::doc::Meta) -> Result<(), String> + Send + Sync>>,
    /// Path is the file the session edits; menu W prefills its write-to
    /// input with it.
    pub path: String,
    /// WriteTo writes the document to an arbitrary path (menu W). None
    /// makes write-to impossible.
    pub write_to:
        Option<Arc<dyn Fn(&str, &Doc, &crate::doc::Meta) -> Result<(), String> + Send + Sync>>,
    /// Copy writes text to the system clipboard (menu y/Y).
    pub copy: Option<Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>>,
    /// Paste reads the system clipboard (menu p/P).
    pub paste: Option<Arc<dyn Fn() -> Result<String, String> + Send + Sync>>,
    /// LoadSchema compiles a schema location for the S picker; None disables
    /// schema switching.
    pub load_schema: Option<Arc<dyn Fn(&str) -> Result<LoadedSchema, String> + Send + Sync>>,
    /// Status is a warning line shown on the root screen (e.g. compile
    /// failure).
    pub status: String,
    /// StartupNotice holds informational session notices (YAML aliases
    /// kept, the SchemaStore source). Dropped on the first key press.
    pub startup_notice: String,
    /// LoadErrors are the validation messages of the file as loaded.
    pub load_errors: Vec<String>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            accessible: false,
            force: false,
            validate: None,
            save: None,
            path: String::new(),
            write_to: None,
            copy: None,
            paste: None,
            load_schema: None,
            status: String::new(),
            startup_notice: String::new(),
            load_errors: Vec::new(),
        }
    }
}

/// FileSession is one file of an interactive session: its form tree,
/// document, format state, and editor wiring.
pub struct FileSession {
    pub schema: Schema,
    pub root: FormNodeId,
    pub doc: Doc,
    pub meta: crate::doc::Meta,
    pub codec: &'static dyn crate::doc::Codec,
    pub opts: Options,
}

/// Outcome reports how the session ended.
#[derive(Debug, Default, Clone)]
pub struct Outcome {
    /// at least one write succeeded during the session
    pub saved: bool,
    /// the last write went through --force despite validation errors
    pub with_errors: bool,
    /// the session ended via ctrl+c/EOF abort rather than a save-and-exit
    /// or a clean quit
    pub aborted: bool,
    /// the session ended cleanly without writing
    pub unchanged: bool,
    /// files written this session, in file order
    pub saved_files: Vec<String>,
    pub err: Option<String>,
}

// --- menu model types (shared by rows/menu/keys) -------------------------------

/// menuKind classifies one screen of the menu tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// KObject: field + drill rows
    Object,
    /// KArray: add + per-item rows
    Items,
    /// KArray with enum items: toggle rows
    EnumArray,
    /// KOneOf: variant rows
    OneOf,
}

/// rowKind classifies one rendered line inside a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RowKind {
    /// scalar/enum/bool/raw: edit in place
    #[default]
    Field,
    /// object/array/oneOf: push child menu
    Drill,
    /// enum-array member checkbox
    Toggle,
    /// "< Add item >"
    Add,
    /// oneOf variant picker line
    Variant,
    /// placeholder for empty menus
    NoItems,
    /// document key the schema does not define
    Unknown,
    /// "< Expand >": materialize a YAML alias occurrence
    Expand,
    /// "<err|warn> N validation errors" status row (root)
    Issues,
}

/// row is one line of a menu. fn/path drive editing and drill-down; idx is
/// the array index for item rows and the option index for toggle rows (-1
/// otherwise); variant points at the FormNode of a oneOf variant; label is
/// the fixed text of synthetic rows (and the display text of search hits).
/// merged names the anchor a YAML merge key pulled this row in from.
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub kind: RowKind,
    pub fn_: Option<FormNodeId>,
    pub path: Vec<PathSeg>,
    pub idx: i64,
    pub variant: Option<FormNodeId>,
    pub label: String,
    pub merged: String,
    /// Unknown row that the schema still defines via patternProperties
    pub in_schema: bool,
}

/// menuState is one level of the navigation stack; cursor/offset are
/// remembered so popping restores the exact view. variant_of is Some when
/// this is the dynamic item menu of an all-object oneOf; one_of is Some on
/// the top-level editor menu of a classic oneOf variant (t type
/// switch).
#[derive(Clone, Debug)]
pub struct MenuState {
    pub kind: MenuKind,
    pub node: FormNodeId,
    pub path: Vec<PathSeg>,
    pub cursor: usize,
    pub offset: usize,
    pub variant_of: Option<FormNodeId>,
    pub one_of: Option<FormNodeId>,
    /// rows marked with m for batch y / d, keyed like Go's sel map
    pub sel: std::collections::HashSet<String>,
    /// v visual select: movement marks rows
    pub visual: bool,
    /// row where visual select started
    pub vanchor: usize,
    /// inline list search (oneOf picker / enum-array toggles)
    pub filter: crate::ui::menu::ListFilter,
}

/// issue is one validation message with its parsed instance location.
#[derive(Clone, Debug, Default)]
pub struct Issue {
    pub msg: String,
    pub path: Option<Vec<PathSeg>>,
    /// every location the message refers to (additionalProperties messages
    /// name several keys).
    pub flagged: Vec<Vec<PathSeg>>,
    /// rows that carry the severity icon for this issue (narrower than
    /// flagged: names valid in the edited value must not be marked).
    pub marks: Vec<Vec<PathSeg>>,
    /// editor-level warning (schema-unknown key), not a validator error
    pub warn: bool,
}

/// The context pure row functions read: the schema and document of the
/// active file. Kept as a struct so the row layer never takes `App`.
pub struct MenuCtx<'a> {
    pub schema: &'a Schema,
    pub doc: &'a Doc,
}

// --- run entry points -----------------------------------------------------------

/// Run drives the whole interactive session over one or more files: the TUI
/// shows the first and s switches between them; accessible mode runs the
/// files sequentially (an abort stops the whole run).
///
/// The accessible branch routes to `accessible::run_accessible`, which lands
/// with its porting phase; until then a TUI-less session is an error (the
/// binary never selects it before the CLI exists).
pub fn run(files: Vec<FileSession>) -> Outcome {
    if files.is_empty() {
        return Outcome {
            err: Some("no files to edit".to_string()),
            ..Default::default()
        };
    }
    if files[0].opts.accessible {
        return accessible::run_accessible(files);
    }
    menu::run_menu(files)
}
