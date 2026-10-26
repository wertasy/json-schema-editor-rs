//! vi-style `!` command modes. `r <cmd>` runs a command and writes its stdout
//! into the value under the menu cursor, `a <cmd>` splits stdout into lines
//! and appends each as an item of a string array, `w <cmd>` streams the live
//! document to the command's stdin, and a bare `<cmd>` only executes (its
//! output goes to the bottom notice).
//!
//! For `r`, a target is either a single string (replace the value) or an
//! array of strings (append the whole output as one item). The schema decides
//! when it can, and runtime values decide for schema-less/raw nodes. A oneOf
//! that admits both a string and an array of strings is treated as a string
//! (replace). The `a` mode only accepts an array-of-strings target.

use std::collections::{BTreeSet, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::doc::{Doc, NodeId, PathSeg, Value};
use crate::smodel::{FormNodeId, Kind, Schema};

/// One parsed `!` input line. `r <cmd>` inserts the command's stdout at the
/// cursor value, `a <cmd>` appends its stdout lines as array items, `w <cmd>`
/// streams the document to the command's stdin, and a bare command executes
/// vi-style without touching the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellMode {
    Execute,
    Read,
    Write,
    AppendLines,
}

/// Split one `!` input line into its mode and shell command. `r`/`a`/`w` are
/// reserved only when followed by whitespace, so commands such as `rm` or
/// `wc` still run as bare shell commands.
pub fn parse_input(line: &str) -> Result<(ShellMode, String), String> {
    let line = line.trim();
    if line.is_empty() {
        return Err("command required".to_string());
    }
    let mut parts = line.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    let mode = match head {
        "r" => ShellMode::Read,
        "a" => ShellMode::AppendLines,
        "w" => ShellMode::Write,
        _ => return Ok((ShellMode::Execute, line.to_string())),
    };
    if rest.is_empty() {
        return Err(format!("{head} needs a shell command"));
    }
    Ok((mode, rest.to_string()))
}

/// Completion for the `!` command line. The first word after an optional
/// `r`/`a`/`w` mode completes executable names from PATH; later words
/// complete filesystem paths. The returned strings are whole-line
/// replacements (prefix preserved), sorted and unique.
pub fn completion_candidates(line: &str) -> Vec<String> {
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    completion_candidates_in(line, &dirs)
}

fn completion_candidates_in(line: &str, path_dirs: &[PathBuf]) -> Vec<String> {
    let (token_start, token, command_word) = completion_token(line);
    let names = if command_word {
        command_candidates(path_dirs, &token)
    } else {
        path_candidates(&token)
    };
    let prefix = &line[..token_start];
    names
        .into_iter()
        .map(|name| format!("{prefix}{name}"))
        .collect()
}

/// Locate the word being completed: its byte start, its text and whether it
/// is the command word (first word after the optional mode).
fn completion_token(line: &str) -> (usize, String, bool) {
    let start = line.len() - line.trim_start().len();
    let rest = &line[start..];
    let first_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let first = &rest[..first_end];
    let mut cmd_start = start;
    if matches!(first, "r" | "a" | "w") && first_end < rest.len() {
        let after = start + first_end;
        cmd_start = after + (line[after..].len() - line[after..].trim_start().len());
    }
    let cmd_part = &line[cmd_start..];
    let word_rel = cmd_part
        .char_indices()
        .rfind(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let token_start = cmd_start + word_rel;
    let token = line[token_start..].to_string();
    let command_word = cmd_part[..word_rel].trim().is_empty();
    (token_start, token, command_word)
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
        // Case-insensitive like the write-path completion (shell `!` words):
        // typing `al` should offer `Alpha.txt` too.
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
fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
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

#[cfg(test)]
mod tests {
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
    fn parse_input_reserves_read_and_write_modes() {
        assert_eq!(
            parse_input("r echo hi"),
            Ok((ShellMode::Read, "echo hi".to_string()))
        );
        assert_eq!(
            parse_input("w cat"),
            Ok((ShellMode::Write, "cat".to_string()))
        );
        assert_eq!(
            parse_input("a printf 'x\\ny\\n'"),
            Ok((ShellMode::AppendLines, "printf 'x\\ny\\n'".to_string()))
        );
        assert_eq!(
            parse_input("echo hi"),
            Ok((ShellMode::Execute, "echo hi".to_string()))
        );
        // only a standalone first word reserves the mode: rm/wc stay commands
        assert_eq!(
            parse_input("rm -rf x"),
            Ok((ShellMode::Execute, "rm -rf x".to_string()))
        );
        assert_eq!(
            parse_input("wc -l"),
            Ok((ShellMode::Execute, "wc -l".to_string()))
        );
        assert!(parse_input("").is_err());
        assert!(parse_input("r").is_err());
        assert!(parse_input("w   ").is_err());
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

    #[cfg(unix)]
    #[test]
    fn command_completion_reads_executables_from_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("frobnicate");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plain = dir.path().join("frobnicate.txt");
        std::fs::write(&plain, "").unwrap();

        let got = completion_candidates_in("r frob", &[dir.path().to_path_buf()]);
        assert_eq!(got, vec!["r frobnicate"]);
    }

    #[test]
    fn path_completion_preserves_the_mode_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
        std::fs::write(dir.path().join("alpine"), "").unwrap();
        std::fs::create_dir(dir.path().join("al-dir")).unwrap();
        std::fs::write(dir.path().join("beta"), "").unwrap();
        let base = dir.path().display().to_string();
        let line = format!("r cat {base}/al");
        let got = completion_candidates_in(&line, &[]);
        assert_eq!(
            got,
            vec![
                format!("r cat {base}/al-dir/"),
                format!("r cat {base}/alpha.txt"),
                format!("r cat {base}/alpine"),
            ]
        );
    }

    #[test]
    fn completion_token_handles_multibyte_whitespace() {
        // U+00A0 and U+3000 are multi-byte whitespace: advancing one byte
        // past their start would slice inside the char and panic.
        for space in ['\u{00A0}', '\u{3000}'] {
            let line = format!("r cat{space}/tmp/x");
            let (token_start, token, command_word) = completion_token(&line);
            assert_eq!(token, "/tmp/x");
            assert!(!command_word);
            assert_eq!(token_start, line.find("/tmp/x").unwrap());
        }
    }

    #[test]
    fn path_completion_matches_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Alpha.txt"), "").unwrap();
        std::fs::write(dir.path().join("beta"), "").unwrap();
        let base = dir.path().display().to_string();
        let got = completion_candidates_in(&format!("r cat {base}/al"), &[]);
        assert_eq!(got, vec![format!("r cat {base}/Alpha.txt")]);
    }
}
