# AGENTS.md

Guidance for coding agents and contributors working on this repository. Read this before changing code.

## What this is

`jse` is a schema-driven terminal editor for JSON/JSONC/YAML configuration files. It began as a Rust port of an upstream Go implementation, but the port is complete and jse now evolves on its own: the Rust code, its tests and `docs/*.md` are the specification. Go references left in module doc comments are historical provenance — do not treat them as authoritative, do not add new ones, and never settle a behavior question by deferring to the Go implementation; decide from the existing tests and docs.

- Binary crate: `jse` (`Cargo.toml`), edition 2024, Rust `stable` (`rust-toolchain.toml`).
- `#![cfg_attr(not(test), forbid(unsafe_code))]` in `src/main.rs`: do not introduce `unsafe`.
- Dependencies are pinned with `=`. Do not add or bump a dependency without a concrete need and without checking the lockfile impact.

## Commands

```sh
cargo test                          # full suite: unit + accessible/pty + schemastore e2e
cargo test --test arch              # A1–A7 architecture invariants (fast)
cargo test pty_multi_file_session   # one test by name
cargo build --release
cargo run -- -s examples/server.schema.json examples/server.config.json
cargo fmt                           # keep formatting local to touched code
cargo clippy --all-targets --all-features  # lint gate; lints live in [lints] (Cargo.toml) + clippy.toml
cargo clippy --all-targets --all-features -- -D clippy::too_many_lines  # CI's architecture gate
```

The e2e tests spawn the built binary. They isolate the schema cache with `JSE_CACHE_DIR`; never let tests touch the user's real cache or the public network. pty tests skip when no pty is available.

Before finishing any behavioral change: run the specific test covering it, then `cargo test`. For UI changes, launch the binary (or the pty e2e) and observe the actual screen; unit tests alone are not proof.

## Architecture map

```
src/main.rs          entry point: module wiring, exit-code mapping, main_test
src/cli.rs           getopt-style flags, Invocation/Flags, usage text (std only)
src/paths.rs         lexical Go-filepath-compatible helpers (std only)
src/session.rs       config load, schema resolution/compilation, save wiring
src/icons.rs         severity glyphs shared by UI/CLI/model
src/doc/             leaf layer (may only use crate::doc and crate::paths)
  jsondoc.rs         arena document tree; order-preserving; exact numeric literals
                     (CopyMode, append_key/idx, encode_value, scalar_text)
  jsoncdoc.rs        JSONC parse + comment capture/write
  yamldoc/           YAML parse (comments, anchors, merge keys) + block emitter
  tomldoc/           TOML parse (tables, dotted keys, table arrays, comments) + emitter
  mod.rs             Codec trait (label/raw_ext/parse_value/write_value + defaults), FORMATS, Meta
src/fetch.rs         local/http(s)/file loading, 24h cache, conditional requests, offline fallback
src/catalog.rs       SchemaStore catalog load + fileMatch glob matching
src/smodel.rs        JSON Schema -> form tree (FormNode arena); degradation to raw
src/validate.rs      save-time validation via jsonschema; document-path error rendering
src/ui/
  mod.rs             FileSession/Options/Outcome, FileState types, MenuState, ValidateFn
  menu.rs            App state, event loop, overlay enum/routing, styles, help text
  keys/              key dispatcher split by feature:
    mod.rs           input bursts, schema switching, list mode, motions, selection
    edit.rs          field editing, arrays, oneOf, value completion
    clipboard.rs     copy/cut + paste
    search.rs        jumps, document search, anchors, merge keys
    overlays.rs      overlay handlers, macros, external $EDITOR
    render.rs        list/overlay rendering
  rows/              pure row model split by feature:
    mod.rs           kinds, row construction, merge hooks
    summary.rs       value summaries
    variant.rs       oneOf signatures/matching
    stack.rs         navigation-stack rebuild
    search.rs        search index
    issues.rs        unknown-key issues
    filter.rs        search filtering
    issue_parse.rs   validator-message parsing
  util.rs            wrap_cursor/page_cursor shared by every list
  files.rs           per-file state, multi-file switching, batch save
  history.rs         undo/redo snapshots (100 steps)
  nav.rs             back/forward location history (100 steps)
  widgets.rs         TextInput/TextArea and schema value apply/validate helpers
  accessible.rs      --accessible line protocol
  lineio.rs          one-line-per-read stdin reader
  schema_picker.rs   SchemaStore picker (TUI + accessible)
  command.rs         vi-style `:` grammar: builtins (w/wa/wq/waq, q/q!), shell
                     escapes, cursor-aware completion (bash-completion for
                     `!` arguments), `%` expansion
  cmd_history.rs     persistent `:` command history file (load/store;
                     path arrives via Options)
  shell.rs           shell execution: `r !` stdout insert, `a !` line append,
                     `w !` document-to-stdin, `!` execute
tests/
  arch.rs            A1–A7 architecture invariants (source scan)
  common/mod.rs      pty + run helpers
  e2e.rs             CLI, accessible scripts, pty flows, YAML anchors/merge, multi-file
  schemastore.rs     catalog matching and picker against tiny_http
```

