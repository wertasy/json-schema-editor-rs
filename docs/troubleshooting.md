# 故障排查

## 启动与 schema

### `no schema found: editing without schema guidance; save-time validation disabled`

这不是错误，表示进入了**无 schema 模式**：文件仍可编辑，保存不校验。想恢复表单和校验，按[解析顺序](schema-sources.md#解析顺序)检查：

1. 是否传了 `-s`；
2. 配置顶层是否有非空 `$schema` 字符串（YAML 也可是文件头 `# yaml-language-server: $schema=...`，TOML 也可是 `#:schema <位置>`）；
3. 文件名能否在 SchemaStore 目录里唯一匹配；
4. 相对 schema 路径是否相对配置文件目录存在，或相对当前工作目录存在。

临时解决：`jse -s ./schema.json config.json`。

### `schema catalog unavailable: ...`

需要 SchemaStore 但目录下载失败（网络、代理、镜像）。无 schema 模式仍可继续编辑；想使用 SchemaStore 可以：

- 提供 `-s`；
- 配置里写入 schema 路径；
- 设置 `JSE_CATALOG_URL` 指向内网镜像；
- 如果之前缓存过目录，断网时会自动使用缓存。

### `schema selection needs a terminal; rerun with --accessible or provide -s`

需要打开全屏 schema 选择器，但 stdin 不是终端。加 `--accessible`（行式选择）或直接 `-s`。

### `config must be a local file path`

位置参数不能是 URL；schema 才支持远端。把远端 schema 放到 `-s`。

### `stdin is not a terminal; rerun with --accessible ...`

全屏 TUI 需要终端。在脚本/CI 中加 `--accessible`，或从真实终端运行。

### `load schema ...` / `schema compile failed, save-time validation disabled`

- schema 文件不存在、不是合法 JSON、远端不可达：修正 schema 或网络。
- 能加载但编译失败：编辑器仍可用，但**保存时不再校验**；根屏状态行会一直显示警告。修复 schema 后重启。

### 远端 schema 不更新

默认 24 小时内直接用缓存，不发请求。强制刷新：

```sh
rm -rf ~/.cache/jse
# 或
JSE_CACHE_DIR=$(mktemp -d) jse -s https://example.com/schema.json config.json
```

### `GET ...: status 404`

URL 写错或 schema 已迁移；404 不会回退到缓存。修正 URL。

### `rate limited` / 429

用缓存启动并提示重试时间；限流窗口（`Retry-After`，缺省 60 秒）跨进程保存。等待后重试，或改用本地 schema。

### `response exceeds size limit`

响应超过 32 MiB。确认 URL 指向的是 schema 而不是网页/大文件。

### 代理不生效

按顺序读取 `HTTPS_PROXY` / `https_proxy`、`HTTP_PROXY` / `http_proxy`、`ALL_PROXY` / `all_proxy`；`NO_PROXY` / `no_proxy` 优先排除。支持 `*`、域名后缀与前导点。代理 URL 不要带多余空格。

## 配置解析

### `parse config ...` / `invalid JSON` / `invalid YAML` / `invalid TOML`

- JSONC 允许注释和尾随逗号；其他 JSON 语法错误仍需修正。
- YAML 只支持单个文档、字符串键；多文档、非字符串键、`.inf`/`.nan` 会报错。
- 空 YAML 文件报错；不存在的文件才会从空对象开始。
- 时间戳按字符串处理；`!!binary` 与未知标签按字符串处理。
- TOML 要求根值是表；重复键/重复表、扩展内联表、`inf`/`nan` 会报错。空 TOML 文件等于空表。
- TOML 日期/时间按字符串展示；未编辑时原字面量保留。

### 保存后格式变了

- 无注释 JSON 输出保持不变。
- JSONC 的尾随逗号不会保留；注释跟随成员。
- YAML 的缩进、引号风格、空行可能被 emitter 归一化；键序、注释、锚点、别名与合并键保留。
- TOML 的空行、点号键写法、引号风格与内联表布局可能被归一化；键序、注释、未编辑的字面量保留。所有元素为表的数组会写成 `[[table]]`。
- 需要逐字节保留时，先用 `cp` 备份再对比。

## 编辑

### 中文/宽字符显示错位

jse 按终端单元格宽度裁剪；如果终端字体把全角字符当半角，或使用非等宽字体，可能错位。换等宽终端字体。

### `<err>` / `<warn>` 显示为方块

安装了 Nerd Font / Font Awesome 私用区图标的终端字体才会显示图标；否则显示空白方块。功能不受影响。

### `y` / `p` 失败

剪贴板工具缺失：

- Linux Wayland：`wl-copy` / `wl-paste`
- Linux X11：`xclip` 或 `xsel`
- macOS：`pbcopy` / `pbpaste`
- Windows：写入用 `clip`；**读取（粘贴）不受支持**

错误提示如 `clipboard: no clipboard tool found` 或 `clipboard: cannot read without paste support`。安装工具后重试；失败不会修改文档。

### `$EDITOR` 打开后内容没生效

- GUI 编辑器需要等待模式：`export EDITOR="code --wait"`。
- 编辑器异常退出 / 语法无效时 jse 保留原文档并提示。
- 内置 raw 编辑和单值外编都按文件自身格式（JSON、YAML 或 TOML）解析。

### 找不到 `< Expand >` / 锚点操作

`&`、`*`、`<`、`ga`、`gr` 只对 YAML 文档有意义；JSON/TOML 文档没有锚点。别名必须真实存在于 YAML 中。

### 删除不了某个键

- schema required 字段或 required merged key 会拒绝删除。
- 被别名引用的锚点要先移除引用，才能删除锚点。
- 要断开合并，先按 `<` 清空合并键。

### 输入框里 `q` 不退出

输入框内 `q` 是普通字符；取消用 `esc`。

## 保存与退出

### 保存被校验拦住

- 根菜单首行的问题计数可 `enter` 打开；`enter` 跳到字段修复。
- 保存错误列表可 `enter` 跳到字段；`n`/`esc` 返回。
- 只有带 `--force` 时错误列表才接受 `y` 强存。
- schema 编译失败时保存期校验停用；先修复 schema。

### 退出后文件没变

- 根屏 `n` 放弃、`ctrl+c` 都不写文件。
- 没有改动时 `esc`/`q` 直接退出，不写文件。
- 多文件时 `ctrl+s` 只保存当前文件；确认保存提示里的路径。
- 保存失败（权限、只读目录、磁盘满）会在底部提示；文件保持原样。

### `Saved to ...; aborted (changes after the last save are lost)`

会话中曾 `ctrl+s`/`w` 保存，之后又修改，然后用 `ctrl+c`/放弃退出。磁盘上是最后一次保存的内容。

### 退出码

见[命令行：退出码与 stdout](cli.md#退出码与-stdout)。

## 性能

- 大文件/大 schema 首次打开会做 schema 编译与校验；远端 schema 首次抓取后缓存。
- 搜索每次打开重建当前子树索引。
- 撤销上限 100 步、宏上限 500 键，内存不会无限增长。

## 报告问题时请附上

```sh
jse --version
uname -a
echo "$TERM"
```

以及：

- 最小可复现的 schema 与配置文件（去掉敏感值）；
- 完整命令与退出码；
- 终端类型（TUI/`--accessible`）；
- 是否能稳定复现，以及发生在哪个按键/保存步骤。
