# Configuration file formats

## Selection by extension

Case-insensitive:

| Extension | Format | Behavior |
|---|---|---|
| `.json` / `.jsonc` | JSONC | A strict superset of JSON: `//` and `/* ... */` comments plus trailing commas in objects/arrays; the rest of the grammar follows JSON. |
| `.yaml` / `.yml` | YAML | See below. |
| `.toml` | TOML | Root table, `[table]` / `[[array]]`, dotted keys, inline tables, arrays, comments, and date-time literals. |
| Other / no extension | JSONC | Historical behavior: always treated as JSON; files with comments still open. |

When the config file does not exist, jse starts from an empty object and creates the file on save. Read/parse failures report an error and exit 1.

## JSONC

- Supports line comments, block comments, and trailing commas.
- Saving a comment-free `.json` produces output identical to what a plain JSON editor writes (no formatting changes introduced).
- Legacy JSON with comments or trailing commas also opens; trailing commas are not preserved on save (the output is canonical JSON + comments).
- String escapes, number syntax, and so on follow JSON.

### How comments follow

Comments attach to keys/values rather than to fixed line numbers:

- Editing a member: comments are preserved.
- Deleting a member: that member's comments disappear with it.
- Adding a member: no comments.
- A comment on the same line as its value, written after the comma, belongs to that value; a comment on the next line belongs to the next key.
- Blank lines are not guaranteed to be preserved.

## YAML

### Anchors and aliases

Alias references are **not expanded into copies**:

```yaml
base: &base
  host: localhost
  port: 8080
prod: *base
```

