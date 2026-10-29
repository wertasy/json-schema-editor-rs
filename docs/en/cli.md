# Command line

```text
jse [-s <schema path|http(s)://URL>] [-p|--pick] [-f|--force]
    [-a|--accessible] [-v|--version] [-h|--help] <config.(json|jsonc|yaml|yml|toml)>...
```

All `<config.(json|jsonc|yaml|yml|toml)>...` positional arguments are local configuration files to edit; a single process can open several at once.

## Flags

| Flag | Description |
|---|---|
| `-s`, `--schema <path\|URL>` | Specify the JSON Schema: local path, `file://`, or `http(s)://`. It is written into the config's schema pointer according to the config format (an existing non-empty pointer is kept). |
| `-p`, `--pick` | Force opening the SchemaStore search picker; overrides `-s` and the config's own `$schema`, and picks once for each file. |
| `-f`, `--force` | Write on save even when schema validation fails. The result line becomes `Saved with validation errors (--force): <path>`. |
| `-a`, `--accessible` | Line-oriented question-and-answer mode without the full-screen TUI. See [Line mode](accessible.md). |
| `-v`, `--version` | Print the version and exit. |
| `-h`, `--help` | Print help and exit. |

Argument parsing is compatible with GNU getopt style:

- Options may appear after positional arguments: `jse a.json -s schema.json`.
- Long options support `--schema=path`; short options can be combined: `-fa`; a value-taking short option can be attached to its value: `-sschema.json` (`-s=path` is also accepted).
- Everything after `--` is treated as a file name. It is required when a file name starts with `-`: `jse -- -name.json`.
- `-h`/`-v` take effect as soon as they are parsed; later arguments are not processed.

## Basic behavior

- **File does not exist**: starts from an empty object document, created on save.
- **Config must be a local path**: putting `http(s)://...` in a positional argument fails with `config must be a local file path`, exit 2.
- **No positional arguments**: errors and prints usage, exit 2.
- **The TUI needs a terminal**: in non-`--accessible` mode, startup is refused when stdin is not a terminal (exit 1), avoiding a garbled UI in a half-piped environment.
- **Everything loads before the UI**: in multi-file mode, a failure to read or parse any file terminates the whole run before the interface appears; each file's schema is parsed individually.
- **Editing works without a schema**: when `-s`, the config pointer, and SchemaStore all come up empty, jse enters schema-less mode (objects as key menus, arrays as item menus, other values by runtime type; no validation on save) — it does not exit because of the missing schema. See [Schema sources](schema-sources.md#4-all-sources-fail-schema-less-mode).
- **Format by extension**: `.json`/`.jsonc` use JSONC, `.yaml`/`.yml` use YAML, `.toml` uses TOML; other extensions are treated as JSONC. See [Configuration file formats](formats.md).

## Multi-file sessions

```sh
jse -s server.schema.json a.json b.yaml c.json
```

- Startup shows the first file; the title line shows the file name with `[1/3]` on the right (submenus keep the file name: `[1/3 a.json]`).
- `s` opens the file switcher (`↑/↓` or `j/k` to move, `enter` to switch, `esc` to close). Files with unsaved changes carry a `●` in the switcher.
- Each file keeps its own breadcrumb, cursor, undo stack, change baseline, and issue list.
- `ctrl+o` / `ctrl+i` (`tab`) move back/forward through the navigation history, across files as well — e.g. after switching to file B, `ctrl+o` returns to the position where you left file A.
- `ctrl+s` / `w` **save only the current file**. When exiting with `esc`/`q` at the root screen and multiple files have changes, jse asks `Save changes in N files? (y/n)`; `y` saves them one by one.
- Batch saves are written in order; when a file fails validation, saving stops at that file and opens the error list — fix it and exit again, or force the write with `--force`.

In `--accessible` mode, files are edited **one at a time** in argument order: each file walks through the question-and-answer flow from the root screen, and the process exits once all files are done.

## Exit codes and stdout

| Exit code | Meaning | stdout result line |
|---|---|---|
| 0 | Saved; or normal exit with no unsaved changes | `Saved to <path>` (one line per file in multi-file mode) / `No changes, nothing written` / `Saved with validation errors (--force): <paths>` |
| 1 | Runtime error: failed to read/parse the config, schema fetch or compile prerequisite failed, TUI error, stdin is not a terminal | Error message goes to stderr |
| 2 | Usage error: unknown flag, no config file, `--pick` aborted, `--pick` picker lacks a terminal | Error message and usage go to stderr |
| 130 | User abort: chose `n` at the root screen to discard, `ctrl+c` | `Aborted, no changes written`; if something was saved earlier: `Saved to <path>; aborted (changes after the last save are lost)` |

Notes:

- A `--force` write is still exit 0.
- `ctrl+c` writes no files; but anything already saved with `ctrl+s`/`w` during the session stays on disk, and its result line notes that changes after the last save are lost.
- Pressing `esc`/`q` at the root screen with no changes exits directly, writes nothing, and prints `No changes, nothing written`.

## Examples

```sh
# Explicit schema
jse -s examples/server.schema.json server.json

# Schema already present in the config's $schema / YAML header / TOML `#:schema`
jse server.yaml
jse Cargo.toml

# Remote schema
jse -s https://example.com/schemas/server.schema.json server.json

# Force the SchemaStore picker (even when the config already has a schema)
jse --pick .github/workflows/ci.yml

# Write despite validation errors
jse --force -s schema.json broken.json

# Line mode (interactive line-by-line Q&A; see docs/accessible.md for scripting)
jse --accessible -s examples/server.schema.json new.json
```

## Environment variables

| Variable | Purpose |
|---|---|
| `JSE_CACHE_DIR` | Overrides the cache directory for remote schemas / the SchemaStore catalog; the `:` command history file (`cmd-history`) also lives here. Default `$XDG_CACHE_HOME/jse` (usually `~/.cache/jse`). When the directory is not writable, caching is silently skipped. |
| `JSE_CATALOG_URL` | Overrides the SchemaStore catalog URL, for testing or intranet mirrors. |
| `EDITOR` / `VISUAL` | External editor; `vi` when neither is set. |
| `HTTPS_PROXY` / `https_proxy`, `HTTP_PROXY` / `http_proxy`, `ALL_PROXY` / `all_proxy` | Proxy for remote requests; the first non-empty value in this order wins. |
| `NO_PROXY` / `no_proxy` | Host list that bypasses the proxy; supports `*`, domain suffixes, and leading dots. |
| `XDG_CACHE_HOME` | Cache root directory; the platform default cache directory is used when unset. |
| `JSE_VERSION_OVERRIDE`, `JSE_COMMIT`, `JSE_BUILD_DATE` | Inject version metadata **at build time only** (used by the release workflow). |

`JSE_CACHE_DIR` cannot disable caching by being set to an empty value; to force a refresh, delete the cache directory or switch to a different one.
