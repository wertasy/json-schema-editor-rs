# Interface and key bindings

## Screen layout

The full-screen TUI is divided into fixed areas:

```
Title line (menu name + multi-file [1/3] / submenu [1/3 a.json] + unsaved ●)
Breadcrumb line ($.server)
Field list (scrollable; cursor row in reverse video)
  ...
Description line (schema help + doc comment for the current row)
variant: <variant type name> <variant signature>  ← oneOf picker and variant editor; signature only when there is no type name
Status line (cache fallback / schema compile failure and other warnings)  ← root screen only
Transient message (Saved, undone, pasted ...)
Bottom help bar (fixed to the last line; common keys / hints specific to the current mode)
```

- **Title line**: the root menu always shows the config file name (`Cargo.toml`, `config.yaml`, `rust-toolchain.toml`, etc.), not the schema root title; submenus prefer the schema `title` and fall back to the document key when there is no title (map member keys, array entries `key[N]`, oneOf field names). This rule is the same for JSON/JSONC/YAML/TOML. Menus for anchor/reference objects append `: &name` / `: *name` to the menu title. When the current file has unsaved changes, a `●` appears to the right of the title. With multiple files, the root menu appends only `[current/total]` before `●` (the title is already the file name, so it is not repeated); submenus append `[current/total filename]`. When the terminal is too narrow for the whole title, the body is truncated and marked with `…`, and `●` is always kept at the end of the line (the multi-file position label is truncated first).
- **Breadcrumb**: the document path, with fields as `.` and array indices as `[N]`, e.g. `$.mounts[0].src`. Levels that do not extend the path, such as oneOf pickers/variant editors, take no segment. Reference menus merge and show both the reference path and the anchor path (`$.a.steps → $.b.steps`). When too wide, the tail is kept and `…` is prepended.
- **Field list**: each row = title/key + value summary on the right. The title column auto-fits between 16–42 cells based on the widest item in the current menu; wider titles are shown in full, and their values are no longer force-aligned.
- **Description line**: the schema `description` of the cursor row and the doc comment (JSONC `//`, YAML/TOML `#`) are shown separately, wrapped automatically, in two colors (schema help / doc comment, with no text prefix). When collapsed, each shows at most 2 lines; anything beyond that is marked with an accent-colored `…` at the end of the last line. Press `c` to expand the full description, and `c` again to collapse. While expanded, pressing any other key first collapses the description and then executes that key. When the description does not fit on one page, `c` opens a separate scrollable comment page. See [Descriptions and the comment page](#descriptions-and-the-comment-page).
- **Status line**: rendered only on the root screen, above the bottom help bar and below the description/transient messages. Informational messages (schema source, YAML alias count) are hidden **after the first key press**; warnings (cache fallback, schema compile failure) persist for the whole session.
- **Transient messages**: `Saved`, `undone`, `Copied ...`, `empty fields hidden`, etc.; they disappear after the next action.
- **`W` input line**: when `W` is pressed in the root menu/list mode, it is fixed above the bottom help bar and shows the `Write to` path input without a `>` prefix; tab completion candidates appear above the input line (same as the command input).
- **`:` input line**: when `:` is pressed in list mode, it is fixed above the bottom help bar; the prompt is just `:` followed by a reverse-video cursor; what you type first is a built-in command (`w`/`wa`/`wq`/`waq`/`q`/`q!` etc.), and only input starting with `!` is a shell escape. Opening the input box automatically shows a dark candidate window above the input line, filtered in real time as you type (at most 8 rows; once the typed prefix is ≥8 characters, candidates omit the common prefix and show only the differing part, while candidates identical to the input are still shown in full; the width auto-fits the differing part — minimum 24 columns, at most filling the terminal's usable width — and over-long items are omitted from the left starting with `…`, keeping the trailing differing part; the selected row is highlighted in reverse video across its full width, the others dimmed, moving with `↑`/`↓`; the candidate window's left edge: full candidate rows align with the start column of the word being completed (typing characters does not move it; realignment happens only when a separator such as a space/`!` starts a new word), diff-only rows with the omitted prefix align with the cursor column, and the whole window shifts left when near the terminal's right edge).
- **Search inputs**: list search (`/`/`r`), oneOf/any-type pickers, enum array members, the issue/error/reference/file lists, and the SchemaStore picker all pin the `search: ` (and `replace: `) line below the title/breadcrumb and above the list; the bottom help bar is always fixed to the terminal's last line. Search opened with `/` always starts from an empty query with the cursor kept on the current row (for pickers, the row of the current value when opened); `esc` cancels back to that row. In replace mode, only the currently focused input shows a reverse-video cursor; the other input is dimmed across its full width with no cursor, and `tab`/`shift+tab` moves the cursor accordingly.

## Row types and markers

| Element | Meaning |
|---|---|
| `*` (red) | A required field per the schema; independent of whether it is filled. |
| `●` (pink, end of row) | The value changed relative to load time / last save; object/array rows are also marked when their subtree contains changes. `●` on the title line means the whole file has unsaved changes. When a row is truncated on a narrow terminal, the cut point is marked with `…`; the trailing `●` and the `<err>`/`<warn>` icons are always kept (`(not in schema)`, `--->` and other annotations are truncated first). |
| `●` (oneOf row) | The currently matched/selected variant. |
| `<err>` (red) | The field has a validation error; parent object/array rows are also marked. |
| `<warn>` (yellow) | Warnings only (e.g. unknown keys); shown when there are no errors. When a row has both an error and a warning, only `<err>` is shown. |
| `(not in schema)` | A key present in the document but not defined by the schema; yellow row, treated as a warning. |
| `--->` | The row can enter a submenu. |
| `—` | No value set and no default value to preview. |
| `>` (red, start of row) | Rows marked by multi-select / range select. |
| `[empty hidden]` | The gray hint right below the list when the `z` filter is on. |
| `● REC n` | While a macro is being recorded, the red counter at the right of the bottom bar. |
| `&name` / `*name` | Badge for a YAML anchor definition / alias reference. |
| `<<: *a *b` | Merge key sources shown on the mapping menu title. |

### Value summary

- Strings green, numbers cyan, booleans magenta, enums amber, objects/arrays bold.
- When unset or explicitly null, the schema default is previewed in a dimmer gray if one exists; with no default, `—` is shown. Explicitly set values are colored by type even when they equal the default.
- Objects: shows the first 2 members as `k=v` in **schema declaration order**, with `+N` beyond that; keys not defined by the schema come after, in document order.
- Arrays: arrays of plain objects show `N items`; other arrays show `[total] first 3 values +N` (objects as `{…}`, nested arrays as `[N]`).
- oneOf: summarized by the currently matched leaf variant; scalar variants show the value directly, enums show the option, and object variants show `variant title {…}` when they have a title.
- In multi-line strings (script blocks etc.), newlines and tabs are folded into spaces; the summary is always a single line.

## List mode key bindings

The following keys take effect in normal menu lists; behavior differs while an input box is open (see the next section).

### Movement and jumping

| Key | Action |
|---|---|
| `↑` / `k`, `↓` / `j` | Move up/down, wrapping at both ends. |
| `ctrl+f` / `PgDn`, `ctrl+b` / `PgUp` | Page down/up (the step is the current number of visible rows; stops at the start/end of the list, no wrapping). |
| `←` / `h` | Go up one level; no action in the root menu (use `esc`/`q` to exit). |
| `→` / `l` / `enter` | Edit the field or enter the submenu; on enum rows, `←`/`→` cycles options. |
| `gg` | Jump to the first row of the list. The first `g` enters the command buffer with **no timeout**; if the next key is not `g`/`a`/`r`, it executes normally and clears the buffer. |
| `Ngg` (array-entry menu) | Jump to the entry with index N; multiple digits accumulate, out-of-range counts are clamped to the last entry, and `0gg` is the first. |
| `G` / `End` | Jump to the last row of the list. |
| `Home` | Jump to the first row of the list. |
| `n` / `N` | Jump to the next/previous entry whose **document value is not null**; skips action rows, oneOf variant rows, and enum toggle rows; wraps at both ends. If the whole menu has no non-null entries, the cursor does not move. |
| `ctrl+j` / `ctrl+k` | Adaptively jump to the next/previous entry of interest (wrapping at both ends), picking candidates by priority: keys changed by replace/undo/redo (after replace-all or finishing `u`/`ctrl+r`, the cursor lands on the first one automatically; further `ctrl+j`/`ctrl+k` cycles the remaining changes) → keys with validation issues (errors or warnings) → search hits from a jump made from search results → keys changed since load/last save (rows carrying the `●` marker, cycled in document order; deletions land on their surviving container). When none of the four cases applies (the document has no changes), it shows `no changes since load/save` and the cursor stays put. Any new document edit clears the replace/undo/redo change list. |
| `f` / `F` | Fast move: waits for one character, then jumps down/up to the next key starting with that character (case-sensitive, wraps at both ends, skips action rows and array entries). While waiting, `u`, `q`, `@`, and `.` are all treated as the target character and do not trigger undo/exit/macro; `esc` cancels. |
| `ctrl+o` | Navigate back in history. |
| `ctrl+i` / `tab` | Navigate forward in history (terminals report ctrl+i as tab). |
| `s` | With multiple files, opens the file switcher; with a single file, shows `only one file is open`. |
| `S` | Opens SchemaStore search and switches the schema of the **current file**: the document content is kept, and the forms, validators, and root menu are rebuilt from the new schema; the selected result is written back as the schema pointer according to the format (for YAML/TOML the `$schema` field is written if the new schema defines one); `esc` cancels. |

`esc`/`q` with a pending `g` buffer only clears the buffer; an active selection at any menu level (`m` marks or range select) is cleared first and you stay in the current menu (message `selection cleared`); only the next `esc`/`q` goes up one level; on the root screen, it then enters the exit flow.

### Editing and structure

| Key | Action |
|---|---|
| `space` | Toggle a boolean or an enum array member; on other rows, same as `enter`. |
| `-` / `=` / `+` | Quickly decrement / increment numeric fields by 1; when the field has no value, writes `-1` / `1` respectively. Keys not defined by the schema but holding a numeric runtime value (unknown keys, array elements, schema-less documents, raw fields) are likewise incremented/decremented from the current value; keys with no value and no schema type do not guess a type. Integers are computed exactly in decimal (unaffected by f64 precision), while decimals and scientific notation are written back in the shortest float form; one key press is one undo step. |
| `enter` | In-place input for scalars; containers enter the submenu; keys not defined by the schema and values without a `type` are edited directly by runtime type, and only when there is no value/null is a type chosen first (JSON built-in types or schema `$defs` named types). |
| `d` / `delete` | Delete the current key-value pair / array entry; protected rows such as required merge keys refuse with a message. |
| `D` | Delete the current array entry (in the entry list or the entry form) after confirmation; deleting inside the form returns to the entry list. |
| `a` / `A` | Object/mapping menus: add a key not defined by the schema **before/after** the cursor member. Array menus: insert an entry **after/before** the cursor entry. |
| `J` / `K` | Move the array entry down/up; one key press is one undo step. |
| `e` | Edit the current row's value with the external `$EDITOR`; strings containing newlines always go through this. |
| `E` | Edit the cursor row's value as raw JSON/YAML/TOML with the external `$EDITOR`; when the cursor is not on a value row, edits the whole document. |
| `ctrl+e` | Materialize YAML aliases; in mapping menus, expands merged values; when the cursor is on a reference row, materializes that reference directly. |
| `&` / `*` | Set/modify a YAML anchor / reference. |
| `<` | Edit the current mapping's `<<` merge key. |
| `ga` / `gr` | Jump to the current row's anchor / to other usages of that anchor. |
| `t` | When the cursor is on a oneOf variant key, opens the variant picker (scalars/objects need no prior editing); for values not defined by the schema / without a `type`, explicitly opens the type picker. |
| `z` | Show/hide object keys that have no value in the document (defaults do not count). When the filter is on, `[empty hidden]` appears below the list; jumping from search or the issue list to a hidden key turns the filter off automatically. |
| `c` | Expand/collapse the current row's description (at most 2 lines per source when collapsed); when it does not fit on one page, opens the separate comment page. |
| `C` | Edit the current key's doc comment: multi-line editing, `ctrl+s` applies, `esc` cancels; only keys already present in the document can be edited; a comment can be added when absent, and empty text removes it. |
| `m` | Toggle the selection mark on the cursor row. |
| `v` | Toggle range select; `j`/`k`, arrow keys, and `gg`/`G` extend the selection. |
| `y` / `Y` | Copy `"key": value` / copy only the value; in array menus, both copy the element value. |
| `x` / `X` | Cut the key-value pair / cut the value; in array menus, both cut the element value. |
| `p` / `P` | Smart paste into the current menu / the container under the cursor; a single value under `P` replaces the row's value. |
| `u` / `ctrl+r` | Undo / redo; the cursor automatically lands on the first location changed by that step (deletions have no corresponding entry row and land on their container), and the remaining changes can be cycled with `ctrl+j`/`ctrl+k` (see above). |
| `:` | Opens the command input at the bottom: built-in commands such as `w`/`wa`/`wq`/`waq`, and `!<command>` runs a shell escape; see the next section. |
| `@` / `.` | Start/stop macro recording / replay the last macro. |

For details on bulk operations, clipboard formats, and arrays/mappings/oneOf, see [Editing](editing.md).

### Descriptions and the comment page

- When the description line is collapsed, the schema help and the doc comment each show at most 2 lines; anything beyond that is marked with an accent-colored `…` at the end of that block's last line. The two use different colors and no longer get `schema:`/`comment:` prefixes. Doc comments are read by member path, kept when a member is edited, removed together with the member on deletion, and absent for new members.
- `c` expands the full description; `c` again collapses it. While the description is expanded, pressing any other key (movement, editing, saving, ...) first collapses it automatically and then executes that key.
- `C` opens the comment editor for the current key directly (only keys that already have a value in the document): multi-line text, `ctrl+s` applies, `esc` cancels. Comments are stored in the file format's comment table and written back in the original style (`//` or `#`) on save; empty text removes the comment; schema-directive markers such as `# yaml-language-server:` / `#:schema` are not shown or overwritten as user comments.
- When the full description does not fit on one screen, `c` does not expand it into the menu but opens the separate comment page: the title shows the field name, and the body wraps to the window width. This page is for viewing only; press `C` to edit comments.
- Inside the comment page, `↑`/`↓` or `j`/`k` scroll line by line, `ctrl+f`/`PgDn`, `ctrl+b`/`PgUp` scroll by page; `c`, `q`, `esc` close it. While the comment page is open, other menu keys have no effect.

### Search, saving, and exit

| Key | Action |
|---|---|
| `/` | Search downward from the current breadcrumb path; inside oneOf/type pickers it becomes search mode, and the enum array member list becomes a filter over toggle rows (`search: ...` below the title; typing filters the options). |
| `r` | Search and replace string values. |
| `ctrl+s` / `w` | Save the current file and stay in the editor; the bottom shows the `Write to <path>` prompt. Inside input boxes, `w` is a plain character. |
| `W` | Save a copy to the given path; `enter` writes the copy, `esc` cancels. |
| `?` | Open/close the full key bindings help. |
| `ctrl+c` | Exit immediately without writing files (exit 130); already saved content is kept. |
| `esc` / `q` (root screen) | Exit directly when there are no changes; ask about saving when there are changes; see below. |

Inside input boxes, `q` is a plain character; always use `esc` to cancel.

## Input and overlay key bindings

### In-place input / raw text areas

| Key | Action |
|---|---|
| `enter` | Apply (in raw text areas and the comment editor, `ctrl+s`). |
| `esc` | Cancel. |
| `tab` / `ctrl+n` / `ctrl+j` | Next completion candidate. |
| `shift+tab` / `ctrl+p` / `ctrl+k` | Previous completion candidate. |
| `↑` / `↓` | With tab completion active, previous / next candidate (cyclic, sharing the same candidate list); no action when inactive. Raw text areas still use `↑`/`↓` to move across lines. |
| `←`/`→`/`Home`/`End` | Move within the line. |
| `alt+f` / `alt+b` | End of the next word / start of the previous word; whitespace and `,.+-=_/\:?!$#@*` are word separators. |
| `ctrl+a` / `ctrl+e` | Start / end of line (in replace mode, `ctrl+r` is replace all). |
| `ctrl+k` / `ctrl+u` / `ctrl+w` | Kill to end of line / kill to start of line / delete the previous word; deleted text goes to the kill buffer (in raw text areas and the comment editor, `ctrl+w` still deletes a word; applying is `ctrl+s`). When the candidate window is open, `ctrl+k` becomes the previous completion candidate (`ctrl+j` the next). |
| `ctrl+y` | Paste the text most recently deleted by `ctrl+k`/`ctrl+u`/`ctrl+w` (the kill buffer); an empty deletion does not overwrite the buffer, so pasting can be repeated. |
| `backspace` / `delete` | Delete characters. |

Opening an input box automatically shows the candidate window, filtered in real time as you type — there is no need to press `tab` first; `tab`/`ctrl+n`, `shift+tab`/`ctrl+p`, `ctrl+j`/`ctrl+k`, or `↑`/`↓` select a candidate. The candidate window has no `>` marker; the selected row is highlighted in reverse video across its full width. When the typed content reaches 8 characters and all candidates start with it, candidate rows omit that common prefix and show only the differing part (case-insensitive, without a `…` marker; candidates identical to the input are still shown in full), so that long URLs/paths do not squeeze the difference to the truncation point. The window width auto-fits the differing part, minimum 24 columns, at most filling the terminal's usable width; over-long candidates are omitted from the left (starting with `…`), the trailing differing part is never omitted, and each row is padded to that width, forming a dark-background rectangle. For in-place editing, the candidate window's left edge aligns with the start column of the input text, and typing characters does not move it; other inputs (anchor/reference etc.) likewise align with the start column of the input text; the `:` command line's candidate window aligns with the start column of the word being completed — typing within the same word does not move it, and it realigns when a separator starts a new word; when candidate rows omit the common prefix, the window instead aligns with the cursor column (for in-place editing it follows the cursor as you type, while the `:` line stays at the end of the current candidate). When the rectangle does not fit near the terminal's right edge, the whole block shifts left with its right edge staying within the terminal's usable width, and candidates always remain fully visible; row content originally to the left of the rectangle (titles, selection marks, etc.) remains visible — only the rectangle itself covers the right-hand area. The selected row is shown in reverse video across its full width, and the other candidates are dimmed. For in-place editing, the candidate window preferentially covers the menu rows below the cursor row, without inserting new lines or pushing the list; when there is not enough space below (e.g. the cursor is at the terminal bottom), it covers the menu rows above the cursor instead, keeping the candidates always visible. Candidates for anchor/reference input likewise cover the rows below their input line. The `W` write path and the `:` command input sit at the bottom, so the candidate window appears above the input line.

Raw text areas (the whole document, fields that cannot be presented as a form, or the `JSON` variant of the any-type picker) also support `↑`/`↓` across lines, `enter` for a newline, `backspace` to join lines, `ctrl+k` to kill to end of line, and similar editing; `ctrl+s` applies, `esc` cancels.

### Command mode `:`

Pressing `:` in list mode opens the command input line at the bottom (prompt `:`). The input line is ordinary single-line text: `←`/`→`, `Home`/`End`, `ctrl+a`/`ctrl+e`, and `ctrl+k`/`ctrl+u`/`ctrl+w`/`ctrl+y` all edit it. `tab` completion is **based only on the content before the cursor** and replaces only the current word before the cursor — text after the cursor is kept as is (e.g. after typing `:!cat /tmp/alZZ`, move the cursor before `ZZ` and press `tab` to get `:!cat /tmp/alpha.txtZZ`).

Built-in commands:

| Input | Action |
|---|---|
| `w` | Save the current file and stay in the editor (same as `ctrl+s`). |
| `w <path>` | Save to the given path (same as `W`; the argument does not start with `!`). |
| `wa` | Save all files with changes and keep the current file (`Saved N files`). |
| `wq` | Save the current file and quit. |
| `waq` | Save all files with changes and quit. |
| `q` | Quit when there are no unsaved changes; with changes, suggests `:wq` or `:q!` instead. |
| `q!` | Discard unsaved changes and quit (nothing written to disk). |
| `w!`, `wa!`, `wq!`, `waq!` | Same as above, but skip validation on save and force writing to disk. |

Validation errors from `w`/`wq`/`waq` still open the error list: `y` force-saves, `n`/`esc` cancels the save (so `wq`/`waq` do not quit).

Shell escapes (starting with `!` in command mode; syntax modeled on vi):

| Input | Action |
|---|---|
| `!<command>` | Runs the command only and shows its output (vi's `:!cmd`), without changing the document; single-line stdout is shown as the bottom message, and multi-line stdout is shown directly as a multi-line message when it fits in the blank rows above the bottom bar — only when it does not fit is a full-screen scroll page opened. |
| `r !<command>` | Reads the command's stdout (vi's `:r !cmd`): replaces a string value directly, and appends one entry to `array[string]`. |
| `a !<command>` | Splits stdout **by lines**, appending each line as a new entry to `array[string]`. |
| `w !<command>` | Writes the entire current document (including comments, in the file's own JSON/JSONC/YAML/TOML syntax) to the command's stdin (vi's `:w !cmd`); the document itself is not modified, and the command's output is shown as the bottom message. |
| `%` | When it appears as a standalone word in a shell command, stands for the current file; it is expanded to a shell-quoted absolute path before execution. `tab` can also expand a `%` before the cursor directly into that path. |
| `!!` | Re-runs the most recent shell escape (`!`/`r !`/`a !`/`w !`, not recorded in history again); when there is none, shows `: no previous shell command`. |

After `r`/`a`/`w`, `!<command>` is required for a shell escape; `w` followed by a plain argument is a save-to path (`w /tmp/a.json`).

| Key | Action |
|---|---|
| `:` (list mode) | Opens the command input above the bottom help bar; built-in command candidates appear immediately and are filtered in real time as you type, with no need to press `tab` first. |
| `enter` | Executes the current command with the system shell (Unix `sh -c`, Windows `cmd /C`). |
| `tab` / `ctrl+n`, `shift+tab` / `ctrl+p`; with a completion selected, `ctrl+j` / `ctrl+k` | Completes the current word before the cursor, replacing only that word and keeping the text after the cursor. Without `!`, completes built-in command names; when the command starts with `!` (or after `r !`, `a !`, `w !`), the first command word completes executables from `$PATH` and subsequent arguments first ask bash's programmable completion — when bash-completion has a registered rule for the command, its results complete the word (subcommands, branch names, service actions, ...); without a rule, without bash or without bash-completion, the filesystem path scan stays; the argument of `w <path>` completes as a path; `%` completes to the current file's full path. Repeated presses cycle candidates. The candidate window sits above the input line, at most 8 rows; once the typed word reaches 8 characters, candidates omit the common prefix and show only the differing part (candidates identical to the input are still shown in full); the width auto-fits the differing part, minimum 24 columns, at most filling the terminal's usable width; over-long items are omitted from the left starting with `…` with the trailing differing part kept; the whole window has a dark background, and the selected row is highlighted in reverse video across its full width (the others dimmed). |
| `↑` / `↓` | With a completion selected, moves the selection back/forward in the candidate window (cyclic, same effect as `shift+tab`/`tab`); with no completion selected, moves through the command history. When `↓` goes past the newest entry, it restores what was being typed before the history was opened. |
| `esc` | Cancels the command input without changing the document. |
| `↑`/`↓` or `j`/`k`, `ctrl+f`/`PgDn`, `ctrl+b`/`PgUp` (multi-line output page) | Scrolls line by line / page by page when the output exceeds one screen; the keys do not move when it does not. |
| `q` / `esc` (multi-line output page) | Closes the output page. |

Bash completion for shell arguments runs once automatically when the caret enters a fresh argument word (usually right after a space, with no `tab` press); further keystrokes inside the word only filter the fetched words locally, and an explicit `tab` re-asks for the word. jse spawns a `bash -c` child that loads bash-completion and runs the command's registered completion rule (`complete -F`/`-C`), waiting synchronously for the script to finish (heavy completion scripts may run the command's own binary, hence one run per argument word). The core script is looked up in the order `JSE_BASH_COMPLETION` (see [Command line](cli.md#environment-variables)) → `/usr/share/bash-completion/bash_completion` → `/etc/bash_completion` → `/usr/local/etc/bash_completion` → `share`/`etc/bash_completion` under `/opt/homebrew`; when none exists, or the command has no completion rule, the filesystem path scan stays.

Command history **persists across sessions**: every executed command (`!!` is not recorded again) is written immediately to the `cmd-history` file in the cache directory (default `$XDG_CACHE_HOME/jse`; `JSE_CACHE_DIR` changes the location, see [Command line](cli.md#environment-variables)), stored one command per line with the oldest dropped first, at most 100 entries; in a new session, `↑`/`↓` in the command input reaches the previous session's commands. When the file is missing or unreadable, history starts empty; write failures affect only persistence, not use within the session.

Rules for writing the output of `r !<command>`:

- When the cursor row's value (including array-entry values) is a string, the output replaces that value directly.
- When the cursor row's value is `array[string]`, the entire output is appended as one entry at the end of the array; the same append happens when an empty array menu is stopped on the `< Add item >` row.
- When a oneOf variant allows both `string` and `array[string]`, it is treated as a string (replace); when only `array[string]` is allowed, it appends.
- For types not defined by the schema (schema-less, raw, unknown keys), the runtime value decides: strings are replaced, and arrays of pure strings are appended to.
- Other types are not written and the bottom message reports the failure. The same applies when the command's exit code is non-zero: nothing is written, and the bottom message shows the first stderr line and the exit code; the trailing newline of stdout is stripped, and the rest (including inner newlines) is written as is.

`a !<command>` accepts only `array[string]` targets: the output is split on `\n` (with the CRLF `\r` removed as well), and every line — including blank lines in the middle — becomes a string entry; the trailing newline produces no empty entry. With the cursor in an array-entry menu, stopping on a scalar entry also appends directly to that array; other targets such as strings report failure.

When the single-line stdout of `w !<command>` and `!<command>` succeeds, the output is compressed to one line and shown as the bottom message. Multi-line stdout of `!<command>` is inlined first: when the number of lines after wrapping to the terminal width does not exceed the blank rows between the menu's last row and the bottom help bar, the message is expanded to multiple lines directly (the menu does not move); only when it does not fit is the scroll page opened, with the output block flush against the bottom help bar (when the content is shorter than a screen, it is laid out from the bottom up), blank lines kept and wrapped to the terminal width; `q`/`esc` close it. Other messages containing newlines are rendered inline the same way. When `w !<command>` has no output, the number of bytes fed in is shown.

Commands run synchronously — while one runs, the UI does not refresh or respond to keys, and there is no timeout; slow commands or commands reading stdin will stall the UI. This matches the semantics of vi's `:!`; avoid running interactive or long-running commands while editing.

### Issue list / error list / reference picker

| Key | Action |
|---|---|
| `↑` / `k` / `ctrl+k`, `↓` / `j` / `ctrl+j` | Move (cyclic). |
| `ctrl+f` / `PgDn`, `ctrl+b` / `PgUp` | Next/previous page (stops at the start/end). |
| `gg` / `Home`, `G` / `End` | First item / last item. |
| `enter` | Select / jump to the corresponding field. |
| `/` | Enter filtering. The input starts empty with the cursor kept on the current row; typing filters by message, path, or value summary (case-insensitive). In the search input, `↑`/`↓`, `tab`/`shift+tab`, and `ctrl+j`/`ctrl+k` move among matches, while ordinary characters such as `j`/`k` are typed. `enter` selects / jumps; with no matches you stay in search and can correct with `backspace`. |
| `esc` | Close. While searching, first exits search and restores the full list; press again to close. |
| `y` (save error list, `--force` only) | Force-write. |
| `n` (save error list) | Cancel the save. |

Scalar enums no longer use a separate picker page: `enter` opens an in-place input box on the field row, the options are listed in an autocomplete-style dropdown, and typing keywords inside the input filters them directly (see "Enum dropdown" under Editing). The `/` search of list-style pages shares the same input rules as the oneOf/type pickers; `search: <query>` is shown below the title, and `(no matches)` appears in the list area when nothing matches. `esc` cancels the search and restores the cursor to the row where search was opened; press again to close the page.

After jumping from the issue list to a field, pressing `esc` in the landing menu returns to the refreshed issue list; drilling down further first walks back level by level, then returns to the list.

### Search / replace

| Key | Action |
|---|---|
| Input | Filters in real time; regex matching by default. |
| `↑` / `ctrl+k`, `↓` / `ctrl+j` | Move among results (cyclic). |
| `ctrl+f` / `PgDn`, `ctrl+b` / `PgUp` | Next/previous page of results (stops at the start/end). |
| `tab` / `shift+tab` | Bring up / switch focus of the replace input box. |
| `alt+i` | Toggle case sensitivity. |
| `alt+x` | Toggle regex / literal substring. |
| `enter` (search box) | Jump to the current result; `esc` at the destination returns to the search list. |
| `enter` (replace box) | Replace the currently highlighted value and move to the next match. |
| `ctrl+r` | Replace all. |
| `esc` | In replace mode, first returns to the normal search list; press again to close. |

The search scope is the schema subtree of the current menu. Even when the current object's value is `null` (e.g. an object just created whose members are not yet filled in), search still lists that object's schema fields in the menu.

### Confirmation dialogs, help, file and schema pickers

| Scenario | Keys |
|---|---|
| `Save changes?` / `Remove this item?` / `Replace current value?` | `y`/`enter`/`space` = yes, `n` = no, `esc` = cancel. |
| Key help `?` | `↑`/`↓`, `j`/`k`, `ctrl+f`/`PgDn`, `ctrl+b`/`PgUp` scroll; `esc` / `q` / `?` closes. |
| File switcher `s` | `↑`/`↓`, `j`/`k`, `ctrl+j`/`ctrl+k`, `ctrl+f`/`PgDn`, `ctrl+b`/`PgUp`, `gg`/`G`/`Home`/`End` move; `/` filters by path; `enter` switches; `esc` cancels search first, then closes on the next press. |
| SchemaStore picker | Type to filter; `↑`/`↓`, `ctrl+j`/`ctrl+k`, `tab`/`shift+tab` move; `enter` selects; `esc`/`ctrl+c` aborts. |
| Write path `W` | Type a path; `tab`/`ctrl+n` and `shift+tab`/`ctrl+p` cycle completions; `enter` writes; `esc` cancels. |
| Anchor/reference `&`/`*` | Type a name; `tab`/`ctrl+n`, `shift+tab`/`ctrl+p` complete; `enter` submits; `esc` cancels. |
| Merge key `<` | Type space/comma-separated anchor names; `tab` completes the current token; `enter` submits; `esc` cancels. |
| Fast move `f`/`F` | Type one character (`u` etc. are plain characters too); `esc` cancels. |

## Navigation history

`ctrl+o` / `ctrl+i` walk a **position history**, recording:

- entering submenus and going back up;
- `gg`/`G`, search jumps, issue-list jumps, and `ga`/`gr` jumps;
- file switches.

A new forward navigation discards forward branches that have not been visited (like a browser). History holds at most 100 entries per direction.

## Root screen exit flow

Pressing `esc`/`q` in the root menu whose breadcrumb is `$`:

1. A pending `g` combination → only clears the buffer.
2. Range-select / multi-select marks → only clears the selection and stays in the editor.
3. No unsaved changes → exits directly, stdout `No changes, nothing written`, exit 0.
4. Unsaved changes → asks `Save changes? (y/n)` (with multiple files and multiple changes, `Save changes in N files? (y/n)`):
   - `y`/`enter`/`space`: saves and quits; on validation failure, enters the error list, where `--force` allows force-saving.
   - `n`: discards the changes and quits, exit 130, stdout `Aborted, no changes written`.
   - `esc`: cancels the prompt and stays in the editor.

In submenus, `esc`/`q` first cancels any active selection; with no selection, it goes up one level and never quits the program.

## On-disk file change detection

When the editor is idle (in the gaps between input polls, about once per 250ms), it checks the on-disk modification time and size of the **currently active file**; multi-file sessions watch only the active file:

- When the disk content differs from the record and no overlay is open, a confirmation dialog pops up: `<path> changed on disk. Reload? (y/n)`:
  - `y`/`enter`/`space`: reloads from disk. The schema stays the same; for schema-less files, the root menu is rebuilt from the new content; the cursor returns to the root level and the modification baseline resets to the disk content (modification marks are cleared; the undo stack is kept, and `u` can undo this reload). If parsing fails, the in-memory version is kept and `Reload failed: …` is shown.
  - `n`/`esc`: keeps editing the in-memory version; that disk content is not asked about again — the prompt reappears only on the next external change.
- The editor's own saves (`ctrl+s`/`w`/`wa` etc.) refresh the record and do not trigger the prompt.
- When the file is deleted, there is no prompt: the in-memory content remains editable, and saving recreates the file.
- Detection is paused while any overlay (inputs, pickers, confirmation dialogs, etc.) is open; the prompt never preempts an interface that is in use.
