//! Command-line parsing: getopt-style flags and the usage text.

// --- command-line parsing (getopt/getopt_long style) ---------------------------

const USAGE_TEXT: &str = "Usage: jse [-s <schema path|http(s)://URL>] [-p|--pick] [-f|--force]
           [-a|--accessible] [-v|--version] [-h|--help] <config.(json|jsonc|yaml|yml|toml)>...

Edit a JSON, JSONC, YAML, or TOML configuration file guided by a JSON Schema.

Schema source (checked in order):
  1. -s, --schema <local path or http(s)://URL>
  2. the config's own schema pointer: a top-level \"$schema\" key, a
     \"# yaml-language-server: $schema=...\" comment for YAML files, or a
     \"#:schema <location>\" comment for TOML files;
     relative paths resolve against the config file's directory, falling
     back to the current working directory when no file exists there
  3. the SchemaStore catalog: a unique fileMatch hit for the file name is
     used directly; an ambiguous or missing match opens an interactive
     search over the catalog (--pick forces the search)
  4. nothing found: the file opens in schema-less mode (objects as key
     lists, arrays as item lists, other values by runtime type) and saves
     without validation; --pick failures remain errors

Remote schemas are cached under $XDG_CACHE_HOME/jse (JSE_CACHE_DIR overrides
the location): a copy from the last 24 hours is reused without a request, and
an unreachable origin falls back to the cached copy instead of failing. The
schema catalog shares that cache; JSE_CATALOG_URL overrides the catalog
endpoint.

Arguments:
  config file   the configuration file to edit (local path); the format
                follows the extension (.json/.jsonc, .yaml/.yml, .toml) and a
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
constructs fall back to a raw editor in the file's format (JSON, YAML, or TOML).";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Invocation {
    Help,
    Version,
    Edit(Flags),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Flags {
    pub(crate) schema: String,
    pub(crate) force: bool,
    pub(crate) accessible: bool,
    pub(crate) pick: bool,
    pub(crate) files: Vec<String>,
}

/// parse_args implements the getopt/getopt_long option syntax used by the
/// CLI: `-s v`, `-sv`, `--schema v`, `--schema=v`, clustered short flags
/// (`-fa`), floating operands (options may follow the config paths), and the
/// `--` terminator. `-h`/`-v` (and their long forms) stop parsing as soon as
/// they are seen. Errors are returned instead of exiting so they can be
/// tested; run() prints the message plus the usage and exits 2.
pub(crate) fn parse_args(args: &[String]) -> Result<Invocation, String> {
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
pub(crate) fn print_usage() {
    eprintln!("{USAGE_TEXT}");
}
