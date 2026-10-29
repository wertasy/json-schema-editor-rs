# jse User Manual

`jse` is a schema-driven terminal configuration editor: it generates forms from a JSON Schema and writes the edited result back to the original file. This manual is aimed at users; contributors and coding agents should see [AGENTS.md](../../AGENTS.md) at the repository root.

## Contents

| Document | When to read |
|---|---|
| [Installation and building](install.md) | First-time install, building from source, release package contents |
| [Command line](cli.md) | Looking up flags, opening multiple files at once, exit codes and environment variables |
| [Schema sources and caching](schema-sources.md) | Config without a schema, using SchemaStore, remote schema cache / offline |
| [Configuration file formats](formats.md) | JSONC/TOML comments, YAML anchors / merge keys, format changes after saving |
| [Interface and key bindings](interface.md) | First time in the TUI, looking up a key, understanding the status line and markers |
| [Editing](editing.md) | Array / map / oneOf editing, copy & paste, multi-select, search & replace, undo, macros |
| [Validation and saving](validation.md) | Which keywords take effect, what to do about errors, save behavior and exit codes |
| [Line mode](accessible.md) | Environments without a TUI, script-driven use, screen readers |
| [Troubleshooting](troubleshooting.md) | Errors, cache, proxy, clipboard, fonts, editor |

## Three reading paths

**First time using jse**

1. [Installation](install.md)
2. [Quick start](../../README.en.md#quick-start) to run an example
3. [Interface and key bindings](interface.md) to memorize `↑/↓`, `enter`, `esc`, `ctrl+s`
4. Turn to [Editing](editing.md) when you need to edit arrays or oneOf

**Attaching a schema to an existing configuration**

1. [Schema sources and caching](schema-sources.md): confirm the resolution order
2. [Configuration file formats](formats.md): confirm where to put the `$schema` / YAML header comment
3. [Validation and saving](validation.md): understand unknown-key warnings and `--force`

**Writing automation scripts**

1. [Line mode](accessible.md): the prompt protocol and EOF behavior
2. [Command line](cli.md): exit codes, stdout result lines
3. [Schema sources and caching](schema-sources.md): `JSE_CACHE_DIR`, `JSE_CATALOG_URL`

## Conventions

- Keys are written `ctrl+s`, `alt+x`, `shift+tab`; `↑`/`↓`/`←`/`→` are also written `up`/`down`/`left`/`right`.
- `<err>` / `<warn>` in menus are two Nerd Font icons: error (red) and warning (yellow). Without the font installed they render as blank squares; functionality is unaffected.
- "Root screen" refers to the outermost menu (breadcrumb `$`).
- "Current file" refers to the file being displayed in a multi-file session; `ctrl+s` saves only the current file, and when exiting from the root screen you can save all modified files in one go.
- Command examples assume execution at the repository root by default.