- Entering the `prod` menu shows the fields of `base`; edits modify the referenced original object directly, and all references change in sync.
- Summaries on the reference line / anchor definition line carry a `*base` / `&base` badge; the menu title gets `: *base` / `: &base` appended.
- `ctrl+e` or `< Expand >` at the top of the menu materializes the reference as an independent copy; afterwards edits only affect the copy. When not expanded, saving still writes `*base`, and the anchor `&base` is kept.
- `&` sets/modifies an anchor, `*` sets/modifies a reference, `ga` jumps to the anchor definition, `gr` jumps to the other usages. See [Editing: YAML anchors and merge keys](editing.md#yaml-anchors-and-merge-keys).

### Merge keys

`<<: *base1` / `<<: [*base1, *base2]` are preserved as-is; menus merge and display them with YAML semantics:

- Merged keys land in the slots of the corresponding schema fields; the mapping's own keys win, and earlier anchors win.
- Value summaries carry a source `*base1` badge; the title line shows `<<: *base1 *base2`.
- Editing a merged key writes back to the source mapping that defines it; all merging mappings change in sync.
- `ctrl+e` materializes a merged value as an independent copy inside the current mapping, shadowing the merged value.
- `<` adds/modifies merge keys.
- A required merged key cannot be deleted; to break the merge, remove the corresponding `<<` first.

### Scalars and tags

- Timestamps (e.g. `2024-01-02T03:04:05Z`) are edited as strings and written back as the original text on save.
- `!!binary` and unknown tags are treated as plain strings; the tag is dropped on save.
- Numbers are normalized to JSON numbers: `0x1F` → `31`, `1_000` → `1000`; literals that are already valid JSON, such as `1.0` and `1e3`, stay as-is.
- `.inf` / `.nan` cannot be represented in JSON and are parse errors.
- Multi-document files (a second document separated by `---`) and non-string mapping keys are parse errors.
- An empty YAML document is an error; a nonexistent file still starts from an empty object.

### Write-back format

YAML is written back by the built-in emitter, styled after yaml.v3's block output:

- 2-space indentation, no width wrapping.
- Empty collections use flow form (`{}` / `[]`).
- Multi-line strings use literal blocks with chomping indicators.
- Double-quoted escapes, including astral characters.
- Blank lines and some quote styles may be normalized; semantics, comments, anchors, and key order are preserved.

## TOML

### Structure and scalars

- A TOML document is treated as a root table: `[server]` and `server.host = ...` both produce nested objects, `[[mounts]]` produces an array of objects, and `{ x = 1 }` is an inline table.
- Comments follow members: a comment on the same line as a key/table header stays on that line; a comment on its own line belongs to the next key/table header; comments at the end of the file stay at the end of the file.
- Hexadecimal/octal/binary integers and underscored numbers (`0x1F`, `0o17`, `0b101`, `1_000`) are written back as the original literal **when unedited**; after a numeric field is edited it is written as canonical decimal.
- Date/time literals (`1979-05-27T07:32:00Z`, `07:32:00`, etc.) are displayed and validated as strings: the original literal is kept when unedited; after the field is edited it is written as a quoted TOML string.
- `inf` / `nan` cannot be represented in JSON and are parse errors; duplicate keys, tables defined twice, and attempts to extend an inline table are also parse errors.
- The root value must be a table. An empty TOML file equals an empty table; a nonexistent file still starts from an empty object.

### Write-back format

TOML is written back in canonical form by the built-in emitter:

- Scalar members come before sub-table headers (required by TOML syntax; later keys would otherwise belong to the preceding table).
- Arrays whose elements are all tables are written as `[[array]]`; other arrays are written inline.
- Key order, comments, and unedited scalar literals are preserved; blank lines, the spelling of dotted keys, quote styles, and inline table layout may be normalized.
- `null` placeholder keys that were created/edited in are not written (TOML has no null); a null inside an array makes saving fail with an error.

## How the schema pointer is recorded

| Format | How the `-s` / SchemaStore selection result is recorded |
|---|---|
| JSON / JSONC | Top-level `"$schema": "<location>"`. An existing non-empty value is kept; an existing empty value / explicit null is filled in place; when missing it is inserted as the first key. |
| YAML | By default writes the file header `# yaml-language-server: $schema=<location>`. **If the loaded schema defines a top-level `$schema` property**, the document's `$schema` field is rewritten (inserted as the first key when missing, filled in place when empty / explicitly null) and the old comment of the same kind is removed; an existing non-empty `$schema` value or comment is kept. |
| TOML | By default writes the file header `#:schema <location>`. **If the loaded schema defines a top-level `$schema` property**, the document's `"$schema"` field is rewritten (inserted as the first key when missing, filled in place when empty / explicitly null) and the old comment of the same kind is removed; an existing non-empty `$schema` value or comment is kept. |

All of these pointers are recognized as schema sources on later starts. `--pick` clears the old pointer before writing the new one.

## Raw editing and external editors

The built-in raw editor, the `$EDITOR` external editor, and editing the value of a key not defined in the schema all use the **file's own format**:

- JSON/JSONC files: JSON text.
- YAML files: YAML text.
- TOML files: the whole document uses TOML; a single value uses a TOML value expression (scalar, `[...]`, `{ ... }`).

Exception: when the `$EDITOR` external editor and the built-in raw editor edit a **string value**, they use the string's own text (no quotes, no escapes) and write it back verbatim as the string on apply.

Editing the whole document, editing a field that cannot be represented as a form, and editing a schema-undefined key with no value all behave the same. Syntax is checked in the corresponding format on submit; semantic validity is left to save-time validation.

## Degradation to raw editing

In the following cases the field is rendered as an "Edit as JSON/YAML/TOML" text area:

- `$ref` is unreachable;
- structures that cannot be represented as a form, such as arrays of arrays;
- unknown types;
- schema depth exceeds 32 levels;
- a recursive `$ref` cannot be expanded within a finite depth.

Submitting the text area only checks syntax; schema validation still acts as the final safety net at save time. See [Validation and saving: degradation rules](validation.md#degradation-rules).

## Adding a new format

Adding a new format only requires:

1. Implementing a conversion module under `src/doc/` that depends only on `src/doc/jsondoc`;
2. Implementing the `Codec` trait from `src/doc/mod.rs` (`extensions`, `parse`, `write`, `empty`, schema pointer read/write, `notices`);
3. Adding the implementation to the `FORMATS` registry in the same file.

Format metadata (comment tables, YAML alias counts, etc.) is stored in `Meta` and saved with the session; `write` must be able to write back faithfully after edits. For the exact constraints see [AGENTS.md](../../AGENTS.md#format-layer).
