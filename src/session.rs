//! Session assembly: parse the config, resolve its schema, compile the form
//! model and wire the save path. The UI never loads schemas itself.

use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::doc::{Codec, Doc, Meta};
use crate::paths::{
    abs_lexical, dir_lexical, file_base, join_lexical, search_stem, split_dir_file,
};
use crate::ui::{FileSession, Options};
use crate::{catalog, doc, fetch, icons, smodel, ui, validate};

const ERR_NO_SCHEMA: &str =
    "no schema found: provide -s or record the schema location in the config";

const NOTE_SCHEMA_LESS: &str =
    "no schema found: editing without schema guidance; save-time validation disabled";

pub(crate) type SaveFn = Arc<dyn Fn(&Doc, &Meta) -> Result<(), String> + Send + Sync>;
pub(crate) type WriteToFn = Arc<dyn Fn(&str, &Doc, &Meta) -> Result<(), String> + Send + Sync>;

/// Why a file could not be opened. The exit code is a CLI concern, not an
/// error value the UI ever sees.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LoadError {
    /// Invalid invocation (exit 2).
    #[error("{0}")]
    Usage(String),
    /// Environment/runtime failure (exit 1).
    #[error("{0}")]
    Runtime(String),
}

impl LoadError {
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            LoadError::Usage(_) => 2,
            LoadError::Runtime(_) => 1,
        }
    }
}

/// Sentinel for Go's errNoSchema (the only error resolveSchemaLoc returns).
#[derive(Debug)]
pub(crate) struct NoSchema;

