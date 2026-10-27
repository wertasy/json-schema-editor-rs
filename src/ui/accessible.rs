//! `--accessible` line protocol: sequential screen-driven sessions over
//! stdin/stdout. Rust port of Go `internal/ui/screens.go` + the accessible
//! branch of `app.go`'s Run, reproducing huh v1.0.0's accessible rendering
//! byte for byte on a pipe (plain text, no ANSI):
//!
//! - input/text fields print `Title ` (trailing space) and read one line;
//!   an invalid line prints the validator error and re-prompts;
//! - select fields print `Title `, one `N. label` line per option, then
//!   `Enter a number between 1 and N: ` (or `There is only one option
//!   available; enter the number 1:`); a blank line accepts the default
//!   (the pre-selected option);
//! - multiselect fields print `Title `, `Select up to N options.`, the
//!   numbered options (`2. ✓ info` selected, `2.   info` not, then
//!   `0.   Confirm selection`), and loop on `Enter a number between 0 and
//!   N: ` until 0 confirms;
//! - confirm fields print `Title [y/N] ` / `Title [Y/n] ` and read one
//!   line (y/yes/n/no, blank = default);
//! - a note prints its title (and description), no input;
//! - the form wrapper prints one blank line after every field.
//!
//! EOF at a prompt accepts the bound default (huh's PromptString breaks out
//! with the default at EOF, terminating the prompt line); the screen loops
//! check `LineReader::eof_seen()` like Go's eofResult and pop/abort there.

use std::io::{BufRead, Write};

use crate::doc::jsondoc::{CopyMode, Doc, PathSeg, Value};
use crate::doc::{append_idx, append_key};
use crate::smodel::{FormNodeId, Kind, Schema};

use crate::ui::lineio::LineReader;
use crate::ui::rows::display_title;
use crate::ui::widgets::{apply_scalar, omit_or_set, validate_input};
use crate::ui::{FileSession, Options, Outcome, Res};

/// run_accessible drives one sequential accessible session per file over
/// the real stdin/stdout (Go Run's accessible branch).
pub fn run_accessible(files: Vec<FileSession>) -> Outcome {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    run_io(files, stdin.lock(), &mut out)
}

/// run_io is run_accessible over injected input/output, so the e2e scripts
/// can be replayed in-process.
fn run_io<R: BufRead, W: Write>(files: Vec<FileSession>, input: R, out: &mut W) -> Outcome {
    if files.is_empty() {
        return Outcome {
            err: Some("no files to edit".to_string()),
            ..Default::default()
        };
    }
    let mut reader = LineReader::new(input);
    let mut saved_paths: Vec<String> = Vec::new();
    let mut saved = false;
    let mut forced = false;
    for f in files {
        let FileSession {
            schema,
            root,
            mut doc,
            mut meta,
            codec,
            opts,
        } = f;
        let mut s = Sess {
            schema: &schema,
            doc: &mut doc,
            meta: &mut meta,
            codec,
            opts: &opts,
            saved_with_errors: false,
            save_err: None,
        };
        let r = match s.schema.nodes[root as usize].kind {
            Kind::Object => section(&mut s, root, &[], true, &mut reader, out),
            Kind::Array => array_manager(&mut s, root, &[], true, &mut reader, out),
            Kind::OneOf => variant_root(&mut s, root, &mut reader, out),
            _ => simple_root(&mut s, root, &mut reader, out),
        };
        if let Some(e) = s.save_err.take() {
            return Outcome {
                err: Some(e),
                saved_files: saved_paths,
                ..Default::default()
            };
        }
        if r != Res::Saved {
            return Outcome {
                saved,
                with_errors: forced,
                aborted: true,
                saved_files: saved_paths,
                ..Default::default()
            };
        }
        saved = true;
        if s.saved_with_errors {
            forced = true;
        }
        if !opts.path.is_empty() {
            saved_paths.push(opts.path.clone());
        }
    }
    Outcome {
        saved,
        with_errors: forced,
        saved_files: saved_paths,
        ..Default::default()
    }
}

// --- session ---------------------------------------------------------------------

/// Sess is Go's app, scoped to one file: the form model, live document,
/// options, and the save bookkeeping saveFlow reports out through.
struct Sess<'a> {
    schema: &'a Schema,
    doc: &'a mut Doc,
    meta: &'a mut crate::doc::Meta,
    codec: &'static dyn crate::doc::Codec,
    opts: &'a Options,
    /// a --force save that skipped validation errors
    saved_with_errors: bool,
    /// a failed Save write carried out of save_flow
    save_err: Option<String>,
}

/// One navigation choice of a select screen (Go navOpt).
struct NavOpt {
    key: String,
    label: String,
}

/// eofResult aborts on the root screen and pops otherwise (Go app.go).
fn eof_result<R: BufRead>(reader: &LineReader<R>, root: bool) -> (Res, bool) {
    if !reader.eof_seen() {
        return (Res::Pop, false);
    }
    if root {
        (Res::Abort, true)
    } else {
        (Res::Pop, true)
    }
}

// --- huh accessible prompts (internal/accessibility.go) --------------------------

/// PromptString: print the prompt, read a line, re-ask on a validator error;
/// EOF breaks the loop (after terminating the prompt line) and the trimmed
/// input or the default is returned.
fn prompt_string<R: BufRead, W: Write>(
    out: &mut W,
    reader: &mut LineReader<R>,
    prompt: &str,
    default: &str,
    mut validate: impl FnMut(&str) -> Result<(), String>,
) -> String {
    let mut input = String::new();
    loop {
        let _ = write!(out, "{prompt}");
        let _ = out.flush();
        match reader.read_line() {
            Some(line) => {
                input = line;
                if let Err(e) = validate(&input) {
                    let _ = writeln!(out, "{e}");
                    continue;
                }
                break;
            }
            None => {
                let _ = writeln!(out);
                break;
            }
        }
    }
    let t = input.trim();
    if t.is_empty() {
        default.to_string()
    } else {
        t.to_string()
    }
}

/// Go accessibility.atoi: an empty (whitespace) string is -1, not an error.
fn parse_int_default(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(-1);
    }
    t.parse().ok()
}

