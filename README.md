# jse — JSON Schema 驱动的终端配置编辑器

`jse` 读取一份 JSON Schema，把配置文件变成 menuconfig 风格的终端界面：单屏滚动列表、面包屑、就地编辑、层级钻取；保存前用 JSON Schema 做权威校验。适合编辑服务端配置、CI 工作流、sing-box 规则这类「有 schema、但手写 JSON/YAML 容易出错」的文件。

支持 **JSON / JSONC / YAML**，保留注释、键序、数字字面量、YAML 锚点/别名与合并键；一个进程可同时打开多个配置文件。也提供 `--accessible` 行式问答模式，可在管道、脚本和屏幕阅读器中使用。

```
Server Configuration                              ●
$
  Name*                   webapp
  Mode                    http
> Server*                 port=8080 host=0.0.0.0 +1  --->
  Logging                 —  --->
  Mounts                  1 item  --->
  Auth                    —  --->
  < Save and exit >
  < Edit raw JSON >
↑/↓ move · enter edit/open · esc back · ctrl+s/w save · / search · ? help
```

## 特性

- **Schema 驱动**：`properties`/`required`、`enum`/`const`、`oneOf`/`anyOf`/`allOf`、`$ref`（含远端）、数组、映射、`patternProperties` 都会生成对应表单；未覆盖的关键字仍在保存时校验。
- **格式保真**：JSONC 注释、YAML 注释、锚点、别名和 `<<` 合并键原样保留；键序与 `1.0` 这类数字字面量不变；写入为同目录临时文件 + rename 的原子写。
- **高效编辑**：vim 风格移动与跳转、多选、复制/剪切/智能粘贴、当前子树搜索与正则替换、撤销/重做、宏录制、`$EDITOR` 外编、文本自动补全；schema 未定义或无 `type` 的键按运行时类型直接编辑，仅在无值时先选内建类型或 schema `$defs` 命名类型。
- **Schema 自动发现**：`-s`、配置自带的 `$schema`、YAML 文件头指令，或 SchemaStore 目录匹配/检索；会话中按 `S` 可重新搜索并切换当前文件的 schema。远端 schema 带 24 小时缓存与离线回退。找不到 schema 时仍可打开文件直接编辑（对象/数组按结构导航，其他值按运行时类型；保存不校验）。
- **错误可见**：加载即列出校验问题，字段行与问题列表实时刷新；未知键作为警告而非错误；`--force` 可强存。
- **多文件会话**：`jse a.json b.json` 同开多个文件，`s` 切换，`ctrl+o`/`ctrl+i` 在导航历史中后退/前进；退出时一次保存全部改动。

## 安装

预编译二进制见 [Releases](https://github.com/wertasy/json-schema-editor-rs/releases)：Linux（musl 静态）/ macOS / Windows，amd64 与 arm64；压缩包内含 `jse`、`README.md`、`docs/`、`AGENTS.md`、`LICENSE`，`checksums.txt` 提供 SHA-256 校验和。

从源码构建：

```sh
cargo build --release
cargo install --path .        # 安装到 ~/.cargo/bin
```

`jse --version` 输出版本、提交与构建时间。Linux 静态发布目标为 `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl`。

## 快速开始

```sh
# 1. 显式指定 schema，编辑已有文件（原地写回）
jse -s examples/server.schema.json examples/server.config.json

# 2. schema 已记录在配置里，直接打开
jse server.json

# 3. 让 jse 从 SchemaStore 按文件名自动匹配或检索 schema
jse .github/workflows/ci.yml

# 4. 没有 schema 来源时直接编辑（无校验，结构仍可导航）
jse notes.json

# 5. 行式模式（无全屏 TUI，适合脚本/管道）
jse --accessible -s examples/server.schema.json server.json
```

文件不存在时从空文档开始，保存时创建。多个文件可一次打开：

```sh
jse -s server.schema.json a.json b.yaml
```

`-s` 指定的 schema 会按文件格式写回配置（JSON/JSONC 顶层 `$schema`；YAML 默认写文件头 `# yaml-language-server: $schema=...`，若 schema 定义了顶层 `$schema` 属性则写入该字段），下次启动可省略 `-s`。

## 键位速查

| 按键 | 作用 |
|---|---|
| `↑`/`↓` 或 `j`/`k` | 移动光标（首尾循环） |
| `enter` / `→` / `l` | 编辑字段或进入子菜单 |
| `←` / `h` / `esc` / `q` | 返回上一级；根屏 `esc`/`q` 退出并询问保存 |
| `space` | 切换布尔值 / 枚举数组成员 |
| `gg` / `G` | 跳到列表首行 / 末行；数组菜单 `Ngg` 跳到第 N 项 |
| `/` / `r` | 在当前路径下搜索 / 搜索替换 |
| `m` / `v` | 标记单项 / 连选；配合 `y`、`x`、`d` 批量操作 |
| `y` / `Y` / `p` / `P` | 复制键值对 / 复制值 / 智能粘贴到菜单 / 粘贴到光标容器 |
| `u` / `ctrl+r` | 撤销 / 重做 |
| `ctrl+s` / `w` | 保存当前文件并留在编辑器 |
| `W` | 另存副本到指定路径 |
| `s` | 多文件时打开文件切换器 |
| `S` | 打开 SchemaStore 搜索，切换当前文件的 schema |
| `ctrl+o` / `ctrl+i` (`tab`) | 导航历史后退 / 前进 |
| `?` | 完整键位帮助 |

完整键位、界面元素与各 overlay 的按键见[界面与键位](docs/interface.md)。

## 文档

| 文档 | 内容 |
|---|---|
| [用户手册](docs/index.md) | 目录与阅读路径 |
| [安装与构建](docs/install.md) | 预编译包、源码构建、运行依赖 |
| [命令行](docs/cli.md) | 参数、多文件、环境变量、退出码 |
| [Schema 来源与缓存](docs/schema-sources.md) | 解析顺序、SchemaStore、远端缓存、离线回退 |
| [配置文件格式](docs/formats.md) | JSONC、YAML、注释/锚点/合并键、格式扩展 |
| [界面与键位](docs/interface.md) | 布局、状态行、完整按键表、文件切换与导航历史 |
| [编辑功能](docs/editing.md) | 字段编辑、剪贴板、多选、搜索替换、撤销、宏、外部编辑器 |
| [校验与保存](docs/validation.md) | 表单关键字、降级规则、问题列表、原子写、退出码 |
| [行式模式](docs/accessible.md) | `--accessible` 提示协议与脚本示例 |
| [故障排查](docs/troubleshooting.md) | 常见错误、代理、剪贴板、字体、环境变量 |
| [AGENTS.md](AGENTS.md) | 编码代理/贡献者的架构地图、约束与工作流 |

## 开发

```sh
cargo test                                        # 单测 + accessible/pty 黑盒 e2e
cargo run -- -s examples/server.schema.json examples/server.config.json
cargo build --release --target x86_64-unknown-linux-musl   # 静态发布构建
```

代码结构、模块边界、测试约定与修改规则见 [AGENTS.md](AGENTS.md)。发布由 `.github/workflows/release.yml` 完成：推送 `v*` 标签即构建六个平台目标并创建 GitHub Release。

## 许可证

[MIT](LICENSE)
