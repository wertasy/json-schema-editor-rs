# AGENTS.md

Guidance for coding agents and contributors working on this repository. Read this before changing code.

## What this is

`jse` is a schema-driven terminal editor for JSON/JSONC/YAML configuration files. It is a Rust port of an upstream Go implementation; module comments name the Go source file each module ports, and tests were ported 1:1 where practical. Treat the Go behavior (documented in comments and tests) as the specification unless the user asks for a change.

- Binary crate: `jse` (`Cargo.toml`), edition 2024, Rust `stable` (`rust-toolchain.toml`).
- `#![cfg_attr(not(test), forbid(unsafe_code))]` in `src/main.rs`: do not introduce `unsafe`.
- Dependencies are pinned with `=`. Do not add or bump a dependency without a concrete need and without checking the lockfile impact.

## Commands

```sh
cargo test                          # full suite: unit + accessible/pty + schemastore e2e
cargo test pty_multi_file_session   # one test by name
cargo build --release
cargo run -- -s examples/server.schema.json examples/server.config.json
cargo fmt                           # keep formatting local to touched code
```

The e2e tests spawn the built binary. They isolate the schema cache with `JSE_CACHE_DIR`; never let tests touch the user's real cache or the public network. pty tests skip when no pty is available.

Before finishing any behavioral change: run the specific test covering it, then `cargo test`. For UI changes, launch the binary (or the pty e2e) and observe the actual screen; unit tests alone are not proof.

## Architecture map

```
src/main.rs          CLI parsing, session assembly, schema resolution, atomic write
src/doc/
  jsondoc.rs         arena document tree; order-preserving; exact numeric literals
  jsoncdoc.rs        JSONC parse + comment capture/write
  yamldoc/           YAML parse (comments, anchors, merge keys) + block emitter
  tomldoc/           TOML parse (tables, dotted keys, table arrays, comments) + emitter
  mod.rs             Codec trait, format registry (FORMATS), Meta, schema pointer helpers
src/fetch.rs         local/http(s)/file loading, 24h cache, conditional requests, offline fallback
src/catalog.rs       SchemaStore catalog load + fileMatch glob matching
src/smodel.rs        JSON Schema -> form tree (FormNode arena); degradation to raw
src/validate.rs      save-time validation via jsonschema; document-path error rendering
src/ui/
  mod.rs             FileSession/Options/Outcome, menu state types
  menu.rs            App state, event loop, overlay routing, styles, help text
  keys.rs            large key dispatcher + rendering of menus/overlays
  rows.rs            pure row model: menus, summaries, variants, search index, stack rebuild
  files.rs           per-file state, multi-file switching, batch save
  history.rs         undo/redo snapshots (100 steps)
  nav.rs             back/forward location history (100 steps)
  widgets.rs         TextInput/TextArea and schema value apply/validate helpers
  accessible.rs      --accessible line protocol
  lineio.rs          one-line-per-read stdin reader
  schema_picker.rs   SchemaStore picker (TUI + accessible)
  merge.rs paste.rs search.rs select.rs   placeholders; actual logic lives elsewhere
tests/
  common/mod.rs      pty + run helpers
  e2e.rs             CLI, accessible scripts, pty flows, YAML anchors/merge, multi-file
  schemastore.rs     catalog matching and picker against tiny_http
```

## Data-model invariants