/// PromptInt: a validated integer between low and high; a blank line takes
/// the default when one is bound.
fn prompt_int<R: BufRead, W: Write>(
    out: &mut W,
    reader: &mut LineReader<R>,
    prompt: &str,
    low: i64,
    high: i64,
    default: Option<i64>,
) -> i64 {
    let default_str = default.map(|d| d.to_string());
    let res = prompt_string(
        out,
        reader,
        prompt,
        default_str.as_deref().unwrap_or(""),
        |s| {
            if s.trim().is_empty() && default_str.is_some() {
                return Ok(());
            }
            match parse_int_default(s) {
                Some(v) if v >= low && v <= high => Ok(()),
                _ => Err(if low == high {
                    format!("Invalid: must be {low}")
                } else {
                    format!("Invalid: must be a number between {low} and {high}")
                }),
            }
        },
    );
    res.trim().parse().unwrap_or(low)
}

/// PromptBool: y/yes/n/no (case-insensitive), blank = default.
fn prompt_bool<R: BufRead, W: Write>(
    out: &mut W,
    reader: &mut LineReader<R>,
    prompt: &str,
    default: bool,
) -> bool {
    parse_bool(&prompt_string(
        out,
        reader,
        prompt,
        if default { "y" } else { "N" },
        |s| {
            if s.trim().is_empty() {
                return Ok(());
            }
            parse_bool(s).map(|_| ()).map_err(|e| e)
        },
    ))
    .unwrap_or(false)
}

/// Go accessibility.parseBool: "" is an error (the default covers it).
fn parse_bool(s: &str) -> Result<bool, String> {
    let s = s.trim().to_lowercase();
    match s.as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => Err("invalid input. please try again".to_string()),
    }
}

/// The blank line huh's accessible form prints after every field.
fn field_end<W: Write>(out: &mut W) {
    let _ = writeln!(out);
}

// --- huh accessible fields --------------------------------------------------------

/// Input field: prompt `Title `, one line, re-ask on validation error.
fn run_input<R: BufRead, W: Write>(
    out: &mut W,
    reader: &mut LineReader<R>,
    title: &str,
    default: &str,
    validate: Option<&dyn Fn(&str) -> Result<(), String>>,
) -> String {
    let v = prompt_string(
        out,
        reader,
        &format!("{title} "),
        default,
        |s| match validate {
            Some(f) => f(s),
            None => Ok(()),
        },
    );
    field_end(out);
    v
}

/// Select field: title line, numbered options, number prompt. Returns the
/// chosen option index. A blank line takes the default (pre-selected) index.
fn run_select<R: BufRead, W: Write>(
    out: &mut W,
    reader: &mut LineReader<R>,
    title: &str,
    labels: &[String],
    default_idx: usize,
) -> usize {
    let _ = writeln!(out, "{title} ");
    for (i, label) in labels.iter().enumerate() {
        let _ = writeln!(out, "{}. {}", i + 1, label);
    }
    let prompt = if labels.len() == 1 {
        "There is only one option available; enter the number 1:".to_string()
    } else {
        format!("Enter a number between 1 and {}: ", labels.len())
    };
    let choice = prompt_int(
        out,
        reader,
        &prompt,
        1,
        labels.len() as i64,
        Some(default_idx as i64 + 1),
    );
    field_end(out);
    (choice - 1) as usize
}

/// MultiSelect field: returns the selected option indexes in option order.
/// `min` carries the field validator (select at least N).
fn run_multiselect<R: BufRead, W: Write>(
    out: &mut W,
    reader: &mut LineReader<R>,
    title: &str,
    labels: &[String],
    selected: &[bool],
    min: Option<usize>,
) -> Vec<usize> {
    let _ = writeln!(out, "{title} ");
    let _ = writeln!(out, "Select up to {} options.", labels.len());
    let mut selected = selected.to_vec();
    let prompt = format!("Enter a number between 0 and {}: ", labels.len());
    loop {
        for (i, label) in labels.iter().enumerate() {
            if selected[i] {
                let _ = writeln!(out, "{}. ✓ {}", i + 1, label);
            } else {
                let _ = writeln!(out, "{}.   {}", i + 1, label);
            }
        }
        let _ = writeln!(out, "0.   Confirm selection");
        let choice = prompt_int(out, reader, &prompt, 0, labels.len() as i64, None);
        if choice <= 0 {
            let sel: Vec<usize> = (0..labels.len()).filter(|i| selected[*i]).collect();
            if let Some(min) = min {
                if sel.len() < min {
                    let _ = writeln!(out, "select at least {min}");
                    continue;
                }
            }
            field_end(out);
            return sel;
        }
        selected[(choice - 1) as usize] = !selected[(choice - 1) as usize];
        let _ = writeln!(out);
    }
}

/// Confirm field: prompt `Title [y/N] ` / `Title [Y/n] `.
fn run_confirm<R: BufRead, W: Write>(
    out: &mut W,
    reader: &mut LineReader<R>,
    title: &str,
    default: bool,
) -> bool {
    let opts = if default { "[Y/n]" } else { "[y/N]" };
    let b = prompt_bool(out, reader, &format!("{title} {opts} "), default);
    field_end(out);
    b
}

/// Note field: prints its title (and description), reads nothing.
fn run_note<W: Write>(out: &mut W, title: &str, description: &str) {
    if !title.is_empty() {
        let _ = writeln!(out, "{title}");
    }
    if !description.is_empty() {
        let _ = writeln!(out, "{description}");
    }
    field_end(out);
}

/// choose runs a single-select screen and returns the chosen key (Go
/// app.go choose; the accessible runForm never errors, so the caller's
/// abort branch is unreachable and this always resolves).
fn choose<R: BufRead, W: Write>(
    out: &mut W,
    reader: &mut LineReader<R>,
    title: &str,
    opts: &[NavOpt],
    def: &str,
) -> String {
    let labels: Vec<String> = opts.iter().map(|o| o.label.clone()).collect();
    let default_idx = opts.iter().position(|o| o.key == def).unwrap_or(0);
    let idx = run_select(out, reader, title, &labels, default_idx);
    opts[idx].key.clone()
}

// --- entries (Go widgets.go scalarEntry/enumEntry/boolEntry/multiEnumEntry/rawEntry)

/// One answered form field, applied to the doc after the whole form
/// completes (Go entries are closures; the data enum carries their state).
enum Entry {
    Scalar {
        fn_id: FormNodeId,
        path: Vec<PathSeg>,
        val: String,
    },
    Enum {
        fn_id: FormNodeId,
        path: Vec<PathSeg>,
        val: String,
    },
    Bool {
        path: Vec<PathSeg>,
        val: bool,
    },
    MultiEnum {
        fn_id: FormNodeId,
        path: Vec<PathSeg>,
        sel: Vec<String>,
    },
    Raw {
        fn_id: FormNodeId,
        path: Vec<PathSeg>,
        text: String,
    },
}

