//! Architecture invariants (AGENTS.md A1–A6), enforced mechanically.
//!
//! This is a binary crate, so `pub` is not an external boundary and the
//! compiler cannot express module-direction rules. These tests scan the
//! production sources instead: comments and inline `#[cfg(test)] mod tests`
//! blocks are ignored, and every violation reports `file:line`.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn production_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    rust_files(&root().join("src"), &mut files);
    files.retain(|p| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name != "tests.rs" && !name.ends_with("_tests.rs")
    });
    files.sort();
    files
}

/// Source lines with comments blanked and the inline test module removed.
/// Line numbers stay valid for error messages.
fn production_lines(path: &Path) -> Vec<String> {
    let raw = fs::read_to_string(path).expect("read source");
    let lines: Vec<&str> = raw.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed == "#[cfg(test)]" {
            let next = lines[i + 1..]
                .iter()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim())
                .unwrap_or("");
            if next.starts_with("mod tests") {
                break;
            }
        }
        if trimmed.starts_with("//") {
            out.push(String::new());
        } else {
            out.push(line.to_string());
        }
        i += 1;
    }
    out
}

fn rel(path: &Path) -> String {
    path.strip_prefix(root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn violations(pred: impl Fn(&str) -> bool, files: &[PathBuf]) -> Vec<String> {
    let mut found = Vec::new();
    for path in files {
        for (i, line) in production_lines(path).iter().enumerate() {
            if pred(line) {
                found.push(format!("{}:{}: {}", rel(path), i + 1, line.trim()));
            }
        }
    }
    found
}

fn is_ui(path: &Path) -> bool {
    rel(path).starts_with("src/ui/")
}

fn is_doc(path: &Path) -> bool {
    rel(path).starts_with("src/doc/")
}

#[test]
fn doc_is_a_leaf_module() {
    let files = production_files();
    let mut found = Vec::new();
    for path in files.iter().filter(|p| is_doc(p)) {
        for (i, line) in production_lines(path).iter().enumerate() {
            if let Some(pos) = line.find("crate::") {
                let rest = &line[pos..];
                if !rest.starts_with("crate::doc") && !rest.starts_with("crate::paths") {
                    found.push(format!("{}:{}: {}", rel(path), i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "doc must stay a leaf (only crate::doc / crate::paths):\n{}",
        found.join("\n")
    );
}

#[test]
fn ui_does_not_load_or_compile_schemas() {
    let files: Vec<PathBuf> = production_files()
        .into_iter()
        .filter(|p| is_ui(p))
        .collect();
    let found = violations(
        |l| {
            l.contains("smodel::load(")
                || l.contains("validate::new(")
                || l.contains("crate::fetch")
                || l.contains("crate::session")
                || l.contains("crate::file_base")
                || l.contains("crate::search_stem")
        },
        &files,
    );
    assert!(
        found.is_empty(),
        "the UI must not load/compile schemas or reach the crate root:\n{}",
        found.join("\n")
    );
}

#[test]
fn core_modules_do_not_depend_on_ui() {
    let files: Vec<PathBuf> = production_files()
        .into_iter()
        .filter(|p| {
            matches!(
                rel(p).as_str(),
                "src/fetch.rs" | "src/validate.rs" | "src/catalog.rs" | "src/smodel.rs"
            )
        })
        .collect();
    let found = violations(|l| l.contains("crate::ui"), &files);
    assert!(
        found.is_empty(),
        "core modules must not depend on the UI:\n{}",
        found.join("\n")
    );
}

#[test]
fn cli_and_paths_are_std_only() {
    let files: Vec<PathBuf> = production_files()
        .into_iter()
        .filter(|p| matches!(rel(p).as_str(), "src/cli.rs" | "src/paths.rs"))
        .collect();
    let found = violations(|l| l.contains("crate::"), &files);
    assert!(
        found.is_empty(),
        "cli/paths must not depend on other modules:\n{}",
        found.join("\n")
    );
}

#[test]
fn helper_definitions_are_unique() {
    let files = production_files();
    let once = [
        "fn copy_into(",
        "fn append_key(",
        "fn append_idx(",
        "fn wrap_cursor(",
        "fn page_cursor(",
        "fn write_indent(out: &mut String",
        "fn encode_value(",
        "fn array_len(",
    ];
    let never = ["fn path_for(", "fn path_idx(", "fn doc_array_len("];
    let mut problems = Vec::new();
    for pat in once {
        let hits = violations(|l| l.contains(pat), &files);
        if hits.len() != 1 {
            problems.push(format!(
                "{pat}: expected 1 definition, found {}",
                hits.len()
            ));
        }
    }
    for pat in never {
        let hits = violations(|l| l.contains(pat), &files);
        if !hits.is_empty() {
            problems.push(format!("{pat}: must not exist:\n{}", hits.join("\n")));
        }
    }
    // scalar_text exists twice by design: the Doc method and TOML's
    // format-specific writer (raw spelling + quoting).
    let scalar_hits = violations(|l| l.contains("fn scalar_text("), &files);
    if scalar_hits.len() != 2 {
        problems.push(format!(
            "fn scalar_text(: expected 2 definitions, found {}",
            scalar_hits.len()
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn no_go_named_helpers() {
    let files = production_files();
    let found = violations(
        |l| {
            let mut rest = l;
            while let Some(i) = rest.find("go_") {
                let after = &rest[i + 3..];
                if after.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
                    return true;
                }
                rest = after;
            }
            false
        },
        &files,
    );
    assert!(
        found.is_empty(),
        "Go-era helper names must be renamed:\n{}",
        found.join("\n")
    );
}

#[test]
fn no_module_wide_dead_code_allow() {
    let files = production_files();
    let found = violations(
        |l| l.trim_start().starts_with("#![allow(dead_code)]"),
        &files,
    );
    assert!(
        found.is_empty(),
        "module-wide dead_code allows hide real rot:\n{}",
        found.join("\n")
    );
}

#[test]
fn shared_document_state_is_gone() {
    let files = production_files();
    let found = violations(
        |l| l.contains("Mutex<Meta>") || l.contains("Mutex<crate::doc::Meta>"),
        &files,
    );
    assert!(
        found.is_empty(),
        "per-file Meta must live in FileState, not a shared mutex:\n{}",
        found.join("\n")
    );
}

#[test]
fn overlay_is_the_only_surface_switch() {
    let files = production_files();
    let found = violations(
        |l| l.contains("pub help: bool") || l.contains("pub file_pick: Option<"),
        &files,
    );
    assert!(
        found.is_empty(),
        "help/file pick must be Overlay variants, not App flags:\n{}",
        found.join("\n")
    );
}

#[test]
fn placeholder_modules_are_gone() {
    for name in ["merge", "paste", "search", "select"] {
        let path = root().join(format!("src/ui/{name}.rs"));
        assert!(!path.exists(), "placeholder module {} is back", rel(&path));
    }
}
