# Editing features

All edits are **written back immediately** to the in-memory document; saving just writes the current document to disk. Each key press produces at most one undo step.

## Field editing

| Field type | `enter` behavior |
|---|---|
| `string` / `integer` / `number` | In-place input below the cursor line; invalid values are blocked in red text and not written. |
| `boolean` | Toggle (`space` also works). |
| `enum` | Opens inline input with the options listed in an autocomplete-style dropdown (see [Enum dropdown](#enum-dropdown) below). `space` toggles array members. |
| Plain object | Enters the sub-menu. |
| Array | Enters the array management menu. |
| Map (`additionalProperties`) | Enters the map menu; keys are document members. |
| `oneOf` / `anyOf` | Variant selection, or direct editing of the current variant. |
| Not form-able / no `type` | Existing values are edited directly by their runtime type (scalars in place, booleans toggled, objects/arrays into sub-menus); the type picker comes first only when there is no value or an explicit null. `t` explicitly opens the type picker. |

Inside the input box:

- `enter` applies, `esc` cancels.
- `tab`/`ctrl+n`, `shift+tab`/`ctrl+p` (aliases `ctrl+j`/`ctrl+k`, active while the candidate window is open) cycle through completion candidates.
- Normal inline movement/deletion keys work; `alt+f`/`alt+b` move by word (whitespace and `,.+-=_/\:?!$#@*` split words), `ctrl+a`/`ctrl+e` go to line start/end, `ctrl+k`/`ctrl+u`/`ctrl+w` delete.
- When the terminal is narrow and the value is wider than the input area, the input box scrolls horizontally and the viewport always follows the cursor (the cursor and the text around it stay visible); text outside the viewport is taken from the original value and is never modified or truncated.

### Text autocomplete

- Candidates first come from **existing values of the same key in the document**: string fields take string values, `integer`/`number` take numeric literals; deduplicated, in document order, excluding this spot.
- Filtered by the entered prefix, case-insensitive.
- **Multi-line values do not participate in completion** (strings containing newlines or tabs), to avoid breaking the hint line.
- When the schema offers a set of values, only those are used first (e.g. in `oneOf`, an "open string + string set" merges into a text input, such as workflow's `shell`). Only when the entered prefix matches none of the options does it fall back to document candidates; the field remains free text, and values not listed by the schema can still be entered.
- Fields with `x-tag-reference` take `tag` values as candidates by reference domain: e.g. `http_client` → `http_clients[].tag` (both singular and plural key names accepted, at any depth); compound domain names also accept a nested match where the last word (including its plural) appears under a member named after the first word, e.g. `dns_server` → `dns.servers[].tag`.
- Scalar variants of `oneOf`/`anyOf` and strings inside object variants complete the same way; `$ref` shared definitions also complete by the field's own key.
- The hint line `tab: a · b · c` shows a scrolling window of three candidates, with `…` on either side indicating more; `tab`/`shift+tab` cycle forward/backward filling in candidates, wrapping at both ends.

### Enum dropdown

- Scalar enums (including discriminant keys in oneOf union menus) no longer open a separate selection page: `enter` opens the input box in place on the field line, with all options listed in an autocomplete-style dropdown; when the current value is among the options, that option is pre-filled and highlighted (when the value is missing — including explicit null — or not among the options, the default value is used, then the first item).
- The input box content is the search keyword: typing filters options by **case-insensitive substring**; when the box still holds the highlighted option, typing directly overwrites the whole value and restarts the search (combobox convention). `↑`/`↓`/`tab` cycle through matches, `backspace` corrects the query.
- `enter` applies the highlighted item; when the box content equals an option (case-insensitive), that option is applied as well. What gets applied is the schema's option node itself: non-string members (booleans, numbers, null) and numeric literals are written back as-is. With no match, `enter` shows `must be one of: ...` in red and lists all options, leaving the document unchanged; `esc` cancels.
- An option identical to the input is **not** filtered out — it is the very value to apply; an empty query lists all options.

## Objects, maps and arrays

### Objects and maps

- Fields are ordered by schema declaration; keys not defined by the schema come after, keeping document order.
- `a` / `A`: add a schema-undefined key before/after the cursor member. Type the key name first, then press `enter`:
  - The key name must not be empty and must not duplicate a schema field or an existing key.
  - Map (`additionalProperties`) values are edited according to the sub-schema; object/array values create an empty container immediately; values without a concrete type pick a type first, then enter in-place input, a sub-menu, or the `JSON` raw text area.
  - When a map is constrained by `patternProperties`, the new key must match the regex, otherwise the inline hint `key must match <pattern>` appears.
- `d`: delete the key-value pair; protected rows such as required merge keys refuse.
- A map's keys are document members; deleting a map key deletes that member.

### Arrays

| Key | Action |
|---|---|
| `< Add item >` / `a` / `A` | `< Add item >` shows when the array is empty; `a` / `A` inserts a new entry **before/after** the cursor entry. |
| `D` | Deletes the current array entry (also works inside the entry form; returns to the entry list after deletion), with confirmation `Remove this item? (y/n)`. |
| `d` / `delete` | Deletes the cursor entry (no confirmation). |
| `J` / `K` | Move entry down / up, cursor follows; one key press = one undo step. |
| `Ngg` | Jump to index N (clamped to the last item when out of bounds, `0gg` first item). |
| `v` or `m` | Selects entry labels `key[N]`, for batch `y`/`x`/`d`. |

- Object entries open the entry form; press `D` to delete the current entry — after confirmation you return to the entry list.
- Scalar entries are inserted with a `null` placeholder; canceling the edit leaves an empty slot, which `d` can delete.
- `d` deletes the current key-value pair or array entry without confirmation (protected rows such as required merge keys refuse); `D` deleting an array entry (in the entry list or inside the entry form) keeps the original this-item row's `y/n` confirmation. The differing confirmation policies are intentional (`d` covers key-value pairs and entry rows, `D` is dedicated to array entries and can also be triggered inside the entry form), not an omission.
- `oneOf` / any-type entries likewise place a `null` placeholder first and open the type picker; after a scalar is picked, editing happens in place on that entry line.
- Object/array entries create an empty container placeholder immediately on insertion.
- Arrays whose `items` is an enum display as a multi-select toggle list; `/` filters by option text, `enter` toggles the current match (the filter persists, so you can keep toggling), `esc` cancels the filter and restores the cursor to the row it was on when the search opened.

### oneOf / anyOf

- One variant per line: `●` marks the current variant + variant name; untitled variants use the structural signature in the title column (consistent with the any-type picker), and when titled, the structural signature is shown on the right — no synthetic `variant N` labels.
- **No value**: pick a type first, then edit.
- **Existing value**: `enter` skips type picking and goes straight into the current variant's editing interface.
- With the cursor on a variant key (the oneOf field row) or inside the variant sub-menu, `t` opens the variant picker; no need to enter scalar editing first (inside the enum variant's dropdown, `t` is a plain character — `esc` out first, then press). Switching an existing value first confirms `Replace current value? (y/n)`; on confirmation, if the original value is a scalar and can serve as an element of the target array (e.g. `string` → `array[string]`, `integer`/`number` → an array of the corresponding element type), the original value is kept as the array's first element, otherwise discarded (or the schema default is written).
- Scalar variants: return to the parent menu and edit in place on the field line; after commit `●` moves to that variant.
- Enum variants: the same inline dropdown as scalar enums (after `esc` exits the edit box, press `t` again to switch variants).
- Object/array/map variants: their own sub-menus (map variants list document keys, with `a`/`d`).
- Arrays whose `items` is an "all-object oneOf": the empty array's `< Add item >` (or any array's `a`/`A`) enters the union field menu, which identifies the variant automatically from already-filled fields (such as a discriminant `const`); changing the discriminant key switches variants. Keys merged in via `<<` participate in identification too.
- The plain oneOf variant picker and the any-type picker share the same search bar component (`/` to enter, the `search:` status line, identical editing/movement keys), but the list contains only that oneOf's own variants — JSON built-in types or `$defs` types are never mixed in.
- The `variant:` line at the bottom of the picker shows the currently matched variant: with a type name it shows `variant: DnsRule {type, inbound, ip_version, query_type, +51}`, without one, only the structural signature.
- `allOf` merges by constraint intersection: same-named `properties`, `items`, `additionalProperties`, `patternProperties` merge recursively (`$ref` on either side takes effect), `required` takes the union, numeric/length/array and object count bounds take the stricter one; any `oneOf`/`anyOf` inside expand distributively into nested variants.

### Keys not defined by the schema

Keys present in the document but missing from the schema display as yellow rows, with `(not in schema)` and a value summary, `<warn>` at end of line:

- With an existing value, `enter` edits directly by the value's runtime type: strings/numbers in-place input, booleans toggle, objects/arrays enter the schema-less sub-menu; no type picker pops up first, and `$EDITOR` is not launched directly.
- Only when there is no value (missing or explicit null) does the type picker open proactively: JSON built-in types `String`, `Number`, `Boolean`, `Object`, `Array`, `Null`, plus named types defined in the schema's top-level `$defs` / `definitions` (in definition order, after `Null` and before `JSON`).
- `t` can explicitly open the same type picker on an existing value, for proactively switching types; confirm `Replace current value? (y/n)` before switching, and scalar-to-compatible-array-variant switches likewise keep the original value as the first element.
- Picking a schema-defined type uses that definition's real form: objects drill down by `properties`, enums enter the inline dropdown, arrays enter the entry menu, and `oneOf` definitions enter their variant picker; definitions degraded to raw (e.g. recursive or unreachable `$ref`) are not listed.
- When there are many types, press `/` inside the variant/type picker to enter search mode. The search bar matches `Select a schema`: the query starts empty with a block cursor, supporting `←`/`→`/`Home`/`End`, `alt+f`/`alt+b` word-wise movement, `backspace`/`delete`, `ctrl+a/e/k/u/w` editing; `tab`/`shift+tab` and `ctrl+j`/`ctrl+k` move through results. The cursor stays on the current row (the current variant when the picker opened); typing narrows the list in real time (matching titles, keys, and structural signatures). `enter` selects, `esc` clears the search and restores the cursor to the row it was on when the search opened; `u` inside search mode is a plain character — it triggers neither undo nor the field search overlay.
- Inside the picker, `String`/`Number` edit in place, `Boolean` toggles directly, `Object`/`Array` enter sub-menus where keys/entries can be added or removed directly, `Null` writes `null`; `JSON` opens the built-in raw text area, pre-filled with the existing value in the current file format (pressing `e` explicitly still opens the external editor).
- `d` deletes.
- Keys added by `a`/`A` have no value, so a type is picked first; the key sits with a `null` placeholder at its insertion position and can be deleted with `d` after canceling type selection.
- It also enters the issue list as an editor-level warning (`unknown key ... not in the schema`) without blocking saving; the top-level `$schema` (the pointer the editor writes back automatically) does not warn. A key matching any `patternProperties` pattern counts as a schema-known key and shows neither the marker nor the warning (multi-pattern `patternProperties` generates no map form; editing still follows the runtime shape). An object whose `additionalProperties` is `true` or a schema is likewise an open mapping: keys outside `properties` show neither the marker nor the warning, and still edit by their runtime shape. oneOf/anyOf object branches follow the same rule: when the value does not pin a single variant, any open branch counts; a pinned variant governs.
- Fields whose type the schema explicitly defines never show this picker: `enter` always edits according to the schema type.
- If `additionalProperties: false` has already produced a validation error, the unknown-key warning is not listed a second time.

## Raw editing and the external editor

- **raw JSON/YAML/TOML external editing**: `E` edits **the value on the cursor line** in `$EDITOR`; when the cursor is not on a value line (or the root is a scalar/empty), it edits the whole document. Content follows the file's own format (JSON/YAML/TOML). On exit it is parsed and applied as one undo step; invalid syntax only warns, and the original document is kept.
- Raw editing of the whole document (built-in text area or `$EDITOR`) re-parses format metadata when applied: comments and schema directives in the new text take effect along with the document, and old comments do not linger.
- **Built-in raw text area**: used for fields that cannot be represented as a form, and for the `JSON` variant in the any-type picker; `ctrl+s` applies, `esc` cancels; syntax errors stay in the text area with a hint.
- **Single-value external editing**: `e` opens `$EDITOR` on the cursor line to edit that value; with no value it hints `no value to edit`. Object/array container rows work too.
- **Multi-line strings**: a string containing newlines does not edit in place on `enter` — it goes straight to `$EDITOR` (the temp file contains only that value; JSON/TOML use escapes, YAML uses block scalars).
- GUI editors need wait mode, e.g. set `EDITOR="code --wait"`.

## Copy, cut and paste

### Copy / cut

`y` / `x` act on **`"key": value` key-value pairs**, `Y` / `X` act on **the value only**.

- Text format follows the file type:
  - JSON/JSONC: `"name": "webapp"`, values in compact JSON; key order and numeric literals like `1.0` are preserved.
  - YAML: `name: webapp`; container values/pairs output as multi-line blocks; anchors/references are exported as plain values.
  - TOML: key-value pairs are written as inline tables `{ name = "webapp" }`; values use TOML value expressions; unedited literals such as datetimes are kept as-is.
- Strings are copied verbatim in every format (no quotes/escaping); numbers/booleans/explicit null as literals.
- **Array entry menu**: both `y`/`Y` copy the element value; both `x`/`X` cut the element value and delete the entry. On other pages `y`/`x` refuse rows without object keys (`y copies object keys only`).
- Fields missing from the document hint `no value to copy`; action rows hint `nothing to copy`.
- Success hints `Copied key-value pair` / `Copied value`; when the system clipboard is unavailable and the copy went only to the built-in clipboard, the hint carries the ` (built-in clipboard)` suffix; failure hints `copy failed: ...` without showing the copied content.
- Cut = copy + delete; a whole batch cut counts as one undo step. When writing to the system clipboard fails, it falls back to the built-in clipboard and the cut still deletes; when the delete side refuses (required merge rows, anchor carriers, etc.), the row is kept, the copied content is kept, and the reason is hinted.
- The system clipboard depends on platform tools, see [Installation: runtime requirements](install.md#runtime-requirements); on Windows it is write-only. With no tools, on tool execution failure, or on read failure, jse falls back to the **session built-in clipboard**: it keeps what `y`/`x` copied during this session, and `p`/`P`/`ctrl+v` read the system clipboard first, pasting the built-in content back on failure. The built-in clipboard lives only within the current session and never enters the system clipboard.

### Smart paste

`p` pastes into **the container the current menu shows**; `P` pastes into **the container the cursor points at** (when the cursor is not on a container row, it falls back to the current menu's container).

- `P` on a value row with a **single value** in the clipboard (`123`, `"hello"`, `true`, `null`): replaces that value directly, one undo step. Value rows include plain fields, object/array/oneOf key-value rows, rows inside the oneOf variant picker, and root scalar/root oneOf documents. With multi-select, every selected row is replaced; selected rows without a value get the key created, hinting `pasted value into N keys`.
- **Plain text** can be pasted directly when the target can hold a string (works in JSON/JSONC/YAML/TOML): the current value is itself a string, or the schema is a text field/enum/unconstrained any-JSON field/a oneOf with string leaves. `y`/`Y` copy strings without quotes in the first place, so `P` pastes them back verbatim; text wrapped in a pair of `""` becomes string content once the quotes are stripped. Targets like plain numbers, booleans, and containers still reject unparseable text.
- Inside any text input box (value editing, the raw text area, search, new key, write path, anchors/merge keys, comment editing), `ctrl+v` or the terminal's `ctrl+shift+v` (bracketed paste) inserts system clipboard text at the cursor (on read failure, the session built-in clipboard is used instead, see above); multi-line text is inserted line by line in the raw text area.
- When `p` encounters a single value it only hints: `a single value replaces a row's value — press P on the row`.
- A complete JSON object `{...}` in the clipboard: keys merge into the map. Existing keys overwrite values in place; new keys are inserted below the cursor row (`p`) or above it (`P`) in clipboard order. When the cursor row has no key, they are appended at the end.
- `P` pastes into the cursor row's own container: object/array rows paste into that value; **oneOf rows use the current variant's object or array value** as the target, while `p` still pastes into the current menu's container.
- The **array entry menu**'s `p`/`P` are positional pastes: clipboard content (partial elements, a complete `[...]`, or a complete `{...}` as a single entry) is inserted below/above the cursor entry; appended when the cursor is not on an entry row.
- Partial content without brackets is recognized automatically as key-value pair / array element; a single trailing comma is stripped automatically.
- **Plain text pasted into an array** splits into entries: multi-line splits by line; a single line splits on **top-level** commas/whitespace (separators inside quotes and brackets do not split — `{"src":"/c"},{"src":"/d"}` is two objects). Each piece is first tried as a JSON value, falling back to a string of the piece's literal text when unparseable. `P` on a string row still takes the whole single-line text as that row's string value; parseable single values (`123`, `"hello"`) are unchanged, and `p` still only hints.
- Type mismatches (key-value pairs into an enum array, array elements into a map, etc.), an empty clipboard, or invalid JSON/YAML/TOML: hint only, no document change (plain text into array targets is not in this category — it splits into entries per the bullet above).
- While editing a YAML document, YAML is accepted as well: block mappings (`x: 1` per line), block sequences (`- 1` per line), flow `{...}`/`[...]`; while editing a TOML document, TOML documents are accepted (`x = 1`, `[table]`) or single TOML values (including plain strings copied by `y`/`Y`); JSON works as usual in every format.
- Each paste = one undo step; validation still runs on save.

### Multi-select and range select

- `m`: toggles the marker on the cursor row. Selectable rows are object menu key-value rows (including schema-undefined keys) and array menu entry labels; action rows are not selectable. Selected rows show a red `>` at line start, and the key/label turns red.
- Once selected:
  - `y` copies everything at once: object menus copy as a compact object/block mapping; array menus compress each element into a single-line compact value and join them with commas (`{"src":"/a"},{"src":"/b"}`; bare-safe strings as-is, other strings quoted), which can be pasted back into any array as-is.
  - `d` deletes everything at once (one undo step; arrays delete in reverse index order; protected rows are skipped with a hint).
  - `x`/`X` batch cut.
  - The selection clears after an action.
  - `esc`/`q` first clears the selection and stays in the current menu; only with no selection do they go up one level.
- `v`: range select. The selection is the contiguous range between the starting point (where `v` was pressed) and the cursor; `j`/`k`/arrow keys extend/shrink it while moving; `gg`/`G` jumps to the first/last selectable row in one step, marking every row passed. `v` ends range select; `esc`/`q` ends it and clears the markers.
- When `y`/`x`/`X`/`d`/`p`/`P` act on a selection they automatically end range select first, then perform the action; after the action the selection clears and `j`/`k` resume normal movement.
- Entering or returning from a sub-menu ends range select; output hints appear in the bottom help bar.

## Search and replace

### Search

`/` searches the current subtree **downward from the current breadcrumb path**. The index is rebuilt when it opens; `esc` returns to the menu as it was before the search opened.

- Matches by **regex** by default; `alt+x` switches to literal substring.
- Case-insensitive by default; `alt+i` toggles.
- Matches titles/key names **or the row's own value**; the overview of object/array/oneOf rows is a child summary and does not participate in matching (the child rows themselves can hit). Unset `—` does not participate.
- Each result row is `path  overview`, with the path format matching the breadcrumb.
- Ordering: rows whose key name exactly matches the query come first, the rest keep document order.
- Queries starting with `$` match by document path: `$`, `$.`, `$[` prefixes; array indices are written `[N]`.
- `enter` jumps to the corresponding menu and positions there; **jumps are reversible**: `esc` at the landing point returns to the search results (rebuilding the index, keeping query and cursor); if you drill down further, you return level by level first, then back to the list. `esc` in the search list closes it and returns to the menu as it was before the search opened.
- Hits in schema-less files and schema-undefined keys rebuild the menu by the value's **runtime shape** (object values enter the key menu, array values the entry menu), so the landing point is directly the highlighted member/entry row — no need to `enter` down level by level.
- A `oneOf` variant key produces a single result (`… —` when unset); when an array element is a variant, `enter` positions to the row containing that key (entering the current variant editor when necessary), never opening the old variant selection page first.
- Only parent objects/array elements actually present in the document are indexed: when a parent object is missing, an array empty, or an index out of bounds, paths like `$.endpoints[0].servers[0].server` are never fabricated.
- Array elements are indexed by document value, regardless of the `items` shape: elements of enum arrays appear as well (`enter` positions to the corresponding toggle row); arrays whose `items` is missing or degraded to raw (e.g. nested arrays) index their internal values by raw path too, and jumping locates the specific element or the field row holding the value by runtime shape; fields hitting an array variant inside `anyOf`/`oneOf` (e.g. sing-box `rule_set: string | string[]`) expand by their array elements and jump to the entry menu.
- Fields whose form layer degraded to raw (boolean schemas, recursive `$ref`, unsupported structures) and raw values of map entries: their internal keys and array elements enter the index too; jumping rebuilds their object/array menu by runtime shape and positions to the specific key/element.

### Replace

- `r` enters replace mode directly; or after opening `/`, press `tab` to bring up the replace input box (`tab` switches search/replace focus).
- The result list shows previews of string value hits: `path  old → new`. Numbers/booleans/enums/containers do not participate; only strings actually present in the document are replaced (defaults of unset fields are not replaced), and strings in schema-undefined keys are replaceable too.
- `enter` (in the replace box): replaces **all occurrences** within the currently highlighted value and moves to the next hit.
- `ctrl+r`: replaces everything at once; closes the overlay and hints `replaced N occurrence(s) in M value(s)`, the cursor lands on the first replaced value automatically, and the remaining replacement spots can be cycled with `ctrl+j`/`ctrl+k`.
- `esc`: in replace mode it first returns to the plain search list (keeping query and cursor); press again to close the overlay. Completed replacements are not undone.
- With an empty query, `enter`/`ctrl+r` do nothing.
- The case toggle and regex mode apply uniformly to both search and replace; the regex replacement string can reference capture groups with `$1`/`${1}`, and `$0` is the whole match.
- Each replacement = one undo step; replace-all is one step too.

## Undo / redo

- In list mode `u` undoes, `ctrl+r` redoes.
- Each key press = one step; batch delete, batch cut, paste, and replace-all each count as one step.
- New modifications clear the redo chain; each side is capped at 100 steps.
- Undo/redo does not change the "last saved" baseline; change markers remain relative to the last save.
- If the current menu path no longer exists after an undo, it falls back to the nearest level that still exists.

## Macros

- `@` starts recording (a red `● REC n` at the right of the bottom bar); press `@` again to stop.
- `.` replays the last macro.
- Inside input boxes (in-place editing, raw editing, search) `@`/`.` are plain characters — they get recorded and typed as usual on replay.
- While recording, `@` in any non-input context (including overlays) stops recording first; when not recording, `@`/`.` are intercepted in all non-input contexts (including the issue list).
- Stopping with an empty buffer = cancel, keeping the previous macro; while recording (non-input contexts), `.` is ignored and not recorded, so replay never triggers nested.
- `q` in non-input contexts equals `esc` and is recorded as `esc`; inside input boxes `q` is a plain character, recorded as usual.
- Memory only, a single macro, capped at 500 keys (it stops when the cap is reached, keeping what was recorded).
- Replay starts immediately from the current interface state; it stops on `ctrl+c` or root screen exit; `ctrl+s` only saves and replay continues. Landing in different menus may produce different results — that is the user's responsibility.

## Save a copy `W`

- In the root menu/list mode, `W` opens a `Write to` input line **at the bottom of the terminal, above the help bar** (no `>` cursor prefix), pre-filled with the current file path and directly editable; the menu list does not yield scroll rows for it.
- `enter` writes to that path, using the same validation gate as `ctrl+s`; on validation failure the error list opens, and with `--force` you can force-save to that path.
- Success hints `Wrote to ...`; `esc` cancels; an empty path hints `path required`.
- Path completion: matches from the current directory are listed in real time below the input line (case-insensitive; directories and symlinks to directories get a `/`; dot-files appear only when the prefix starts with `.`); `tab`/`ctrl+n`, `shift+tab`/`ctrl+p` cycle; `enter` always writes the current inline text.
- `W` only writes a copy: the session save target, change baseline, and unsaved state all stay unchanged.

## YAML anchors and merge keys

### Anchors / aliases

- Entering the menu of an alias object shows a `< Expand >` row at the top (`ctrl+e` also materializes the reference under the cursor). Below it the referenced original object is displayed; viewing and editing act on the original, and all references change in sync.
- `&`: sets/modifies the current value's anchor. The input box is pre-filled with the current name; emptying it deletes the anchor (refused with the reference count when referenced); renaming syncs all references automatically; duplicate names are refused.
- `*`: makes the current value reference an anchor. `tab` completes among existing anchor names, candidates filtered by the field schema's type, skipping anchors that would create circular references; the entered prefix filters in real time. An empty name removes the reference.
- `ga`: jumps from a reference/anchor row to its definition. `gr`: jumps to the anchor's other usages (the remaining `*alias` references and maps merging it via `<<`; a picker opens when there are several).
- `< Expand >` / `ctrl+e` materializes an independent copy (hinting `alias expanded`); from then on editing affects only the copy; when not expanded, saving writes `*base` as-is.
- Anchor identity survives undo/redo.

### Merge keys

- `<<: *base1` / `<<: [*base1, *base2]` are preserved as-is; the map menu displays them merged with YAML semantics.
- Order: merged keys land in the slots of their corresponding schema fields; this map's own keys and earlier anchors take priority; schema-undefined merged keys come after this map's own rows, in merge order.
- Merge chains resolve layer by layer: an anchor's own keys take priority over the keys it inherits via `<<`; inherited keys point along the chain to the map that actually defines them.
- Value summaries carry a source `*base1` badge, and the title line shows `<<: *base1 *base2`.
- Editing a merged key (in-place input, `space`, enum cycling, `e`, `&`/`*`, drilling down, `d`) writes to the source map that defines it, and all merging parties change in sync.
- A required merged key cannot be deleted with `d` (that would make all merging parties lose the key).
- `ctrl+e` materializes the value as an independent copy within the current map and shadows the merged value (hinting `merged key expanded`).
- `<` edits the current map's merge keys: enter space/comma-separated anchor names; empty input clears the merge; `tab` completes the current token.
- Merge keys participate in oneOf variant identification and required marking.

## File switching and multiple files

- `s` opens the file switcher; each file keeps its breadcrumb, cursor, undo stack, baseline, issue list, and unsaved marker.
- `ctrl+o`/`ctrl+i` move back/forward in the navigation history, across files.
- `ctrl+s`/`w` save only the current file; on root screen exit, all changed files can be saved in one go.
- `W` save-a-copy affects only the target file, not the session.
