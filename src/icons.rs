//! Severity glyphs shared by the TUI, the CLI status line and the
//! model-layer warnings: Nerd Font / Font Awesome private-use codepoints, so
//! a terminal needs a patched (Nerd) font to draw them — an unpatched one
//! shows empty boxes. Kept in a leaf module because the TUI, the fetcher and
//! the form model all print severity marks and none of them may depend on
//! the others.

/// Err marks a validation error: U+F057 (fa-times-circle).
pub const ERR: &str = "\u{f057}";

/// Warn marks a warning: U+F071 (fa-exclamation-triangle).
pub const WARN: &str = "\u{f071}";
