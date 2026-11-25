//! Tiny style layer producing the ANSI SGR sequences lipgloss emitted. Each
//! render closes with ESC[0m like lipgloss.

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
/// sug_run styles one run of a completion row: `base` carries the row's
/// own look (`"7"` = reverse video for the selected row, `"2;48;5;236"`
/// = faint on dark for the rest) and `hit` adds the bold red matched
/// highlight on top of it.
pub fn sug_run(base: &str, hit: bool, s: &str) -> String {
    if hit {
        render(&format!("{base};1;31"), s)
    } else {
        render(base, s)
    }
}
/// Diff badges on rows whose content differs from the load/last-save
/// baseline: `+` added (green), `-` removed (red), `~` modified
/// (orange).
pub fn mark_add(s: &str) -> String {
    render("38;5;114", s)
}
pub fn mark_del(s: &str) -> String {
    render("38;5;203", s)
}
pub fn mark_mod(s: &str) -> String {
    render("38;5;214", s)
}
