# Schema sources and caching

jse looks for a JSON Schema in a fixed order to generate the form and uses the first one it finds. When all sources fail it does not refuse to open the file; instead it enters **schema-less mode** and edits JSON/YAML/TOML directly.

## Resolution order

### 1. Explicit `-s` / `--schema`

```sh
jse -s ./schemas/server.json config.json
jse -s https://example.com/schemas/server.json config.json
```

- Values starting with `http://` / `https://` are fetched remotely.
- Other values are treated as local paths (`file://` is also accepted).
- The location given via `-s` is written into the config's schema pointer, so it can be omitted next time. If a non-empty pointer already exists, the original value is kept, not overwritten; `--pick` is the exception and does overwrite.

### 2. Schema pointer in the config itself

Without `-s`, the config file itself is read:

| Format | Pointer form | Notes |
|---|---|---|
| JSON / JSONC | Top-level `"$schema": "<location>"` | Must be a string; an empty string does not count. |
| YAML | Header comment `# yaml-language-server: $schema=<location>` | A top-level `$schema` key also works; the explicit key takes precedence. |
| TOML | Header comment `#:schema <location>` | A top-level `"$schema"` key also works; the explicit key takes precedence. |

- URLs are used directly.
- Relative paths without a scheme are resolved first relative to the **directory containing the config file**; when not found there, they fall back to the **current working directory**.
- Absolute paths are used directly.
- When `$schema` is defined in the schema itself (e.g. sing-box), it also appears in the editor as an ordinary field and can be modified.

### 3. The SchemaStore catalog

