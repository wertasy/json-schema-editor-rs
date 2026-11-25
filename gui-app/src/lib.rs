//! Thin Native Shell app crate around the jse GUI: the nsh runners
//! (`nsh run` / `nsh package`) build this crate with the editor library's
//! `gui` feature enabled and mount its root component.
//!
//! It owns the `#[native_module]` entry (`jse/native`) rather than the
//! library, so the generated runners serve exactly one UI set.

pub use jse::gui::App;

/// The app's UI set: the root component the nsh runners mount.
#[nsh_core::prelude::native_module("jse/native")]
fn native(registry: &mut nsh_core::prelude::Registry) {
    registry.register::<App>();
}
