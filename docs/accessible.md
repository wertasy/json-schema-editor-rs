# 行式模式（`--accessible`）

`--accessible` 用顺序问答代替全屏 TUI：纯文本、无 ANSI 控制序列，从 stdin 读一行行答案，适合管道、脚本、CI 和屏幕阅读器。

```sh
jse --accessible -s examples/server.schema.json server.json
```

非 `--accessible` 模式下 stdin 不是终端会直接报错；加上该参数后，stdin 可以是管道或文件。

## 提示协议

### 文本 / 数字输入

```text
Name webapp
```

- 提示是 `标题 `（末尾一个空格），读取一行。
- 行内校验失败会打印错误并重新提示；空行取绑定默认值。
- required 字段且没有默认值时，**不能靠空行通过**，必须给出内容。
- raw 字段（整份文档或不可表单化子树）也是一行文本，内容按文件格式（JSON/YAML）；空行表示删除该值/不修改。

### 单选

```text
Mode 
1. (none)
2. http
3. grpc
4. tcp
Enter a number between 1 and 4: 
```

- 标题单独一行，然后编号选项。
- 空行接受预选默认项。
- 只有一个选项时提示 `There is only one option available; enter the number 1:`。
- 可选枚举会多一个 `(none)` 选项；required 枚举没有。
- 数字越界会打印 `Invalid: must be a number between 1 and N` 并重问。

### 多选（枚举数组成员）

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

- 反复输入编号切换选中状态。
- `0` 确认；不满足 `minItems`/required 时打印 `select at least N` 并继续。
- 空行等效 `0`。

### 确认

```text
Debug [y/N] 
```

- `y`/`yes`/`n`/`no`，大小写不敏感。
- 空行取默认；默认是 `y` 时显示 `[Y/n]`。
- 非法输入打印 `invalid input. please try again` 并重问。

### 说明行

信息提示（schema 来源、缓存回退、schema 编译失败等）在根屏开头作为无输入说明打印，之后接一个空行。

## 菜单结构

### 对象菜单

先按 schema 顺序逐字段提问，最后出现 `Navigate` 单选：

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

- 非根对象：第一项是 `Done (back)`，默认选中。
- 根对象：导航项在前，然后是 `Save and exit` 与 `Edit raw JSON`（YAML 文件显示 `Edit raw YAML`）；默认选中 `Save and exit`。
- 数组/对象/oneOf 子节点显示为 `Edit ... ▸`；枚举数组直接作为多选字段，不进入子菜单。
- 没有可编辑字段时打印 `Nothing to edit here`。

### 数组菜单

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

- 默认选 `< Add item`；空行会进入新增流程，而不是保存。
- 选择条目进入条目表单。
- `Move item up…` / `Move item down…` 先选条目再移动；越界移动是静默 no-op。
- `Remove item…` 先选条目，再确认 `Remove item N? [Y/n]`。
- 根数组的保存项是 `Save and exit`；嵌套数组是 `Done (back)`。

### oneOf

先选择变体（`1. 标题` …），再进入变体编辑器；根 oneOf 编辑完后自动进入保存校验，嵌套 oneOf 编辑完后返回。

### 标量根

直接提问该值，然后自动进入保存校验。

## 保存与校验

- 选择 `Save and exit` 后校验；有错误时只询问：

  ```text
  Save anyway? [y/N] 
  ```

  错误明细**不会逐条打印**。只有启动时带 `--force` 且回答 `y` 才写入；否则返回菜单。
- 写入成功后进程继续下一个文件（多文件），全部完成后退出。
- 写入失败会在 stderr 报告并中止，exit 1。
- 退出码与 stdout 结果行与 TUI 模式一致，见[命令行](cli.md#退出码与-stdout)。

## EOF 与脚本

- 提示处遇到 EOF 会结束当前提示并取默认值（不会在原提示上无限重问）。
- 当前屏幕处理完后，下一次循环检测到 EOF：
  - 根屏：中止会话；
  - 子屏：返回上一级。
- 但默认值可能先产生一个结果：根对象菜单的默认导航项是 `Save and exit`，因此“字段默认 + EOF”可能直接保存并 exit 0；根数组菜单的默认项是 `< Add item`，可能先新增再中止。
- 因此脚本应覆盖实际会问到的所有字段与导航选择，不要依赖 EOF 的默认值。
- 非根对象默认导航项是 `Done (back)`，所以空行回车可以接受默认路径。
- 提示与结果都写在 stdout；错误写 stderr。

最小脚本示例（schema 只有可选字段，直接保存）：

```sh
printf '\n' | jse --accessible -s mini.schema.json out.json
```

有必填字段时必须逐项给值，再在根菜单选择 `Save and exit`（或让空行选中它）。多文件按参数顺序依次完成。

## SchemaStore 选择器

无 schema 时，`--accessible` 使用行式检索：

```text
Select a schema for ci.yml
? Schema search [ci]: 
  1) GitHub Workflow — GitHub Actions workflow
? Number (0 = new search, enter = abort): 1
```

- 空行保留当前查询；`0` 重新搜索；直接回车中止。
- 最多列 15 条，更多时提示细化查询；无结果显示 `no matches` 并回到搜索提示。
- 非数字或越界打印 `! invalid number` 并重问。
- EOF 中止。

## 限制

- 没有实时问题列表、字段行标记、面包屑、多选/宏/撤销/搜索替换/文件切换器。
- 文本输入都是单行；多行 YAML/JSON 内容需要写成一行（JSON 用 `\n` 转义，YAML 用流式或引号形式）。
- 校验只在保存时进行；加载时的既有错误不会在行式流程里单独列出。
- 多个文件顺序处理，不能在一个文件内跳到另一个文件。
