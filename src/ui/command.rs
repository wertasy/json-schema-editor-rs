//! vi-style `:` command line: builtin commands plus `!` shell escapes.
//!
//! The command line is a normal one-line [`TextInput`](crate::ui::widgets::TextInput),
//! so completion works at the cursor, not on the whole buffer. `!` switches
//! to shell completion (executables from `PATH`, then filesystem paths);
//! without it the builtin command names complete. In a shell command `%`
//! stands for the session file and tab expands it to the absolute path.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::ui::shell::ShellMode;

/// A parsed `:` command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `w`, `wa`, `wq`, `waq` and their `!` (forced) forms
    Save { all: bool, force: bool, quit: bool },
    /// `w <path>`
    WriteTo { path: String },
    /// `q`, `q!`
    Quit { force: bool },
    /// `!<cmd>`, `r !<cmd>`, `a !<cmd>`, `w !<cmd>`
    Shell { mode: ShellMode, command: String },
    /// `!!`: rerun the previous shell command
    RepeatShell,
}

/// Builtin commands offered by completion. The order is the display order;
/// filtering preserves it.
pub const BUILTINS: &[&str] = &[
    "w", "wa", "wq", "waq", "q", "w!", "wa!", "wq!", "waq!", "q!", "r", "a",
];

/// Parse one `:` input line (without the prompt).
pub fn parse_command(line: &str) -> Result<Command, String> {
    let line = line.trim();
    if line.is_empty() {
        return Err("command required".to_string());
    }
    if let Some(rest) = line.strip_prefix('!') {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err("shell command required".to_string());
        }
        if rest == "!" {
            return Ok(Command::RepeatShell);
        }
        return Ok(Command::Shell {
            mode: ShellMode::Execute,
            command: rest.to_string(),
        });
    }
    let (head, rest) = split_head(line);
    match head {
        "w" => {
            if rest.is_empty() {
                Ok(Command::Save {
                    all: false,
                    force: false,
                    quit: false,
                })
            } else if rest.starts_with('!') {
                match shell_arg(rest) {
                    Some(command) => Ok(Command::Shell {
                        mode: ShellMode::Write,
                        command,
                    }),
                    None => Err("shell command required".to_string()),
                }
            } else {
                Ok(Command::WriteTo {
                    path: rest.to_string(),
                })
            }
        }
        "w!" => no_arg(
            head,
            rest,
            Command::Save {
                all: false,
                force: true,
                quit: false,
            },
        ),
        "wa" => no_arg(
            head,
            rest,
            Command::Save {
                all: true,
                force: false,
                quit: false,
            },
        ),
        "wa!" => no_arg(
            head,
            rest,
            Command::Save {
                all: true,
                force: true,
                quit: false,
            },
        ),
        "wq" => no_arg(
            head,
            rest,
            Command::Save {
                all: false,
                force: false,
                quit: true,
            },
        ),
        "wq!" => no_arg(
            head,
            rest,
            Command::Save {
                all: false,
                force: true,
                quit: true,
            },
        ),
        "waq" => no_arg(
            head,
            rest,
            Command::Save {
                all: true,
                force: false,
                quit: true,
            },
        ),
        "waq!" => no_arg(
            head,
            rest,
            Command::Save {
                all: true,
                force: true,
                quit: true,
            },
        ),
        "q" => no_arg(head, rest, Command::Quit { force: false }),
        "q!" => no_arg(head, rest, Command::Quit { force: true }),
        "r" | "a" => {
            let mode = if head == "r" {
                ShellMode::Read
            } else {
                ShellMode::AppendLines
            };
            match shell_arg(rest) {
                Some(command) => Ok(Command::Shell { mode, command }),
                None => Err(format!("{head} needs !<command>")),
            }
        }
        _ => Err(format!("unknown command: {head}")),
    }
}

fn split_head(line: &str) -> (&str, &str) {
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    (&line[..end], line[end..].trim())
}

fn no_arg(head: &str, rest: &str, command: Command) -> Result<Command, String> {
    if rest.is_empty() {
        return Ok(command);
    }
    Err(format!("{head} takes no argument"))
}