impl Entry {
    fn apply(&self, s: &mut Sess<'_>) {
        let schema = s.schema;
        let doc: &mut Doc = s.doc;
        match self {
            Entry::Scalar { fn_id, path, val } => {
                let f = &schema.nodes[*fn_id as usize];
                let _ = apply_scalar(doc, f, &schema.doc, path, val);
            }
            Entry::Enum { fn_id, path, val } => {
                let f = &schema.nodes[*fn_id as usize];
                let val_id = f.enum_.iter().find(|o| o.encoded == *val).map(|o| o.val);
                let _ = omit_or_set(
                    doc,
                    path,
                    val.is_empty(),
                    f,
                    &schema.doc,
                    |doc| match val_id {
                        Some(id) => doc.copy_into(&schema.doc, id, CopyMode::Preserve),
                        None => doc.new_null(),
                    },
                );
            }
            Entry::Bool { path, val } => {
                let v = doc.new_bool(*val);
                let _ = doc.set(path, v);
            }
            Entry::MultiEnum { fn_id, path, sel } => {
                let f = &schema.nodes[*fn_id as usize];
                if sel.is_empty() && !f.required {
                    doc.remove(path);
                    return;
                }
                let options = &schema.nodes[f.items.unwrap() as usize].enum_;
                let mut items = Vec::with_capacity(sel.len());
                for enc in sel {
                    if let Some(o) = options.iter().find(|o| &o.encoded == enc) {
                        items.push(doc.copy_into(&schema.doc, o.val, CopyMode::Preserve));
                    }
                }
                let arr = doc.new_array();
                doc.node_mut(arr).value = Value::Array(items);
                let _ = doc.set(path, arr);
            }
            Entry::Raw { fn_id, path, text } => {
                let f = &schema.nodes[*fn_id as usize];
                let s_ = text.trim();
                if s_.is_empty() {
                    let _ = omit_or_set(doc, path, true, f, &schema.doc, |d| d.root);
                    return;
                }
                match s.codec.parse_value(s_) {
                    Ok(d2) => {
                        let _ = omit_or_set(doc, path, false, f, &schema.doc, |doc| {
                            doc.copy_into(&d2, d2.root, CopyMode::Preserve)
                        });
                    }
                    Err(_) => {} // unreachable: the prompt validated the text
                }
            }
        }
    }
}

