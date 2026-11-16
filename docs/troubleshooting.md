# Troubleshooting

## Startup and schema

### `no schema found: editing without schema guidance; save-time validation disabled`

This is not an error; it means you are in **schema-less mode**: the file stays editable and saving is not validated. To restore forms and validation, check the [resolution order](schema-sources.md#resolution-order):

1. whether `-s` was passed;
2. whether the config top level has a non-empty `$schema` string (YAML may also use a file header `# yaml-language-server: $schema=...`, and TOML may use `#:schema <location>`);
3. whether the file name uniquely matches an entry in the SchemaStore directory;
4. whether the relative schema path exists relative to the config file's directory, or relative to the current working directory.

Quick fix: `jse -s ./schema.json config.json`.

### `schema catalog unavailable: ...`

SchemaStore is needed but the catalog download failed (network, proxy, mirror). Schema-less mode still lets you keep editing; to use SchemaStore you can:

- provide `-s`;
- write the schema path into the config;
- set `JSE_CATALOG_URL` to an intranet mirror;
- if the catalog was cached before, the cache is used automatically when offline.

### `schema selection needs a terminal; rerun with --accessible or provide -s`

The full-screen schema picker needs to open, but stdin is not a terminal. Add `--accessible` (line-based selection) or pass `-s` directly.

### `config must be a local file path`

The positional argument cannot be a URL; only schemas support remote sources. Put the remote schema in `-s`.

### `stdin is not a terminal; rerun with --accessible ...`

The full-screen TUI requires a terminal. Add `--accessible` in scripts/CI, or run from a real terminal.

### `load schema ...` / `schema compile failed, save-time validation disabled`

- Schema file missing, not valid JSON, or remote unreachable: fix the schema or the network.
- Loads but fails to compile: the editor stays usable, but **save-time validation is disabled**; the root screen status line keeps showing the warning. Restart after fixing the schema.

### Remote schema not updating

By default the cache is used directly for 24 hours without sending requests. To force a refresh:

```sh
rm -rf ~/.cache/jse
# or
JSE_CACHE_DIR=$(mktemp -d) jse -s https://example.com/schema.json config.json
```

### `GET ...: status 404`

The URL is wrong or the schema has moved; a 404 does not fall back to the cache. Fix the URL.

### `rate limited` / 429

jse starts from the cache and reports the retry time; the rate-limit window (`Retry-After`, 60 seconds by default) is persisted across processes. Retry later, or switch to a local schema.

### `response exceeds size limit`

The response exceeds 32 MiB. Make sure the URL points to a schema, not a web page or a large file.

### Proxy not taking effect

Proxies are read in order: `HTTPS_PROXY` / `https_proxy`, `HTTP_PROXY` / `http_proxy`, `ALL_PROXY` / `all_proxy`; `NO_PROXY` / `no_proxy` exclusions take priority. `*`, domain suffixes, and leading dots are supported. Do not put extra spaces in proxy URLs.

## Config parsing

### `parse config ...` / `invalid JSON` / `invalid YAML` / `invalid TOML`

- JSONC allows comments and trailing commas; other JSON syntax errors still need fixing.
- YAML only supports a single document and string keys; multi-document input, non-string keys, and `.inf`/`.nan` are errors.
- An empty YAML file is an error; only a missing file starts from an empty object.
- Timestamps are treated as strings; `!!binary` and unknown tags are treated as strings.
- TOML requires the root value to be a table; duplicate keys/duplicate tables, extended inline tables, and `inf`/`nan` are errors. An empty TOML file equals an empty table.
- TOML dates/times are displayed as strings; unedited literals are preserved verbatim.

### Format changed after saving

- Comment-free JSON output is unchanged.
- JSONC trailing commas are not preserved; comments follow their member.
- YAML indentation, quote style, and blank lines may be normalized by the emitter; key order, comments, anchors, aliases, and merge keys are preserved.
- TOML blank lines, dotted-key style, quote style, and inline-table layout may be normalized; key order, comments, and unedited literals are preserved. Arrays whose elements are all tables are written as `[[table]]`.
- For byte-for-byte preservation, back up with `cp` first and diff afterwards.

## Editing

### Chinese / wide characters misaligned

jse clips by terminal cell width; if your terminal font treats full-width characters as half-width, or you use a non-monospace font, display may misalign. Switch to a monospace terminal font.

### `<err>` / `<warn>` shown as boxes

Icons appear only when the terminal font includes Nerd Font / Font Awesome private-use-area glyphs; otherwise blank boxes are shown. Functionality is unaffected.

### Missing clipboard tools

- Linux Wayland: `wl-copy` / `wl-paste`
- Linux X11: `xclip` or `xsel`
- macOS: `pbcopy` / `pbpaste`
- Windows: writing uses `clip`; **reading (paste) is not supported**

When no clipboard tool is present, jse automatically falls back to the session's built-in clipboard (fallback rules and scope in [Editing](editing.md#copy--cut)): `y`/`x` still succeed (the hint carries a ` (built-in clipboard)` suffix), and `p`/`P`/`ctrl+v`, unable to read the system clipboard, paste back what was copied in this session.

`clipboard: no clipboard tool found` / `clipboard: cannot read without paste support` appear only when nothing has been copied yet in this session; once a tool is installed you can exchange the clipboard with other applications. A failure never modifies the document.

### `$EDITOR` opened but content not applied

- GUI editors need wait mode: `export EDITOR="code --wait"`.
- If the editor exits abnormally / the result is syntactically invalid, jse keeps the original document and notifies you.
- Both built-in raw editing and single-value external editing parse in the file's own format (JSON, YAML, or TOML); string values are the exception — they open unquoted and apply verbatim as string text (see [Editing](editing.md#raw-editing-and-the-external-editor)).

### `< Expand >` / anchor operations not found

`&`, `*`, `<`, `ga`, `gr` only apply to YAML documents; JSON/TOML documents have no anchors. Aliases must actually exist in the YAML.

### Cannot delete a key

- Schema-required fields and required merged keys refuse deletion.
- An anchor referenced by an alias must have its references removed before the anchor can be deleted.
- To break a merge, first press `<` to clear the merge key.

### `q` does not quit inside input boxes

Inside an input box `q` is a normal character; cancel with `esc`.

## Saving and quitting

### Save blocked by validation

- The issue count on the first line of the root menu can be opened with `enter`; `enter` jumps to the field to fix it.
- In the save error list `enter` jumps to the field; `n`/`esc` returns.
- Only with `--force` does the error list accept `y` to force-save.
- When schema compilation fails, save-time validation is disabled; fix the schema first.

### File unchanged after quitting

- Root screen `n` (discard) and `ctrl+c` never write the file.
- With no changes, `esc`/`q` exits directly without writing.
- With multiple files, `ctrl+s` only saves the current file; check the path in the save confirmation prompt.
- Save failures (permissions, read-only directory, disk full) show a message at the bottom; the file stays as it was.

### `Saved to ...; aborted (changes after the last save are lost)`

You saved with `ctrl+s`/`w` during the session, then made further changes, then quit with `ctrl+c`/discard. The disk holds the content of the last save.

### Exit codes

See [Command line: exit codes and stdout](cli.md#exit-codes-and-stdout).

## Performance

- Opening large files / large schemas performs schema compilation and validation the first time; remote schemas are cached after the first fetch.
- Search rebuilds the index for the current subtree on every open.
- Undo is capped at 100 steps and macros at 500 keys, so memory does not grow without bound.

## What to include when reporting issues

```sh
jse --version
uname -a
echo "$TERM"
```

And:

- a minimal reproducible schema and config file (sensitive values removed);
- the full command and exit code;
- terminal type (TUI/`--accessible`);
- whether it reproduces reliably, and at which key press/save step it happens.
