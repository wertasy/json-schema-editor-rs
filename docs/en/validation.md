# Validation and saving

jse treats the form as an editing aid and **JSON Schema validation** as the authoritative judgment: keywords the form does not cover, cross-field constraints, `additionalProperties: false` and the like are all checked at save time by the `jsonschema` library.

## Supported form keywords

| Category | Keywords | Form behavior |
|---|---|---|
| Meta | `title` `description` `default` `examples[0]` | Title, description, pre-filled value, placeholder |
| Scalar | `type: string \| integer \| number \| boolean` | Text input; boolean toggles |
| Enum | `enum` / `const` | Inline dropdown (`enter` edits in place and typing filters by keyword — see [Editing features: Enum dropdown](editing.md#enum-dropdown)); all-const / single-value enum variants merge into one enum |
| Numeric constraints | `minimum` `maximum` `exclusiveMinimum` `exclusiveMaximum` `multipleOf` | Validated immediately on input |
| String constraints | `minLength` `maxLength` `pattern` | Validated immediately on input |
| Object | `properties` `required` | Fields in schema order; required blocks empty values |
| Object map | `additionalProperties` | Document members are the keys; `a` adds, `d` deletes, values are edited per the sub-schema |
| Pattern map | `patternProperties` (single pattern + `additionalProperties: false`) | Same as map, but keys added by `a` must match the regex |
| Array | `items` `minItems` `maxItems` | Enum items → multi-select; scalars → input entry by entry; objects → a form per entry |
| Composition | `oneOf` / `anyOf` | Pick a variant, then into the sub-editor; open string + string enum merges into a text input with completion |
| Composition | `allOf` | Constraints intersect: same-named `properties`/`items`/`additionalProperties`/`patternProperties` merge recursively (`$ref` on either side takes effect), `required` takes the union, numeric/length/array and object count bounds take the stricter one; `oneOf`/`anyOf` inside expand distributively |
| Reference | `$ref` | Local pointers and remote refs; cache + cycle protection, depth cap 32 |

Other keywords do not participate in form generation but are still validated at save.

## Degradation rules

- Unreachable `$ref`, array-of-array, unknown types, schema depth over 32 levels, recursive `$ref`: with a value, edit directly by runtime type; with no value/null, use the type picker (String/Number/Boolean/Object/Array/Null, schema `$defs`/`definitions` named types, JSON); picking `JSON` renders a raw text area, with only a syntax check on submit — semantics are left to save-time validation.
- Schema compile failure (invalid schema, unreachable remote ref, etc.): **save-time validation is disabled**, and the root screen status line shows `<warn> schema compile failed, save-time validation disabled: ...`. Saving then has no schema backstop.
- Loaded config already has validation issues: editing is not blocked; the root menu's first line shows the issue count. Load-time errors are display only; after every document modification, validation re-runs and refreshes.
- Explicit non-goals: form validation for `format`, an array reordering UI, forms for `if-then-else`/`not`, map-style editing for multi-pattern `patternProperties`, remote config paths.
- Multi-pattern `patternProperties` generates no map form, but keys matching any pattern are treated as schema-known: no `(not in schema)` marker and no unknown-key warning; keys and values are still edited by runtime shape, with the regex and value schema validated at save.
- Intersection constraints the form cannot express (multiple `pattern`, `not`, `if`/`then`/`else`, multiple `format`, multi-pattern `patternProperties`, etc.) are not dropped: they remain in the original schema; editing may allow intermediate values, but at save time `jsonschema` still validates all of them. `--force` is the only way around.

## Issue list

First line of the root menu:

```text
<err> 2 validation errors · <warn> 1 warning
```

- `enter` opens the issue list; `↑`/`↓` (or `ctrl+j`/`ctrl+k`) select, `enter` jumps to the corresponding field, `esc` closes.
- Entries are also colored by severity: errors red, warnings yellow; locations use document paths, such as `at '$.server.port':`, `at '$.items[0].tag':`.
- **Live refresh**: after every modification the counts, the `<err>`/`<warn>` at field line ends, and the entries update immediately; once everything is fixed, the status line hides automatically.
- After jumping from the issue list, press `esc` in the landing menu to return to the refreshed issue list (cursor preserved); if you drill down further, you return level by level first, then back to the list.
- Search jumps or reopening the list replaces that return point.

## Saving

### Triggers

| Action | Behavior |
|---|---|
| `ctrl+s` / `w` | Saves the current file and stays in the editor; success hints `Write to <path>`, change markers reset. |
| Root screen `esc`/`q` → `y` | Saves all changed files and exits. |
| `W` | Saves a copy to a given path, leaving the session save target unchanged. |
| `--accessible` | Choose `Save and exit` on the root screen. |

### Validation gate

- The current document is validated before saving; with errors, a full-screen error list opens, titled `Cannot save: validation errors`.
- `enter` jumps to the offending field; `n`/`esc` returns to keep editing.
- Only when started with `--force` does the error list allow `y` to force-save; the force-save result line is `Saved with validation errors (--force): <path>`, and the exit code is still 0.
- In `--accessible` mode validation errors are **not printed one by one**; it only asks `Save anyway? [y/N]`, and writes only when `--force` is also set and the answer is `y`. Answering `n` returns to the root screen.

### Write details

- **Atomic write**: a temp file in the same directory + rename; permission bits of an existing file are preserved.
- Key order and numeric literals (such as `1.0`) are preserved as-is.
- Unknown keys outside the schema and the `$schema` key are preserved.
- The output format is decided by the extension; changes from raw editing and `$EDITOR` external editing are written back in the original format as well.
- YAML comments, anchors, aliases, and merge keys are written back following the rules in [Configuration file formats](formats.md).
- TOML comments, key order, and unedited scalar literals are written back following the rules in [Configuration file formats](formats.md); the syntax requires scalar members to come before sub-table headers.

### Multiple files

- `ctrl+s` saves only the current file.
- On root screen exit with several changed files, it asks `Save changes in N files? (y/n)`; `y` saves them one by one (current file first), exiting only when all succeed.
- If a file fails validation, the save flow stops at that file and opens the error list; fix it and exit again, or force-save with `--force`.
- A save failure (permissions, disk errors, etc.) is hinted and the session stays open, with the other files left unsaved.

## Exit codes

| Exit code | Meaning |
|---|---|
| 0 | Saved; or a normal exit with no changes; or `--force` force-save |
| 1 | Runtime error (reading/parsing, schema loading, TUI errors, stdin not a terminal, etc.) |
| 2 | Usage error (arguments, missing config file, `--pick` aborted) |
| 130 | User abort (`n` give up, `ctrl+c`) |

For the complete list of stdout result lines, see [Command line: Exit codes and stdout](cli.md#exit-codes-and-stdout).
