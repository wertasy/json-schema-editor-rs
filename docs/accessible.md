# Line mode (`--accessible`)

`--accessible` replaces the full-screen TUI with sequential question-and-answer: plain text, no ANSI control sequences; it reads answers line by line from stdin, which suits pipes, scripts, CI, and screen readers.

```sh
jse --accessible -s examples/server.schema.json server.json
```

Without `--accessible`, non-terminal stdin fails immediately; with the flag, stdin may be a pipe or a file.

## Prompt protocol

### Text / number input

```text
Name webapp
```

- The prompt is `title ` (a single trailing space), and one line is read.
- Failed inline validation prints the error and re-prompts; an empty line takes the binding default.
- A required field without a default **cannot be satisfied with an empty line**; you must provide content.
- raw fields (a whole document, or a subtree that cannot be form-mapped) are also a single line of text, with content in the file's format (JSON/YAML/TOML); an empty line deletes the value / makes no change.

### Single select

```text
Mode 
1. (none)
2. http
3. grpc
4. tcp
Enter a number between 1 and 4: 
```

- The title is on its own line, followed by numbered options.
- An empty line accepts the preselected default.
- With a single option the prompt is `There is only one option available; enter the number 1:`.
- Optional enums get an extra `(none)` option; required enums do not.
- Out-of-range numbers print `Invalid: must be a number between 1 and N` and ask again.

### Multi-select (enum array members)

```text
Levels 
Select up to 4 options.
1.   debug
2. ✓ info
3.   warn
4.   error
0.   Confirm selection
Enter a number between 0 and 4: 
```

- Entering a number repeatedly toggles its selected state.
- `0` confirms; when `minItems`/required is not satisfied it prints `select at least N` and continues.
- An empty line is equivalent to `0`.

### Confirmation

```text
Debug [y/N] 
```

- `y`/`yes`/`n`/`no`, case-insensitive.
- An empty line takes the default; when the default is `y` it shows `[Y/n]`.
- Invalid input prints `invalid input. please try again` and asks again.

### Info lines

Informational prompts (schema source, cache fallback, schema compile failure, etc.) are printed at the top of the root screen as notes that take no input, followed by an empty line.

## Menu structure

### Object menu

Fields are asked one by one in schema order, and a `Navigate` single select appears at the end:

```text
Name webapp
Mode ...
Server ...
...

Navigate 
1. Edit Server ▸
2. Edit Logging ▸
3. Save and exit
4. Edit raw JSON
Enter a number between 1 and 4: 
```

- Non-root objects: the first item is `Done (back)`, selected by default.
- Root objects: navigation items come first, then `Save and exit` and `Edit raw JSON` (YAML/TOML files show `Edit raw YAML` / `Edit raw TOML` instead); `Save and exit` is selected by default.
- Array/object/oneOf children appear as `Edit ... ▸`; enum arrays act directly as multi-select fields and never enter a submenu.
- With no editable fields it prints `Nothing to edit here`.

### Array menu

```text
Tags items
1. < Add item
2. 0: prod
3. 1: staging
4. Move item up…
5. Move item down…
6. Remove item…
7. Save and exit
Enter a number between 1 and 7: 
```

- `< Add item` is the default; an empty line enters the add flow rather than saving.
- Selecting an entry opens the entry form.
- `Move item up…` / `Move item down…` select an entry first, then move it; moving out of range is a silent no-op.
- `Remove item…` selects an entry first, then confirms with `Remove item N? [Y/n]`.
- The save item for a root array is `Save and exit`; for a nested array it is `Done (back)`.

### oneOf

First choose a variant (`1. Title` …), then enter the variant editor; after editing a root oneOf it automatically proceeds to save-time validation, while a nested oneOf returns to its parent when done.

### Scalar roots

The value is asked for directly, then save-time validation runs automatically.

## Saving and validation

- Validation runs after choosing `Save and exit`; on errors it only asks:

  ```text
  Save anyway? [y/N] 
  ```

  Error details are **not printed one by one**. The file is written only when started with `--force` and you answer `y`; otherwise it returns to the menu.
- After a successful write the process continues with the next file (multi-file runs) and exits once all are done.
- Write failures are reported on stderr and abort with exit 1.
- Exit codes and the stdout result line match TUI mode; see [Command line](cli.md#exit-codes-and-stdout).

## EOF and scripting

- EOF at a prompt ends the current prompt and takes the default (it never re-asks the same prompt endlessly).
- After the current screen finishes, EOF is detected on the next loop iteration:
  - root screen: the session aborts;
  - child screens: return one level up.
- But a default may produce a result first: the root object menu's default navigation item is `Save and exit`, so "field defaults + EOF" may save directly and exit 0; the root array menu's default item is `< Add item`, which may add one item before aborting.
- Scripts should therefore supply every field and navigation choice that will actually be asked, and must not rely on EOF defaults.
- Non-root objects default to `Done (back)`, so an empty line + enter accepts the default path.
- Prompts and results are written to stdout; errors go to stderr.

Minimal script example (the schema only has optional fields, saved directly):

```sh
printf '\n' | jse --accessible -s mini.schema.json out.json
```

When required fields exist you must supply each value, then choose `Save and exit` in the root menu (or let an empty line select it). Multiple files are processed one after another in argument order.

## SchemaStore picker

Without a schema, `--accessible` uses line-based retrieval:

```text
Select a schema for ci.yml
? Schema search [ci]: 
  1) GitHub Workflow — GitHub Actions workflow
? Number (0 = new search, enter = abort): 1
```

- An empty line keeps the current query; `0` searches again; bare enter aborts (during automatic resolution this enters schema-less mode; with `--pick` it exits).
- At most 15 entries are listed; beyond that you are prompted to refine the query; no results print `no matches` and return to the search prompt.
- Non-numeric or out-of-range input prints `! invalid number` and asks again.
- EOF aborts.

Even without a schema, line mode can still edit: an object root screen offers only `Save and exit` and `Edit raw JSON/YAML/TOML` (raw editing of the whole document, entered on one line); array roots can still add and remove items one by one and raw-edit entries; scalar roots edit the raw value directly. Saving performs no validation.

## Limitations

- No live issue list, field-row markers, breadcrumbs, multi-select/macros/undo/search-and-replace/file switcher.
- Text input is always single-line; multi-line YAML/JSON/TOML content must be written on one line (JSON/TOML use `\n` escapes, YAML uses flow or quoted form).
- Validation happens only at save time; pre-existing errors at load time are not listed separately in the line flow.
- Multiple files are processed sequentially; you cannot switch to another file while editing one.
