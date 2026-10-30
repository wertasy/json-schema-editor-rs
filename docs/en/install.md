# Installation and building

## Prebuilt binaries

Download the archive for your platform from [Releases](https://github.com/wertasy/json-schema-editor-rs/releases):

| Platform | Target |
|---|---|
| Linux amd64 | `x86_64-unknown-linux-musl` (statically linked) |
| Linux arm64 | `aarch64-unknown-linux-musl` (statically linked) |
| macOS amd64 | `x86_64-apple-darwin` |
| macOS arm64 | `aarch64-apple-darwin` |
| Windows amd64 | `x86_64-pc-windows-msvc` |
| Windows arm64 | `aarch64-pc-windows-msvc` |

The archive contains `jse` (`jse.exe` on Windows), `README.md`, `docs/`, `AGENTS.md`, `LICENSE`; `checksums.txt` provides SHA-256 checksums. Example:

```sh
tar -xzf jse-v0.9.0-linux-amd64.tar.gz
mkdir -p ~/.local/bin
mv jse ~/.local/bin/
jse --version
```

Linux static builds do not depend on glibc and can be dropped straight into a container or a minimal distro image.

## Building from source

Requires Rust stable (`rust-toolchain.toml` pins `stable`; the source uses edition 2024).

```sh
cargo build --release
./target/release/jse --version
```

Install into Cargo's bin directory:

```sh
cargo install --path .
```

You can also install from crates.io (crate name `json-schema-editor`, binary still `jse`):

```sh
cargo install json-schema-editor
```

The install location is usually `~/.cargo/bin/jse`; make sure that directory is on your `PATH`.

### Static release builds

Linux releases use musl:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

This needs `musl-gcc` (Debian/Ubuntu: `sudo apt-get install musl-tools`), and Cargo must use it as the linker:

```toml
# .cargo/config.toml
[target.x86_64-unknown-linux-musl]
linker = "musl-gcc"
```

arm64 cross-compilation additionally needs `gcc-aarch64-linux-gnu`, with `linker` set to `aarch64-linux-gnu-gcc`. For exactly what the release workflow does, see `.github/workflows/release.yml`.

## Version number and build metadata

`jse --version` prints something like:

```
jse v0.9.0 (commit 1a2b3c4, built 2026-10-22T09:00:00Z)
```

- The version number prefers the `JSE_VERSION_OVERRIDE` environment variable. Otherwise it is derived from git:
  - The highest reachable semver tag is used, e.g. `v0.9.0`; with no reachable tag it is `v0.0.0`.
  - When HEAD is past a tag, that tag is kept: the commit and build time are shown separately, so no extra development version is constructed;
  - Outside a git checkout: the crate version is used, e.g. `v0.9.0`.
- When the working tree has uncommitted changes (including untracked files; gitignored ones don't count), `+dirty` is appended to the version, e.g. `jse v0.3.1-0+dirty (...)`; when the version itself ends with `+incompatible`, `.dirty` is used instead.
- The commit is taken from `JSE_COMMIT`, otherwise from the first 7 characters of the current git HEAD.
- The build time is taken from `JSE_BUILD_DATE` (UTC, ISO 8601), otherwise from the time of this build.
- When commit/build information is unavailable, the parenthesized part is omitted, e.g. after a local `cargo install` only `jse v0.9.0` is shown.

The release workflow injects these three variables before building, so version information is complete in Release packages; after the GitHub Release is created, the workflow also publishes the same version to crates.io.

## Runtime requirements

- **Terminal**: the full-screen TUI requires stdin to be a terminal; when stdin is not a terminal, startup is refused with a hint to use `--accessible` (exit 1).
- **Config path**: must be a local file; `http(s)://` as a config argument is rejected (only schemas support remote).
- **Schema**: local path, `http(s)://` or `file://`; for the resolution order see [Schema sources and caching](schema-sources.md).
- **External editor**: optional. If `$EDITOR` is set it is used, otherwise `$VISUAL`, falling back to `vi`. GUI editors need a wait mode (e.g. `code --wait`), otherwise jse reads the temp file before the editor has finished writing it.
- **Clipboard**: optional, only used for operations like `y`/`x`/`p`. On Linux, `wl-copy`/`wl-paste` (Wayland), `xclip`, `xsel` are tried in order; macOS uses `pbcopy`/`pbpaste`; on Windows writing uses `clip`, reading is unsupported. When a tool is missing (or fails to run), jse falls back to the session's built-in clipboard: copy/paste within the session keeps working and the message carries the `(built-in clipboard)` suffix; details in [Editing](editing.md#copy--cut).
- **Fonts**: the `<err>`/`<warn>` icons need a Nerd Font / Font Awesome private-use-area font; without one they render as blank squares, functionality is unaffected.
- **Network**: remote schemas and the SchemaStore catalog go over HTTP(S), honoring `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY` and `NO_PROXY`, with a 15 second timeout and a 32 MiB response body limit.

## Upgrading

Just replace the binary. Config files and cache formats have no migration step; the remote schema cache is reused for 24 hours — to force a refresh, delete the cache directory (see [Remote schema cache](schema-sources.md#remote-schema-cache)).