/// shell_arg parses the `!<command>` part of `r`/`a`/`w`.
fn shell_arg(rest: &str) -> Option<String> {
    let command = rest.trim().strip_prefix('!')?.trim();
    (!command.is_empty()).then(|| command.to_string())
}

/// One completion result: the candidate replaces `start..cursor` in the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// byte offset of the start of the word being completed
    pub start: usize,
    /// the word text before the cursor (shown to the user, used to hide a
    /// shared prefix in the candidate window)
    pub prefix: String,
    pub candidates: Vec<String>,
}

/// Completion for the `:` line at `cursor`. Only the text before the cursor
/// matters; text after it is left untouched by the caller.
pub fn completion_candidates(line: &str, cursor: usize, file_path: &str) -> Completion {
    completion_candidates_in(line, cursor, file_path, &path_dirs())
}

fn completion_candidates_in(
    line: &str,
    cursor: usize,
    file_path: &str,
    path_dirs: &[PathBuf],
) -> Completion {
    let cursor = line.floor_char_boundary(cursor);
    let prefix = &line[..cursor];
    let token_start = word_start(prefix);
    if let Some(region) = shell_region(prefix).filter(|&r| cursor >= r) {
        let start = token_start.max(region);
        let token = &prefix[start..];
        let command_word = prefix[region..start].trim().is_empty();
        let candidates = if token == "%" {
            if file_path.is_empty() {
                Vec::new()
            } else {
                vec![shell_quote(&absolute_path(file_path))]
            }
        } else if command_word {
            command_candidates(path_dirs, token)
        } else {
            path_candidates(token)
        };
        return Completion {
            start,
            prefix: token.to_string(),
            candidates,
        };
    }
    builtin_completion(prefix, token_start)
}

fn builtin_completion(prefix: &str, token_start: usize) -> Completion {
    let lead = prefix.len() - prefix.trim_start().len();
    let token = &prefix[token_start..];
    // Typing the command word itself: filter the builtin list. The typed
    // token itself is already on the line and offers nothing to complete.
    if token_start <= lead {
        return Completion {
            start: token_start,
            prefix: token.to_string(),
            candidates: BUILTINS
                .iter()
                .filter(|c| c.starts_with(token) && **c != token)
                .map(|c| c.to_string())
                .collect(),
        };
    }
    let first_word = prefix[lead..token_start].trim_end();
    let candidates = match first_word {
        // `r !cmd` / `a !cmd` need the shell marker; offer it.
        "r" | "a" if token.is_empty() => vec!["!".to_string()],
        // `w <path>` is the write-to form; complete filesystem paths.
        "w" if !token.starts_with('!') => path_candidates(token),
        _ => Vec::new(),
    };
    Completion {
        start: token_start,
        prefix: token.to_string(),
        candidates,
    }
}

/// shell_region returns the byte offset just past the `!` that switches the
/// rest of the line to shell completion.
fn shell_region(prefix: &str) -> Option<usize> {
    let lead = prefix.len() - prefix.trim_start().len();
    let rest = &prefix[lead..];
    if rest.starts_with('!') {
        return Some(lead + 1);
    }
    let word_end = lead + rest.find(char::is_whitespace).unwrap_or(rest.len());
    let word = &prefix[lead..word_end];
    if !matches!(word, "r" | "a" | "w") {
        return None;
    }
    let after = &prefix[word_end..];
    let rel = after.find('!')?;
    // only the first argument switches to shell mode (`w foo!bar` does not)
    if after[..rel].trim().is_empty() {
        Some(word_end + rel + 1)
    } else {
        None
    }
}

/// word_start is the byte offset of the word under the cursor.
fn word_start(prefix: &str) -> usize {
    prefix
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0)
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

