# 安装与构建

## 预编译二进制

从 [Releases](https://github.com/wertasy/json-schema-editor-rs/releases) 下载对应平台压缩包：

| 平台 | 目标 |
|---|---|
| Linux amd64 | `x86_64-unknown-linux-musl`（静态链接） |
| Linux arm64 | `aarch64-unknown-linux-musl`（静态链接） |
| macOS amd64 | `x86_64-apple-darwin` |
| macOS arm64 | `aarch64-apple-darwin` |
| Windows amd64 | `x86_64-pc-windows-msvc` |
| Windows arm64 | `aarch64-pc-windows-msvc` |

压缩包内含 `jse`（Windows 为 `jse.exe`）、`README.md`、`docs/`、`AGENTS.md`、`LICENSE`；`checksums.txt` 提供 SHA-256 校验和。示例：

```sh
tar -xzf jse-v0.7.0-linux-amd64.tar.gz
mkdir -p ~/.local/bin
mv jse ~/.local/bin/
jse --version
```

Linux 静态包不依赖 glibc，可直接放进容器或最小发行版镜像。

## 从源码构建

需要 Rust stable（`rust-toolchain.toml` 指定 `stable`；源码使用 edition 2024）。

```sh
cargo build --release
./target/release/jse --version
```

安装到 Cargo 的 bin 目录：

```sh
cargo install --path .
```

也可以从 crates.io 安装（crate 名 `json-schema-editor`，二进制仍为 `jse`）：

```sh
cargo install json-schema-editor
```

安装位置通常是 `~/.cargo/bin/jse`；确保该目录在 `PATH` 中。

### 静态发布构建

Linux 发布使用 musl：

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

需要 `musl-gcc`（Debian/Ubuntu：`sudo apt-get install musl-tools`），并让 Cargo 使用它作为链接器：

```toml
# .cargo/config.toml
[target.x86_64-unknown-linux-musl]
linker = "musl-gcc"
```

arm64 交叉编译还需 `gcc-aarch64-linux-gnu`，并把 `linker` 设为 `aarch64-linux-gnu-gcc`。发布工作流里的做法可直接参考 `.github/workflows/release.yml`。

## 版本号与构建元数据

`jse --version` 输出形如：

```
jse v0.7.0 (commit 1a2b3c4, built 2026-10-22T09:00:00Z)
```

- 版本号优先取环境变量 `JSE_VERSION_OVERRIDE`。否则从 git 推导：
  - 取最高可达的 semver tag，例如 `v0.7.0`；没有可达 tag 时为 `v0.0.0`。
  - HEAD 在 tag 之后仍沿用该 tag：提交号和构建时间已单独显示，不再构造额外的开发版本号；
  - 不在 git 检出内：用 crate 版本，例如 `v0.7.0`。
- 工作区有未提交改动（含未跟踪文件，gitignore 的不算）时在版本号后追加 `+dirty`，例如 `jse v0.3.1-0+dirty (...)`；版本本身以 `+incompatible` 结尾时改用 `.dirty`。
- 提交号取 `JSE_COMMIT`，否则尝试当前 git HEAD 的前 7 位。
- 构建时间取 `JSE_BUILD_DATE`（UTC，ISO 8601），否则取本次构建时间。
- 没有提交/构建信息时省略括号部分，例如本地 `cargo install` 后只显示 `jse v0.7.0`。

发布工作流在构建前注入这三个变量，所以 Release 包中的版本信息完整；GitHub Release 创建后，工作流还会把同一版本发布到 crates.io。

## 运行环境

- **终端**：全屏 TUI 需要 stdin 是终端；stdin 不是终端时会拒绝启动并提示改用 `--accessible`（exit 1）。
- **配置路径**：必须是本地文件；`http(s)://` 作为配置参数会被拒绝（schema 才支持远端）。
- **Schema**：本地路径、`http(s)://` 或 `file://`；解析顺序见 [Schema 来源与缓存](schema-sources.md)。
- **外部编辑器**：可选。设置了 `$EDITOR` 用它，否则用 `$VISUAL`，再退回 `vi`。GUI 编辑器需要等待模式（如 `code --wait`），否则 jse 会在编辑器还没写完时就读取临时文件。
- **剪贴板**：可选，只在 `y`/`x`/`p` 等操作时用到。Linux 依次尝试 `wl-copy`/`wl-paste`（Wayland）、`xclip`、`xsel`；macOS 用 `pbcopy`/`pbpaste`；Windows 写入用 `clip`，读取不受支持。缺少工具（或工具执行失败）时 jse 退回会话内置剪贴板，会话内复制/粘贴照常可用，提示带 `(built-in clipboard)` 后缀；细节见[编辑功能](editing.md#复制--剪切)。
- **字体**：`<err>`/`<warn>` 图标需要 Nerd Font / Font Awesome 私用区字体；没有时显示空白方块，功能不受影响。
- **网络**：远端 schema 与 SchemaStore 目录走 HTTP(S)，遵循 `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY` 与 `NO_PROXY`，超时 15 秒，响应体上限 32 MiB。

## 升级

替换二进制即可。配置文件和缓存格式没有迁移步骤；远端 schema 缓存在 24 小时内复用，需要强制刷新时删除缓存目录（见 [远端 schema 缓存](schema-sources.md#远端-schema-缓存)）。