## Data-model invariants

- `Doc` is an arena: nodes are addressed by `NodeId`, objects keep member order, numbers keep their exact literal text. Never round-trip a number through `f64` or rebuild a document just to move it.
- When a number is *synthesized* (quick `+`/`-`, a YAML scalar that is not valid JSON), spell it the way ECMAScript `Number::toString` does (`1000000`, `0.000001`, `1e-7`, `1e+21`): that is what JS-based LSP/format tooling emits. Untouched literals are never rewritten, so this only applies to values the user actually edits. `jsondoc::number_text` is the single implementation.
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
- The focused description preview merges the schema `description` with the document comment (format meta, read through `Options::comment_at`) and is rendered as separate colored blocks (`DescSource::Schema`/`Comment`); when collapsed, each block is capped at `menu::DESC_BLOCK_PREVIEW_LINES` with an emphasized trailing `…`. `c` expands it transiently (`desc_expanded`; any other key folds it first) or opens `Overlay::Comment` when it cannot fit one screen; that page consumes keys (j/k scrolling, c/q/esc close) and must stay out of `list_mode`. `C` opens `Overlay::CommentEdit` for the focused row (only keys present in the document; empty buffer adds a comment, empty result removes it); applying with ctrl+s writes through `Options::set_comment` into the session meta (format directives preserved), and the next save emits it.
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

## Architecture invariants (enforced by `tests/arch.rs`; A8 by CI's clippy gate)

These are mechanical gates, not suggestions. A change that violates A1–A7 fails `cargo test`; A8 fails the clippy gate.

- **A1 `doc` is a leaf.** `src/doc/**` may reference only `crate::doc` and `crate::paths`. Formats never know about the UI, schema model, fetch layer or session.
- **A2 the UI never loads or compiles schemas.** `src/ui/**` must not call `smodel::load`, `validate::new`, `crate::fetch` or `crate::session`; schema switching goes through `Options::load_schema` and the `LoadedSchema` shape. The catalog picker may query `catalog::` (terminal I/O).
- **A3 core modules never depend on the UI.** `fetch`, `validate`, `catalog`, `smodel` must not name `crate::ui`.
- **A4 `cli`/`paths` are std-only.** Flag parsing and lexical path helpers do not import other crate modules.
- **A5 one definition per helper.** `copy_into`, `append_key`, `append_idx`, `wrap_cursor`, `page_cursor`, `write_indent`, `encode_value`, `array_len` are defined once. The legacy names `path_for`, `path_idx`, `doc_array_len` must not reappear, and no `go_*` identifiers may return (a port leftover; enforced by `no_go_named_helpers`). No module-wide `#![allow(dead_code)]`.
- **A6 per-file state lives in `FileState`.** The document, its `Meta`, its `Codec` and its `Options` are owned per file; no `Arc<Mutex<Meta>>` or other shared mutable document state. `Options` holds only environment I/O (save/write-to targets, clipboard, schema loader).
- **A7 one open surface.** `App.overlay: Option<Overlay>` is the only overlay switch: help and the file picker are variants, and `route_key` matches every variant exhaustively. Adding a surface means adding a variant plus its `typing()` arm, never a new `App` flag.
- **A8 no monoliths.** `clippy::too_many_lines` (threshold 250 in `clippy.toml`) blocks new 250+ line functions; move a feature into its `keys/`/`rows/` submodule instead of appending.

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