fn command_candidates(path_dirs: &[PathBuf], prefix: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for dir in path_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // the typed command itself offers nothing to complete
            if name == prefix || !name.starts_with(prefix) || !is_executable_file(&e.path()) {
                continue;
            }
            out.insert(name);
        }
    }
    out.into_iter().collect()
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn path_candidates(token: &str) -> Vec<String> {
    let sep = token.rfind(['/', '\\']);
    let (dir_part, prefix) = match sep {
        Some(i) => (&token[..=i], &token[i + 1..]),
        None => ("", token),
    };
    let read = if dir_part.is_empty() { "." } else { dir_part };
    let Ok(entries) = std::fs::read_dir(read) else {
        return Vec::new();
    };
    let mut out = BTreeSet::new();
    let lower = prefix.to_lowercase();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && !prefix.starts_with('.') {
            continue;
        }
        // Case-insensitive like the write-path completion: typing `al`
        // should offer `Alpha.txt` too. The entry spelled exactly like the
        // typed prefix is already on the line; a directory still differs by
        // its trailing `/`.
        if name.to_lowercase() == lower {
            continue;
        }
        if !name.to_lowercase().starts_with(&lower) {
            continue;
        }
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false)
            || std::fs::metadata(e.path())
                .map(|m| m.is_dir())
                .unwrap_or(false);
        let name = if is_dir { format!("{name}/") } else { name };
        out.insert(format!("{dir_part}{name}"));
    }
    out.into_iter().collect()
}

