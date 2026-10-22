# jse 用户手册

`jse` 是一个 schema 驱动的终端配置编辑器：按 JSON Schema 生成表单，把编辑结果写回原文件。本手册面向使用者；贡献者和编码代理请看仓库根目录的 [AGENTS.md](../AGENTS.md)。

## 目录

| 文档 | 什么时候读 |
|---|---|
| [安装与构建](install.md) | 第一次安装、从源码构建、发布包内容 |
| [命令行](cli.md) | 查参数、一次打开多个文件、看退出码与环境变量 |
| [Schema 来源与缓存](schema-sources.md) | 配置没带 schema、想用 SchemaStore、远端 schema 缓存/离线 |
| [配置文件格式](formats.md) | JSONC/TOML 注释、YAML 锚点/合并键、保存后格式变化 |
| [界面与键位](interface.md) | 第一次打开 TUI、查某个键、理解状态行和标记 |
| [编辑功能](editing.md) | 数组/映射/oneOf 编辑、复制粘贴、多选、搜索替换、撤销、宏 |
| [校验与保存](validation.md) | 哪些关键字生效、出错怎么办、保存行为和退出码 |
| [行式模式](accessible.md) | 无 TUI 环境、脚本驱动、屏幕阅读器 |
| [故障排查](troubleshooting.md) | 报错、缓存、代理、剪贴板、字体、编辑器 |

## 三条阅读路径

**第一次使用**

1. [安装](install.md)
2. [快速开始](../README.md#快速开始) 跑一个示例
3. [界面与键位](interface.md) 记住 `↑/↓`、`enter`、`esc`、`ctrl+s`
4. 需要改数组或 oneOf 时看[编辑功能](editing.md)

**给已有配置接上 schema**

1. [Schema 来源与缓存](schema-sources.md)：确认解析顺序
2. [配置文件格式](formats.md)：确认 `$schema` / YAML 文件头注释写在哪
3. [校验与保存](validation.md)：理解未知键警告和 `--force`

**写自动化脚本**

1. [行式模式](accessible.md)：提示协议与 EOF 行为
2. [命令行](cli.md)：退出码、stdout 结果行
3. [Schema 来源与缓存](schema-sources.md)：`JSE_CACHE_DIR`、`JSE_CATALOG_URL`

## 约定

- 按键写法：`ctrl+s`、`alt+x`、`shift+tab`；`↑`/`↓`/`←`/`→` 也写作 `up`/`down`/`left`/`right`。
- 菜单里的 `<err>` / `<warn>` 是两个 Nerd Font 图标：错误（红）与警告（黄）。未安装对应字体时显示为空白方块，不影响功能。
- “根屏”指最外层菜单（面包屑为 `$`）。
- “当前文件”指多文件会话中被显示的那个文件；`ctrl+s` 只保存当前文件，根屏退出时可一次保存所有有改动的文件。
- 命令示例默认在仓库根目录执行。