## Error types

- Use `thiserror` enums where the caller branches on the cause (the CLI load path uses `session::LoadError` to map usage/runtime failures to exit codes 2/1; `DocError`/`SmodelError` mark the doc/model boundaries).
- Everywhere else keep `Result<_, String>`: those strings are the user-visible message contract (`editor: invalid YAML: …`, validation rows, notices). Do not wrap a message in a single-variant enum just to say "typed"; add a variant only when a caller matches on it.

## Changing behavior

1. Locate the closest existing test and the doc section that states the behavior.
2. Make the smallest change that preserves unrelated behavior; follow local patterns and module doc comments.
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

`.github/workflows/release.yml` builds Linux musl (amd64/arm64), macOS and Windows (amd64/arm64) on `v*` tags, injects `JSE_VERSION_OVERRIDE`/`JSE_COMMIT`/`JSE_BUILD_DATE`, packages binary + README + docs + LICENSE, writes `checksums.txt`, creates the GitHub Release, and then publishes the crate to crates.io. The publish job uses the `CARGO_REGISTRY_TOKEN` repository secret, verifies that the tag matches the `Cargo.toml` version, and skips publishing when that version already exists on crates.io.

A release bumps the version in `Cargo.toml`, `Cargo.lock`, and the version examples in `docs/install.md` before tagging:

```sh
git tag -a vX.Y.Z -m "jse vX.Y.Z"
git push origin vX.Y.Z
```

Once the workflow has created the GitHub Release, replace the auto-generated body with curated release notes (`gh release edit vX.Y.Z --notes-file <file>`): user-facing changes only — what a user of the previous version gains or must watch out for — written as two self-contained sections, Chinese first and English after, with category headings (features / fixes / upgrade notes); never interleave the two languages line-by-line. End with the `Full Changelog` compare link. The workflow's `--generate-notes` only seeds that body; do not ship a release whose notes are just the changelog link.

Do not hand-edit build metadata; `build.rs` and the workflow own it.

Local builds take the highest reachable semver tag (`v0.0.0` when there is none), plus `+dirty` for an uncommitted worktree (untracked files count). Commits after the tag keep its version; `COMMIT` and `BUILD_DATE` identify the exact build. `JSE_VERSION_OVERRIDE` replaces the git-derived version. `src/vcsver.rs` only parses, canonicalizes and orders tags; `build.rs` `include!`s it and the crate compiles it under `cfg(test)` for unit tests. Build scripts rerun only for declared paths, so `build.rs` watches tracked files, their top-level directories, `HEAD`, `index`, `refs`, and `packed-refs`.

## Documentation rules

- [README.md](README.md) is the human landing page: what it is, install, quick start, key highlights, doc index. Keep it short.
- `docs/*.md` is the user manual (Chinese). Each page owns one topic; cross-link instead of duplicating details.
- `docs/en/` mirrors `docs/*.md` 1:1 (same filenames), and `README.en.md` mirrors `README.md`; Chinese is primary. When a user-visible change touches a `docs/*.md` page or README, update the English mirror in the same change and keep cross-file anchors matching.
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
