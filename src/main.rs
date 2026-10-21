//! jse — schema-driven terminal editor for JSON/JSONC/YAML config files.
//! Rust port of the Go implementation (behavior spec: internal/**, cmd/jse).
#![cfg_attr(not(test), forbid(unsafe_code))]

mod catalog;
mod doc;
mod fetch;
mod icons;
mod smodel;
mod ui;
mod validate;
mod version;

#[cfg(test)]
mod catalog_tests;
#[cfg(test)]
mod fetch_tests;
#[cfg(test)]
mod validate_tests;

use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::doc::{Codec, Doc, Meta, Parsed, jsondoc};
use crate::ui::{FileSession, NodeRef, Options, Outcome, RawCodec};

const ERR_NO_SCHEMA: &str =
    "no schema found: provide -s or record the schema location in the config";

const USAGE_TEXT: &str = "Usage: jse [-s <schema path|http(s)://URL>] [-p|--pick] [-f|--force]
           [-a|--accessible] [-v|--version] [-h|--help] <config.(json|jsonc|yaml|yml)>...

Edit a JSON, JSONC, or YAML configuration file guided by a JSON Schema.

Schema source (checked in order):
  1. -s, --schema <local path or http(s)://URL>
  2. the config's own schema pointer: a top-level \"$schema\" key, or a
     \"# yaml-language-server: $schema=...\" comment for YAML files;
     relative paths resolve against the config file's directory, falling
     back to the current working directory when no file exists there
  3. the SchemaStore catalog: a unique fileMatch hit for the file name is
     used directly; an ambiguous or missing match opens an interactive
     search over the catalog (--pick forces the search)
  4. error if none of the above applies

Remote schemas are cached under $XDG_CACHE_HOME/jse (JSE_CACHE_DIR overrides
the location): a copy from the last 24 hours is reused without a request, and
an unreachable origin falls back to the cached copy instead of failing. The
schema catalog shares that cache; JSE_CATALOG_URL overrides the catalog
endpoint.

Arguments:
  config file   the configuration file to edit (local path); the format
                follows the extension (.json/.jsonc, .yaml/.yml) and a
                missing file starts empty and is created on save.
                Several files open in one session: the first is shown, s
                opens the file switcher, and ctrl+o/ctrl+i step
                back/forward across switches; each file keeps its
                breadcrumb and cursor. A name beginning with \"-\" needs the
                \"--\" terminator: jse -- -name.json

Flags:
  -s, --schema <path|URL>  JSON Schema: local path or http(s):// URL
  -p, --pick               choose the schema from the SchemaStore catalog
                           (overrides --schema and \"$schema\")
  -f, --force              save even when schema validation fails
  -a, --accessible         line-based prompts instead of the full-screen TUI
  -v, --version            print the version and exit
  -h, --help               show this help

Keys: ↑/↓ move · space toggle · enter select · tab next · esc back · ctrl+c quit

Supported form keywords: type, enum/const, numeric and string constraints,
properties/required, items/minItems/maxItems, local and remote $ref,
oneOf/anyOf/allOf. Everything else is still validated on save; unsupported
constructs fall back to a raw editor in the file's format (JSON or YAML).";

type ValidateFn = Arc<dyn Fn(&Doc) -> Vec<String> + Send + Sync>;
type SaveFn = Arc<dyn Fn(&Doc) -> Result<(), String> + Send + Sync>;
type WriteToFn = Arc<dyn Fn(&str, &Doc) -> Result<(), String> + Send + Sync>;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags = match parse_args(&args) {
        Ok(Invocation::Help) => {
            print_usage();
            return 0;
        }
        Ok(Invocation::Version) => {
            println!("{}", version::version_line());
            return 0;
        }
        Ok(Invocation::Edit(flags)) => flags,
        Err(msg) => {
            eprintln!("{msg}");
            print_usage();
            return 2;
        }
    };

    if flags.files.is_empty() {
        eprintln!("no config file: pass <config.(json|jsonc|yaml|yml)> as a positional argument");
        print_usage();
        return 2;
    }

    // The full-screen TUI needs a real stdin; fail before loading anything.
    if !flags.accessible && !std::io::stdin().is_terminal() {
        eprintln!("stdin is not a terminal; rerun with --accessible for line-based prompts");
        return 1;
    }

    // Every file opens in one session (the first is shown); any file that
    // cannot load or resolve a schema aborts the whole run before the UI
    // starts.
    let mut sessions: Vec<FileSession> = Vec::with_capacity(flags.files.len());
    for config in &flags.files {
        match load_session(
            config,
            &flags.schema,
            flags.force,
            flags.pick,
            flags.accessible,
        ) {
            Ok(s) => sessions.push(s),
            Err(code) => return code,
        }
    }

    let outcome: Outcome = ui::run(sessions);
    if let Some(err) = &outcome.err {
        eprintln!("ui: {err}");
        return 1;
    }
    if outcome.aborted {
        if outcome.saved {
            if outcome.saved_files.len() == 1 {
                // A ctrl+s during the session already wrote the file; edits
                // made after that save are discarded.
                println!(
                    "Saved to {}; aborted (changes after the last save are lost)",
                    outcome.saved_files[0]
                );
            } else {
                for p in &outcome.saved_files {
                    println!("Saved to {p}");
                }
                println!("Aborted (changes after the last save are lost)");
            }
        } else {
            println!("Aborted, no changes written");
        }
        return 130;
    }
    if !outcome.saved {
        if outcome.unchanged {
            // Clean quit on a document identical to the loaded/last-saved
            // state: nothing to write.
            println!("No changes, nothing written");
            return 0;
        }
        println!("Aborted, no changes written");
        return 130;
    }
    if outcome.with_errors {
        println!(
            "Saved with validation errors (--force): {}",
            outcome.saved_files.join(", ")
        );
        return 0;
    }
    for p in &outcome.saved_files {
        println!("Saved to {p}");
    }
    0
}