If the first two steps produce nothing at startup, or when you press `S` during a session, jse searches the public [SchemaStore](https://www.schemastore.org/) catalog (`https://www.schemastore.org/api/json/catalog.json`). Startup resolution matches the config file's absolute path against `fileMatch` patterns:

- Patterns without `/` match only the **file name** (case-insensitive): `package.json`, `tsconfig*.json`, `*.srv.json`.
- Patterns containing `/` match from the tail of the path by directory segments; `**` crosses any number of directories, while `*`/`?`/`[...]` apply within a single segment. Examples: `**/.cdf/config.yaml`, `.platform/applications.yml`.
- The same URL listed by multiple entries is deduplicated.
- **Single hit**: used directly, no picker.
- **Multiple hits or none**: the search picker opens.
- `--pick` forces the picker open, even when `-s` or a config pointer already exists.

The catalog's HTTP cache is **stale-while-revalidate**: as long as a local cache exists it is returned immediately even past 24 hours, while a conditional refresh runs in the background; the dropdown / matching / `S` never wait on the network. The fresh copy obtained in the background serves later reads in this session (e.g. pressing `S` again) or the next start. Only the very first run with **no cache at all** makes a synchronous request.

TUI picker:

| Key | Action |
|---|---|
| Typing | Filters by name or description (case-insensitive) |
| `↑`/`↓`, `ctrl+k`/`ctrl+j` | Move up / down |
| `tab` / `shift+tab` | Move down / up |
| `enter` | Select (no action when the result list is empty) |
| `esc` / `ctrl+c` | Abort (falls back to schema-less mode during automatic resolution; exits under `--pick`) |

The initial query is the file name without its extension; the first text typed replaces this preset query rather than appending to it.

`--accessible` picker:

```text
Select a schema for ci.yml
? Schema search [ci]: 
  1) GitHub Workflow — GitHub Actions workflow
? Number (0 = new search, enter = abort): 1
```

An empty line keeps the current query; `0` starts a new search; bare enter aborts (falls back to schema-less mode during automatic resolution, exits under `--pick`); at most 15 entries are shown, with a prompt to refine the search beyond that. Out-of-range input prints `! invalid number` and asks again. EOF counts as abort.

The selected URL is written back as the schema pointer in the config's format (top-level `$schema` for JSON/JSONC; the header comment by default for YAML; the `#:schema` comment by default for TOML; the latter two write the document field when the schema defines a top-level `$schema` property), so the next start uses step 2.

Pressing `S` during a session searches and switches the current file's schema: the selected URL overwrites the old pointer, the form, validator, and root menu are rebuilt, and the document content is preserved; cancelling keeps the original schema. This key exists only in the full-screen TUI; the `--accessible` line mode has none.

### 4. All sources fail: schema-less mode

When there is no `-s`, the config has no pointer, and SchemaStore selected nothing, the file still opens:

- The root screen status line shows `no schema found: editing without schema guidance; save-time validation disabled`.
- Objects are edited through the **schema-less key menu**: every document key is listed directly, `enter` edits by runtime type, and `a`/`A` adds keys with a choice from built-in types.
- Arrays enter item management; scalars/other root values are edited by runtime type.
- `E` edits the value on the cursor line in an external editor; when the cursor is not on a value line, it edits the whole document.
- No `$schema` pointer is written; no schema validation runs at save. Without a schema, document keys are no longer marked `(not in schema)` and no unknown-key warnings are produced.
- When the catalog is unavailable, only an extra notice `schema catalog unavailable: ...` is appended; editing is not blocked.
- Aborting the picker during automatic resolution (`esc`, enter/EOF) means "no schema wanted" and proceeds into schema-less mode.
- `--pick` is an explicit request: a catalog download failure exits 1; an aborted picker still reports `no schema found: ...` and exits 2.
- When the picker is needed but stdin is not a terminal: `schema selection needs a terminal; rerun with --accessible or provide -s`, exit 2.

## Schema fetch failures

| Situation | Behavior |
|---|---|
| Local path missing / unreadable | exit 1 |
| HTTP 404 and other client errors | exit 1, never masked by the cache |
| Network errors, timeouts, 5xx, with no cache | exit 1 |
| Network errors, timeouts, 5xx, with a cache | Start from the cache; the status line shows `<warn> using the cached schema: ...` |
| HTTP 429 (rate limited) | Prefer the cache with a notice; error if there is none. The rate-limit window (`Retry-After`, default 60 seconds) is persisted across processes so that every start does not hit the origin |
| Response body over 32 MiB | exit 1 (`response exceeds size limit`) |

Remote `$ref`s (relative to the base URI, `$id`) are fetched with the same load/cache logic. When a ref is unreachable, that field degrades to raw text editing without blocking the other fields; if save-time validation cannot compile because of it, it is disabled (see [Validation and saving](validation.md#degradation-rules)).

## Remote schema cache

Remote schemas and the SchemaStore catalog share one cache.

- **Location**: `$JSE_CACHE_DIR` (explicit override) → `$XDG_CACHE_HOME/jse` → `jse` under the platform default cache directory. On Linux this is usually `~/.cache/jse`. Each URL is cached as `cache/fetch/<sha256>.body` plus a same-named `.json` metadata file.
- **Valid for 24 hours**: counted from the last successful fetch/renewal. Within that window the cache is read directly with **no requests at all**.
- **Conditional requests after expiry**: with `If-None-Match` / `If-Modified-Since`. A `304` keeps using the cache and renews it; only a `200` rewrites the cache.
- **SchemaStore catalog exception**: an expired catalog also returns the cached copy first and refreshes conditionally in the background, so restricted networks / slow proxies never hold up startup; schema bodies still follow the 24-hour rule above.
- **Offline fallback**: network errors or 5xx start from the cache; 404 and other client errors are not masked.
- **One fetch per run**: the form model and the validator share the same bytes; failures for the same URL are memoized too, so multi-file starts do not retry per file. The catalog's background refresh is also deduplicated by URL, so multiple files or repeated `S` presses do not stack requests.
- **Local files are not cached**: local paths and `file://` are read every time.
- **Unwritable cache directory**: silently degrades to no caching; functionality is unaffected.

To force a refresh:

```sh
rm -rf ~/.cache/jse
# or use a throwaway cache directory
JSE_CACHE_DIR=$(mktemp -d) jse -s https://example.com/schema.json config.json
```

## Intranet / mirrors

```sh
JSE_CATALOG_URL=https://mirror.example.com/catalog.json \
JSE_CACHE_DIR=/var/cache/jse \
jse config.json
```

`JSE_CATALOG_URL` accepts any location `fetch::load` supports (an http(s) URL or a local path) and follows the same 24-hour caching and offline fallback.