/// scalarEntry asks one text input; the returned Entry applies it.
fn scalar_entry<R: BufRead, W: Write>(
    s: &Sess<'_>,
    fn_id: FormNodeId,
    path: &[PathSeg],
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Entry {
    let f = &s.schema.nodes[fn_id as usize];
    let cur = s.doc.get(path);
    let mut bind = cur
        .and_then(|id| s.doc.scalar_text(id))
        .unwrap_or_default()
        .to_string();
    if bind.is_empty() {
        if let Some(d) = f.default_ {
            bind = s.schema.doc.scalar_text(d).unwrap_or_default().to_string();
        }
    }
    let has_validator = !f.json_type.is_empty()
        || f.required
        || f.min.is_some()
        || f.max.is_some()
        || f.min_len.is_some()
        || f.max_len.is_some()
        || !f.pattern.is_empty()
        || f.multiple_of.is_some();
    let fnode = f.clone();
    let validator = if has_validator {
        Some(move |s: &str| validate_input(&fnode, s) as Result<(), String>)
    } else {
        None
    };
    let val = run_input(
        out,
        reader,
        &f.title,
        &bind,
        validator
            .as_ref()
            .map(|f| f as &dyn Fn(&str) -> Result<(), String>),
    );
    Entry::Scalar {
        fn_id,
        path: path.to_vec(),
        val,
    }
}

/// enumEntry asks one select over the enum options (leading "(none)" for
/// optional fields).
fn enum_entry<R: BufRead, W: Write>(
    s: &Sess<'_>,
    fn_id: FormNodeId,
    path: &[PathSeg],
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Entry {
    let f = &s.schema.nodes[fn_id as usize];
    let mut bind = String::new();
    if let Some(cur) = s.doc.get(path) {
        if !matches!(s.doc.value(cur), Value::Null) {
            let enc = s.doc.encode_value(cur);
            if f.enum_.iter().any(|o| o.encoded == enc) {
                bind = enc;
            }
        }
    }
    if bind.is_empty() {
        if let Some(d) = f.default_ {
            let enc = s.schema.doc.encode_value(d);
            if !enc.is_empty() {
                bind = enc;
            }
        }
    }
    let mut labels = Vec::with_capacity(f.enum_.len() + 1);
    let mut encodings = Vec::with_capacity(f.enum_.len() + 1);
    if !f.required {
        labels.push("(none)".to_string());
        encodings.push(String::new());
    }
    for o in &f.enum_ {
        labels.push(o.display.clone());
        encodings.push(o.encoded.clone());
    }
    let default_idx = encodings.iter().position(|e| *e == bind).unwrap_or(0);
    let idx = run_select(out, reader, &f.title, &labels, default_idx);
    Entry::Enum {
        fn_id,
        path: path.to_vec(),
        val: encodings[idx].clone(),
    }
}

/// boolEntry asks one confirm.
fn bool_entry<R: BufRead, W: Write>(
    s: &Sess<'_>,
    fn_id: FormNodeId,
    path: &[PathSeg],
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Entry {
    let f = &s.schema.nodes[fn_id as usize];
    let mut b = false;
    if let Some(cur) = s.doc.get(path) {
        if let Value::Bool(v) = s.doc.value(cur) {
            b = *v;
        } else if let Some(d) = f.default_ {
            if let Value::Bool(v) = s.schema.doc.value(d) {
                b = *v;
            }
        }
    } else if let Some(d) = f.default_ {
        if let Value::Bool(v) = s.schema.doc.value(d) {
            b = *v;
        }
    }
    let val = run_confirm(out, reader, &f.title, b);
    Entry::Bool {
        path: path.to_vec(),
        val,
    }
}

/// multiEnumEntry edits arrays whose items are enums as one multiselect.
fn multi_enum_entry<R: BufRead, W: Write>(
    s: &Sess<'_>,
    fn_id: FormNodeId,
    path: &[PathSeg],
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Entry {
    let f = &s.schema.nodes[fn_id as usize];
    let options = &s.schema.nodes[f.items.unwrap() as usize].enum_;
    let mut bind: Vec<String> = Vec::new();
    if let Some(cur) = s.doc.get(path) {
        if let Value::Array(items) = s.doc.value(cur) {
            for it in items {
                bind.push(s.doc.encode_value(*it));
            }
        }
    }
    let labels: Vec<String> = options.iter().map(|o| o.display.clone()).collect();
    let selected: Vec<bool> = options.iter().map(|o| bind.contains(&o.encoded)).collect();
    let min = if f.required || f.min_items >= 1 {
        Some(f.min_items.max(1) as usize)
    } else {
        None
    };
    let sel_idx = run_multiselect(out, reader, &f.title, &labels, &selected, min);
    let sel: Vec<String> = sel_idx
        .into_iter()
        .map(|i| options[i].encoded.clone())
        .collect();
    Entry::MultiEnum {
        fn_id,
        path: path.to_vec(),
        sel,
    }
}

/// rawEntry edits one subtree as raw text in the codec's syntax.
fn raw_entry<R: BufRead, W: Write>(
    s: &Sess<'_>,
    fn_id: FormNodeId,
    path: &[PathSeg],
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Entry {
    let f = &s.schema.nodes[fn_id as usize];
    let label = s.codec.label();
    let mut prefill = String::new();
    if let Some(cur) = s.doc.get(path) {
        if !matches!(s.doc.value(cur), Value::Null) {
            if let Ok(t) = s.codec.write_value(s.doc, cur) {
                prefill = t;
            }
        }
    }
    let required = f.required;
    let codec = s.codec;
    let text = run_input(
        out,
        reader,
        &f.title,
        &prefill,
        Some(&move |s: &str| {
            let t = s.trim();
            if t.is_empty() {
                if required {
                    return Err("required".to_string());
                }
                return Ok(());
            }
            match codec.parse_value(t) {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("invalid {label}: {e}")),
            }
        }),
    );
    Entry::Raw {
        fn_id,
        path: path.to_vec(),
        text,
    }
}

/// editSingle asks one entry's field and applies it. Go returns false only
/// on a form error, which the accessible mode never raises.
fn edit_single<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    fn_id: FormNodeId,
    path: &[PathSeg],
    reader: &mut LineReader<R>,
    out: &mut W,
) -> bool {
    let e = match s.schema.nodes[fn_id as usize].kind {
        Kind::Scalar => scalar_entry(s, fn_id, path, reader, out),
        Kind::Enum => enum_entry(s, fn_id, path, reader, out),
        Kind::Boolean => bool_entry(s, fn_id, path, reader, out),
        _ => raw_entry(s, fn_id, path, reader, out),
    };
    e.apply(s);
    true
}

// --- screens (Go screens.go) -------------------------------------------------------

/// section renders one object screen: its scalar fields plus a trailing
/// Navigate select for children. It re-renders after every drill-down until
/// the user goes back, saves, or aborts.
fn section<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    node: FormNodeId,
    path: &[PathSeg],
    root: bool,
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Res {
    loop {
        let (r, eof) = eof_result(reader, root);
        if eof {
            return r;
        }
        let n = &s.schema.nodes[node as usize];
        let mut entries: Vec<Entry> = Vec::new();
        let mut navs: Vec<NavOpt> = Vec::new();

        if root && (!s.opts.status.is_empty() || !s.opts.startup_notice.is_empty()) {
            // info notices (StartupNotice) lead, warnings follow; the
            // accessible flow has no persistent status area, so the root
            // note carries both
            let note = format!("{}\n{}", s.opts.startup_notice, s.opts.status)
                .trim()
                .to_string();
            run_note(out, &note, "");
        }
        let children = n.children.clone();
        for cid in children {
            let cpath = append_key(path, &s.schema.nodes[cid as usize].key);
            let c = &s.schema.nodes[cid as usize];
            match c.kind {
                Kind::Scalar => {
                    entries.push(scalar_entry(s, cid, &cpath, reader, out));
                }
                Kind::Enum => {
                    entries.push(enum_entry(s, cid, &cpath, reader, out));
                }
                Kind::Boolean => {
                    entries.push(bool_entry(s, cid, &cpath, reader, out));
                }
                Kind::Raw => {
                    entries.push(raw_entry(s, cid, &cpath, reader, out));
                }
                Kind::Array
                    if c.items.is_some()
                        && s.schema.nodes[c.items.unwrap() as usize].kind == Kind::Enum =>
                {
                    entries.push(multi_enum_entry(s, cid, &cpath, reader, out));
                }
                _ => {
                    // KObject / KArray / KOneOf → drill down
                    let label = if c.kind == Kind::Array {
                        format!("Edit items ({}) ▸", s.doc.array_len(&cpath))
                    } else {
                        format!("Edit {} ▸", c.title)
                    };
                    navs.push(NavOpt {
                        key: format!("c:{}", navs.len()),
                        label,
                    });
                }
            }
        }

        let (first, label) = if root {
            ("save", "Save and exit")
        } else {
            ("back", "Done (back)")
        };
        if root {
            navs.push(NavOpt {
                key: first.to_string(),
                label: label.to_string(),
            });
            navs.push(NavOpt {
                key: "raw".to_string(),
                label: format!("Edit raw {}", s.codec.label()),
            });
        } else {
            navs.insert(
                0,
                NavOpt {
                    key: first.to_string(),
                    label: label.to_string(),
                },
            );
        }

        if entries.is_empty() {
            run_note(out, "Nothing to edit here", "");
        }
        let labels: Vec<String> = navs.iter().map(|o| o.label.clone()).collect();
        let default_idx = navs.iter().position(|o| o.key == first).unwrap_or(0);
        let nav_idx = run_select(out, reader, "Navigate", &labels, default_idx);
        let nav = navs[nav_idx].key.clone();

        for e in &entries {
            e.apply(s);
        }

        if nav == "save" {
            if save_flow(s, reader, out) == Res::Saved {
                return Res::Saved;
            }
        } else if nav == "back" {
            return Res::Pop;
        } else if nav == "raw" {
            raw_editor(s, None, reader, out);
        } else if let Some(rest) = nav.strip_prefix("c:") {
            let idx: usize = rest.parse().unwrap_or(0);
            let mut nav_idx = 0;
            let mut child = None;
            for c in s.schema.nodes[node as usize].children.clone() {
                let k = s.schema.nodes[c as usize].kind;
                let is_enum_array = k == Kind::Array
                    && s.schema.nodes[c as usize]
                        .items
                        .map(|it| s.schema.nodes[it as usize].kind == Kind::Enum)
                        == Some(true);
                if k == Kind::Object || k == Kind::Array || k == Kind::OneOf {
                    if !is_enum_array {
                        if nav_idx == idx {
                            child = Some(c);
                            break;
                        }
                        nav_idx += 1;
                    }
                }
            }
            let Some(child) = child else { continue };
            let key = s.schema.nodes[child as usize].key.clone();
            let cpath = append_key(path, &key);
            let r = match s.schema.nodes[child as usize].kind {
                Kind::Object => section(s, child, &cpath, false, reader, out),
                Kind::Array => array_manager(s, child, &cpath, false, reader, out),
                _ => variant(s, child, &cpath, reader, out),
            };
            if r == Res::Saved {
                return Res::Saved;
            }
        }
    }
}

/// arrayManager renders the item manager for an array node.
fn array_manager<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    node: FormNodeId,
    path: &[PathSeg],
    root: bool,
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Res {
    loop {
        let (r, eof) = eof_result(reader, root);
        if eof {
            return r;
        }
        let n = s.doc.array_len(path);
        let mut opts = vec![NavOpt {
            key: "add".to_string(),
            label: "< Add item".to_string(),
        }];
        if n > 0 {
            if let Some(cur) = s.doc.get(path) {
                if let Value::Array(items) = s.doc.value(cur) {
                    for (i, it) in items.iter().enumerate() {
                        opts.push(NavOpt {
                            key: format!("i:{i}"),
                            label: format!("{}: {}", i, s.doc.summary(*it)),
                        });
                    }
                }
            }
            opts.push(NavOpt {
                key: "move-up".to_string(),
                label: "Move item up…".to_string(),
            });
            opts.push(NavOpt {
                key: "move-down".to_string(),
                label: "Move item down…".to_string(),
            });
            opts.push(NavOpt {
                key: "remove".to_string(),
                label: "Remove item…".to_string(),
            });
        }
        if root {
            opts.push(NavOpt {
                key: "save".to_string(),
                label: "Save and exit".to_string(),
            });
        } else {
            opts.push(NavOpt {
                key: "back".to_string(),
                label: "Done (back)".to_string(),
            });
        }

        let title = format!("{} items", s.schema.nodes[node as usize].title);
        let choice = choose(out, reader, &title, &opts, &opts[0].key.clone());
        if choice == "add" {
            edit_item(
                s,
                s.schema.nodes[node as usize].items,
                path,
                usize::MAX,
                reader,
                out,
            );
        } else if choice == "remove" {
            remove_item(s, path, n, reader, out);
        } else if choice == "move-up" {
            move_item(s, path, n, -1, reader, out);
        } else if choice == "move-down" {
            move_item(s, path, n, 1, reader, out);
        } else if choice == "save" {
            if save_flow(s, reader, out) == Res::Saved {
                return Res::Saved;
            }
        } else if choice == "back" {
            return Res::Pop;
        } else if let Some(rest) = choice.strip_prefix("i:") {
            let idx: usize = rest.parse().unwrap_or(0);
            edit_item(
                s,
                s.schema.nodes[node as usize].items,
                path,
                idx,
                reader,
                out,
            );
        }
    }
}

/// removeItem runs the two-step removal flow (pick item, confirm).
fn remove_item<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    path: &[PathSeg],
    _n: usize,
    reader: &mut LineReader<R>,
    out: &mut W,
) {
    let mut opts = Vec::new();
    if let Some(cur) = s.doc.get(path) {
        if let Value::Array(items) = s.doc.value(cur) {
            for (i, it) in items.iter().enumerate() {
                opts.push(NavOpt {
                    key: i.to_string(),
                    label: format!("{}: {}", i, s.doc.summary(*it)),
                });
            }
        }
    }
    if opts.is_empty() {
        return;
    }
    let choice = choose(out, reader, "Remove item", &opts, &opts[0].key.clone());
    let Ok(idx) = choice.parse::<usize>() else {
        return;
    };
    let confirm = run_confirm(out, reader, &format!("Remove item {idx}?"), true);
    if !confirm {
        return;
    }
    s.doc.remove(&append_idx(path, idx));
}

/// moveItem relocates one array element up (-1) or down (+1): pick the
/// item, then shift it. The first item moved up (or last moved down) is a
/// silent no-op: the manager simply re-renders unchanged.
fn move_item<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    path: &[PathSeg],
    _n: usize,
    delta: i64,
    reader: &mut LineReader<R>,
    out: &mut W,
) {
    let mut opts = Vec::new();
    if let Some(cur) = s.doc.get(path) {
        if let Value::Array(items) = s.doc.value(cur) {
            for (i, it) in items.iter().enumerate() {
                opts.push(NavOpt {
                    key: i.to_string(),
                    label: format!("{}: {}", i, s.doc.summary(*it)),
                });
            }
        }
    }
    if opts.is_empty() {
        return;
    }
    let dir = if delta > 0 { "down" } else { "up" };
    let choice = choose(
        out,
        reader,
        &format!("Move item {dir}"),
        &opts,
        &opts[0].key.clone(),
    );
    let Ok(idx) = choice.parse::<usize>() else {
        return;
    };
    let to = idx as i64 + delta;
    if to >= 0 {
        s.doc.move_(path, idx, to as usize);
    }
}

/// editItem edits array element idx (idx == usize::MAX appends a new one).
fn edit_item<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    items: Option<FormNodeId>,
    parent_path: &[PathSeg],
    idx: usize,
    reader: &mut LineReader<R>,
    out: &mut W,
) {
    let Some(items) = items else { return };
    let item_path = if idx == usize::MAX {
        append_idx(parent_path, s.doc.array_len(parent_path))
    } else {
        append_idx(parent_path, idx)
    };
    match s.schema.nodes[items as usize].kind {
        Kind::Object => {
            section(s, items, &item_path, false, reader, out);
        }
        Kind::Array => {
            array_manager(s, items, &item_path, false, reader, out);
        }
        Kind::OneOf => {
            variant(s, items, &item_path, reader, out);
        }
        _ => {
            edit_single(s, items, &item_path, reader, out);
        }
    }
}