// --- command-line parsing (getopt/getopt_long style) ---------------------------

#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Help,
    Version,
    Edit(Flags),
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Flags {
    schema: String,
    force: bool,
    accessible: bool,
    pick: bool,
    files: Vec<String>,
}

/// parse_args implements the getopt/getopt_long option syntax used by the
/// CLI: `-s v`, `-sv`, `--schema v`, `--schema=v`, clustered short flags
/// (`-fa`), floating operands (options may follow the config paths), and the
/// `--` terminator. `-h`/`-v` (and their long forms) stop parsing as soon as
/// they are seen. Errors are returned instead of exiting so they can be
/// tested; run() prints the message plus the usage and exits 2.
fn parse_args(args: &[String]) -> Result<Invocation, String> {
    let mut f = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;

        if arg == "--" {
            f.files.extend(args[i..].iter().cloned());
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            f.files.push(arg.clone());
            continue;
        }

        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            match name {
                "schema" => {
                    f.schema = match inline {
                        Some(value) => value.to_string(),
                        None if i < args.len() => {
                            let value = args[i].clone();
                            i += 1;
                            value
                        }
                        None => {
                            return Err("option '--schema' requires an argument".to_string());
                        }
                    };
                }
                "pick" | "force" | "accessible" => {
                    if inline.is_some() {
                        return Err(format!("option '--{name}' doesn't allow an argument"));
                    }
                    match name {
                        "pick" => f.pick = true,
                        "force" => f.force = true,
                        _ => f.accessible = true,
                    }
                }
                "help" | "version" => {
                    if inline.is_some() {
                        return Err(format!("option '--{name}' doesn't allow an argument"));
                    }
                    return Ok(if name == "help" {
                        Invocation::Help
                    } else {
                        Invocation::Version
                    });
                }
                _ => return Err(format!("unrecognized option '--{name}'")),
            }
            continue;
        }

        let body = &arg[1..];
        for (off, ch) in body.char_indices() {
            match ch {
                'p' => f.pick = true,
                'f' => f.force = true,
                'a' => f.accessible = true,
                'h' => return Ok(Invocation::Help),
                'v' => return Ok(Invocation::Version),
                's' => {
                    let rest = &body[off + 1..];
                    f.schema = if rest.is_empty() {
                        if i < args.len() {
                            let value = args[i].clone();
                            i += 1;
                            value
                        } else {
                            return Err("option requires an argument -- 's'".to_string());
                        }
                    } else {
                        // `-s=value` stays accepted for compatibility with the
                        // previous Go flag style; getopt itself would keep the
                        // leading '=' in the value.
                        rest.strip_prefix('=').unwrap_or(rest).to_string()
                    };
                    break;
                }
                _ => return Err(format!("invalid option -- '{ch}'")),
            }
        }
    }
    Ok(Invocation::Edit(f))
}

