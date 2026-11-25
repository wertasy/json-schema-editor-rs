# GUI (Native Shell)

jse has a second front end: a windowed GUI built on [Native Shell](https://nsh.dev)
(NSH) instead of the terminal. It drives the same editor core — the
document arena, the schema form tree, save wiring and validation are the
terminal UI's, so a config file edits and saves identically in both. The
`gui` cargo feature selects which front end the binary carries; the
artifacts are built per platform from the same source.

## Building

The default build is the terminal UI. The GUI replaces it in the same
`jse` binary:

```sh
cargo build --release --features gui
```

Platform notes for the desktop binary:

- **Linux**: needs GTK 4.14+ where it runs (`libgtk-4-dev clang` to build).
- **Windows**: Windows 10 1809+; built with the MSVC toolchain.
- **macOS**: macOS 14+.
- **iOS**: iOS 15+; the device package must be signed (`--sign <identity>`,
  plus `--profile <.mobileprovision>`) to install; an unsigned build is for
  re-signing, and `nsh package --target ios-simulator` produces an app that
  runs in Simulator without any certificate.

To build the packaged apps (Linux app bundle, Windows program folder and
installer sources, Android APK) use the Native Shell CLI:

```sh
npm not required — https://nsh.dev/install.sh
nsh package --target linux     # .tar.gz with .desktop entry
nsh package --target windows   # program folder + Inno Setup script
nsh package --target android   # APK (arm64-v8a and armeabi-v7a)
nsh package --target macos     # .app, ad hoc signed
nsh package --target ios       # device package: .ipa when signed with
                               # --sign <identity>, unsigned .app otherwise
```

`nsh.json` at the repository root points the CLI at `gui-app/`, a thin
crate that enables the library's `gui` feature for the generated runners.
Android needs the Android SDK, an NDK and a JDK (`nsh component add
android` first); the desktop targets need the platform's native toolchain.

## Running

The GUI binary takes the same arguments as the terminal one —
`jse [-s schema] config.(json|jsonc|yaml|yml|toml)` — and opens an editor
window per session, the first file active. Without arguments it starts at
the open screen (also the Android launch state): type a config path and
press Open.

## Editing

The window is a list of the current schema level's fields, like the
terminal UI's menu screen. Tap a row to act on it:

- **Field** (string, number, enum): opens an edit sheet. Apply writes the
  value with the terminal's rules (empty input removes optional keys and
  seeds defaults; enum values commit from the schema's options).
- **Boolean / enum-array member**: toggles in place.
- **Section, array, oneOf** (rows with a `→` marker): drills in; the
  header's back button pops one level.
- **Array item**: the trailing ✕ deletes it and ⇥ inserts after it; the
  `＋` row and the toolbar's `＋` add items and keys.
- **Unknown key** (marked "not in schema"): edits by runtime type; ✕
  removes it.

The toolbar carries: back, the file name with its unsaved mark (●), the
breadcrumb, the empty-fields filter (∅, the terminal's `z`), add (＋),
the issue counter (⚠), the file switcher (when several files are open),
and Save. Saving validates exactly like the terminal: with problems the
save is refused and a validation-errors sheet lists them (a `--force`
session can write past them); pressing a problem jumps to it. Closing the window without saving discards changes
(there is no save-on-exit prompt); save deliberately with Save.

Sheet apply/cancel map to the terminal's enter/esc: what Apply commits is
what Enter would commit in the terminal UI, including validation errors
staying on screen instead of writing.

## Differences from the terminal UI

The editing semantics (write-through applies, key omission rules, oneOf
switches, YAML anchors and merges, save-time validation, `--force`,
atomic saves) are shared. Not exposed in the GUI yet: keyboard-driven
navigation (the list is pointer/touch first), `/` search and `r` replace,
undo/redo surfaces (snapshots are still recorded per action), document
comment editing (`C`), external `$EDITOR` edits, `:` commands and
write-to (`W`), and the SchemaStore picker (`S`). The terminal binary
remains the interface for those.