/// variant renders the oneOf/anyOf variant picker followed by the chosen
/// variant's editor.
fn variant<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    node: FormNodeId,
    path: &[PathSeg],
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Res {
    let variants = s.schema.nodes[node as usize].variants.clone();
    let mut opts = Vec::with_capacity(variants.len());
    for (i, v) in variants.iter().enumerate() {
        opts.push(NavOpt {
            key: format!("v:{i}"),
            label: display_title(s.schema, *v),
        });
    }
    let title = s.schema.nodes[node as usize].title.clone();
    let choice = choose(out, reader, &title, &opts, "v:0");
    let Ok(idx) = choice.strip_prefix("v:").unwrap_or("0").parse::<usize>() else {
        return Res::Pop;
    };
    let v = variants[idx];
    match s.schema.nodes[v as usize].kind {
        Kind::Object => section(s, v, path, false, reader, out),
        Kind::Array => array_manager(s, v, path, false, reader, out),
        _ => {
            edit_single(s, v, path, reader, out);
            Res::Pop
        }
    }
}

/// rawEditor edits a whole subtree (None = entire document) in the raw
/// syntax selected by Options.Raw.
fn raw_editor<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    path: Option<&[PathSeg]>,
    reader: &mut LineReader<R>,
    out: &mut W,
) {
    let label = s.codec.label();
    let cur = match path {
        None => Some(s.doc.root),
        Some(p) => s.doc.get(p),
    };
    let mut prefill = String::new();
    if let Some(cur) = cur {
        if !matches!(s.doc.value(cur), Value::Null) {
            let r = match path {
                None => s.codec.write(s.doc, s.meta),
                Some(_) => s.codec.write_value(s.doc, cur),
            };
            if let Ok(t) = r {
                prefill = t;
            }
        }
    }
    let codec = s.codec;
    // Go rawEditor validates the whole document through decodeRawDoc (nil
    // path) and a subtree through decodeRawValue; the two differ only for
    // YAML (adopt vs value codec).
    let is_whole = path.is_none();
    let title = format!("Edit raw {label}");
    let text = run_input(
        out,
        reader,
        &title,
        &prefill,
        Some(&move |s: &str| {
            let t = s.trim();
            if t.is_empty() {
                return Ok(());
            }
            let r = if is_whole {
                codec.parse(t).map(|_| ())
            } else {
                codec.parse_value(t).map(|_| ())
            };
            match r {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("invalid {label}: {e}")),
            }
        }),
    );
    let s_ = text.trim();
    if s_.is_empty() {
        if let Some(p) = path {
            s.doc.remove(p);
        }
        return;
    }
    let decoded = match path {
        None => match s.codec.parse(s_) {
            Ok(crate::doc::Parsed { doc, meta }) => {
                *s.meta = meta;
                Ok(doc)
            }
            Err(e) => Err(e),
        },
        Some(_) => s.codec.parse_value(s_),
    };
    let Ok(d2) = decoded else { return };
    match path {
        None => *s.doc = d2,
        Some(p) => {
            let v = s.doc.copy_into(&d2, d2.root, CopyMode::Preserve);
            let _ = s.doc.set(p, v);
        }
    }
}

