# GUI（Native Shell）

jse 有第二个前端：基于 [Native Shell](https://nsh.dev)（NSH）的窗口界面，取代终端界面。
它驱动同一套编辑器核心——文档 arena、schema 表单树、保存接线与校验都来自终端 UI，
因此同一个配置文件在两个前端中的编辑与保存行为一致。`gui` cargo feature 决定二进制
携带哪个前端；各平台产物从同一份源码构建。

## 构建

默认构建是终端 UI。GUI 替换同一 `jse` 二进制里的前端：

```sh
cargo build --release --features gui
```

桌面二进制的平台要求：

- **Linux**：运行处需要 GTK 4.14+（构建需要 `libgtk-4-dev clang`）。
- **Windows**：Windows 10 1809+；用 MSVC 工具链构建。
- **macOS**：macOS 14+。
- **iOS**：iOS 15+；设备包必须签名（`--sign <identity>`，可选
  `--profile <.mobileprovision>`）才能安装；未签名构建供重签使用，
  `nsh package --target ios-simulator` 产出可直接在 Simulator 运行的应用。

需要打包好的应用产物（Linux 应用包、Windows 程序目录与安装脚本、Android APK）时，
用 Native Shell CLI：

```sh
nsh package --target linux     # .tar.gz，含 .desktop 入口
nsh package --target windows   # 程序目录 + Inno Setup 脚本
nsh package --target android   # APK（arm64-v8a 与 armeabi-v7a）
nsh package --target macos     # .app，ad hoc 签名
nsh package --target ios       # 设备包：--sign <identity> 时为 .ipa，
                               # 否则为未签名 .app
```

仓库根目录的 `nsh.json` 指向 `gui-app/`——一个为生成的 runner 打开库 `gui` feature 的
薄 crate。Android 需要 Android SDK、NDK 与 JDK（先 `nsh component add android`）；
桌面目标需要各自平台的原生工具链。

## 运行

GUI 二进制接受与终端版相同的参数——
`jse [-s schema] config.(json|jsonc|yaml|yml|toml)`——为每个会话打开编辑窗口，第一个
文件处于激活状态。不带参数时从打开页开始（也是 Android 的启动状态）：输入配置路径点
Open，或点 **Browse…** 用系统选择器挑文件（桌面为打开对话框，Android/iOS
为文档选择器）。选中的文件会复制到应用数据目录，后续编辑与保存都作用于
该副本——移动端暂不支持写回提供方。另外 NSH 0.7.0 的 Android 应用无法注册
为任意文件类型的打开方式（`nsh.json` 的 `fileTypes` 在 Android 只注册分享
面板），文件管理器的"打开方式"列表暂时不会出现 jse。

## 编辑

窗口是当前 schema 层级的字段列表，与终端 UI 的菜单屏一致。点按行执行对应动作：

- **字段**（字符串、数字、枚举）：打开编辑表。Apply 按终端的规则写入（空输入删除可选
  键并回填默认值；枚举值从 schema 的候选中提交）。
- **布尔 / 枚举数组成员**：就地切换。
- **小节、数组、oneOf**（带 `→` 标记的行）：进入下一级；头部的返回按钮回退一级。
- **数组项**：尾部 ✕ 删除、⇥ 在其后插入；`＋` 行和工具栏的 `＋` 添加数组项与键。
- **未知键**（标注 "not in schema"）：按运行时类型编辑；✕ 删除。

工具栏依次为：返回、文件名与未保存标记（●）、面包屑、空字段过滤（∅，即终端的 `z`）、
添加（＋）、问题计数（⚠）、文件切换（多文件时）、保存。保存与终端一样先校验：有问题时
拒绝保存并弹出校验错误表（`--force` 会话可从表中强制写入）；点按问题项可跳转。直接关窗不保存（不会弹出保存确认）——
请显式点 Save。

表的 Apply/Cancel 对应终端的 enter/esc：Apply 提交的内容就是终端 Enter 会提交的内容，
校验错误会留在屏幕上而不是写入。

## 与终端 UI 的差异

编辑语义（写穿式应用、键省略规则、oneOf 切换、YAML anchors/merge、保存时校验、
`--force`、原子写入）完全共享。GUI 暂未提供的入口：键盘驱动的列表导航（以指针/触摸
为主）、`/` 搜索与 `r` 替换、撤销/重做界面（快照仍按操作记录）、文档注释编辑（`C`）、
外部 `$EDITOR`、`:` 命令与 write-to（`W`）、SchemaStore 选择器（`S`）。这些仍以终端
二进制为准。