fn print_usage() {
    eprintln!("{USAGE_TEXT}");
}

// --- session assembly ------------------------------------------------------------

/// loadSession builds one file's editor session: load the document (missing
/// file starts empty), resolve the schema (-s / the file's own pointer /
/// catalog, --pick forces the picker per file), compile the validator, wire
/// save/write-to and the raw codec. Err is an exit status after the error was
/// printed.
fn load_session(
    config: &str,
    schema_flag: &str,
    force: bool,
    pick: bool,
    accessible: bool,
) -> Result<FileSession, i32> {
    if !http_prefix(config).is_empty() {
        eprintln!("config must be a local file path");
        return Err(2);
    }

    let codec = doc::codec_for(config);

    // Load config; a missing file starts empty and is created on save.
    let (mut parsed, loaded) = match std::fs::read(config) {
        Ok(data) => match codec.parse(&String::from_utf8_lossy(&data)) {
            Ok(p) => (p, true),
            Err(e) => {
                eprintln!("parse config {config}: {e}");
                return Err(1);
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (codec.empty(), false),
        Err(e) => {
            eprintln!("read config {config}: {e}");
            return Err(1);
        }
    };

    let mut status: Vec<String> = Vec::new(); // warnings, shown for this file's session
    let mut startup: Vec<String> = codec.notices(&parsed.doc, &parsed.meta); // info notices, hidden after the first key press

    // A -s location is recorded in the file's own convention (a top-level
    // "$schema" member, or a yaml-language-server comment for YAML) when the
    // file has none, keeping the saved config self-describing.
    if !schema_flag.is_empty() {
        codec.set_schema_location(&mut parsed.doc, &mut parsed.meta, schema_flag);
    }
    // A catalog pick must overwrite an existing location, so drop it first.
    if pick {
        codec.clear_schema_location(&mut parsed.doc, &mut parsed.meta);
    }

    let mut schema_loc = String::new();
    // Go's resolveErr is either nil or exactly errNoSchema (resolveSchemaLoc
    // returns nothing else), so a bool carries all reachable state.
    let mut resolve_err = false;
    if !pick {
        match resolve_schema_loc(schema_flag, config, codec, &parsed.doc, &parsed.meta) {
            Ok(loc) => schema_loc = loc,
            Err(NoSchema) => resolve_err = true,
        }
    }

    if pick || resolve_err {
        let loc = catalog::loc();
        let entries = match catalog::load(&loc) {
            Ok(e) => e,
            Err(cerr) => {
                if pick {
                    eprintln!("load schema catalog: {cerr}");
                    return Err(1);
                }
                eprintln!("{ERR_NO_SCHEMA}");
                eprintln!("schema catalog unavailable: {cerr}");
                return Err(2);
            }
        };
        let note = fetch::note(&loc);
        if !note.is_empty() {
            status.push(note.replacen("cached schema", "cached catalog", 1));
        }
        let mut chosen: Option<catalog::Entry> = None;
        if !pick {
            let abs = go_abs(config); // Match needs the absolute path
            let m = catalog::match_entries(&entries, &abs);
            if m.len() == 1 {
                chosen = m.into_iter().next();
            }
        }
        let chosen = match chosen {
            Some(c) => c,
            None => {
                if !accessible && !std::io::stdin().is_terminal() {
                    if resolve_err {
                        eprintln!("{ERR_NO_SCHEMA}");
                    }
                    eprintln!(
                        "schema selection needs a terminal; rerun with --accessible or provide -s"
                    );
                    return Err(2);
                }
                let base = file_base(config);
                match ui::schema_picker::pick(
                    &entries,
                    &ui::schema_picker::PickOptions {
                        accessible,
                        filename: base.to_string(),
                        query: search_stem(base),
                    },
                ) {
                    Some(c) => c,
                    None => {
                        eprintln!("{ERR_NO_SCHEMA}");
                        return Err(2);
                    }
                }
            }
        };
        schema_loc = chosen.url.clone();
        codec.set_schema_location(&mut parsed.doc, &mut parsed.meta, &schema_loc);
        startup.push(format!("schema from SchemaStore: {}", chosen.name));
    }

    let model = match smodel::load(&schema_loc) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("load schema {schema_loc}: {e}");
            return Err(1);
        }
    };
    let root = model.root;

    let mut check: Option<Arc<validate::V>> = None;
    let mut load_errors: Vec<String> = Vec::new();
    let note = fetch::note(&schema_loc);
    if !note.is_empty() {
        status.push(note); // offline: cached schema in use
    }
    match validate::new(&schema_loc) {
        Err(verr) => {
            let msg = verr.replace('\n', " ");
            status.push(format!(
                "{} schema compile failed, save-time validation disabled: {msg}",
                icons::WARN
            ));
        }
        Ok(v) => {
            let v = Arc::new(v);
            if loaded {
                load_errors = v.check(&parsed.doc);
            }
            check = Some(v);
        }
    }

    // Raw editors use the file's own format (YAML for .yaml/.yml, JSON
    // otherwise). The menu owns the live Doc, while the format metadata
    // (JSONC comments, YAML anchors/directives) lives in this shared slot:
    // marshal_doc writes the caller's document with the CURRENT meta, and
    // adopt_doc installs a freshly parsed meta so writes after a raw-doc
    // round trip use the adopted comment state (Go closes over the session
    // and replaces it in AdoptDoc; the Arc<Mutex<Meta>> is that session).
    let shared_meta: Arc<Mutex<Meta>> = Arc::new(Mutex::new(parsed.meta));
    let raw = if matches!(file_ext(config).to_lowercase().as_str(), ".yaml" | ".yml") {
        RawCodec {
            label: "YAML",
            ext: ".yaml",
            marshal_doc: {
                let shared_meta = Arc::clone(&shared_meta);
                Arc::new(move |doc: &Doc| {
                    let meta = lock(&shared_meta);
                    codec.write(doc, &meta).map_err(|e| e.to_string())
                })
            },
            unmarshal_doc: Arc::new(|data: &str| {
                doc::yamldoc::parse(data)
                    .map(|(d, _)| d)
                    .map_err(|e| e.to_string())
            }),
            adopt_doc: Some({
                let shared_meta = Arc::clone(&shared_meta);
                Arc::new(move |data: &str| {
                    let Parsed { doc, meta } = codec.parse(data).map_err(|e| e.to_string())?;
                    *lock(&shared_meta) = meta.clone();
                    Ok((doc, meta))
                })
            }),
            marshal_value: Arc::new(|r: NodeRef<'_>| yaml_value_text(r.doc, r.id)),
            unmarshal_value: Arc::new(|data: &str| {
                doc::yamldoc::parse(data)
                    .map(|(d, _)| d)
                    .map_err(|e| e.to_string())
            }),
        }
    } else {
        // Go's zero ui.RawCodec: plain jsondoc round trips, no comment
        // adoption (comments survive saves via the session meta instead).
        RawCodec {
            label: "",
            ext: "",
            marshal_doc: Arc::new(|doc: &Doc| Ok(doc.write())),
            unmarshal_doc: Arc::new(|data: &str| jsondoc::parse(data).map_err(|e| e.to_string())),
            adopt_doc: None,
            marshal_value: Arc::new(|r: NodeRef<'_>| Ok(r.doc.write_subtree(r.id))),
            unmarshal_value: Arc::new(|data: &str| jsondoc::parse(data).map_err(|e| e.to_string())),
        }
    };

    let opts = Options {
        accessible,
        force,
        raw,
        validate: check.map(|v| -> ValidateFn { Arc::new(move |doc: &Doc| v.check(doc)) }),
        save: {
            let shared_meta = Arc::clone(&shared_meta);
            let path = config.to_string();
            Some(
                Arc::new(move |doc: &Doc| write_session(&shared_meta, codec, doc, &path)) as SaveFn,
            )
        },
        path: config.to_string(),
        write_to: {
            let shared_meta = Arc::clone(&shared_meta);
            Some(Arc::new(move |path: &str, doc: &Doc| {
                write_session(&shared_meta, codec, doc, path)
            }) as WriteToFn)
        },
        copy: None,
        paste: None,
        status: status.join("\n"),
        startup_notice: startup.join("\n"),
        load_errors,
    };

    Ok(FileSession {
        schema: model,
        root,
        doc: parsed.doc,
        opts,
    })
}

/// write_session renders the live document in the file's own format (with
/// the session's current meta) and lands it atomically.
fn write_session(
    shared_meta: &Mutex<Meta>,
    codec: &'static dyn Codec,
    doc: &Doc,
    path: &str,
) -> Result<(), String> {
    let text = {
        let meta = lock(shared_meta);
        codec.write(doc, &meta).map_err(|e| e.to_string())?
    };
    atomic_write(path, &text)
}

fn http_prefix(p: &str) -> &str {
    if p.starts_with("http://") || p.starts_with("https://") {
        p
    } else {
        ""
    }
}

// --- schema source resolution ----------------------------------------------------

/// Sentinel for Go's errNoSchema (the only error resolveSchemaLoc returns).
#[derive(Debug)]
struct NoSchema;

/// resolveSchemaLoc implements the schema source resolution order.
fn resolve_schema_loc(
    s_flag: &str,
    c_flag: &str,
    codec: &'static dyn Codec,
    doc: &Doc,
    meta: &Meta,
) -> Result<String, NoSchema> {
    if !s_flag.is_empty() {
        return Ok(s_flag.to_string());
    }
    if let Some(u) = codec.schema_location(doc, meta) {
        if !u.is_empty() {
            if u.starts_with("http://") || u.starts_with("https://") {
                return Ok(u);
            }
            if Path::new(&u).is_absolute() {
                return Ok(u);
            }
            // Prefer the config file's directory; fall back to the process
            // working directory when no file exists there.
            let from_config = go_join(&go_dir(c_flag), &u);
            return match std::fs::metadata(&from_config) {
                Ok(_) => Ok(from_config),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(u),
                Err(_) => Ok(from_config),
            };
        }
    }
    Err(NoSchema)
}

// --- filesystem helpers ----------------------------------------------------------

/// atomicWrite writes a document to path via a same-directory temp file +
/// rename, preserving the mode of an existing file.
fn atomic_write(path: &str, contents: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let mut mode: u32 = 0o644;
    if let Ok(md) = std::fs::metadata(path) {
        mode = md.permissions().mode() & 0o777;
    }
    let (dir, base) = split_dir_file(path);
    let mut tmp: Option<(std::fs::File, std::path::PathBuf)> = None;
    for attempt in 0..10_000u64 {
        let candidate = Path::new(&dir).join(format!(".jse-{}-{base}", temp_suffix(attempt)));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => {
                tmp = Some((f, candidate));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    let Some((mut f, tmp_path)) = tmp else {
        return Err(format!("open {dir}.jse-*- {base}: file exists"));
    };
    let done = (|| -> std::io::Result<()> {
        f.write_all(contents.as_bytes())?;
        let mut perms = f.metadata()?.permissions();
        perms.set_mode(mode);
        f.set_permissions(perms)?;
        f.flush()?;
        drop(f);
        std::fs::rename(&tmp_path, path)
    })();
    match done {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path); // no-op after a successful rename
            Err(e.to_string())
        }
    }
}

fn temp_suffix(attempt: u64) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let pid = u64::from(std::process::id());
    let v =
        nanos ^ (pid << 17) ^ (SEQ.fetch_add(1, Ordering::Relaxed) << 3) ^ attempt.rotate_left(7);
    format!("{v:012x}")
}

fn split_dir_file(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(i) => (path[..=i].to_string(), path[i + 1..].to_string()),
        None => (".".to_string(), path.to_string()),
    }
}

