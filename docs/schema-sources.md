# Schema 来源与缓存

jse 必须知道用哪份 JSON Schema 生成表单。解析按固定顺序进行，找到即用；全部失败才报错。

## 解析顺序

### 1. 显式 `-s` / `--schema`

```sh
jse -s ./schemas/server.json config.json
jse -s https://example.com/schemas/server.json config.json
```

- `http://` / `https://` 开头按远端抓取。
- 其他值按本地路径（也接受 `file://`）。
- `-s` 指定的位置会写进配置的 schema 指向，下次可省略。已有非空指向时保留原值，不覆盖；`--pick` 例外，它会覆盖。

### 2. 配置自带的 schema 指向

没有 `-s` 时读取配置文件：

| 格式 | 指向写法 | 说明 |
|---|---|---|
| JSON / JSONC | 顶层 `"$schema": "<位置>"` | 必须是字符串；空字符串不算。 |
| YAML | 文件头注释 `# yaml-language-server: $schema=<位置>` | 也可用顶层 `$schema` 键，显式键优先。jse 不会向 YAML 添加 `$schema` 键。 |

- URL 直接使用。
- 无 scheme 的相对路径先相对**配置文件所在目录**解析；该目录下不存在时，回退到**当前工作目录**。
- 绝对路径直接使用。
- `$schema` 键在 schema 里有定义时（如 sing-box），它也会作为普通字段出现在编辑器里，可以修改。

### 3. SchemaStore 目录

前两步都没有结果时，下载 [SchemaStore](https://www.schemastore.org/) 公共目录（`https://www.schemastore.org/api/json/catalog.json`），按配置文件的绝对路径做 `fileMatch` 匹配：

- 不带 `/` 的模式只匹配**文件名**（大小写不敏感）：`package.json`、`tsconfig*.json`、`*.srv.json`。
- 带 `/` 的模式从路径尾部按目录段匹配，`**` 跨任意多级目录，`*`/`?`/`[...]` 只在单段内生效。例如 `**/.cdf/config.yaml`、`.platform/applications.yml`。
- 同一 URL 被多个条目列出时去重。
- **唯一命中**：直接采用，不弹选择器。
- **多个命中或没有命中**：打开检索选择器。
- `--pick` 强制打开选择器，即使已有 `-s` 或配置指向。

TUI 选择器：

| 按键 | 作用 |
|---|---|
| 输入 | 按名称或描述过滤（大小写不敏感） |
| `↑`/`↓`、`ctrl+k`/`ctrl+j` | 上下移动 |
| `tab` / `shift+tab` | 下 / 上移动 |
| `enter` | 选定（结果为空时不动作） |
| `esc` / `ctrl+c` | 中止 |

初始查询词是文件名去掉扩展名；第一次输入文本会替换这个预置查询，而不是追加。

`--accessible` 选择器：

```text
Select a schema for ci.yml
? Schema search [ci]: 
  1) GitHub Workflow — GitHub Actions workflow
? Number (0 = new search, enter = abort): 1
```

空行保留当前查询；`0` 重新搜索；直接回车中止；最多显示 15 条，超出提示继续细化。输入越界会提示 `! invalid number` 并重问。EOF 视为中止。

选中的 URL 会按配置格式写回 schema 指向（JSON/JSONC 顶层 `$schema`；YAML 文件头注释），下次启动走第 2 步。

### 4. 全部失败

- 没有 `-s`、配置没有指向、目录也不可用：`no schema found: provide -s or record the schema location in the config`，exit 2。
- 目录下载失败且需要检索：附带 `schema catalog unavailable: ...`，exit 2。
- `--pick` 下目录下载失败：exit 1。
- 选择器被中止：同样报 `no schema found: ...`，exit 2。
- 需要选择器但 stdin 不是终端：提示 `schema selection needs a terminal; rerun with --accessible or provide -s`，exit 2。

## schema 抓取失败

| 情况 | 行为 |
|---|---|
| 本地路径不存在 / 不可读 | exit 1 |
| HTTP 404 等客户端错误 | exit 1，不会被缓存掩盖 |
| 网络错误、超时、5xx，且无缓存 | exit 1 |
| 网络错误、超时、5xx，且有缓存 | 用缓存启动，状态行提示 `<warn> using the cached schema: ...` |
| HTTP 429（限流） | 优先用缓存并提示；无缓存则报错。限流窗口（`Retry-After`，缺省 60 秒）跨进程保存，避免每次启动都打源站 |
| 响应体超过 32 MiB | exit 1（`response exceeds size limit`） |

远端 `$ref`（相对 base URI、`$id`）也会按同一套加载/缓存逻辑抓取。ref 不可达时该字段降级为原始文本编辑，不阻止其他字段编辑；保存期校验若因此无法编译会停用（见[校验与保存](validation.md#降级规则)）。

## 远端 schema 缓存

远端 schema 与 SchemaStore 目录共用缓存。

- **位置**：`$JSE_CACHE_DIR`（显式覆盖）→ `$XDG_CACHE_HOME/jse` → 平台默认缓存目录下的 `jse`。Linux 通常是 `~/.cache/jse`。每个 URL 缓存为 `cache/fetch/<sha256>.body` 与同名 `.json` 元数据。
- **有效期 24 小时**：从上次成功获取/续期起算。期内直接读缓存，**不发起任何请求**。
- **过期后条件请求**：带 `If-None-Match` / `If-Modified-Since`。收到 `304` 继续用缓存并续期；`200` 才重写缓存。
- **离线回退**：网络错误或 5xx 用缓存启动；404 等客户端错误不掩盖。
- **同一次运行只抓一次**：表单模型与校验器共用同一份字节；同一 URL 失败也记忆化，多文件启动不会为每个文件重试。
- **本地文件不缓存**：本地路径与 `file://` 每次读取。
- **目录不可写**：静默降级为不缓存，功能不受影响。

强制刷新：

```sh
rm -rf ~/.cache/jse
# 或使用一次性缓存目录
JSE_CACHE_DIR=$(mktemp -d) jse -s https://example.com/schema.json config.json
```

## 内网 / 镜像

```sh
JSE_CATALOG_URL=https://mirror.example.com/catalog.json \
JSE_CACHE_DIR=/var/cache/jse \
jse config.json
```

`JSE_CATALOG_URL` 接受任何 `fetch::load` 支持的位置（http(s) URL 或本地路径），并遵循同样的 24 小时缓存与离线回退。
