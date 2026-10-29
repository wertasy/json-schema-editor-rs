# jse — JSON Schema-driven terminal configuration editor

`jse` reads a JSON Schema and turns a configuration file into a menuconfig-style terminal interface: a single-screen scrolling list, breadcrumbs, in-place editing, and hierarchical drill-down; before saving, the JSON Schema acts as the authoritative validation. It suits files that have a schema but are error-prone to write by hand in JSON/YAML/TOML, such as server configs, CI workflows, and sing-box rule sets.

It supports **JSON / JSONC / YAML / TOML**, preserving comments, key order, number literals, YAML anchors/aliases and merge keys; one process can open multiple configuration files at once. It also provides a `--accessible` line-mode Q&A mode for use in pipes, scripts, and with screen readers.

```
Server Configuration                              ●
$
  Name*                   webapp
  Mode                    http
> Server*                 port=8080 host=0.0.0.0 +1  --->
  Logging                 —  --->
  Mounts                  1 item  --->
  Auth                    —  --->
↑/↓ move · enter edit/open · esc back · ctrl+s/w save · / search · : command · ? help
```

## Features

- **Schema-driven**: `properties`/`required`, `enum`/`const`, `oneOf`/`anyOf`/`allOf`, `$ref` (including remote), arrays, maps, and `patternProperties` all generate corresponding forms; keywords not covered are still validated at save time.
- **Format fidelity**: JSONC/TOML comments, YAML comments, anchors, aliases, and `<<` merge keys are preserved as-is; key order, number literals like `1.0`, and TOML hexadecimal/underscored numbers and date-time literals stay unchanged unless edited; writes use an atomic write to a temp file in the same directory + rename.
- **Efficient editing**: vim-style movement and jumps, multi-select, copy/cut/smart paste, search and regex replace within the current subtree, undo/redo, macro recording, `$EDITOR` external editing, and text auto-completion; keys undefined in the schema or without a `type` are edited directly by runtime type — only when a key has no value do you first pick a built-in type or a named type from the schema `$defs`.
- **Schema auto-discovery**: `-s`, the config's own `$schema`, YAML file-header directives, TOML `#:schema` directives, or SchemaStore directory matching/search; during a session, `S` re-searches and switches the current file's schema. Remote schemas come with a 24-hour cache and offline fallback. When no schema is found, the file still opens for direct editing (objects/arrays are navigated by structure, other values by runtime type; saving skips validation).
- **Visible errors**: validation issues are listed at load time, and field lines and the issue list refresh in real time; unknown keys are warnings, not errors; `--force` forces a save.
- **Multi-file sessions**: `jse a.json b.json` opens several files at once, `s` switches between them, and `ctrl+o`/`ctrl+i` move backward/forward through navigation history; on exit, all changes are saved in one pass.

## Installation