/// searchStem is the file name without its extension, the initial catalog
/// search term. A name with no extension stays whole (".cshrc").
fn search_stem(base: &str) -> String {
    let stem = base.strip_suffix(file_ext(base)).unwrap_or(base);
    if stem.is_empty() {
        base.to_string()
    } else {
        stem.to_string()
    }
}

/// Go filepath.Ext: the suffix beginning at the final dot of the final
/// element (".cshrc" → ".cshrc", "a.b.json" → ".json", "a" → "").
fn file_ext(path: &str) -> &str {
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
fn file_base(path: &str) -> &str {
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
fn go_abs(path: &str) -> String {
    if Path::new(path).is_absolute() {
        return go_clean(path);
    }
    match std::env::current_dir() {
        Ok(cwd) => go_join(&cwd.to_string_lossy(), path),
        Err(_) => String::new(),
    }
}

/// Go filepath.Join of two elements (empties dropped, result cleaned).
fn go_join(a: &str, b: &str) -> String {
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
    go_clean(&parts.join("/"))
}

/// Go filepath.Dir: everything up to the final separator, cleaned.
fn go_dir(path: &str) -> String {
    match path.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(i) => go_clean(&path[..i]),
    }
}

/// Go filepath.Clean, purely lexical: collapses separators, drops "." and
/// resolves ".." without touching the filesystem.
fn go_clean(path: &str) -> String {
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

// --- raw YAML value export -------------------------------------------------------

/// yaml_value_text renders one subtree as YAML (Go yamldoc.Write(w, n, nil)):
/// a standalone document with anchors/aliases dropped and merges resolved.
/// yamldoc's emitter walks Doc::root, so the subtree is deep-copied into a
/// fresh arena first; numeric literals and merge refs are preserved.
fn yaml_value_text(doc: &Doc, id: crate::doc::NodeId) -> Result<String, String> {
    let mut sub = Doc::new();
    sub.root = copy_value(doc, id, &mut sub);
    doc::yamldoc::write(&sub, None).map_err(|e| e.to_string())
}

fn copy_value(src: &Doc, id: crate::doc::NodeId, dst: &mut Doc) -> crate::doc::NodeId {
    use crate::doc::{Member, Value};
    let merge: Vec<crate::doc::NodeId> = src
        .node(id)
        .merge
        .iter()
        .map(|&m| copy_value(src, m, dst))
        .collect();
    let value = match src.value(id) {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|m| Member {
                    key: m.key.clone(),
                    val: copy_value(src, m.val, dst),
                })
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.iter().map(|&i| copy_value(src, i, dst)).collect())
        }
        Value::Str(s) => Value::Str(s.clone()),
        Value::Num(lit) => Value::Num(lit.clone()),
        Value::Bool(b) => Value::Bool(*b),
        Value::Null => Value::Null,
    };
    dst.push(jsondoc::NodeData {
        value,
        anchor: String::new(),
        merge,
    })
}

