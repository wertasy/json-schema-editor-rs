# 校验与保存

jse 把表单当作编辑辅助，把 **JSON Schema 校验**当作权威判定：表单没覆盖的关键字、跨字段约束、`additionalProperties: false` 等，全部在保存时由 `jsonschema` 库检查。

## 支持的表单关键字

| 类别 | 关键字 | 表单行为 |
|---|---|---|
| 元信息 | `title` `description` `default` `examples[0]` | 标题、描述、预填值、占位符 |
| 标量 | `type: string \| integer \| number \| boolean` | 文本输入；布尔切换 |
| 枚举 | `enum` / `const` | 选项少时行内选择，多时选择器；全 const/单值 enum 变体合并为一个枚举 |
| 数值约束 | `minimum` `maximum` `exclusiveMinimum` `exclusiveMaximum` `multipleOf` | 输入时即时校验 |
| 字符串约束 | `minLength` `maxLength` `pattern` | 输入时即时校验 |
| 对象 | `properties` `required` | 字段按 schema 顺序；required 空值拦截 |
| 对象映射 | `additionalProperties` | 文档成员即键；`a` 添加、`d` 删除，值按子 schema 编辑 |
| 模式映射 | `patternProperties`（单个模式 + `additionalProperties: false`） | 同映射，但 `a` 新增键必须匹配该正则 |
| 数组 | `items` `minItems` `maxItems` | 枚举 items → 多选；标量 → 逐条输入；对象 → 逐条表单 |
| 组合 | `oneOf` / `anyOf` | 变体选择后进入子编辑器；开放字符串 + 字符串枚举合并为文本输入并给补全 |
| 组合 | `allOf` | 浅合并（properties 拼接、required 并集、约束取更严者）；其中 `oneOf`/`anyOf` 按分配律展开 |
| 引用 | `$ref` | 本地指针与远端 ref；缓存 + 环保护，深度上限 32 |

其他关键字不参与表单生成，但保存时仍会校验。

## 降级规则

- 不可达 `$ref`、数组套数组、未知类型、schema 深度超过 32 层、递归 `$ref`：有值时按运行时类型直接编辑；无值/null 时使用类型选择器（String/Number/Boolean/Object/Array/Null、schema `$defs`/`definitions` 命名类型、JSON），选 `JSON` 时渲染为 raw 文本区，提交时只做语法校验，语义交给保存期校验。
- schema 编译失败（非法 schema、远端 ref 不可达等）：**停用保存期校验**，根屏状态行提示 `<warn> schema compile failed, save-time validation disabled: ...`。此时保存不再有 schema 兜底。
- 加载的配置已有校验问题：不阻止编辑，根菜单首行显示问题计数。加载时的错误只用于展示；每次文档修改后会重新校验并刷新。
- 明确不做：`format` 的表单校验、数组重排序 UI、`if-then-else`/`not` 的表单化、多模式 `patternProperties`、远端配置路径。

## 问题列表

根菜单首行：

```text
<err> 2 validation errors · <warn> 1 warning
```

- `enter` 打开问题列表；`↑`/`↓`（或 `ctrl+j`/`ctrl+k`）选择，`enter` 跳到对应字段，`esc` 关闭。
- 条目同时按严重度着色：错误红、警告黄；位置使用文档路径，如 `at '$.server.port':`、`at '$.items[0].tag':`。
- **实时刷新**：每次修改后计数、字段行尾 `<err>`/`<warn>` 与条目立即更新；全部修完后状态行自动隐藏。
- 从问题列表跳转后，在落点菜单按 `esc` 回到刷新过的问题列表（光标保留）；继续下钻则先逐级返回，再回列表。
- 搜索跳转或重新打开列表会替换该返回点。

## 保存

### 触发方式

| 操作 | 行为 |
|---|---|
| `ctrl+s` / `w` | 保存当前文件并留在编辑器；成功提示 `Write to <路径>`，变更标记清零。 |
| 根菜单 `< Save and exit >` | 保存当前文件（多文件时保存所有有改动的文件）并退出；不弹保存询问。 |
| 根屏 `esc`/`q` → `y` | 保存所有有改动的文件并退出。 |
| `W` | 另存副本到指定路径，不改变会话保存目标。 |
| `--accessible` | 在根屏选择 `Save and exit`。 |

### 校验门

- 保存前校验当前文档；有错误则打开全屏错误列表，标题 `Cannot save: validation errors`。
- `enter` 跳到出错字段；`n`/`esc` 返回继续编辑。
- 只有启动时带 `--force`，错误列表才允许 `y` 强存；强存结果行是 `Saved with validation errors (--force): <path>`，退出码仍为 0。
- `--accessible` 模式下校验错误**不逐条打印**，只询问 `Save anyway? [y/N]`；只有同时带 `--force` 且回答 `y` 才写入。回答 `n` 返回根屏。

### 写入细节

- **原子写**：同目录临时文件 + rename；保留已存在文件的权限位。
- 键序、数字字面量（如 `1.0`）原样保留。
- schema 之外的未知键、`$schema` 键都保留。
- 输出格式由扩展名决定；raw 编辑和 `$EDITOR` 外编的修改同样按原格式写回。
- YAML 的注释、锚点、别名、合并键按[配置文件格式](formats.md)的规则写回。

### 多文件

- `ctrl+s` 只保存当前文件。
- 根屏退出时若有多个文件有改动，询问 `Save changes in N files? (y/n)`；`y` 逐个保存（当前文件优先），全部成功才退出。
- 若某个文件校验失败，保存流程停在那个文件并打开错误列表；修复后重新退出，或带 `--force` 强存。
- 保存失败（权限、磁盘错误等）会提示并留在会话，其他文件保持未保存状态。

## 退出码

| 退出码 | 含义 |
|---|---|
| 0 | 已保存；或无改动正常退出；或 `--force` 强存 |
| 1 | 运行错误（读取/解析、schema 加载、TUI 错误、stdin 不是终端等） |
| 2 | 用法错误（参数、缺少配置文件、`--pick` 中止） |
| 130 | 用户中止（`n` 放弃、`ctrl+c`） |

stdout 结果行的完整列表见[命令行：退出码与 stdout](cli.md#退出码与-stdout)。