/// file_arg is the absolute spelling of the session file, used by `%`; a
/// path that cannot be made absolute is returned unchanged.
pub fn absolute_path(path: &str) -> String {
    std::path::absolute(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

/// shell_quote quotes one path for the platform shell.
pub fn shell_quote(path: &str) -> String {
    #[cfg(windows)]
    {
        format!("\"{}\"", path.replace('"', "\\\""))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// expand_percent replaces each standalone `%` word in a shell command with
/// the quoted absolute session path. Whitespace is preserved verbatim, so
/// quoting inside the command is never rewritten.
pub fn expand_percent(command: &str, file_path: &str) -> String {
    if file_path.is_empty() {
        return command.to_string();
    }
    let quoted = shell_quote(&absolute_path(file_path));
    let mut out = String::with_capacity(command.len() + quoted.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word == "%" {
            out.push_str(&quoted);
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in command.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_save_quit_and_shell_forms() {
        assert_eq!(
            parse_command("w").unwrap(),
            Command::Save {
                all: false,
                force: false,
                quit: false
            }
        );
        assert_eq!(
            parse_command("wa").unwrap(),
            Command::Save {
                all: true,
                force: false,
                quit: false
            }
        );
        assert_eq!(
            parse_command("wq").unwrap(),
            Command::Save {
                all: false,
                force: false,
                quit: true
            }
        );
        assert_eq!(
            parse_command("waq!").unwrap(),
            Command::Save {
                all: true,
                force: true,
                quit: true
            }
        );
        assert_eq!(parse_command("q!").unwrap(), Command::Quit { force: true });
        assert_eq!(
            parse_command("w /tmp/out.json").unwrap(),
            Command::WriteTo {
                path: "/tmp/out.json".into()
            }
        );
        assert_eq!(
            parse_command("!printf hi").unwrap(),
            Command::Shell {
                mode: ShellMode::Execute,
                command: "printf hi".into()
            }
        );
        assert_eq!(
            parse_command("r !cat").unwrap(),
            Command::Shell {
                mode: ShellMode::Read,
                command: "cat".into()
            }
        );
        assert_eq!(
            parse_command("a !printf 'x\\ny\\n'").unwrap(),
            Command::Shell {
                mode: ShellMode::AppendLines,
                command: "printf 'x\\ny\\n'".into()
            }
        );
        assert_eq!(
            parse_command("w !cat").unwrap(),
            Command::Shell {
                mode: ShellMode::Write,
                command: "cat".into()
            }
        );
        assert_eq!(parse_command("!!").unwrap(), Command::RepeatShell);
        assert!(parse_command("").is_err());
        assert!(parse_command("!").is_err());
        assert!(parse_command("w !").is_err());
        assert!(parse_command("nope").is_err());
        assert!(parse_command("q now").is_err());
        assert!(parse_command("r cat").is_err(), "shell needs the ! marker");
    }

    #[test]
    fn completes_builtins_then_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
        std::fs::write(dir.path().join("beta"), "").unwrap();
        let base = dir.path().display().to_string();

        let c = completion_candidates("w", 1, "");
        assert_eq!(c.start, 0);
        assert_eq!(c.prefix, "w");
        assert!(c.candidates.contains(&"wa".to_string()));
        assert!(c.candidates.contains(&"wq!".to_string()));
        // the typed token itself is never offered
        assert!(
            !c.candidates.contains(&"w".to_string()),
            "{:?}",
            c.candidates
        );

        let line = format!("w {base}/al");
        let c = completion_candidates(&line, line.len(), "");
        assert_eq!(c.start, 2);
        assert_eq!(c.candidates, vec![format!("{base}/alpha.txt")]);

        // a fully typed file path has no differing tail to offer
        let line = format!("w {base}/alpha.txt");
        let c = completion_candidates(&line, line.len(), "");
        assert!(
            c.candidates.is_empty(),
            "typed path offered: {:?}",
            c.candidates
        );

        // empty argument after r/a offers the shell marker
        let c = completion_candidates("r ", 2, "");
        assert_eq!(c.candidates, vec!["!"]);
        assert_eq!(c.start, 2);

        // text after the cursor does not affect the candidates
        let line = format!("w {base}/al tail");
        let cursor = line.find(" tail").unwrap();
        let c = completion_candidates(&line, cursor, "");
        assert_eq!(c.start, 2);
        assert_eq!(c.prefix, format!("{base}/al"));
    }

    #[cfg(unix)]
    #[test]
    fn completes_shell_commands_from_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("frobnicate");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.path().join("frobnicate.txt"), "").unwrap();

        // `:` shell escape, and the `r !` / `w !` forms
        let dirs = [dir.path().to_path_buf()];
        for line in ["!frob", "r !frob", "w !frob"] {
            let c = completion_candidates_in(line, line.len(), "", &dirs);
            let start = line.find("frob").unwrap();
            assert_eq!(c.start, start, "{line}");
            assert_eq!(c.prefix, "frob", "{line}");
            assert_eq!(c.candidates, vec!["frobnicate"], "{line}");
        }

        let c = completion_candidates_in("!frob", 3, "", &dirs);
        assert_eq!(
            c.start, 1,
            "editing the leading ! keeps the command token start"
        );
        assert_eq!(c.prefix, "fr");
        assert_eq!(c.candidates, vec!["frobnicate"]);

        // the typed command itself is never offered
        let line = "!frobnicate";
        let c = completion_candidates_in(line, line.len(), "", &dirs);
        assert!(
            c.candidates.is_empty(),
            "typed command offered: {:?}",
            c.candidates
        );
    }

    #[test]
    fn completes_percent_to_the_quoted_absolute_path() {
        let c = completion_candidates("!cat %", 6, "rel/config.json");
        assert_eq!(c.start, 5);
        assert_eq!(c.prefix, "%");
        let want = shell_quote(&absolute_path("rel/config.json"));
        assert_eq!(c.candidates, vec![want]);
        let c = completion_candidates("!cat %", 6, "");
        assert!(c.candidates.is_empty(), "no session file, no expansion");
    }

    #[test]
    fn expands_percent_words_and_keeps_quoting() {
        let quoted = shell_quote(&absolute_path("a b.json"));
        let got = expand_percent("cat % | head 'a  b'", "a b.json");
        assert_eq!(got, format!("cat {quoted} | head 'a  b'"));
        assert_eq!(expand_percent("printf %s x", "a.json"), "printf %s x");
        assert_eq!(expand_percent("echo '%'", "a.json"), "echo '%'");
        assert_eq!(expand_percent("cat %", ""), "cat %");
    }

    #[test]
    fn completion_token_handles_multibyte_whitespace() {
        for space in ['\u{00A0}', '\u{3000}'] {
            let line = format!("!cat{space}/tmp/x");
            let c = completion_candidates(&line, line.len(), "");
            assert_eq!(c.start, line.find("/tmp/x").unwrap());
            assert_eq!(c.prefix, "/tmp/x");
        }
    }
}