/// saveFlow validates (error list → --force confirm) and writes the document
/// through opts.save. Returns Res::Saved when the write succeeded — or when
/// it failed, after recording the error in save_err (Res::Saved unwinds the
/// screen loop; Run surfaces the error).
fn save_flow<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Res {
    if let Some(validate) = &s.opts.validate {
        let errs = validate(s.doc);
        if !errs.is_empty() {
            const MAX_SHOWN: usize = 15;
            let mut shown: Vec<String> = errs.iter().take(MAX_SHOWN).cloned().collect();
            if errs.len() > MAX_SHOWN {
                shown.push(format!("… and {} more", errs.len() - MAX_SHOWN));
            }
            let _desc = shown.join("\n"); // descriptions are not printed in accessible mode
            let proceed = run_confirm(out, reader, "Save anyway?", false);
            if !s.opts.force || !proceed {
                return Res::Pop;
            }
            s.saved_with_errors = true;
        }
    }
    if let Some(save) = &s.opts.save {
        if let Err(e) = save(s.doc, s.meta) {
            s.save_err = Some(format!("write: {e}"));
        }
    }
    Res::Saved
}

/// variantRoot handles a oneOf root: pick → edit → save.
fn variant_root<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    root: FormNodeId,
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Res {
    loop {
        let (r, eof) = eof_result(reader, true);
        if eof {
            return r;
        }
        let node = root;
        let variants = s.schema.nodes[node as usize].variants.clone();
        let mut opts = Vec::with_capacity(variants.len());
        for (i, v) in variants.iter().enumerate() {
            opts.push(NavOpt {
                key: format!("v:{i}"),
                label: display_title(s.schema, *v),
            });
        }
        let title = s.schema.nodes[node as usize].title.clone();
        let choice = choose(out, reader, &title, &opts, "v:0");
        let Ok(idx) = choice.strip_prefix("v:").unwrap_or("0").parse::<usize>() else {
            return Res::Abort;
        };
        let v = variants[idx];
        match s.schema.nodes[v as usize].kind {
            Kind::Object => {
                if section(s, v, &[], false, reader, out) == Res::Saved {
                    return Res::Saved;
                }
            }
            Kind::Array => {
                if array_manager(s, v, &[], false, reader, out) == Res::Saved {
                    return Res::Saved;
                }
            }
            _ => {
                edit_single(s, v, &[], reader, out);
            }
        }
        if save_flow(s, reader, out) == Res::Saved {
            return Res::Saved;
        }
    }
}

/// simpleRoot handles scalar/enum/boolean/raw root schemas: edit then save.
fn simple_root<R: BufRead, W: Write>(
    s: &mut Sess<'_>,
    root: FormNodeId,
    reader: &mut LineReader<R>,
    out: &mut W,
) -> Res {
    loop {
        let (r, eof) = eof_result(reader, true);
        if eof {
            return r;
        }
        if !edit_single(s, root, &[], reader, out) {
            return Res::Abort;
        }
        if save_flow(s, reader, out) == Res::Saved {
            return Res::Saved;
        }
    }
}