/// Mutex lock that survives poisoning (a panic in one editor path must not
/// turn every later save into a lock error).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

#[cfg(test)]
mod main_test {
    use super::*;

    fn session_with_schema(cfg: &str, loc: &str) -> Parsed {
        let codec = doc::codec_for(cfg);
        let mut p = codec.empty();
        codec.set_schema_location(&mut p.doc, &mut p.meta, loc);
        p
    }

    /// Restores the process working directory on every exit path (Go t.Chdir).
    struct CwdGuard(std::path::PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    #[test]
    fn test_resolve_schema_loc_relative() {
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join("config");
        let work_dir = root.path().join("work");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        let cfg = config_dir.join("cfg.json");
        let cfg = cfg.to_str().unwrap();
        let write_schema = |path: &Path| {
            std::fs::write(path, br#"{"type":"object"}"#).unwrap();
        };

        let _cwd = CwdGuard(std::env::current_dir().unwrap());
        std::env::set_current_dir(&work_dir).unwrap();

        // config directory wins
        write_schema(&config_dir.join("schema.json"));
        write_schema(&work_dir.join("schema.json"));
        let sess = session_with_schema(cfg, "schema.json");
        let got = resolve_schema_loc("", cfg, doc::codec_for(cfg), &sess.doc, &sess.meta).unwrap();
        assert_eq!(got, config_dir.join("schema.json").to_str().unwrap());

        // falls back to cwd
        write_schema(&work_dir.join("fallback.json"));
        let sess = session_with_schema(cfg, "fallback.json");
        let got = resolve_schema_loc("", cfg, doc::codec_for(cfg), &sess.doc, &sess.meta).unwrap();
        assert_eq!(got, "fallback.json");
        let abs = go_abs(&got);
        assert_eq!(abs, work_dir.join("fallback.json").to_str().unwrap());
    }

    #[test]
    fn test_search_stem() {
        let cases = [
            ("server.json", "server"),
            ("a.b.json", "a.b"),
            (".cshrc", ".cshrc"), // Ext(".cshrc") is the whole name
            ("noext", "noext"),
        ];
        for (input, want) in cases {
            assert_eq!(search_stem(input), want, "search_stem({input:?})");
        }
    }

    #[test]
    fn test_parse_args() {
        fn parse(input: &[&str]) -> Result<Invocation, String> {
            let args: Vec<String> = input.iter().map(|s| s.to_string()).collect();
            super::parse_args(&args)
        }
        fn edit(
            schema: &str,
            force: bool,
            accessible: bool,
            pick: bool,
            files: &[&str],
        ) -> Invocation {
            Invocation::Edit(Flags {
                schema: schema.to_string(),
                force,
                accessible,
                pick,
                files: files.iter().map(|s| s.to_string()).collect(),
            })
        }

        let cases: Vec<(&str, Vec<&str>, Result<Invocation, String>)> = vec![
            ("short version", vec!["-v"], Ok(Invocation::Version)),
            ("long version", vec!["--version"], Ok(Invocation::Version)),
            ("short help", vec!["-h"], Ok(Invocation::Help)),
            ("long help", vec!["--help"], Ok(Invocation::Help)),
            // The first terminal flag encountered wins.
            ("version then help", vec!["-vh"], Ok(Invocation::Version)),
            ("help then version", vec!["-hv"], Ok(Invocation::Help)),
            (
                "version ignores files",
                vec!["a.json", "-v"],
                Ok(Invocation::Version),
            ),
            (
                "schema separate",
                vec!["-s", "a", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "schema attached",
                vec!["-sa", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "short equals compatibility",
                vec!["-s=a", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "long equals",
                vec!["--schema=a", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "long separate",
                vec!["--schema", "a", "b.json"],
                Ok(edit("a", false, false, false, &["b.json"])),
            ),
            (
                "flags after path",
                vec!["b.json", "-f", "-a"],
                Ok(edit("", true, true, false, &["b.json"])),
            ),
            (
                "short cluster",
                vec!["-fa", "b.json"],
                Ok(edit("", true, true, false, &["b.json"])),
            ),
            (
                "full short cluster",
                vec!["-pfa", "b.json"],
                Ok(edit("", true, true, true, &["b.json"])),
            ),
            (
                "repeated booleans",
                vec!["-ff", "--force", "b.json"],
                Ok(edit("", true, false, false, &["b.json"])),
            ),
            (
                "double dash terminator",
                vec!["--", "-x.json"],
                Ok(edit("", false, false, false, &["-x.json"])),
            ),
            (
                "double dash stops options",
                vec!["--", "-v", "-h"],
                Ok(edit("", false, false, false, &["-v", "-h"])),
            ),
            (
                "lone dash is an operand",
                vec!["-", "b.json", "--force"],
                Ok(edit("", true, false, false, &["-", "b.json"])),
            ),
            (
                "schema value may start with dash",
                vec!["--schema", "-x"],
                Ok(edit("-x", false, false, false, &[])),
            ),
            (
                "schema consumes double dash",
                vec!["-s", "--"],
                Ok(edit("--", false, false, false, &[])),
            ),
            (
                "missing short schema value",
                vec!["-s"],
                Err("option requires an argument -- 's'".to_string()),
            ),
            (
                "missing long schema value",
                vec!["--schema"],
                Err("option '--schema' requires an argument".to_string()),
            ),
            (
                "unknown short",
                vec!["-x"],
                Err("invalid option -- 'x'".to_string()),
            ),
            (
                "unknown long",
                vec!["--unknown"],
                Err("unrecognized option '--unknown'".to_string()),
            ),
            (
                "single-dash long is rejected",
                vec!["-force"],
                Err("invalid option -- 'o'".to_string()),
            ),
            (
                "old --s spelling is rejected",
                vec!["--s"],
                Err("unrecognized option '--s'".to_string()),
            ),
            (
                "boolean long takes no value",
                vec!["--force=false"],
                Err("option '--force' doesn't allow an argument".to_string()),
            ),
            (
                "help takes no value",
                vec!["--help=x"],
                Err("option '--help' doesn't allow an argument".to_string()),
            ),
        ];
        for (name, input, want) in cases {
            assert_eq!(parse(&input), want, "{name}");
        }
    }

    #[test]
    fn test_version_line_formats() {
        assert_eq!(
            version::version_line_from("v0.2.1", "b2b73a4", "2026-10-18T13:18:54Z"),
            "jse v0.2.1 (commit b2b73a4, built 2026-10-18T13:18:54Z)"
        );
        assert_eq!(
            version::version_line_from("v0.2.1", "none", "unknown"),
            "jse v0.2.1"
        );
        assert_eq!(
            version::version_line_from("dev", "63d1088", "unknown"),
            "jse dev (commit 63d1088)"
        );
    }
}
