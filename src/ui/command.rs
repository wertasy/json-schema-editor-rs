//! vi-style `:` command line: builtin commands plus `!` shell escapes.
//!
//! The command line is a normal one-line [`TextInput`](crate::ui::widgets::TextInput),
//! so completion works at the cursor, not on the whole buffer. `!` switches
//! to shell completion (executables from `PATH`, then filesystem paths);
//! without it the builtin command names complete. A shell argument word
//! consults bash's programmable completion once — automatically when the
//! word begins (usually right after a space) and again on an explicit tab
//! press — and keeps the local scan when bash-completion has no rule for
//! the command; further keystrokes inside the word filter the fetched
//! words. In a shell command `%` stands for the session file and tab
//! expands it to the absolute path.

use std::collections::{BTreeSet, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::doc::{Doc, NodeId, PathSeg, Value};
use crate::smodel::{EnumOpt, FormNodeId, Kind, Schema};
use crate::ui::util::display_width;
use crate::ui::widgets::TextInput;
// the module owns `Command` (the parsed `:` line), so the process type
// needs an alias
use std::process::{Command as Process, Stdio};

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

/// CommandState is the `:` command input: one line at the bottom, with
/// up/down walking the executed command history. Completion candidates
/// replace the word before the cursor only; `sug_start..sug_end` records the
/// text the current cycle owns so repeated tab presses do not rescan it.
#[derive(Debug)]
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
    /// bash programmable completion cache: one fetch per shell argument
    /// word, filtered locally while the caret stays inside it
    pub bash_cache: BashCache,
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
    if let Some((_, start, token, command_word)) = shell_token(prefix) {
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

/// shell_token describes the word before the cursor when it sits in the
/// shell region: the byte offset just past the `!`, the word start, the
/// word itself and whether it is the shell command word (empty text after
/// the `!`).
fn shell_token(prefix: &str) -> Option<(usize, usize, &str, bool)> {
    let region = shell_region(prefix)?;
    let start = word_start(prefix).max(region);
    let token = &prefix[start..];
    let command_word = prefix[region..start].trim().is_empty();
    Some((region, start, token, command_word))
}

/// BashCache remembers the argument word the auto window last fetched
/// programmable completion for: one bash run per argument word, then local
/// prefix filtering while the caret stays inside it. The explicit tab
/// press always re-asks the scripts for the word's full context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BashCache {
    /// byte offset of the fetched word's start (0 = nothing fetched; a
    /// shell argument word always starts at or after the `!`)
    word_start: usize,
    /// the line up to that offset, so an edited command invalidates the
    /// cache (history recall, deletion, a moved `!` region)
    prefix: String,
    /// the word text at fetch time
    token: String,
    /// COMPREPLY words from that fetch
    candidates: Vec<String>,
}

impl BashCache {
    /// auto_candidates feeds the auto-filtering window: a fresh shell
    /// argument word runs bash once, further keystrokes inside the word
    /// filter the fetched words. A word without a rule keeps rescanning
    /// the filesystem locally without re-running bash.
    pub fn auto_candidates(&mut self, line: &str, cursor: usize, file_path: &str) -> Completion {
        self.auto_candidates_env(line, cursor, file_path, &[])
    }

    /// auto_candidates_env is [`auto_candidates`] with extra environment
    /// entries for the driver; tests point JSE_BASH_COMPLETION at a fake
    /// core script.
    fn auto_candidates_env(
        &mut self,
        line: &str,
        cursor: usize,
        file_path: &str,
        envs: &[(&str, &str)],
    ) -> Completion {
        let base = completion_candidates(line, cursor, file_path);
        let Some((region, start, token, command_word)) = shell_context(line, cursor) else {
            *self = Self::default();
            return base;
        };
        if command_word || token == "%" {
            *self = Self::default();
            return base;
        }
        let line_prefix = &line[..start];
        if self.word_start == start && self.prefix == line_prefix {
            if self.candidates.is_empty() {
                // bash had no opinion for this word: keep the local scan
                return base;
            }
            return Completion {
                start,
                prefix: token.to_string(),
                candidates: filter_prefix(&self.candidates, token),
            };
        }
        let candidates = bash_completion_env(&line[region..], cursor - region, envs);
        self.store(start, line_prefix, token, candidates.clone());
        if candidates.is_empty() {
            return base;
        }
        Completion {
            start,
            prefix: token.to_string(),
            candidates,
        }
    }

    /// tab_candidates is the completion behind an explicit tab press: it
    /// re-asks bash for the word's full context (an identical fetch is
    /// reused) and replaces the filesystem candidates when the command has
    /// a completion rule; the command word itself, `%`, builtin words and
    /// everything outside a shell region never consult bash.
    pub fn tab_candidates(&mut self, line: &str, cursor: usize, file_path: &str) -> Completion {
        self.tab_candidates_env(line, cursor, file_path, &[])
    }

    /// tab_candidates_env is [`tab_candidates`] with extra environment
    /// entries for the driver; tests point JSE_BASH_COMPLETION at a fake
    /// core script.
    fn tab_candidates_env(
        &mut self,
        line: &str,
        cursor: usize,
        file_path: &str,
        envs: &[(&str, &str)],
    ) -> Completion {
        let base = completion_candidates(line, cursor, file_path);
        let Some((region, start, token, command_word)) = shell_context(line, cursor) else {
            return base;
        };
        if command_word || token == "%" {
            return base;
        }
        if self.word_start == start
            && self.prefix == line[..start]
            && self.token == token
            && !self.candidates.is_empty()
        {
            return Completion {
                start,
                prefix: token.to_string(),
                candidates: self.candidates.clone(),
            };
        }
        let candidates = bash_completion_env(&line[region..], cursor - region, envs);
        self.store(start, &line[..start], token, candidates.clone());
        if candidates.is_empty() {
            return base;
        }
        Completion {
            start,
            prefix: token.to_string(),
            candidates,
        }
    }

    /// store remembers one fetch for the word at `start`.
    fn store(&mut self, start: usize, prefix: &str, token: &str, candidates: Vec<String>) {
        *self = Self {
            word_start: start,
            prefix: prefix.to_string(),
            token: token.to_string(),
            candidates,
        };
    }
}

/// shell_context is [`shell_token`] on the text before the cursor, with
/// the caret clamped to a char boundary.
fn shell_context(line: &str, cursor: usize) -> Option<(usize, usize, &str, bool)> {
    let cursor = line.floor_char_boundary(cursor);
    shell_token(&line[..cursor])
}

/// filter_prefix keeps the cached words starting with the typed prefix,
/// case-insensitive like the window's other candidate filters.
fn filter_prefix(candidates: &[String], prefix: &str) -> Vec<String> {
    let lower = prefix.to_lowercase();
    candidates
        .iter()
        .filter(|c| c.to_lowercase().starts_with(&lower))
        .cloned()
        .collect()
}

/// bash_completion_env is [`bash_completion`] with extra environment
/// entries for the driver; tests use it to point JSE_BASH_COMPLETION at a
/// fake core script and JSE_FAKE_DIR at fake completion files.
fn bash_completion_env(line: &str, point: usize, extra_envs: &[(&str, &str)]) -> Vec<String> {
    if !cfg!(unix) {
        return Vec::new();
    }
    let (words, cword) = comp_words(line, point);
    let Some(cmd) = words.first() else {
        return Vec::new();
    };
    let Ok(out) = Process::new("bash")
        .arg("-c")
        .arg(BASH_COMPLETION_DRIVER)
        .arg("jse")
        .arg(cmd)
        .arg(cword.to_string())
        .args(&words)
        .env("COMP_LINE", line)
        .env("COMP_POINT", point.to_string())
        .env("COMP_KEY", "9")
        .env("COMP_TYPE", "9")
        .envs(extra_envs.iter().copied())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for word in String::from_utf8_lossy(&out.stdout).lines() {
        let word = word.trim_end_matches('\r');
        if !word.is_empty() && seen.insert(word.to_string()) {
            candidates.push(word.to_string());
        }
    }
    candidates
}

/// comp_words splits the shell line the way readline builds COMP_WORDS
/// (unquoted ASCII whitespace, quotes kept) and picks COMP_CWORD: the word
/// under the caret, or one past the last word for the empty word in a
/// trailing whitespace run.
fn comp_words(line: &str, point: usize) -> (Vec<String>, usize) {
    let bytes = line.as_bytes();
    let mut words: Vec<(usize, &str)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i > start {
            words.push((start, &line[start..i]));
        }
    }
    let cword = words
        .iter()
        .position(|(s, w)| *s <= point && point <= s + w.len())
        .unwrap_or(words.len());
    (
        words.into_iter().map(|(_, w)| w.to_string()).collect(),
        cword,
    )
}

