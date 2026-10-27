//! Lexical path helpers with Go `filepath` semantics (cleaning, extension,
//! base name). They never touch the filesystem: schema pointers may name files
//! that do not exist yet.

use std::path::Path;

/// searchStem is the file name without its extension, the initial catalog
/// search term. A name with no extension stays whole (".cshrc").
pub(crate) fn search_stem(base: &str) -> String {
    let stem = base.strip_suffix(file_ext(base)).unwrap_or(base);
    if stem.is_empty() {
        base.to_string()
    } else {
        stem.to_string()
    }
}
/// Go filepath.Ext: the suffix beginning at the final dot of the final
/// element (".cshrc" → ".cshrc", "a.b.json" → ".json", "a" → "").
pub(crate) fn file_ext(path: &str) -> &str {
    let bytes = path.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b'/' => return "",
            b'.' => return &path[i..],
            _ => {}
        }
    }
    ""
}
/// Go filepath.Base ("a/b/" → "b", "/" → "/", "" → ".").
pub(crate) fn file_base(path: &str) -> &str {
    if path.is_empty() {
        return ".";
    }
    let mut p = path;
    while p.ends_with('/') {
        p = &p[..p.len() - 1];
    }
    if let Some(i) = p.rfind('/') {
        p = &p[i + 1..];
    }
    if p.is_empty() { "/" } else { p }
}
/// Go filepath.Abs, lexically (no filesystem access). A missing working
/// directory yields "" like Go's discarded error; no catalog entry then
/// matches and the picker opens.
pub(crate) fn abs_lexical(path: &str) -> String {
    if Path::new(path).is_absolute() {
        return clean_lexical(path);
    }
    match std::env::current_dir() {
        Ok(cwd) => join_lexical(&cwd.to_string_lossy(), path),
        Err(_) => String::new(),
    }
}
/// Go filepath.Join of two elements (empties dropped, result cleaned).
pub(crate) fn join_lexical(a: &str, b: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !a.is_empty() {
        parts.push(a);
    }
    if !b.is_empty() {
        parts.push(b);
    }
    if parts.is_empty() {
        return String::new();
    }
    clean_lexical(&parts.join("/"))
}
/// Go filepath.Dir: everything up to the final separator, cleaned.
pub(crate) fn dir_lexical(path: &str) -> String {
    match path.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(i) => clean_lexical(&path[..i]),
    }
}
/// Go filepath.Clean, purely lexical: collapses separators, drops "." and
/// resolves ".." without touching the filesystem.
pub(crate) fn clean_lexical(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let bytes = path.as_bytes();
    let rooted = bytes[0] == b'/';
    let n = bytes.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut r = 0usize;
    let mut dotdot = 0usize;
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if bytes[r] == b'/' {
            r += 1;
        } else if bytes[r] == b'.' && (r + 1 == n || bytes[r + 1] == b'/') {
            r += 1;
        } else if bytes[r] == b'.'
            && r + 1 < n
            && bytes[r + 1] == b'.'
            && (r + 2 == n || bytes[r + 2] == b'/')
        {
            r += 2;
            if out.len() > dotdot {
                out.pop();
                while out.len() > dotdot && out.last() != Some(&b'/') {
                    out.pop();
                }
                // Go's window ends before the separator too: drop it.
                if out.len() > dotdot {
                    out.pop();
                }
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            while r < n && bytes[r] != b'/' {
                out.push(bytes[r]);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return ".".to_string();
    }
    String::from_utf8_lossy(&out).into_owned()
}
pub(crate) fn split_dir_file(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(i) => (path[..=i].to_string(), path[i + 1..].to_string()),
        None => (".".to_string(), path.to_string()),
    }
}