/// loadSession builds one file's editor session: load the document (missing
/// file starts empty), resolve the schema (-s / the file's own pointer /
/// catalog, --pick forces the picker per file), compile the validator, wire
/// save/write-to and the raw codec. Err is an exit status after the error was
/// printed.
pub(crate) fn load_session(
    config: &str,
    schema_flag: &str,
    force: bool,
    pick: bool,
    accessible: bool,
) -> Result<FileSession, LoadError> {
    if !http_prefix(config).is_empty() {
        return Err(LoadError::Usage(
            "config must be a local file path".to_string(),
        ));
    }

    let codec = doc::codec_for(config);

    // Load config; a missing file starts empty and is created on save.
    let (mut parsed, loaded) = match std::fs::read(config) {
        Ok(data) => match codec.parse(&String::from_utf8_lossy(&data)) {
            Ok(p) => (p, true),
            Err(e) => {
                return Err(LoadError::Runtime(format!("parse config {config}: {e}")));
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (codec.empty(), false),
        Err(e) => {
            return Err(LoadError::Runtime(format!("read config {config}: {e}")));
        }
    };

    let mut status: Vec<String> = Vec::new(); // warnings, shown for this file's session
    let mut startup: Vec<String> = codec.notices(&parsed.doc, &parsed.meta); // info notices, hidden after the first key press

    // The location is recorded after the schema loads: YAML chooses between
    // a top-level "$schema" member and the directive comment based on whether
    // the schema itself declares that property.
    let mut record_loc: Option<String> = if !pick && !schema_flag.is_empty() {
        Some(schema_flag.to_string())
    } else {
        None
    };
    // A catalog pick must overwrite an existing location, so drop it first.
    if pick {
        codec.clear_schema_location(&mut parsed.doc, &mut parsed.meta);
    }

    let mut schema_loc: Option<String> = None;
    if !pick {
        if let Ok(loc) = resolve_schema_loc(schema_flag, config, codec, &parsed.doc, &parsed.meta) {
            schema_loc = Some(loc);
        }
    }

    // With no schema source the file is still editable: the catalog is the
    // last chance to find one, and any failure — or cancelling the automatic
    // picker — just drops into schema-less editing. An explicit --pick stays
    // authoritative (its failures are errors).
    if pick || schema_loc.is_none() {
        let loc = catalog::loc();
        match catalog::load(&loc) {
            Ok(entries) => {
                let note = fetch::note(&loc);
                if !note.is_empty() {
                    status.push(note.replacen("cached schema", "cached catalog", 1));
                }
                let mut chosen: Option<catalog::Entry> = None;
                if !pick {
                    let abs = abs_lexical(config); // Match needs the absolute path
                    let m = catalog::match_entries(&entries, &abs);
                    if m.len() == 1 {
                        chosen = m.into_iter().next();
                    }
                }
                let chosen = match chosen {
                    Some(c) => Some(c),
                    None => {
                        if !accessible && !std::io::stdin().is_terminal() {
                            if pick {
                                return Err(LoadError::Usage(
                                    "schema selection needs a terminal; rerun with --accessible or provide -s"
                                        .to_string(),
                                ));
                            }
                            None
                        } else {
                            let base = file_base(config);
                            match ui::schema_picker::pick(
                                &entries,
                                &ui::schema_picker::PickOptions {
                                    accessible,
                                    filename: base.to_string(),
                                    query: search_stem(base),
                                },
                            ) {
                                Some(c) => Some(c),
                                None => {
                                    // Automatic lookup: cancelling the picker
                                    // is a valid "no schema" answer, so the
                                    // file still opens. --pick asked for a
                                    // schema explicitly and stays fatal.
                                    if pick {
                                        return Err(LoadError::Usage(ERR_NO_SCHEMA.to_string()));
                                    }
                                    None
                                }
                            }
                        }
                    }
                };
                if let Some(chosen) = chosen {
                    let loc = chosen.url.clone();
                    startup.push(format!("schema from SchemaStore: {}", chosen.name));
                    record_loc = Some(loc.clone());
                    schema_loc = Some(loc);
                }
            }
            Err(cerr) => {
                if pick {
                    return Err(LoadError::Runtime(format!("load schema catalog: {cerr}")));
                }
                status.push(format!("schema catalog unavailable: {cerr}"));
            }
        }
    }

    let (model, check, load_errors): (smodel::Schema, Option<ui::ValidateFn>, Vec<String>) =
        match &schema_loc {
            Some(loc) => {
                let compiled = match compile_schema(loc) {
                    Ok(m) => m,
                    Err(e) => {
                        return Err(LoadError::Runtime(format!("load schema {loc}: {e}")));
                    }
                };
                if !compiled.status.is_empty() {
                    status.push(compiled.status.clone());
                }
                let load_errors = if loaded {
                    compiled
                        .validate
                        .as_ref()
                        .map(|v| v(&parsed.doc))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                (compiled.schema, compiled.validate, load_errors)
            }
            None => {
                status.push(format!("{} {NOTE_SCHEMA_LESS}", icons::WARN));
                (smodel::untyped(&parsed.doc), None, Vec::new())
            }
        };

    // Keep the saved config self-describing. YAML uses the document's
    // "$schema" field when the schema declares one; otherwise it keeps the
    // yaml-language-server comment convention.
    if let Some(loc) = &record_loc {
        let has_member = smodel::root_has_property(&model, "$schema");
        codec.set_schema_location_with(&mut parsed.doc, &mut parsed.meta, loc, has_member);
    }
    let root = model.root;

    let opts = Options {
        accessible,
        force,
        validate: check,
        save: {
            let path = config.to_string();
            Some(
                Arc::new(move |doc: &Doc, meta: &Meta| write_session(codec, doc, meta, &path))
                    as SaveFn,
            )
        },
        path: config.to_string(),
        write_to: Some(Arc::new(move |path: &str, doc: &Doc, meta: &Meta| {
            write_session(codec, doc, meta, path)
        }) as WriteToFn),
        copy: None,
        paste: None,
        load_schema: Some(Arc::new(|loc: &str| compile_schema(loc))),
        cmd_history_file: fetch::cache_file("cmd-history"),
        status: status.join("\n"),
        startup_notice: startup.join("\n"),
        load_errors,
    };

    Ok(FileSession {
        schema: model,
        root,
        doc: parsed.doc,
        meta: parsed.meta,
        codec,
        opts,
    })
}
/// Compiles one schema location into the session shape both the CLI and the
/// S-key picker install: form tree, validator, status lines.
fn compile_schema(loc: &str) -> Result<ui::LoadedSchema, String> {
    let schema = smodel::load(loc).map_err(|e| e.to_string())?;
    let mut status = fetch::note(loc);
    let validate = match validate::new(loc) {
        Err(e) => {
            if !status.is_empty() {
                status.push('\n');
            }
            status.push_str(&format!(
                "{} schema compile failed, save-time validation disabled: {}",
                icons::WARN,
                e.replace('\n', " ")
            ));
            None
        }
        Ok(v) => {
            let v = Arc::new(v);
            Some(Arc::new(move |doc: &Doc| v.check(doc)) as ui::ValidateFn)
        }
    };
    let schema_member = smodel::root_has_property(&schema, "$schema");
    Ok(ui::LoadedSchema {
        schema,
        validate,
        status,
        schema_member,
    })
}
/// write_session renders the live document in the file's own format (with
/// the session's current meta) and lands it atomically.
fn write_session(
    codec: &'static dyn Codec,
    doc: &Doc,
    meta: &Meta,
    path: &str,
) -> Result<(), String> {
    let text = codec.write(doc, meta).map_err(|e| e.to_string())?;
    atomic_write(path, &text)
}
fn http_prefix(p: &str) -> &str {
    if p.starts_with("http://") || p.starts_with("https://") {
        p
    } else {
        ""
    }
}
/// resolveSchemaLoc implements the schema source resolution order.
pub(crate) fn resolve_schema_loc(
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
            let from_config = join_lexical(&dir_lexical(c_flag), &u);
            return match std::fs::metadata(&from_config) {
                Ok(_) => Ok(from_config),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(u),
                Err(_) => Ok(from_config),
            };
        }
    }
    Err(NoSchema)
}
/// atomicWrite writes a document to path via a same-directory temp file +
/// rename, preserving the mode of an existing file.
fn atomic_write(path: &str, contents: &str) -> Result<(), String> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    let mut mode: u32 = 0o644;
    #[cfg(unix)]
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
        #[cfg(unix)]
        {
            let mut perms = f.metadata()?.permissions();
            perms.set_mode(mode);
            f.set_permissions(perms)?;
        }
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