// --- e2e script mirrors (Go e2e/e2e_test.go) ---------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Meta, codec_for};
    use parking_lot::Mutex;
    use std::io::Cursor;
    use std::sync::Arc;

    const GO: &str = env!("CARGO_MANIFEST_DIR");

    /// A JSON session wired like main.rs: codec save, save-time validation,
    /// and the -s schema location recorded in the document.
    fn json_session(
        schema_loc: &str,
        cfg_text: &str,
        cfg_path: &str,
        saved: &Arc<Mutex<Option<String>>>,
    ) -> FileSession {
        let model = crate::smodel::load(schema_loc).expect("load schema");
        let codec = codec_for(cfg_path);
        let mut parsed = if cfg_text.is_empty() {
            codec.empty()
        } else {
            codec.parse(cfg_text).expect("parse config")
        };
        codec.set_schema_location(&mut parsed.doc, &mut parsed.meta, schema_loc);
        let validate = match crate::validate::new(schema_loc) {
            Ok(v) => Some(Arc::new(move |doc: &Doc| v.check(doc)) as _),
            Err(_) => None,
        };
        let save = {
            let saved = Arc::clone(saved);
            Some(Arc::new(move |doc: &Doc, meta: &Meta| {
                let text = codec.write(doc, meta).map_err(|e| e.to_string())?;
                *saved.lock() = Some(text);
                Ok(())
            }) as _)
        };
        FileSession {
            schema: model,
            root: 0, // replaced below
            doc: parsed.doc,
            meta: parsed.meta,
            codec,
            opts: Options {
                accessible: true,
                validate,
                save,
                path: cfg_path.to_string(),
                ..Options::default()
            },
        }
        .with_root()
    }

    /// A YAML/TOML session is the same wiring: the codec owns the format, so
    /// there is no separate raw-codec branch to mirror.
    fn yaml_session(
        schema_loc: &str,
        cfg_text: &str,
        cfg_path: &str,
        saved: &Arc<Mutex<Option<String>>>,
    ) -> FileSession {
        json_session(schema_loc, cfg_text, cfg_path, saved)
    }

    /// Fill in the model's real root id (FileSession carries it separately
    /// because the schema arena owns the tree).
    trait WithRoot {
        fn with_root(self) -> Self;
    }
    impl WithRoot for FileSession {
        fn with_root(mut self) -> Self {
            self.root = self.schema.root;
            self
        }
    }

    fn run(files: Vec<FileSession>, script: &str) -> (Outcome, String) {
        let mut out: Vec<u8> = Vec::new();
        let oc = run_io(files, Cursor::new(script.as_bytes().to_vec()), &mut out);
        (oc, String::from_utf8(out).expect("utf-8 output"))
    }

    /// Asserts the substrings appear in `out` in the given order.
    fn in_order(out: &str, seq: &[&str]) {
        let mut pos = 0;
        for want in seq {
            match out[pos..].find(want) {
                Some(i) => pos += i + want.len(),
                None => {
                    panic!("prompt sequence: {want:?} not found after byte {pos}\n---\n{out}---")
                }
            }
        }
    }

    fn saved_json(saved: &Arc<Mutex<Option<String>>>) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(&saved.lock().clone().expect("saved"))
            .expect("saved text parses")
    }

    fn top_level_keys(saved: &Arc<Mutex<Option<String>>>) -> String {
        let text = saved.lock().clone().expect("saved");
        let doc = crate::doc::jsondoc::parse(&text).expect("parse saved");
        let mut keys = Vec::new();
        if let Value::Object(members) = doc.value(doc.root) {
            for m in members {
                keys.push(m.key.clone());
            }
        }
        keys.join(",")
    }

    /// TestCreateAccessible: the full creation script against
    /// examples/server.schema.json produces the expected document, key
    /// order, and prompt protocol.
    #[test]
    fn create_accessible() {
        let schema_loc = format!("{GO}/examples/server.schema.json");
        let saved = Arc::new(Mutex::new(None));
        let sess = json_session(&schema_loc, "", "/tmp/out.json", &saved);
        // createScript (Go e2e_test.go): every section once, then save.
        let script = concat!(
            "demo\n",                         // Name
            "2\n",                            // Mode → http
            "1\n",                            // Navigate → Server
            "8080\nlocalhost\ny\n\n\n",       // Port, Host, Debug=y, Ratio=omit, Done
            "demo\n\n2\n",                    // root rerun: Name, Mode=http, Navigate → Logging
            "2\n0\n",                         // Levels: toggle info, confirm
            "2\n",                            // Logging navigate → tags items
            "1\nprod\n6\n",                   // mgr: add item, value, done
            "0\n1\n",                         // Logging rerun: confirm levels, Done
            "demo\n\n3\n",                    // root: → Mounts
            "1\ndemos/ROOT\n/var/www\n\n6\n", // mgr: add mount {src,dst}, done
            "demo\n\n4\n",                    // root: → Auth
            "2\nsecret123\n\n",               // variant Token, token, done
            "demo\n\n5\n",                    // root: save
        );
        let (oc, out) = run(vec![sess], script);
        assert!(oc.saved && !oc.aborted, "outcome {oc:?}\n{out}");
        assert!(oc.err.is_none(), "{:?}", oc.err);

        // The prompt protocol, in script lockstep.
        in_order(
            &out,
            &[
                "Name ",
                "Mode ",
                "1. (none)\n2. http\n3. grpc\n4. tcp\nEnter a number between 1 and 4: ",
                "Navigate ",
                "1. Edit Server ▸",
                "Port ",
                "Host ",
                "Debug [y/N] ",
                "Ratio ",
                "There is only one option available; enter the number 1:",
                "Name ",
                "Edit items (0) ▸",
                "Levels ",
                "Select up to 4 options.\n1.   debug\n",
                "0.   Confirm selection",
                "Tags items",
                "< Add item",
                "0: prod",
                "Move item up…",
                "Remove item…",
                "Levels ",
                "2. ✓ info",
                "Mounts items",
                "0: {…}",
                "Auth ",
                "None\n2. Token",
                "Token ",
                "Save and exit",
            ],
        );
        let want: serde_json::Value = serde_json::json!({
            "$schema": schema_loc,
            "name": "demo",
            "mode": "http",
            "server": {"port": 8080, "host": "localhost", "debug": true},
            "logging": {"levels": ["info"], "tags": ["prod"]},
            "mounts": [{"src": "demos/ROOT", "dst": "/var/www"}],
            "auth": {"token": "secret123"},
        });
        assert_eq!(saved_json(&saved), want, "\n{out}");
        assert_eq!(
            top_level_keys(&saved),
            "$schema,name,mode,server,logging,mounts,auth"
        );
        assert_eq!(oc.saved_files, vec!["/tmp/out.json".to_string()]);
    }

    /// TestPrefillAccessible: an illegal enum value is replaced, untouched
    /// values and schema-external keys survive, key order is preserved.
    #[test]
    fn prefill_accessible() {
        let schema_loc = format!("{GO}/examples/server.schema.json");
        let cfg = std::fs::read_to_string(format!("{GO}/examples/server.config.json")).unwrap();
        let saved = Arc::new(Mutex::new(None));
        let sess = json_session(&schema_loc, &cfg, "/tmp/edit.json", &saved);
        // Re-type required Name, replace the illegal enum value, save.
        let (oc, out) = run(vec![sess], "demo\n3\n5\n");
        assert!(oc.saved && !oc.aborted, "outcome {oc:?}\n{out}");
        in_order(
            &out,
            &[
                "Name ",
                "Mode ",
                "1. (none)",
                "3. grpc",
                "Navigate ",
                "5. Save and exit",
            ],
        );
        let got = saved_json(&saved);
        assert_eq!(got["mode"], "grpc", "illegal enum not replaced:\n{out}");
        assert_eq!(got["extra_unknown_key"], 42.0, "schema-external key lost");
        assert_eq!(got["name"], "demo");
        assert_eq!(
            got["server"]["port"], 8080,
            "untouched values changed: {got}"
        );
        assert_eq!(
            top_level_keys(&saved),
            "$schema,name,mode,server,logging,mounts,extra_unknown_key"
        );
    }

    /// TestMoveAccessible: "Move item up…" relocates beta in the three-item
    /// tags array.
    #[test]
    fn move_accessible() {
        let schema_loc = format!("{GO}/examples/server.schema.json");
        let cfg = r#"{"$schema": "server.schema.json", "name": "demo", "server": {"port": 8080}, "logging": {"tags": ["alpha", "beta", "gamma"]}}"#;
        let saved = Arc::new(Mutex::new(None));
        let sess = json_session(&schema_loc, cfg, "/tmp/move.json", &saved);
        let script = concat!(
            "demo\n",      // Name re-typed (required)
            "2\n2\n",      // Mode → http, Navigate → Logging
            "2\n0\n",      // Levels: toggle info, confirm
            "2\n",         // Navigate → Tags items (n=3)
            "5\n2\n",      // "Move item up…" → pick 1: beta
            "8\n0\n1\n",   // items manager Done, levels confirm, Logging Done
            "demo\n\n5\n", // Name, Mode default, Save and exit
        );
        let (oc, out) = run(vec![sess], script);
        assert!(oc.saved && !oc.aborted, "outcome {oc:?}\n{out}");
        in_order(
            &out,
            &[
                "Tags items",
                "1. < Add item\n2. 0: alpha\n3. 1: beta\n4. 2: gamma\n",
                "5. Move item up…",
                "Move item up ",
                "1. 0: alpha\n2. 1: beta\n3. 2: gamma",
            ],
        );
        let got = saved_json(&saved);
        let tags = got["logging"]["tags"].as_array().unwrap();
        let names: Vec<&str> = tags.iter().map(|t| t.as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec!["beta", "alpha", "gamma"],
            "tags after move: {got}"
        );
    }

    /// TestValidationBlock: input validation errors surface (required,
    /// minimum), a save with validation errors is declined, the raw editor
    /// fixes the document, and a clean save lands.
    #[test]
    fn validation_block() {
        let schema_text = r#"{
  "type": "object",
  "properties": {"port": {"type": "integer", "title": "Port", "minimum": 1024}},
  "required": ["port"],
  "additionalProperties": false
}"#;
        let tmp = std::env::temp_dir().join("jse-rust-e2e");
        std::fs::create_dir_all(&tmp).unwrap();
        let schema_path = tmp.join("strict.schema.json");
        std::fs::write(&schema_path, schema_text).unwrap();
        let schema_loc = schema_path.to_str().unwrap().to_string();
        let saved = Arc::new(Mutex::new(None));
        let cfg = r#"{"port": 80, "rogue": 1}"#;
        let sess = json_session(&schema_loc, cfg, "/tmp/strict.json", &saved);
        // empty → required error; 1 → minimum error; 8080 → accepted.
        // Save → save-time errors (rogue key) → decline; fix via raw editor; save.
        let script = concat!(
            "\n1\n8080\n\n",     // Port attempts, navigate=save
            "n\n",               // decline save-anyway
            "8080\n2\n",         // Port, navigate → raw editor
            "{\"port\":8080}\n", // raw JSON replaces doc
            "8080\n1\n",         // Port, navigate=save → valid
        );
        let (oc, out) = run(vec![sess], script);
        assert!(oc.saved && !oc.aborted, "outcome {oc:?}\n{out}");
        assert!(out.contains("required"), "required error not shown:\n{out}");
        assert!(
            out.contains("must be at least 1024"),
            "minimum error not shown:\n{out}"
        );
        in_order(
            &out,
            &[
                "Port ",
                "required",
                "must be at least 1024",
                "Save anyway? [y/N] ",
                "Edit raw JSON ",
            ],
        );
        let got = saved_json(&saved);
        assert_eq!(got, serde_json::json!({"port": 8080}), "final doc: {got}");
    }

    /// TestYAMLEditingAccessible: the value changes and its comments (plus
    /// the schema directive) survive the save.
    #[test]
    fn yaml_editing_accessible() {
        let tmp = std::env::temp_dir().join("jse-rust-e2e");
        std::fs::create_dir_all(&tmp).unwrap();
        let schema_path = tmp.join("mini.schema.json");
        std::fs::write(&schema_path, r#"{"type":"object","properties":{"name":{"type":"string","title":"Name"}},"required":["name"]}"#).unwrap();
        let schema_loc = schema_path.to_str().unwrap().to_string();
        let saved = Arc::new(Mutex::new(None));
        let cfg = "# config header\nname: demo # keep me\n";
        let sess = yaml_session(&schema_loc, cfg, "/tmp/config.yaml", &saved);
        let (oc, out) = run(vec![sess], "newname\n\n");
        assert!(oc.saved && !oc.aborted, "outcome {oc:?}\n{out}");
        let text = saved.lock().clone().expect("saved");
        assert!(
            text.contains(&format!("# yaml-language-server: $schema={schema_loc}")),
            "directive missing:\n{text}"
        );
        assert!(
            text.contains("# config header"),
            "header comment lost:\n{text}"
        );
        assert!(
            text.contains("name: newname # keep me"),
            "value or line comment lost:\n{text}"
        );
        assert!(!text.contains("name: demo"), "old value survived:\n{text}");
        assert!(
            !text.contains("$schema:"),
            "JSON-style $schema key written into YAML:\n{text}"
        );
        let (doc, _) = crate::doc::yamldoc::parse(&text).expect("saved YAML parses");
        assert_eq!(
            doc.to_value(doc.root),
            serde_json::json!({"name": "newname"})
        );
    }
}
