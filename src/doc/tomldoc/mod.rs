//! TOML layer: converts between TOML and the document tree (arena).
//!
//! A TOML document is a root table. Headers (`[table]`, `[[array]]`) and
//! dotted keys build the nested objects of [`Doc`]; scalar spellings JSON
//! cannot express (hex/octal/binary integers, underlined numbers, date/time
//! values) keep their source text on `NodeData::raw`, so an untouched file
//! saves unchanged. Edits write canonical TOML; key order and comments are
//! retained.
//!
//! Comments attach to the member they describe: a trailing comment stays on
//! the key/value or table-header line, and own-line comments become the next
//! key/header's leading comments (or the document tail).

mod emit;
mod parse;

pub use emit::{write, write_value};
pub use parse::{parse, parse_value};

use crate::doc::jsondoc::{Doc, PathSeg};
use crate::doc::{CommentSet, Meta};

/// SchemaDirective returns the location from a `#:schema <loc>` header
/// comment (the Taplo convention).
pub fn schema_directive(_doc: &Doc, meta: &Meta) -> Option<String> {
    for cs in meta.comments.values() {
        // only leading comments: `x = 1 #:schema ...` is not a directive
        for slot in [&cs.head, &cs.key_head] {
            for line in slot.split('\n') {
                if let Some(loc) = directive_loc(line) {
                    return Some(loc);
                }
            }
        }
    }
    None
}

/// SetSchemaDirective records loc in the document header. The line goes into
/// the root comment set, so it stays at the top of the file.
pub fn set_schema_directive(_doc: &Doc, meta: &mut Meta, loc: &str) {
    if loc.is_empty() || schema_directive(_doc, meta).is_some() {
        return;
    }
    let line = format!("#:schema {loc}");
    let cs = meta.comments.entry(Vec::new()).or_default();
    if cs.head.is_empty() {
        cs.head = line;
    } else {
        cs.head = format!("{line}\n{}", cs.head);
    }
}

/// ClearSchemaDirective removes `#:schema` lines from every comment slot (a
/// --pick must be able to replace the old location).
pub fn clear_schema_directive(_doc: &Doc, meta: &mut Meta) {
    let mut empty: Vec<Vec<PathSeg>> = Vec::new();
    for (path, cs) in meta.comments.iter_mut() {
        let mut changed = false;
        let slots = [
            &mut cs.head,
            &mut cs.key_head,
            &mut cs.line,
            &mut cs.key_line,
            &mut cs.foot,
            &mut cs.key_foot,
        ];
        for slot in slots {
            if slot.is_empty() {
                continue;
            }
            let kept: Vec<&str> = slot.split('\n').filter(|l| !is_directive(l)).collect();
            if kept.len() != slot.split('\n').count() {
                changed = true;
            }
            *slot = kept.join("\n");
        }
        if changed && *cs == CommentSet::default() {
            empty.push(path.clone());
        }
    }
    for path in empty {
        meta.comments.remove(&path);
    }
}

fn directive_loc(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("#:schema")?;
    if rest.is_empty() {
        return None;
    }
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').map(str::trim_start).unwrap_or(rest);
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

fn is_directive(line: &str) -> bool {
    directive_loc(line).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_round_trip() {
        let (doc, mut meta) = parse("a = 1\n").unwrap();
        assert_eq!(schema_directive(&doc, &meta), None);
        set_schema_directive(&doc, &mut meta, "https://example.com/s.json");
        assert_eq!(
            schema_directive(&doc, &meta).as_deref(),
            Some("https://example.com/s.json")
        );
        let text = write(&doc, &meta).unwrap();
        assert!(
            text.starts_with("#:schema https://example.com/s.json\na = 1\n"),
            "{text}"
        );

        let (doc2, mut meta2) = parse(&text).unwrap();
        assert_eq!(
            schema_directive(&doc2, &meta2).as_deref(),
            Some("https://example.com/s.json")
        );
        clear_schema_directive(&doc2, &mut meta2);
        assert_eq!(schema_directive(&doc2, &meta2), None);
        assert!(!write(&doc2, &meta2).unwrap().contains("#:schema"));
    }

    #[test]
    fn directive_accepts_equals_and_leading_space() {
        assert_eq!(
            directive_loc("  #:schema=https://e/x.json").as_deref(),
            Some("https://e/x.json")
        );
        assert_eq!(directive_loc("#:schema"), None);
        assert_eq!(directive_loc("# normal"), None);
    }
}