/// BASH_COMPLETION_DRIVER is the bash program behind [`bash_completion`]:
/// it sources the bash-completion machinery (JSE_BASH_COMPLETION overrides
/// the core script), loads the command's completion, finds the registered
/// rule and runs it with COMP_* arranged like a real tab press. Exit 3
/// means no bash-completion machinery, exit 4 no rule for the command.
const BASH_COMPLETION_DRIVER: &str = r#"core=""
for f in "$JSE_BASH_COMPLETION" \
  /usr/share/bash-completion/bash_completion \
  /etc/bash_completion \
  /usr/local/etc/bash_completion \
  /opt/homebrew/share/bash-completion/bash_completion \
  /opt/homebrew/etc/bash_completion; do
  [ -n "$f" ] && [ -f "$f" ] && . "$f" 2>/dev/null && { core=1; break; }
done
[ -n "$core" ] || exit 3
cmd=$1 cword=$2
shift 2
COMP_WORDS=("$@")
COMP_CWORD=$cword
COMP_TYPE=9
COMP_KEY=9
export COMP_LINE COMP_POINT COMP_KEY COMP_TYPE
_completion_loader "$cmd" 2>/dev/null
spec=$(complete -p "$cmd" 2>/dev/null)
[ -n "$spec" ] || exit 4
set -f
# complete -p re-quotes multi-word values ("complete -W 'a b' x"); word
# splitting needs the shell parser, not bare splitting
eval "set -- $spec"
shift
[ $# -gt 0 ] && set -- "${@:1:$#-1}"
args=("$@")
mode=""
fn=""
i=0
while [ $i -lt ${#args[@]} ]; do
  case ${args[$i]} in
  -F | -C) mode=${args[$i]}; fn=${args[$((i + 1))]:-}; break ;;
  -o | -A | -W | -G | -X | -P | -S) i=$((i + 2)) ;;
  *) i=$((i + 1)) ;;
  esac
done
COMPREPLY=()
case $mode in
-F)
  # bash's documented contract: $1 command name, $2 word being completed,
  # $3 previous word (jse never completes the command word, so COMP_CWORD
  # is at least 1 here)
  "$fn" "$cmd" "${COMP_WORDS[$COMP_CWORD]:-}" "${COMP_WORDS[$((COMP_CWORD - 1))]:-}"
  ;;
-C)
  while IFS= read -r w; do
    [ -n "$w" ] && COMPREPLY+=("$w")
  done <<< "$(eval "$fn" "$cmd" "${COMP_WORDS[$COMP_CWORD]:-}" \
    "${COMP_WORDS[$((COMP_CWORD - 1))]:-}" 2>/dev/null)"
  ;;