Prebuilt binaries are on [Releases](https://github.com/wertasy/json-schema-editor-rs/releases): Linux (static musl) / macOS / Windows, amd64 and arm64; each archive contains `jse`, `README.md`, `docs/`, `AGENTS.md`, `LICENSE`, and `checksums.txt` provides SHA-256 checksums.

Building from source:

```sh
cargo build --release
cargo install --path .        # install to ~/.cargo/bin
cargo install json-schema-editor  # install jse from crates.io
```

`jse --version` prints the version, commit, and build time. The Linux static release targets are `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl`.

## Quick start

```sh
# 1. Specify a schema explicitly; edit an existing file (written back in place)
jse -s examples/server.schema.json examples/server.config.json

# 2. The schema is already recorded in the config; just open it
jse server.json

# 3. Let jse match or search for a schema from SchemaStore by file name
jse .github/workflows/ci.yml

# 4. Edit directly when there is no schema source (no validation; structure is still navigable)
jse notes.json

# 5. Line mode (no full-screen TUI; good for scripts/pipes)
jse --accessible -s examples/server.schema.json server.json
```

When a file does not exist, editing starts from an empty document, created on save. Multiple files can be opened at once:

```sh
jse -s server.schema.json a.json b.yaml
```

The schema given via `-s` is written back into the config according to the file format (top-level `$schema` for JSON/JSONC; YAML by default writes the file header `# yaml-language-server: $schema=...`; TOML by default writes the file header `#:schema <location>`; if the schema defines a top-level `$schema` property, that field is written), so `-s` can be omitted on the next launch.

## Key bindings at a glance

| Key | Action |
|---|---|
| `↑`/`↓` or `j`/`k` | Move the cursor (wraps at both ends) |
| `ctrl+f`/`PgDn`, `ctrl+b`/`PgUp` | Page down/up (list, overlays, and comment/help pages) |
| `enter` / `→` / `l` | Edit a field or enter a submenu |
| `←` / `h` / `esc` / `q` | Go back one level; on the root screen `esc`/`q` exits and asks to save |
| `space` | Toggle a boolean / enum array member |
| `-` / `=` / `+` | Quickly decrement / increment a number by 1; fill in `-1` / `1` when there is no value. Also works for unknown object keys, array elements, and runtime numbers not defined by the schema |
| `c` | Expand/collapse the description of the current line (schema help + doc comments); opens a scrollable comment page when it does not fit on one page |
| `C` | Edit the doc comment of the current key (adds one for keys in the document that lack a comment; empty text deletes it) |
| `gg` / `G` | Jump to the first / last line of the list; in array menus `Ngg` jumps to item N |
| `/` / `r` | Search under the current path / search and replace |
| `m` / `v` | Mark a single item / range select; combine with `y`, `x`, `d` for batch operations |
| `y` / `Y` / `p` / `P` | Copy key-value pair / copy value / smart paste into the menu / paste into the container at the cursor |
| `u` / `ctrl+r` | Undo / redo |
| `:` | vi-style command mode: `w`/`wa`/`wq`/`waq` save (add `!` to skip validation), `q`/`q!` quit; `!<cmd>` only runs a shell command, `r !<cmd>` writes the output into the value at the cursor, `a !<cmd>` appends each line to a string array, `w !<cmd>` feeds the document to the command's stdin; `%` stands for the current file; tab completes built-in commands, `$PATH` commands, or paths based only on the text before the cursor and preserves the text after it; `!!` re-runs the previous shell command |
| `E` / `ctrl+e` | Edit the value of the cursor line in an external editor (raw JSON/YAML/TOML; edits the whole document when not on a value line) / materialize YAML aliases and merged values |
| `ctrl+s` / `w` | Save the current file and stay in the editor |
| `W` | Save a copy to a specified path |
| `s` | Open the file switcher when multiple files are open |
| `S` | Open SchemaStore search to switch the current file's schema |
| `ctrl+o` / `ctrl+i` (`tab`) | Navigate history backward / forward |
| `?` | Full key-binding help |

For the complete key bindings, interface elements, and the keys of each overlay, see [Interface and key bindings](docs/en/interface.md).

## Documentation

| Document | Contents |
|---|---|
| [User manual](docs/en/index.md) | Contents and reading paths |
| [Installation and building](docs/en/install.md) | Prebuilt packages, building from source, runtime requirements |
| [Command line](docs/en/cli.md) | Arguments, multiple files, environment variables, exit codes |
| [Schema sources and caching](docs/en/schema-sources.md) | Resolution order, SchemaStore, remote cache, offline fallback |
| [Configuration file formats](docs/en/formats.md) | JSONC, YAML, TOML, comments/anchors/merge keys, format extensions |
| [Interface and key bindings](docs/en/interface.md) | Layout, status line, full key table, file switching and navigation history |
| [Editing](docs/en/editing.md) | Field editing, clipboard, multi-select, search and replace, undo, macros, external editor |
| [Validation and saving](docs/en/validation.md) | Form keywords, degradation rules, issue list, atomic write, exit codes |
| [Line mode](docs/en/accessible.md) | The `--accessible` prompt protocol and script examples |
| [Troubleshooting](docs/en/troubleshooting.md) | Common errors, proxies, clipboard, fonts, environment variables |
| [AGENTS.md](AGENTS.md) | Architecture map, constraints, and workflow for coding agents/contributors |

## Development

```sh
cargo test                                        # unit tests + accessible/pty black-box e2e
cargo run -- -s examples/server.schema.json examples/server.config.json
cargo build --release --target x86_64-unknown-linux-musl   # static release build
```

For code structure, module boundaries, testing conventions, and change rules, see [AGENTS.md](AGENTS.md). Releases are handled by `.github/workflows/release.yml`: pushing a `v*` tag builds the six platform targets, creates a GitHub Release, and publishes the crate to crates.io.

## License

[MIT](LICENSE)