- `Doc` is an arena: nodes are addressed by `NodeId`, objects keep member order, numbers keep their exact literal text. Never round-trip a number through `f64` or rebuild a document just to move it.
- `NodeData::raw` is the exact TOML source spelling for values whose Doc form is canonicalized (non-decimal/underlined integers, date/time literals). It is empty for other formats. Writing through a value replaces the node (dropping the old spelling); copying a subtree preserves it.
- `Doc::clone()` preserves alias sharing: YAML aliases and merge refs are the same `NodeId` in multiple places. Undo snapshots rely on this. Use `detach`/`copy` only when the user explicitly materializes a copy.
- A YAML `<<` member is represented as a null-valued node carrying `merge` refs. It is not an ordinary null value; keep merge handling in edges/rows.
- `Meta` carries comments keyed by document path, JSONC tail comments, and the YAML alias count. A save must render with the session's **current** meta. Whole-document raw edits must adopt the freshly parsed meta (`adopt_doc`), or comments/directives are lost.
- Comments follow their member: editing keeps them, deleting drops them, adding creates none. Preserve that when adding operations.
- A JSON/JSONC file with no comments must write byte-for-byte like plain JSON (`JsoncCodec::write` delegates). Do not “improve” output formatting.
- Schema location conventions: JSON/JSONC use top-level `$schema`; YAML uses the `# yaml-language-server: $schema=...` header; TOML uses `#:schema <location>`; for YAML/TOML, when the loaded schema declares a top-level `$schema` property the document member is used instead (and any directive is removed). `set_schema_location`/`set_schema_location_with` keep an existing non-empty value; `--pick` clears first.
- A session with no schema uses `Schema::schema_less`: the root follows the document's runtime shape (object → key menu, array → item menu, other → runtime-typed field). Do not flag its keys as unknown, and rebuild the root after a whole-document raw edit changes the root type.
- Local relative `$schema` paths resolve against the config file's directory, falling back to the current working directory only when the file does not exist there.

## UI invariants

- Editing is **write-through** to the live document; there is no form submit. Saving only serializes the current document.
- Exactly one overlay can be open: `App.overlay: Option<Overlay>`. Opening a surface must close/replace the previous one. Overlay routing order is in `menu.rs::route_key`; update history `Inputs`/`list_mode` predicates when adding a surface.
- The focused description preview is capped at `menu::COMMENT_PREVIEW_LINES` with a `...` cut. `c` expands it transiently (`desc_expanded`; any other key folds it first) or opens `Overlay::Comment` when it cannot fit one screen; that page consumes keys (j/k scrolling, c/q/esc close) and must stay out of `list_mode`.
- Per-file state lives in `FileState` (`files[cur]`): menu stack, cursor, selection, undo/redo, baseline, live issues, parked search/issues returns, navigation origin. Switching files is an index switch; do not add global fields that should be per-file.
- Undo is snapshot-per-key: the dispatcher snapshots before a key and pushes only when the document changed. Any new mutating key must go through that path; do not mutate the document outside the dispatcher without ensuring history/issue refresh.
- The change marker (`●`) compares against `baseline` (load/last save), not undo state. Saving refreshes the baseline.
- `ctrl+s`/`w` saves the **active file only**. Root exit saves all dirty files. Keep stdout result lines and exit codes stable.
- `S` switches the active file's schema through the SchemaStore picker: replace the model/validator, reset that file's stack and navigation, record the new location through `Options::replace_schema_location`, and do not push an undo snapshot. The nested picker disables raw mode/leaves the alternate screen, so the caller must restore both.
- Issues are recomputed after each document mutation; row markers (`<err>`/`<warn>`), root counts, and parked-list returns must stay consistent.
- Selection marks are per menu state; array item labels (`key[N]`) are selectable, action rows are not.
- The root screen is special: `esc`/`q` clears a pending `g` buffer, then a live selection, then exits or prompts to save. `left`/`h` never exits the root.
- `q` aliases `esc` outside text inputs; inside inputs it is a literal character.
- The TUI drains a whole terminal input burst and repaints once per burst (`menu.rs::read_event_burst`); runs of identical `j`/`k`/`up`/`down` collapse through `App::update_repeat`/`move_plain`. That fast path must stay side-effect free (no document mutation, history push or menu-stack change); inputs, overlays and macro recording fall back to one update per key. `KeyEventKind::Repeat` acts like a press; only `Release` is ignored.

## Behavioral contracts

Treat these as public API; tests and docs assert them:

- CLI: getopt-style parsing (`-sv`, `--schema=v`, clustered flags, options after operands, `--`). Multiple config files open one session; each resolves its own schema. All files load before the UI starts.
- Exit codes: `0` saved/unchanged, `1` runtime error, `2` usage error, `130` user abort. Exact stdout lines are in `docs/cli.md`.
- Schema resolution order: `-s` → config pointer → SchemaStore (unique match auto, ambiguous/missing picker) → schema-less editing (`smodel::untyped`; no validation, unknown-key warnings suppressed). `--pick` overrides both and keeps its own failures fatal.
- Remote cache: 24h fresh, conditional revalidation, 5xx/network fallback, 404 hard error, 429 cooldown. Same cache for the catalog and remote `$ref`s. One fetch per URL per process. The SchemaStore catalog uses stale-while-revalidate (`fetch::load_stale_first`): an expired cache is returned immediately and revalidated on a background thread.
- Save-time validation is authoritative; `--force` is the only bypass. A schema compile failure disables validation and shows a persistent warning.
- Saves are atomic (same-directory temp + rename) and preserve the existing file mode, key order, numeric literals, unknown keys, and `$schema`.
- Do not expand YAML aliases on save; do not drop anchors or `<<`.
- Error/notice strings are user-visible. If you change one intentionally, update tests and the relevant `docs/*.md` page.

## Changing behavior

1. Locate the closest existing test and the doc section that states the behavior.
2. Make the smallest change that preserves unrelated behavior; follow local patterns and Go-parity comments.
3. Add/adjust tests for consumer-visible behavior: boundaries, precedence, errors, state transitions. Do not add tests that only restate implementation (wiring, mock echoes, source text).
4. Update [README.md](README.md) if the feature list/keys change, the matching `docs/*.md` page for user behavior, and this file if architecture/invariants change.
5. Run the focused test, then `cargo test`.

When a change invalidates an existing behavior test, decide from the contract: update it only for an intentional behavior change; delete tests that pin incidental wording/implementation rather than rewrite them around the old behavior.

## Format layer

To add a format:

1. Implement `Codec` in `src/doc/` using only `jsondoc`/`Meta`.
2. Register it in `src/doc/mod.rs::FORMATS`.
3. Make `write` faithful (comments, ordering, literals) and supply schema-location read/write/clear.
4. Wire the raw codec in `src/main.rs::load_session` (label, extension, doc/value marshal + unmarshal, `adopt_doc` when format metadata must survive).
5. Add round-trip/comment/anchor tests and a docs page.

## Release

`.github/workflows/release.yml` builds Linux musl (amd64/arm64), macOS and Windows (amd64/arm64) on `v*` tags, injects `JSE_VERSION_OVERRIDE`/`JSE_COMMIT`/`JSE_BUILD_DATE`, packages binary + README + docs + LICENSE, writes `checksums.txt`, and creates the GitHub Release.

```sh
git tag -a vX.Y.Z -m "jse vX.Y.Z"
git push origin vX.Y.Z
```

Do not hand-edit build metadata; `build.rs` and the workflow own it.

Local builds take the highest reachable semver tag (`v0.0.0` when there is none), plus `+dirty` for an uncommitted worktree (untracked files count). Commits after the tag keep its version; `COMMIT` and `BUILD_DATE` identify the exact build. `JSE_VERSION_OVERRIDE` replaces the git-derived version. `src/vcsver.rs` only parses, canonicalizes and orders tags; `build.rs` `include!`s it and the crate compiles it under `cfg(test)` for unit tests. Build scripts rerun only for declared paths, so `build.rs` watches tracked files, their top-level directories, `HEAD`, `index`, `refs`, and `packed-refs`.

## Documentation rules

- [README.md](README.md) is the human landing page: what it is, install, quick start, key highlights, doc index. Keep it short.
- `docs/*.md` is the user manual (Chinese). Each page owns one topic; cross-link instead of duplicating details.
- This file is for agents/contributors: architecture, invariants, commands, workflow. Do not put user-facing tutorials here.
- Any user-visible change updates exactly one primary docs page (plus README if it changes the headline behavior/keys).
- Avoid duplicating the same table in README and docs; README links to the full reference.

## Don't

- Don't add abstractions or dependencies for a single call site.
- Don't reformat or refactor unrelated code in a behavior change.
- Don't weaken or delete a test to make a change pass; first determine whether the test encodes a contract.
- Don't bypass `Options::save`/`write_to` or write files directly from UI code.
- Don't mutate a `Doc` without refreshing issues, history, and change markers.
- Don't commit secrets or real service configs; `config.json` is intentionally ignored.
