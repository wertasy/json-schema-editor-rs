use super::*;
use crate::doc::jsondoc::{Member, NodeData};
use crate::doc::{Doc, NodeId, Value};
use crate::smodel::{EnumOpt, FormNode, Kind, Schema};
use crate::ui::{MenuCtx, MenuKind, MenuState};
use regex::Regex;
use tempfile::tempdir;

#[path = "harness_tests.rs"]
mod harness;
#[path = "menu_rows_tests.rs"]
mod menu_rows;
#[path = "search_tests.rs"]
mod search;
#[path = "stack_tests.rs"]
mod stack;
#[path = "summary_tests.rs"]
mod summary;
#[path = "variant_tests.rs"]
mod variant;

use harness::*;
