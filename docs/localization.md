# 语言包与本地化

程序内嵌 `resources/locales/zh-CN.toml` 和 `resources/locales/en-US.toml`。编译使用 `include_str!`，GUI 与纯命令行版本都可在没有外置文件时使用中文和英文。默认语言为简体中文。

## 选择语言

窗口顶部的“语言 / Language”下拉框可切换内置及有效的外置语言。切换立即更新界面、已有结果、进度、完成提示和错误弹窗，不改变检测数据。语言包在启动时读取，修改文件后需要重启。

```powershell
scan-angle.exe --lang en-US
scan-angle.exe --lang en-US --help
scan-angle.exe "D:\images" --lang en-US
scan-angle.exe "D:\images" --lang-file "D:\translations\fr-FR.toml"
```

`--lang-file` 单独使用时自动选择文件声明的语言；同时传入 `--lang` 时两者必须匹配。显式指定的损坏文件会报错并以退出码 2 结束。不存在的语言也会报错，不会默默选择另一种语言。

GUI 下拉框选择的语言保存到当前用户的设置目录：Windows 为 `%LOCALAPPDATA%\scan-angle\language.toml`，其他平台为 `$XDG_CONFIG_HOME/scan-angle/language.toml` 或 `~/.config/scan-angle/language.toml`。命令行显式语言选项优先于 GUI 保存的选择。普通批处理始终默认中文，不读取 GUI 偏好。不可读的偏好会显示提示并继续启动；过期或不可用的语言选择会被忽略。

## 外置覆盖及新增语言

在 **EXE 所在目录** 创建 `locales` 目录；它与启动程序时的工作目录无关。

```text
scan-angle.exe
locales/
  zh-CN.toml   # 可选：覆盖内置中文
  en-US.toml   # 可选：覆盖内置英文
  fr-FR.toml   # 可选：新增语言
```

文件编码为 UTF-8。自动发现的文件名必须与文件中 `locale` 一致（忽略大小写），例如 `fr-FR.toml`；显式 `--lang-file` 允许其他文件名。标识使用 ASCII 字母、数字及单个连字符分隔的非空片段，例如 `zh-CN`、`en-US`、`fr-FR`，不接受路径。语言标识匹配时忽略大小写，例如 `en-us` 与 `en-US` 相同。

一个可直接使用的最小第三语言文件：

```toml
schema_version = 1
locale = "fr-FR"
name = "Français"
fallback = "en-US"

[messages]
"app.title" = "Détection de l’inclinaison"
"gui.heading" = "Détection de l’inclinaison"
"gui.button.start" = "Analyser"
"report.header.file" = "Fichier"
"status.trusted" = "Fiable"
"status.needs_review" = "À vérifier"
"gui.progress.files" = "{completed} / {total} fichiers"
```

可以复制英文文件制作完整翻译，也可以仅提供需要覆盖的条目。`name` 使用该语言自身的名称，显示在下拉框。`fallback` 可省略，新增语言默认回退英文；如显式设置，只允许 `zh-CN` 或 `en-US`，且不能与自身相同。

查找顺序为：当前语言外置条目 → 同语言内置条目 → 回退语言。缺少条目不会显示空白。内置语言保持自身的回退规则；新增语言使用其声明的回退规则。已有语言包无需补齐后续版本增加的所有消息。

## 消息键与占位符

`[messages]` 使用带引号的稳定语义键，例如 `"gui.button.start"`；不要把中文原文当作键，也不要去掉包含点号的键外面的引号。

- 翻译整句话，可调整命名占位符的位置。
- 占位符集合必须与内置英文相同，例如 `{completed}`、`{total}`；不可改名或遗漏。
- `{{`、`}}` 表示字面花括号；普通换行可写作 `\n`，也可使用 TOML 多行字符串。
- 文件只承载文字，不运行代码、不计算表达式。

程序检查 TOML 格式、版本、元数据、字符串类型、重复键、占位符及语言标识。未知消息键会提示并忽略。自动发现的损坏文件会提示并跳过；自动发现的非字符串或占位符错误条目会提示并回退，文件中其他有效翻译继续使用。显式指定文件中格式、版本、类型或占位符错误会使加载失败。CLI 提示写入 stderr，`--json` 的 stdout 保持纯 JSON；GUI 会显示语言加载提示。

可用下面的命令验证外置文件能否加载：

```powershell
scan-angle.exe --lang-file ".\locales\fr-FR.toml" --help
```

界面的字形显示还需要字体支持。当前程序沿用系统中文字体回退；新增其他文字系统时，请检查运行机器是否有对应字形。

## 结果兼容性

界面语言只影响 GUI、终端表格、帮助及程序自身的错误提示。CSV 表头和状态/说明、JSON 字段和状态/说明、预览文件名、`angles.csv`、`details.json` 及默认 `_角度结果` 目录后缀沿用原有约定，不随界面语言变化。第三方解码器和操作系统的原始错误详情保留其原始语言。

检测逻辑使用 `Status`、`DetectionReason` 枚举，显示时才查找翻译。导出边界保留原来的中文序列化值，不能通过编辑语言包改变数据协议。

## 开发验证

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo test --locked --no-default-features
```

测试覆盖内置资源键和占位符一致性、第三语言与局部覆盖、无效文件回退、独立工作目录、帮助及参数错误、GUI 已有结果重新渲染、偏好读写，以及中英文运行时导出内容一致。
