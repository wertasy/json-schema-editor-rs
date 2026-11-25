//! Model-level tests for the menu TUI: key-message injection plus doc/state
//! assertions. Rust port of the Go internal/ui menu_*_test.go suites — every
//! test that builds the model, sends keys and asserts doc/state. Bubbletea
//! View-string rendering tests are dropped (the Rust view layer emits ANSI
//! and is exercised by the interactive path, not asserted here).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::doc::jsondoc::{self, Member};
use crate::doc::{Doc, Meta, NodeData, NodeId, PathSeg, Value};
use crate::smodel::{EnumOpt, FormNode, FormNodeId, Kind, Schema};
use crate::ui::command::fuzzy_match;
use crate::ui::menu::{alias_at, same_menu_row};
use crate::ui::util::char_width;
use crate::ui::{FileSession, Issue, Options};

#[path = "arrays_tests.rs"]
mod arrays;
#[path = "commands_tests.rs"]
mod commands;
#[path = "completion_tests.rs"]
mod completion;
#[path = "editing_tests.rs"]
mod editing;
#[path = "harness_tests.rs"]
mod harness;
#[path = "menu_basics_tests.rs"]
mod menu_basics;
#[path = "merge_anchors_tests.rs"]
mod merge_anchors;
#[path = "nav_rings_tests.rs"]
mod nav_rings;
#[path = "paste_clipboard_tests.rs"]
mod paste_clipboard;
#[path = "save_write_tests.rs"]
mod save_write;
#[path = "search_replace_tests.rs"]
mod search_replace;
#[path = "session_files_tests.rs"]
mod session_files;
#[path = "unknown_issues_tests.rs"]
mod unknown_issues;
#[path = "variants_tests.rs"]
mod variants;

use harness::*;
