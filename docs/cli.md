# 命令行

```text
jse [-s <schema路径|http(s)://URL>] [-p|--pick] [-f|--force]
    [-a|--accessible] [-v|--version] [-h|--help] <config.(json|jsonc|yaml|yml|toml)>...
```

一切以 `/config.(json|jsonc|yaml|yml|toml)>...` 位置参数都是要编辑的本地配置文件；一个进程可以同时打开多个。

## 参数

| 参数 | 说明 |
|---|---|
| `-s`, `--schema <path\|URL>` | 指定 JSON Schema：本地路径、`file://`、`http(s)://`。会按配置文件格式写入其 schema 指向（已有非空指向则保留）。 |
| `-p`, `--pick` | 强制打开 SchemaStore 检索选择器；覆盖 `-s` 与配置自带的 `$schema`，并为每个文件各选一次。 |
| `-f`, `--force` | 保存时即使 schema 校验失败也写入。结果行变为 `Saved with validation errors (--force): <path>`。 |
| `-a`, `--accessible` | 行式问答模式，不使用全屏 TUI。详见[行式模式](accessible.md)。 |
| `-v`, `--version` | 打印版本并退出。 |
| `-h`, `--help` | 打印帮助并退出。 |

参数解析兼容 GNU getopt 风格：

- 选项可出现在位置参数之后：`jse a.json -s schema.json`。
- 长选项支持 `--schema=path`；短选项可合并：`-fa`；带值的短选项可紧贴值：`-sschema.json`（`-s=path` 也接受）。
- `--` 之后全部当作文件名。文件名以 `-` 开头时必须用它：`jse -- -name.json`。
- `-h`/`-v` 一经解析立即生效，不再处理后续参数。

## 基本行为

- **文件不存在**：从空对象文档开始，保存时创建。
- **配置必须是本地路径**：把 `http(s)://...` 放在位置参数会报 `config must be a local file path`，exit 2。
- **没有位置参数**：报错并打印用法，exit 2。
- **TUI 需要终端**：非 `--accessible` 模式下 stdin 不是终端会拒绝启动（exit 1），避免在半管道环境里画出错乱的界面。
- **先全部加载再进界面**：多文件模式下，任一文件读取或解析失败都会在界面出现前终止整个运行；每个文件各自解析 schema。
- **没有 schema 也能编辑**：`-s`、配置指向和 SchemaStore 都找不到时进入无 schema 模式（对象为键菜单、数组为条目菜单、其他值按运行时类型；保存不校验），不会因缺少 schema 退出。详见[Schema 来源](schema-sources.md#4-全部失败无-schema-模式)。
- **格式按扩展名**：`.json`/`.jsonc` 用 JSONC，`.yaml`/`.yml` 用 YAML，`.toml` 用 TOML；其它扩展名按 JSONC。详见[配置文件格式](formats.md)。

## 多文件会话

```sh
jse -s server.schema.json a.json b.yaml c.json
```

- 启动显示第一个文件；标题行是文件名，右侧显示 `[1/3]`（子菜单里仍带文件名：`[1/3 a.json]`）。
- `s` 打开文件切换器（`↑/↓` 或 `j/k` 移动，`enter` 切换，`esc` 关闭）。有未保存改动的文件在切换器里带 `●`。
- 每个文件保留自己的面包屑、光标、撤销栈、变更基线和问题列表。
- `ctrl+o` / `ctrl+i`（`tab`）按导航历史后退/前进，可跨文件回退，例如切到文件 B 后再 `ctrl+o` 会回到文件 A 的离开位置。
- `ctrl+s` / `w` **只保存当前文件**。根屏按 `esc`/`q` 退出时若有多个文件有改动，询问 `Save changes in N files? (y/n)`，`y` 会逐个保存；`< Save and exit >` 行不询问，直接保存所有有改动的文件。
- 批量保存会按顺序写入；某文件校验失败时停在那个文件并打开错误列表，修复后重新退出，或带 `--force` 强存。

`--accessible` 模式下文件按参数顺序**逐个**编辑：每个文件从根屏走一遍问答，全部完成后进程退出。

## 退出码与 stdout

| 退出码 | 含义 | stdout 结果行 |
|---|---|---|
| 0 | 已保存；或无未保存改动时正常退出 | `Saved to <path>`（多文件每行一个）/ `No changes, nothing written` / `Saved with validation errors (--force): <paths>` |
| 1 | 运行错误：读取/解析配置失败、schema 抓取或编译前置失败、TUI 错误、stdin 不是终端 | 错误信息写到 stderr |
| 2 | 用法错误：未知参数、没有配置文件、`--pick` 中止、`--pick` 选择器缺少终端 | 错误信息与用法写到 stderr |
| 130 | 用户中止：根屏选 `n` 放弃、`ctrl+c` | `Aborted, no changes written`；若此前保存过：`Saved to <path>; aborted (changes after the last save are lost)` |

注意：

- `--force` 强存仍是 exit 0。
- `ctrl+c` 不写文件；但会话中已经 `ctrl+s`/`w` 保存的内容会留在磁盘上，其结果行会说明最后一次保存之后的修改已丢失。
- 根屏在没有改动时按 `esc`/`q` 直接退出，不写文件，输出 `No changes, nothing written`。

## 示例

```sh
# 显式 schema
jse -s examples/server.schema.json server.json

# schema 已写在配置的 $schema / YAML 文件头 / TOML `#:schema` 里
jse server.yaml
jse Cargo.toml

# 远端 schema
jse -s https://example.com/schemas/server.schema.json server.json

# 强制从 SchemaStore 检索（即使配置已有 schema）
jse --pick .github/workflows/ci.yml

# 校验失败也写入
jse --force -s schema.json broken.json

# 行式模式（交互式逐行问答；脚本示例见 docs/accessible.md）
jse --accessible -s examples/server.schema.json new.json
```

## 环境变量

| 变量 | 作用 |
|---|---|
| `JSE_CACHE_DIR` | 覆盖远端 schema / SchemaStore 目录缓存目录。默认 `$XDG_CACHE_HOME/jse`（通常 `~/.cache/jse`）。目录不可写时静默不缓存。 |
| `JSE_CATALOG_URL` | 覆盖 SchemaStore 目录地址，用于测试或内网镜像。 |
| `EDITOR` / `VISUAL` | 外部编辑器；都没有时用 `vi`。 |
| `HTTPS_PROXY` / `https_proxy`、`HTTP_PROXY` / `http_proxy`、`ALL_PROXY` / `all_proxy` | 远端请求代理，按此顺序取第一个非空值。 |
| `NO_PROXY` / `no_proxy` | 不走代理的主机列表；支持 `*`、域名后缀与前导点。 |
| `XDG_CACHE_HOME` | 缓存根目录；未设置时用平台默认缓存目录。 |
| `JSE_VERSION_OVERRIDE`、`JSE_COMMIT`、`JSE_BUILD_DATE` | **仅构建时**注入版本元数据（发布工作流使用）。 |

`JSE_CACHE_DIR` 不能通过空值禁用缓存；想强制刷新就删除缓存目录或换一个目录。