*)
  [ ${#args[@]} -gt 0 ] &&
    COMPREPLY=($(compgen "${args[@]}" -- "${COMP_WORDS[$COMP_CWORD]:-}" 2>/dev/null))
  ;;
esac
set +f
printf '%s\n' "${COMPREPLY[@]}"
"#;

fn builtin_completion(prefix: &str, token_start: usize) -> Completion {
    let lead = prefix.len() - prefix.trim_start().len();
    let token = &prefix[token_start..];
    // Typing the command word itself: filter the builtin list.
    if token_start <= lead {
        return Completion {
            start: token_start,
            prefix: token.to_string(),
            candidates: BUILTINS
                .iter()
                .filter(|c| c.starts_with(token))
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
            if !name.starts_with(prefix) || !is_executable_file(&e.path()) {
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
        // should offer `Alpha.txt` too.
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

// --- shell escapes: r !, a !, w !, ! (runs what the grammar parses) ---
// vi-style shell escapes behind the `:` command line. `r !<cmd>` runs a
// command and writes its stdout into the value under the menu cursor,
// `a !<cmd>` splits stdout into lines and appends each as an item of a

/// How a shell escape uses the command. `r !<cmd>` inserts the command's
/// stdout at the cursor value, `a !<cmd>` appends its stdout lines as array
/// items, `w !<cmd>` streams the document to the command's stdin, and
/// `!<cmd>` executes without touching the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellMode {
    Execute,
    Read,
    Write,
    AppendLines,
}

/// compact_output turns command output into one notice line (the bottom bar
/// is single-line); empty output has no notice.
pub fn compact_output(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// How the command output lands in the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellTargetKind {
    /// replace the value at the target path
    String,
    /// append the output as a new item of the array at the target path
    StringArray,
}

/// The value slot a command's output is written to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellTarget {
    pub path: Vec<PathSeg>,
    pub kind: ShellTargetKind,
}

impl ShellTarget {
    pub fn new(path: Vec<PathSeg>, kind: ShellTargetKind) -> ShellTarget {
        ShellTarget { path, kind }
    }
}

/// shell_command builds the platform shell invocation for one command line.
fn shell_command(command: &str) -> Process {
    #[cfg(windows)]
    {
        let mut c = Process::new("cmd");
        c.args(["/C", command]);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Process::new("sh");
        c.args(["-c", command]);
        c
    }
}

/// status_error renders a failed command as the first non-empty stderr line
/// plus its exit status.
fn status_error(out: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let msg = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("command failed");
    match out.status.code() {
        Some(code) => format!("{msg} (exit {code})"),
        None => msg.to_string(),
    }
}

/// run_capture runs `command` through the platform shell and returns stdout
/// on success.
pub fn run_capture(command: &str) -> Result<String, String> {
    // The TUI owns the terminal: never let the child read the key stream.
    let out = shell_command(command)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run shell: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    Err(status_error(&out))
}

/// run_with_input runs `command` with `input` on stdin (vi `:w !cmd`) and
/// returns its stdout on success. The writer runs on a thread so a command
/// that emits output before reading all input cannot fill a pipe and deadlock.
pub fn run_with_input(command: &str, input: &str) -> Result<String, String> {
    let mut child = shell_command(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run shell: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "cannot open command stdin".to_string())?;
    let data = input.as_bytes().to_vec();
    let writer = std::thread::spawn(move || {
        // A command may close stdin early; that is not a command failure.
        let _ = stdin.write_all(&data);
    });
    let out = match child.wait_with_output() {
        Ok(out) => out,
        Err(e) => {
            let _ = writer.join();
            return Err(format!("cannot wait for shell: {e}"));
        }
    };
    let _ = writer.join();
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    Err(status_error(&out))
}

/// output_text normalizes captured stdout for insertion: trailing newlines
/// (the shell's line terminator) are dropped; everything else is preserved.
pub fn output_text(stdout: &str) -> String {
    stdout.trim_end_matches(['\r', '\n']).to_string()
}

/// output_lines splits stdout into array items: one item per line, CRLF
/// normalized, and no phantom item after a trailing line terminator.
pub fn output_lines(stdout: &str) -> Vec<String> {
    stdout.lines().map(str::to_string).collect()
}

/// node_target classifies one schema node. Arrays count only when their item
/// node resolves to a single string; nested arrays do not. A oneOf prefers a
/// string variant over an array-of-strings variant and ignores unrelated
/// variants.
pub fn node_target(schema: &Schema, node: FormNodeId) -> Option<ShellTargetKind> {
    fn walk(
        schema: &Schema,
        node: FormNodeId,
        seen: &mut HashSet<FormNodeId>,
    ) -> Option<ShellTargetKind> {
        if !seen.insert(node) {
            return None;
        }
        let n = &schema.nodes[node as usize];
        let out = match n.kind {
            Kind::Scalar if n.json_type == "string" => Some(ShellTargetKind::String),
            Kind::Array => n
                .items
                .and_then(|items| walk(schema, items, seen))
                .filter(|k| *k == ShellTargetKind::String)
                .map(|_| ShellTargetKind::StringArray),
            Kind::OneOf => {
                let mut string = false;
                let mut array = false;
                for &v in &n.variants {
                    match walk(schema, v, seen) {
                        Some(ShellTargetKind::String) => string = true,
                        Some(ShellTargetKind::StringArray) => array = true,
                        None => {}
                    }
                }
                if string {
                    Some(ShellTargetKind::String)
                } else if array {
                    Some(ShellTargetKind::StringArray)
                } else {
                    None
                }
            }
            _ => None,
        };
        seen.remove(&node);
        out
    }
    let mut seen = HashSet::new();
    walk(schema, node, &mut seen)
}

/// runtime_target classifies a document value by its runtime shape: a string,
/// or an array whose items are all strings (an empty array counts).
pub fn runtime_target(doc: &Doc, value: NodeId) -> Option<ShellTargetKind> {
    match doc.value(value) {
        Value::Str(_) => Some(ShellTargetKind::String),
        Value::Array(items) if items.iter().all(|i| matches!(doc.value(*i), Value::Str(_))) => {
            Some(ShellTargetKind::StringArray)
        }
        _ => None,
    }
}

/// resolve_target picks the insertion target for one menu row. Schema
/// classification wins; nodes the schema leaves untyped (raw, schema-less,
/// or an item without a type) fall back to the value's runtime shape.
pub fn resolve_target(
    schema: &Schema,
    doc: &Doc,
    node: Option<FormNodeId>,
    path: &[PathSeg],
) -> Option<ShellTarget> {
    let kind = match node {
        Some(n) => {
            let f = &schema.nodes[n as usize];
            match node_target(schema, n) {
                Some(k) => Some(k),
                None if f.kind == Kind::Raw => doc.get(path).and_then(|v| runtime_target(doc, v)),
                None if f.kind == Kind::Scalar && f.json_type.is_empty() => {
                    doc.get(path).and_then(|v| runtime_target(doc, v))
                }
                None if f.kind == Kind::Array
                    && f.items
                        .is_none_or(|i| schema.nodes[i as usize].kind == Kind::Raw) =>
                {
                    doc.get(path).and_then(|v| runtime_target(doc, v))
                }
                None => None,
            }
        }
        None => doc.get(path).and_then(|v| runtime_target(doc, v)),
    }?;
    Some(ShellTarget::new(path.to_vec(), kind))
}

/// node_is_string_array reports whether a schema node describes an array of
/// strings. A oneOf accepts the mode when any variant is such an array
/// (the `a` mode deliberately does not apply the string-first preference of
/// `r`, since the user asked for line items).
pub fn node_is_string_array(schema: &Schema, node: FormNodeId) -> bool {
    fn walk(schema: &Schema, node: FormNodeId, seen: &mut HashSet<FormNodeId>) -> bool {
        if !seen.insert(node) {
            return false;
        }
        let n = &schema.nodes[node as usize];
        let out = match n.kind {
            Kind::Array => n
                .items
                .map(|items| node_target(schema, items) == Some(ShellTargetKind::String))
                .unwrap_or(false),
            Kind::OneOf => n.variants.iter().any(|&v| walk(schema, v, seen)),
            _ => false,
        };
        seen.remove(&node);
        out
    }
    let mut seen = HashSet::new();
    walk(schema, node, &mut seen)
}

/// resolve_array_target resolves an array-of-strings slot for the `a` mode.
/// Schema types win; raw/schema-less values fall back to the runtime shape,
/// which must already be an array of strings.
pub fn resolve_array_target(
    schema: &Schema,
    doc: &Doc,
    node: Option<FormNodeId>,
    path: &[PathSeg],
) -> Option<ShellTarget> {
    let array = match node {
        Some(n) => {
            let f = &schema.nodes[n as usize];
            node_is_string_array(schema, n)
                || ((f.kind == Kind::Raw
                    || (f.kind == Kind::Array
                        && f.items
                            .is_none_or(|i| schema.nodes[i as usize].kind == Kind::Raw)))
                    && doc
                        .get(path)
                        .and_then(|v| runtime_target(doc, v))
                        .is_some_and(|k| k == ShellTargetKind::StringArray))
        }
        None => doc
            .get(path)
            .and_then(|v| runtime_target(doc, v))
            .is_some_and(|k| k == ShellTargetKind::StringArray),
    };
    array.then(|| ShellTarget::new(path.to_vec(), ShellTargetKind::StringArray))
}

/// apply writes the command output at the target: replacement for a string,
/// append for an array of strings.
pub fn apply(doc: &mut Doc, target: &ShellTarget, output: &str) -> Result<(), String> {
    let node = doc.new_str(output);
    match target.kind {
        ShellTargetKind::String => doc
            .set(&target.path, node)
            .map_err(|e| format!("cannot write value: {}", e.msg)),
        ShellTargetKind::StringArray => doc
            .append(&target.path, node)
            .map_err(|e| format!("cannot append item: {}", e.msg)),
    }
}

/// apply_lines appends one string per output line to a string array. A
/// non-array value is rejected before any line is written, so the document
/// never ends up partially modified.
pub fn apply_lines(doc: &mut Doc, target: &ShellTarget, lines: &[String]) -> Result<(), String> {
    match doc.get(&target.path) {
        Some(n) if !matches!(doc.value(n), Value::Array(_)) && !doc.empty(n) => {
            return Err("target is not an array".to_string());
        }
        _ => {}
    }
    for line in lines {
        let node = doc.new_str(line.clone());
        doc.append(&target.path, node)
            .map_err(|e| format!("cannot append item: {}", e.msg))?;
    }
    Ok(())
}

// --- completion ----------------------------------------------------------------

/// cmd_sug_refresh rebuilds the auto-completion list for the word before
/// the cursor without choosing a candidate; tab/arrows select from it.
/// The scan is cheap except at a word start: a fresh shell argument word
/// runs bash's programmable completion once (BashCache), later keystrokes
/// filter the fetched words locally.
pub(crate) fn cmd_sug_refresh(s: &mut CommandState, file_path: &str) {
    let line = s.ti.value().to_string();
    let cursor = s.ti.cursor_byte();
    let done = s.bash_cache.auto_candidates(&line, cursor, file_path);
    cmd_sug_apply(s, done, cursor);
}

/// cmd_tab_refresh is the fresh scan of an explicit tab press: it always
/// re-asks bash's programmable completion for the word's full context
/// (shell argument words only) and stores the fetch for later filtering.
pub(crate) fn cmd_tab_refresh(s: &mut CommandState, file_path: &str) {
    let line = s.ti.value().to_string();
    let cursor = s.ti.cursor_byte();
    let done = s.bash_cache.tab_candidates(&line, cursor, file_path);
    cmd_sug_apply(s, done, cursor);
}

/// cmd_sug_apply stores one scan result as the active completion list.
fn cmd_sug_apply(s: &mut CommandState, done: Completion, cursor: usize) {
    s.sug_prefix = done.prefix;
    s.sug = done.candidates;
    s.sug_idx = None;
    s.sug_line.clear();
    s.sug_start = done.start;
    s.sug_end = cursor;
    // The candidate window aligns with the word being completed: deriving
    // the anchor from the token start keeps the window still while the user
    // types inside one word; a separator that starts a new word re-anchors.
    s.sug_col = display_width(&s.ti.value()[..s.sug_start]);
}

/// cmd_complete applies the next/previous completion. A fresh cycle scans the
/// word before the cursor (builtin command, PATH command, bash completion or
/// filesystem path) and inserts the candidate there, leaving the text after
/// the cursor alone; repeated tab presses walk the same candidate list.
pub(crate) fn cmd_complete(s: &mut CommandState, forward: bool, file_path: &str) {
    let value = s.ti.value().to_string();
    let inserted = &value[s.sug_start.min(value.len())..s.sug_end.min(value.len())];
    // Entering a directory or the shell marker needs a fresh scan, not a
    // cycle over the single applied candidate.
    if s.sug_idx.is_none()
        || value != s.sug_line
        || inserted.ends_with(['/', '\\'])
        || inserted == "!"
    {
        cmd_tab_refresh(s, file_path);
        if s.sug.is_empty() {
            return;
        }
        let idx = if forward { 0 } else { s.sug.len() - 1 };
        cmd_apply_sug(s, idx);
        return;
    }
    cmd_complete_step(s, forward);
}

/// cmd_completion_active reports whether the input still holds a candidate
/// from the current completion list (so arrows should move within it).
pub(crate) fn cmd_completion_active(s: &CommandState) -> bool {
    s.sug_idx.is_some() && s.ti.value() == s.sug_line && s.ti.cursor_byte() == s.sug_end
}

/// cmd_complete_step moves the candidate cursor without rescanning the
/// token; tab and, while active, the arrow keys use this.
pub(crate) fn cmd_complete_step(s: &mut CommandState, forward: bool) {
    let Some(i) = s.sug_idx else { return };
    let n = s.sug.len();
    if n == 0 {
        return;
    }
    let idx = if forward {
        (i + 1) % n
    } else {
        (i + n - 1) % n
    };
    cmd_apply_sug(s, idx);
}

fn cmd_apply_sug(s: &mut CommandState, idx: usize) {
    let candidate = s.sug[idx].clone();
    s.ti.replace_range(s.sug_start, s.sug_end, &candidate);
    s.sug_end = s.sug_start + candidate.len();
    s.sug_line = s.ti.value().to_string();
    s.sug_idx = Some(idx);
    s.hist_idx = None;
}

/// completionCandidate reports whether v can be offered as a completion:
/// empty text never is, and multiline values are skipped.
pub(crate) fn completion_candidate(v: &str) -> bool {
    !v.is_empty() && !v.contains('\r') && !v.contains('\n')
}

/// fuzzy_match matches query against text case-insensitively as a
/// subsequence and returns the matched char indices plus a quality score
/// (lower is better): 0 exact, 1 prefix, 2 substring, 3 subsequence. The
/// indices drive the red matched-text highlight in the dropdown.
pub(crate) fn fuzzy_match(query: &str, text: &str) -> Option<(u8, Vec<usize>)> {
    if query.is_empty() {
        return Some((4, Vec::new()));
    }
    let lq: Vec<char> = query.to_lowercase().chars().collect();
    let lt: Vec<char> = text.to_lowercase().chars().collect();
    if lq.len() > lt.len() {
        return None;
    }
    if lt == lq {
        return Some((0, (0..lt.len()).collect()));
    }
    if lt.starts_with(&lq) {
        return Some((1, (0..lq.len()).collect()));
    }
    if let Some(pos) = lt.windows(lq.len()).position(|w| w == lq.as_slice()) {
        return Some((2, (pos..pos + lq.len()).collect()));
    }
    // subsequence: greedy left-to-right keeps the earliest occurrence of
    // every query char, so the highlight stays where a reader expects it
    let mut q = 0usize;
    let mut hits = Vec::with_capacity(lq.len());
    for (i, c) in lt.iter().enumerate() {
        if q < lq.len() && *c == lq[q] {
            hits.push(i);
            q += 1;
        }
    }
    if q == lq.len() { Some((3, hits)) } else { None }
}

/// fuzzy_filter keeps candidates that fuzzy_match the query, best match
/// first (score, then the candidate's original order). An empty query keeps
/// everything unhighlighted and unsorted.
pub(crate) fn fuzzy_filter(
    cands: impl IntoIterator<Item = String>,
    query: &str,
    usable: impl Fn(&str) -> bool,
) -> Vec<(String, Vec<usize>)> {
    let mut matched: Vec<(u8, usize, String, Vec<usize>)> = cands
        .into_iter()
        .enumerate()
        .filter(|(_, c)| usable(c))
        .filter_map(|(i, c)| fuzzy_match(query, &c).map(|(score, hits)| (score, i, c, hits)))
        .collect();
    matched.sort_by_key(|(score, i, ..)| (*score, *i));
    matched.into_iter().map(|(.., c, hits)| (c, hits)).collect()
}

/// filterCompletions fuzzy-matches candidates against the query
/// (case-insensitive subsequence; prefix/substring rank first); an empty
/// query keeps everything. Returns each candidate with its matched char
/// indices for the red highlight.
pub(crate) fn filter_completions(cands: &[String], query: &str) -> Vec<(String, Vec<usize>)> {
    fuzzy_filter(cands.iter().cloned(), query, |c| completion_candidate(c))
}

/// filter_enum_keywords fuzzy-matches option displays against the query
/// (keyword search in the enum dropdown); an empty query keeps every
/// option. Unlike filter_completions an exact match stays listed: the
/// input text itself names a selectable option.
pub(crate) fn filter_enum_keywords(opts: &[EnumOpt], query: &str) -> Vec<(String, Vec<usize>)> {
    fuzzy_filter(opts.iter().map(|o| o.display.clone()), query, |_| true)
}

/// enumIndex returns the cursor position of the current (or default) value
/// in fn's option list.
pub(crate) fn enum_index(schema: &Schema, fn_: FormNodeId, vdoc: &Doc, v: Option<NodeId>) -> usize {
    let f = &schema.nodes[fn_ as usize];
    let mut enc = String::new();
    if let Some(v) = v {
        if !matches!(vdoc.value(v), Value::Null) {
            enc = vdoc.encode_value(v);
        }
    }
    if !enc.is_empty() {
        for (i, o) in f.enum_.iter().enumerate() {
            if o.encoded == enc {
                return i;
            }
        }
    }
    if let Some(d) = f.default_ {
        let de = schema.doc.encode_value(d);
        for (i, o) in f.enum_.iter().enumerate() {
            if o.encoded == de {
                return i;
            }
        }
    }
    0
}

/// enumChoiceError names the options a failed enum input must choose from,
/// spelled as JSON literals so empty-string members stay visible.
pub(crate) fn enum_choice_error(opts: &[EnumOpt]) -> String {
    let names: Vec<&str> = opts.iter().map(|o| o.encoded.as_str()).collect();
    format!("must be one of: {}", names.join(", "))
}

/// strip_query removes the already-typed prefix from a candidate. Matching is
/// case-insensitive, mirroring suggestion filtering; the returned slice is
/// the differing tail.
pub(crate) fn strip_query<'a>(candidate: &'a str, query: &str) -> Option<&'a str> {
    if query.is_empty() {
        return None;
    }
    let mut end = 0usize;
    let mut cand = candidate.char_indices();
    for q in query.chars() {
        let Some((idx, c)) = cand.next() else {
            return None;
        };
        if c != q && !c.to_lowercase().eq(q.to_lowercase()) {
            return None;
        }
        end = idx + c.len_utf8();
    }
    Some(&candidate[end..])
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
        // the typed command word itself is offered too
        assert!(c.candidates.contains(&"w".to_string()));

        let line = format!("w {base}/al");
        let c = completion_candidates(&line, line.len(), "");
        assert_eq!(c.start, 2);
        assert_eq!(c.candidates, vec![format!("{base}/alpha.txt")]);

        // a fully typed path is offered as its own candidate
        let line = format!("w {base}/alpha.txt");
        let c = completion_candidates(&line, line.len(), "");
        assert_eq!(c.candidates, vec![format!("{base}/alpha.txt")]);

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

        // the fully typed command itself is offered too
        let c = completion_candidates_in("!frobnicate", "!frobnicate".len(), "", &dirs);
        assert_eq!(c.candidates, vec!["frobnicate"]);
    }

    #[test]
    fn shell_words_follow_the_caret() {
        assert_eq!(
            comp_words("git che", 7),
            (vec!["git".to_string(), "che".to_string()], 1)
        );
        // caret in a trailing whitespace run: empty word past the last one
        assert_eq!(comp_words("git ", 4), (vec!["git".to_string()], 1));
        assert_eq!(comp_words("git", 0), (vec!["git".to_string()], 0));
        // collapsed whitespace runs leave the empty word past the end
        assert_eq!(
            comp_words("git  x", 4),
            (vec!["git".to_string(), "x".to_string()], 2)
        );
        assert_eq!(comp_words("", 0), (Vec::new(), 0));
    }

    #[test]
    #[cfg(unix)]
    fn bash_completion_driver_runs_registered_rules() {
        // a unix machine without bash (minimal containers) skips like the
        // pty tests do without a pty
        if std::process::Command::new("bash")
            .arg("-c")
            .arg("exit 0")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let core = dir.path().join("core.sh");
        std::fs::write(
            &core,
            r#"_completion_loader() { [ -f "$JSE_FAKE_DIR/$1" ] && . "$JSE_FAKE_DIR/$1"; }"#,
        )
        .unwrap();
        let comps = dir.path().join("comps");
        std::fs::create_dir(&comps).unwrap();
        std::fs::write(
            comps.join("mysvc"),
            r#"
_mysvc() {
  local cur=${COMP_WORDS[COMP_CWORD]}
  COMPREPLY=($(compgen -W "start stop status" -- "$cur"))
}
complete -F _mysvc mysvc
"#,
        )
        .unwrap();
        // a legacy function reading the documented positional arguments
        // instead of COMP_WORDS
        std::fs::write(
            comps.join("legacysvc"),
            concat!(
                "\n_legacysvc() { COMPREPLY=($(compgen -W \"alpha beta\" -- \"${2-}\")); }\n",
                "complete -F _legacysvc legacysvc\n"
            ),
        )
        .unwrap();
        // a bare -W word list without a function exercises the compgen
        // fallback with a quoted multi-word value
        std::fs::write(
            comps.join("wsvc"),
            "complete -W \"start stop status\" wsvc\n",
        )
        .unwrap();
        let envs = [
            ("JSE_BASH_COMPLETION", core.to_str().unwrap()),
            ("JSE_FAKE_DIR", comps.to_str().unwrap()),
        ];
        // a registered -F rule answers with its words
        let line = "mysvc st";
        assert_eq!(
            bash_completion_env(line, line.len(), &envs),
            ["start", "stop", "status"]
        );
        // the empty word lists everything
        let line = "mysvc ";
        assert_eq!(
            bash_completion_env(line, line.len(), &envs),
            ["start", "stop", "status"]
        );
        // the function gets the word being completed as $2
        let line = "legacysvc al";
        assert_eq!(bash_completion_env(line, line.len(), &envs), ["alpha"]);
        // a quoted -W word list survives complete -p round-trip
        let line = "wsvc st";
        assert_eq!(
            bash_completion_env(line, line.len(), &envs),
            ["start", "stop", "status"]
        );
        // a command without a completion rule stays silent
        let line = "nosvc st";
        assert!(bash_completion_env(line, line.len(), &envs).is_empty());
    }

    #[test]
    fn tab_candidates_keeps_local_candidates_without_a_bash_rule() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
        let base = dir.path().display().to_string();

        // no rule for the command: the filesystem scan stays
        let line = format!("!zzjse-nosuchcmd {base}/al");
        let c = BashCache::default().tab_candidates(&line, line.len(), "");
        assert_eq!(c.candidates, vec![format!("{base}/alpha.txt")]);

        // `%` never reaches bash
        let c = BashCache::default().tab_candidates("!cat %", 6, "rel/config.json");
        assert_eq!(
            c.candidates,
            vec![shell_quote(&absolute_path("rel/config.json"))]
        );

        // the command word completes from PATH, not from a completion rule
        let c = BashCache::default().tab_candidates("!cat", 4, "");
        assert_eq!(
            c.candidates,
            completion_candidates("!cat", 4, "").candidates
        );

        // outside a shell region: builtin words only, bash not consulted
        let c = BashCache::default().tab_candidates("w", 1, "");
        assert!(c.candidates.contains(&"wa".to_string()));
    }

    #[test]
    #[cfg(unix)]
    fn auto_candidates_run_once_per_argument_word() {
        // a unix machine without bash (minimal containers) skips like the
        // pty tests do without a pty
        if std::process::Command::new("bash")
            .arg("-c")
            .arg("exit 0")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("runs.log");
        let core = dir.path().join("core.sh");
        std::fs::write(
            &core,
            concat!(
                "echo run >> \"$JSE_FAKE_LOG\"\n",
                r#"_completion_loader() { [ -f "$JSE_FAKE_DIR/$1" ] && . "$JSE_FAKE_DIR/$1"; }"#,
            ),
        )
        .unwrap();
        let comps = dir.path().join("comps");
        std::fs::create_dir(&comps).unwrap();
        std::fs::write(
            comps.join("mysvc"),
            concat!(
                "\n_mysvc() {\n",
                "  local cur=${COMP_WORDS[COMP_CWORD]}\n",
                "  COMPREPLY=($(compgen -W \"start stop status\" -- \"$cur\"))\n",
                "}\n",
                "complete -F _mysvc mysvc\n"
            ),
        )
        .unwrap();
        let envs = [
            ("JSE_BASH_COMPLETION", core.to_str().unwrap()),
            ("JSE_FAKE_DIR", comps.to_str().unwrap()),
            ("JSE_FAKE_LOG", log.to_str().unwrap()),
        ];
        let runs = || {
            std::fs::read_to_string(&log)
                .map(|c| c.lines().count())
                .unwrap_or(0)
        };
        let mut cache = BashCache::default();
        // the space opens the argument word: one fetch, the full list
        let c = cache.auto_candidates_env("!mysvc ", 7, "", &envs);
        assert_eq!(c.candidates, ["start", "stop", "status"]);
        // typing inside the word only filters the fetched words
        let c = cache.auto_candidates_env("!mysvc star", 11, "", &envs);
        assert_eq!(c.candidates, ["start"]);
        assert_eq!(runs(), 1, "no respawn while typing inside the word");
        // an explicit tab re-asks for the word's full context
        let c = cache.tab_candidates_env("!mysvc star", 11, "", &envs);
        assert_eq!(c.candidates, ["start"]);
        assert_eq!(runs(), 2);
        // the next space starts a new word: one fetch for it
        let c = cache.auto_candidates_env("!mysvc stop ", 12, "", &envs);
        assert_eq!(c.candidates, ["start", "stop", "status"]);
        assert_eq!(runs(), 3);
        // a command without a rule fetches once, then falls back locally
        let line = format!("!nosvc {}/al", dir.path().display());
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
        let c = cache.auto_candidates_env(&line, line.len(), "", &envs);
        assert_eq!(
            c.candidates,
            vec![format!("{}/alpha.txt", dir.path().display())]
        );
        let c = cache.auto_candidates_env(&line, line.len(), "", &envs);
        assert_eq!(
            c.candidates,
            vec![format!("{}/alpha.txt", dir.path().display())]
        );
        assert_eq!(runs(), 4, "no rule: no respawn while typing the word");
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

#[cfg(test)]
mod shell_tests {
    use super::*;
    use crate::doc::jsondoc::{Doc, Member};
    use crate::smodel::{FormNode, Schema};

    fn schema(nodes: Vec<FormNode>) -> Schema {
        Schema {
            root: 0,
            nodes,
            defs: Vec::new(),
            doc: Doc::new(),
            schema_less: false,
        }
    }

    fn scalar(jt: &str) -> FormNode {
        FormNode {
            kind: Kind::Scalar,
            json_type: jt.into(),
            ..FormNode::default()
        }
    }

    fn array(items: FormNodeId) -> FormNode {
        FormNode {
            kind: Kind::Array,
            items: Some(items),
            ..FormNode::default()
        }
    }

    fn one_of(variants: Vec<FormNodeId>) -> FormNode {
        FormNode {
            kind: Kind::OneOf,
            variants,
            ..FormNode::default()
        }
    }

    #[test]
    fn string_and_array_targets_are_classified() {
        let s = schema(vec![scalar("string"), scalar("number"), array(0)]);
        assert_eq!(node_target(&s, 0), Some(ShellTargetKind::String));
        assert_eq!(node_target(&s, 1), None);
        assert_eq!(node_target(&s, 2), Some(ShellTargetKind::StringArray));
    }

    #[test]
    fn one_of_prefers_string_over_string_array() {
        let s = schema(vec![
            scalar("string"),
            scalar("number"),
            array(0),
            one_of(vec![0, 2]),
            one_of(vec![2, 0]),
            one_of(vec![2, 1]),
            one_of(vec![1]),
        ]);
        assert_eq!(node_target(&s, 3), Some(ShellTargetKind::String));
        assert_eq!(node_target(&s, 4), Some(ShellTargetKind::String));
        assert_eq!(node_target(&s, 5), Some(ShellTargetKind::StringArray));
        assert_eq!(node_target(&s, 6), None);
    }

    #[test]
    fn runtime_targets_cover_strings_and_string_arrays() {
        let mut doc = Doc::new();
        let s = doc.new_str("x");
        assert_eq!(runtime_target(&doc, s), Some(ShellTargetKind::String));
        let a = doc.new_array();
        let x = doc.new_str("a");
        let y = doc.new_str("b");
        if let Value::Array(items) = &mut doc.nodes[a as usize].value {
            items.extend([x, y]);
        }
        assert_eq!(runtime_target(&doc, a), Some(ShellTargetKind::StringArray));
        let empty = doc.new_array();
        assert_eq!(
            runtime_target(&doc, empty),
            Some(ShellTargetKind::StringArray)
        );
        let n = doc.new_num("1");
        assert_eq!(runtime_target(&doc, n), None);
    }

    #[test]
    fn raw_nodes_fall_back_to_the_runtime_shape() {
        let s = schema(vec![FormNode {
            kind: Kind::Raw,
            ..FormNode::default()
        }]);
        let mut doc = Doc::new();
        let root = doc.new_object();
        doc.root = root;
        let v = doc.new_str("old");
        let members = vec![Member {
            key: "k".into(),
            val: v,
        }];
        doc.nodes[root as usize].value = Value::Object(members);
        let target = resolve_target(&s, &doc, Some(0), &[PathSeg::key("k")]).unwrap();
        assert_eq!(target.kind, ShellTargetKind::String);
        apply(&mut doc, &target, "new").unwrap();
        let got = doc.get(&[PathSeg::key("k")]).unwrap();
        assert!(matches!(doc.value(got), Value::Str(s) if s == "new"));
    }

    #[test]
    fn output_keeps_content_and_drops_the_shell_newline() {
        assert_eq!(output_text("hello\n"), "hello");
        assert_eq!(output_text("a\nb\r\n"), "a\nb");
        assert_eq!(output_text(""), "");
    }

    #[test]
    fn output_lines_split_crlf_and_keep_inner_empty_lines() {
        assert_eq!(output_lines("a\r\nb\n"), vec!["a", "b"]);
        assert_eq!(output_lines("a\n\nb\n"), vec!["a", "", "b"]);
        assert!(output_lines("").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_reports_output_and_failure() {
        assert_eq!(run_capture("printf hello").unwrap(), "hello");
        let err = run_capture("printf boom >&2; exit 4").unwrap_err();
        assert!(err.contains("boom"), "{err}");
        assert!(err.contains("exit 4"), "{err}");
    }

    #[test]
    fn apply_appends_to_string_arrays() {
        let mut doc = Doc::new();
        let root = doc.new_object();
        doc.root = root;
        let a = doc.new_array();
        let x = doc.new_str("x");
        if let Value::Array(items) = &mut doc.nodes[a as usize].value {
            items.push(x);
        }
        doc.nodes[root as usize].value = Value::Object(vec![Member {
            key: "tags".into(),
            val: a,
        }]);
        let target = ShellTarget::new(vec![PathSeg::key("tags")], ShellTargetKind::StringArray);
        apply(&mut doc, &target, "y").unwrap();
        let a = doc.get(&[PathSeg::key("tags")]).unwrap();
        let len = match doc.value(a) {
            Value::Array(items) => items.len(),
            _ => panic!("not an array"),
        };
        assert_eq!(len, 2);
    }

    #[test]
    fn compact_output_joins_lines_for_the_one_line_notice() {
        assert_eq!(compact_output("a\n\n b \n"), "a b");
        assert_eq!(compact_output("\n"), "");
    }

    #[cfg(unix)]
    #[test]
    fn run_with_input_feeds_stdin_and_returns_stdout() {
        assert_eq!(
            run_with_input("cat", "{\"name\":\"old\"}\n").unwrap(),
            "{\"name\":\"old\"}\n"
        );
        let err = run_with_input("cat >/dev/null; exit 3", "x").unwrap_err();
        assert!(err.contains("exit 3"), "{err}");
    }

    #[test]
    fn apply_lines_appends_each_line_and_rejects_non_arrays() {
        let mut doc = Doc::new();
        let root = doc.new_object();
        doc.root = root;
        let a = doc.new_array();
        let x = doc.new_str("x");
        if let Value::Array(items) = &mut doc.nodes[a as usize].value {
            items.push(x);
        }
        doc.nodes[root as usize].value = Value::Object(vec![Member {
            key: "tags".into(),
            val: a,
        }]);
        let target = ShellTarget::new(vec![PathSeg::key("tags")], ShellTargetKind::StringArray);
        apply_lines(&mut doc, &target, &["one".to_string(), "two".to_string()]).unwrap();
        let a = doc.get(&[PathSeg::key("tags")]).unwrap();
        let got: Vec<String> = match doc.value(a) {
            Value::Array(items) => items
                .iter()
                .map(|i| match doc.value(*i) {
                    Value::Str(s) => s.clone(),
                    _ => "<not a string>".into(),
                })
                .collect(),
            _ => panic!("not an array"),
        };
        assert_eq!(got, vec!["x", "one", "two"]);

        let s = doc.new_str("scalar");
        let p = vec![PathSeg::key("name")];
        let _ = doc.set(&p, s);
        let string_target = ShellTarget::new(p, ShellTargetKind::StringArray);
        assert!(apply_lines(&mut doc, &string_target, &["nope".to_string()]).is_err());
    }

    #[test]
    fn array_target_accepts_string_arrays_and_array_variants() {
        let s = schema(vec![
            scalar("string"),
            array(0),
            one_of(vec![0, 1]),
            scalar("number"),
        ]);
        let mut doc = Doc::new();
        let root = doc.new_object();
        doc.root = root;
        let a = doc.new_array();
        doc.nodes[root as usize].value = Value::Object(vec![Member {
            key: "tags".into(),
            val: a,
        }]);
        let target = resolve_array_target(&s, &doc, Some(2), &[PathSeg::key("tags")])
            .expect("oneOf arrays are accepted");
        assert_eq!(target.kind, ShellTargetKind::StringArray);
        assert!(
            resolve_array_target(&s, &doc, Some(0), &[PathSeg::key("tags")]).is_none(),
            "a plain string is not an append target"
        );
        assert!(resolve_array_target(&s, &doc, Some(3), &[PathSeg::key("tags")]).is_none());
    }
}
