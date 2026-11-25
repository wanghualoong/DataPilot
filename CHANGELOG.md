# 更新日志 / Changelog

由 `Resources/about/changelog.json` 生成（`python3 -I scripts/gen_changelog.py`），App 内「设置 → 关于 → 更新日志」显示同一内容。

## 0.1.11 · 2026-11-25

开源版不再预设任何单位的模型网关、文档识别服务器和本地模型同步网关（功能都保留，可在设置里填写）；单位可以用本地默认值文件预设这些地址。

The open-source build no longer presets any organisation's model gateway, OCR server or local-model sync gateway (the features stay and can be set in Settings); an organisation can preset them with a local defaults file.

### 改进 / Improved

- 内置默认值改为 Resources/defaults.json（中性）加可选的 defaults.local.json（单位预设：网关预设排在最前、文档识别服务器、同步网关）；配置向导和「登录并选择 Key」从中读取预设和登录提示 / Built-in defaults now come from Resources/defaults.json (neutral) plus an optional defaults.local.json (organisation presets listed first, OCR server, sync gateways); the setup wizard and Log in & choose key read presets and login hints from it
- 没有配置文档识别服务器时，读取扫描件会提示到 设置 → 通用 → 文档识别 填写服务器地址；设置页显示提示 / Without an OCR server, reading a scanned file asks you to set one in Settings → General → Document parsing; the settings page says so too
- 本地模型同步的网关默认为空（不同步），可在 设置 → 通用 中添加 / Local-model sync gateways default to none; add them in Settings → General
- 代码、测试、文档和更新日志中不再出现单位专用的名称和地址；不再读取旧版钥匙串服务名 / No organisation-specific names or addresses in code, tests, docs or changelog; the legacy keychain service name is no longer read
- 打包：DP_PUBLIC=1 打对外发布版（不带本地默认值文件，并检查包内不含其中的地址） / Build: DP_PUBLIC=1 makes the public build (without the local defaults file, checked for its addresses)

## 0.1.10 · 2026-11-24

browser 工具下载的文件同时复制到系统「下载」文件夹；不支持看图的模型不再浪费轮数截图；「最大轮数」默认调到 120，到上限时说明怎样继续和在哪里调高；修复 ClickHouse / Oracle 连接，数据库连接弹窗更快。

Browser downloads are also copied to the system Downloads folder; models without image input no longer waste steps on screenshots; max turns defaults to 120 and the step-limit notice explains how to go on and where to raise the limit; ClickHouse / Oracle connection fixes and a faster database connection dialog.

### 问题修复 / Fixed

- ClickHouse：所有连接都报「got multiple values for keyword argument 'settings'」连不上；账号本身只读时报「Setting readonly is readonly」。现在逐项设置只读和超时，账号已只读时沿用；会话无法设为只读时拒绝连接（已用官方公开测试库实测） / ClickHouse: every connection failed ("multiple values for keyword argument 'settings'") and read-only accounts failed with "Setting readonly is readonly"; read-only / timeout are now set one by one, already read-only accounts work, sessions that cannot be made read-only are refused (tested on the public playground)
- Oracle：列出表的查询里同一个绑定参数用了两次却只传一个值，测试连接和读取表结构都会失败；改为按名字绑定 / Oracle: the table-list query used one positional bind twice with a single value, so testing a connection and reading the schema failed; named binds now

### 改进 / Improved

- 看板卡片的显示开关（图例 / 标签 / 网格线 / 折线点 / 坐标轴）和 Excel 导出的单元格处理改为 DataPilot 自己的实现（效果不变）；Chat2DB 在「关于」页改列为设计思路参考，不再附带其许可文件 / Dashboard card display switches and Excel export cell handling are now DataPilot's own implementation (same results); Chat2DB is listed under design ideas in About, its license file is no longer bundled
- 数据库连接：点「+」立刻弹出（类型列表和连接列表不再等数据内核启动）；保存新端口的连接后弹窗立即关闭，内核在后台重启放行端口；保存时按钮显示「保存中…」 / Database connections: the + button opens at once (type and connection lists no longer wait for the data kernel); saving a connection on a new port closes the dialog at once while the kernel restarts in the background; the Save button shows Saving…
- 「最大轮数」默认从 60 调到 120（浏览器长流程等任务不再中途停下）；已在设置里手动改过的值保持不变 / Max turns now defaults to 120 instead of 60 (long browser flows no longer stop midway); a value set in Settings is kept
- browser 工具下载的文件除了存到对话工作区的 downloads/，还会复制一份到系统「下载」文件夹（重名时自动加 (1)、(2)），Agent 会告诉你在哪里找到 / Files downloaded by the browser tool are saved to the workspace downloads/ and also copied to the system Downloads folder (renamed (1), (2)… on conflicts); the agent tells you where
- 当前模型不支持看图时，browser 工具不再提供截图动作，避免截了图模型却看不到、白白耗掉轮数 / With a model that cannot read images, the browser tool no longer offers screenshots, so no steps are wasted on images the model cannot see
- 达到「最大轮数」时提示写明已执行多少步、回复「继续」可接着做，以及可在 设置 → 通用 → 最大轮数 调高（最高 300）；后台通知同样提示回复「继续」 / The step-limit notice says how many steps ran, that replying "continue" goes on, and that Settings → General → Max turns raises it (up to 300); the background notification says the same

## 0.1.9 · 2026-11-23

新增诊断日志（慢或失败的任务有据可查、可导出诊断包）和数据备份 / 恢复（导出 zip、在另一台电脑合并或替换恢复、每周自动备份）。

Diagnostic log (slow or failed tasks are recorded; export a diagnostics zip) and backup / restore (export a zip, merge or replace on another computer, weekly automatic backup).

### 新功能 / Added

- 诊断日志：每次任务、模型调用（首字耗时、总耗时、token、状态）、工具调用、数据内核崩溃 / 重启、SSH 隧道、文档识别、数据库抽取都记一行；超过 3 分钟的任务、30 秒还没出字、2 分钟的工具标为慢。只记耗时、状态和错误信息，不记对话内容、数据或密钥；单个文件 5 MB 轮换，最多 5 个，保留 14 天 / Diagnostic log: turns, model calls (time to first token, total, tokens, status), tools, kernel crashes / restarts, SSH tunnels, OCR and database extracts, with slow tags. Timings, status and errors only — no chat text, data or secrets; 5 MB × 5 files, 14 days
- 设置 → 通用：「打开日志文件夹」「导出诊断包」（日志 + 版本 / 自检结果）；/doctor 显示最近 10 次慢或失败的任务 / Settings → General: open log folder, export diagnostics zip (logs + version / self-check); /doctor lists the last 10 slow or failed tasks
- 备份与恢复（设置 → 通用）：导出对话、记忆、技能、DATAPILOT.md、设置和所选项目的 .datapilot/（可选含项目原始文件）为一个 zip，带清单和逐文件 SHA-256 校验；API Key 和密码不进备份，恢复后列出需要重新填写的项 / Backup & restore (Settings → General): chats, memory, skills, DATAPILOT.md, settings and the chosen projects' .datapilot/ (optionally project files) in one zip with a manifest and per-file SHA-256; keys and passwords never included — the restore lists what to enter again
- 恢复前先校验并预览；默认合并（只添加本机没有的对话，记忆以较新的为准，本机设置不变），也可替换（先自动备份当前数据）；项目文件夹不存在时可选择新位置（需先信任该文件夹）；防止恶意路径的安全解压 / Restore verifies and previews first; merge by default (missing chats added, newer memory wins, local settings kept) or replace (current data backed up first); missing project folders can be relinked after trusting the new folder; safe unzip
- 每周自动备份到 backups/（保留最近 4 份，不含项目数据），可在设置中关闭 / Weekly automatic backup to backups/ (keeps 4, no project data); can be turned off

### 问题修复 / Fixed

- browser 工具：自身宽高为 0、文字靠浮动子元素显示的链接（如 Oracle 下拉框底部的「Search ... F12」）被当成不可见，快照里没有、按文字也点不到；现在按其中有尺寸的子元素判断可见并在那里点击 / browser tool: links that have no box themselves and show their text in floated children (Oracle's "Search ... F12" under a dropdown) were treated as invisible; visibility and the click point now use the first rendered child
- browser 工具：长下拉列表（如上千个项目）把列表后面的「Search ...」入口挤出了页面快照，Agent 找不到；现在每个列表只列前 15 项并注明剩余数量，列表后的入口照常列出。同时补上只有图标的按钮（如 Move）、可聚焦的搜索结果行，名字重复的按钮（Search / OK）附上 id 以区分属于哪个字段；按文字点击只点可见元素（不再误点隐藏的同名按钮）；click 支持 text=按文字点击；press 支持组合键（Control+A、Shift+Tab、F12） / browser tool: long dropdowns (thousands of projects) pushed the "Search ..." entry after them out of the snapshot; lists now show 15 items plus a count, so entries after them stay listed. Icon-only buttons (Move), focusable result rows and ids for repeated names (Search / OK) are included; click-by-text only hits visible elements; click accepts text=; press accepts key combos (Control+A, Shift+Tab, F12)
- computer 工具自动拒绝操作浏览器（Chrome / Safari / Edge 等）：打开浏览器、或浏览器在最前面时的操作都会被拒绝，并提示改用 browser 工具，不会再去操作你日常用的 Chrome / The computer tool now refuses to drive web browsers (Chrome / Safari / Edge …) and points to the browser tool, so it never operates your everyday Chrome
- Windows：DataPilot 不在最前面时任务完成却没有系统通知；现在改用系统的前台窗口判断是否在前面，同时让任务栏图标闪烁提示（通知被专注助手屏蔽时也能看到） / Windows: no system notification when a task finished while DataPilot was in the background; the foreground check now asks the OS, and the taskbar button also flashes (visible even when Focus Assist hides toasts)
- Mac 上用搜狗 / 拼音输入法按回车上屏英文时，会直接把消息发出去；现在输入法组字时的回车只上屏、不发送（问答卡、重命名、权限规则等输入框同样处理） / macOS: pressing Enter to commit English text in Sogou / Pinyin input methods sent the message; Enter during IME composition now only commits the text (also in answer, rename and rule inputs)
- browser 工具：专用 Chrome 窗口顶部不再提示「不受支持的命令行标记 --disable-blink-features=AutomationControlled」；启动时自带的 about:blank 空白标签页直接拿来用，不再多留一个 / browser tool: the DataPilot Chrome window no longer shows the "unsupported command-line flag" warning bar, and the blank tab Chrome opens at start is reused instead of left behind
- browser 工具：在页面新打开的标签页里开始的下载（如 Oracle 报表导出 Excel）存到了系统「下载」文件夹，工具一直等不到；现在下载目录对整个专用 Chrome 生效，所有标签页的下载都进对话工作区 downloads/ / browser tool: downloads started in a tab the page opened (e.g. an Oracle report exported to Excel) went to the system Downloads folder and were never seen; the download folder now applies to the whole DataPilot Chrome, so every tab downloads into the chat's downloads/ folder
- 同一工具用相同参数调用 3 次后插入的提示，被放在了工具调用和它的结果之间，之后每次发消息都报「HTTP 400: insufficient tool messages following tool_calls message」；现在提示放在结果之后，已出错的旧对话发送时也会自动理顺顺序，可以继续使用 / The repeated-call hint was inserted between a tool call and its result, so every later request failed with HTTP 400 (insufficient tool messages); hints now go after the results and existing chats are reordered when sent
- Windows：项目里的 .datapilot 文件夹设为隐藏（与 macOS 一致，也与 Git 的 .git 做法相同），已有项目下次打开时生效；outputs 仍然可见 / Windows: the project's .datapilot folder is now hidden (as on macOS, and like Git's .git); existing projects on next open; outputs stays visible
- Windows：普通 NTFS 文件夹（如从备份恢复或从别处复制来的项目、下载目录）里数据内核报「PermissionError: Access is denied …\artifacts」启动失败；现在每次启动都重新检查沙箱授权，失败时先对整个工作区重新授权并自动重启内核，仍不行则该项目改为不隔离运行（仍在 Job Object 中）；内核在启动阶段退出时立即报错，不再等满 90 秒 / Windows: the data kernel failed with "PermissionError: Access is denied …\artifacts" in normal NTFS folders (restored or copied projects, Downloads); sandbox grants are now checked on every start, a denied start re-grants the whole workspace and restarts automatically, and if that still fails the project runs without the AppContainer (still in the Job Object); a kernel that exits during startup is reported at once instead of after 90 s
- Windows：项目放在 exFAT / FAT32 格式的 U 盘或移动硬盘、网络驱动器或整个磁盘根目录（如 E:\）时，数据内核报「Access is denied: 'E:\\'」启动失败；现在这类位置改为不隔离运行（仍在 Job Object 中、仍限制环境变量），并在日志中记录原因 / Windows: projects on exFAT / FAT32 drives, network drives or a drive root (E:\) made the data kernel fail with "Access is denied"; such locations now run without the AppContainer (still in the Job Object) and the reason is logged
- SQL / 表格结果里的数字不再只显示 4 位有效数字：金额保留小数（3505.57 不再显示成 3506，-70980000 不再显示成 -7.098e+07），对账类分析不会再看不到分位 / Numbers in SQL / table results no longer keep only 4 significant digits: amounts keep their decimals (3505.57 instead of 3506, -70980000 instead of -7.098e+07)
- 模型把参数类型写错（如 "preview": "True"、"limit": "10"）时自动转换，不再直接报错 / Tool arguments with the wrong JSON type ("preview": "True", "limit": "10") are converted instead of failing
- python 输出里不再刷 pandas 的版本提示（Pandas4Warning / FutureWarning），少占上下文、少干扰模型 / pandas deprecation / future notices no longer fill python outputs

### 改进 / Improved

- 窗口启动时完整显示在屏幕内：Windows 默认最大化；Mac 在小屏幕上自动缩到可用区域（不被 Dock / 菜单栏挡住）并居中 / The window starts fully on screen: maximized on Windows; on Mac shrunk to the usable screen area (menu bar and Dock excluded) and centred on small screens
- 右侧「文件」面板：单击文件会高亮选中，面板顶部和悬停提示里说明「双击用默认程序打开」 / Files panel: clicking a file highlights it, and a hint says double-click opens it
- 点击消息的复制按钮后，图标变成对勾并显示「已复制」，1.5 秒后恢复 / The message copy button turns into a check mark with "Copied" for a moment after copying
- browser 工具改为 Codex 方式：打开一个可见的 DataPilot 专用 Chrome 窗口（独立配置，登录状态保留），你能看到每一步、自己登录或接管；新增按键、悬停、标签页切换（自动跟随新打开的页面）、同源 iframe、弹窗处理、真实鼠标点击、下载到工作区 downloads/，以及 wait_for 长时间等待（定时刷新或点「刷新」按钮，等待期间不调用模型，最长 6 小时）；不会在密码框里输入 / browser tool now works the Codex way: a visible DataPilot-only Chrome window (own profile, logins persist) you can watch, log in to or take over; adds key presses, hover, tab switching (follows new tabs), same-origin iframes, dialogs, real mouse clicks, downloads to the workspace downloads/ folder and wait_for for long waits (reload or click a refresh button periodically, no model calls while waiting, up to 6 h); never types into password fields
- Windows 上选择项目文件夹时，对话框里也能看到文件夹中的文件（只能选文件夹，和 Mac 一致） / Windows: the folder picker also lists the files in each folder (only folders can be chosen, like on Mac)
- Mac 与 Windows 统一字体：界面（含设置里的模型名、路径、版本号）统一用系统 UI 字体（苹方 / Segoe UI + 微软雅黑），只有 SQL、JSON、命令等代码用等宽字体（SF Mono / Cascadia Mono 或 Consolas），其中的中文用苹方 / 微软雅黑（Windows 上不再显示成宋体） / Same fonts on Mac and Windows: the system UI font everywhere (including model names, paths and versions in Settings); monospace (SF Mono / Cascadia Mono or Consolas) only for code such as SQL, JSON and commands, with PingFang / Microsoft YaHei for Chinese (no more SimSun on Windows)
- core.log 超过 5 MB 自动轮换，旧文件 14 天后删除 / core.log rotates at 5 MB; the old file is removed after 14 days

## 0.1.8 · 2026-11-23

Python 环境加入 matplotlib，python 工具可以直接画图（中文字体正常显示）。

matplotlib is now bundled: the python tool can draw figures (Chinese labels render correctly).

### 新功能 / Added

- 内置 Python 环境加入 matplotlib（Mac 与 Windows），图会自动保存并显示在工具卡片里；默认使用系统中文字体（Mac：冬青黑体 / 苹方，Windows：微软雅黑） / matplotlib bundled (Mac and Windows); figures are saved and shown in the tool card, using a Chinese system font by default

## 0.1.7 · 2026-11-23

修复中文 Windows 上数据内核一直报「没有运行或刚被重启」的问题；/doctor 会真正试启动内核。

Fixed the data kernel constantly reported as “not running” on Chinese Windows; /doctor now really starts the kernel.

### 问题修复 / Fixed

- 中文 Windows：Python 内核按 GBK 输出，含中文的结果会让核心误判内核已退出，表现为一直「数据内核没有运行或刚被重启」；现在内核固定用 UTF-8 通信，核心遇到异常字节也不会再断开 / Chinese Windows: the kernel wrote GBK, so any reply with Chinese text made the core think the kernel had exited (“kernel not running”); the kernel now always talks UTF-8 and the core no longer drops the connection on bad bytes

### 改进 / Improved

- /doctor 在沙箱里实际启动一次数据内核，失败时直接显示原因；Python 一行的版本显示修正 / /doctor really starts the data kernel in the sandbox and shows why it fails; Python version line fixed
- Windows 沙箱对安装目录的只读授权失败只记一次日志，不再每次启动都刷屏 / Windows: the read-only sandbox grant failure under Program Files is logged once instead of on every start

## 0.1.6 · 2026-11-23

切到其他窗口时，任务完成、出错或需要审批会弹出系统通知；首次配置默认选第一个网关预设；修复 Windows 上选择模型列表的勾选框不对齐。

System notifications when a task finishes, fails or needs approval while DataPilot is in the background; the first gateway preset is the first-run default; model picker checkboxes aligned on Windows.

### 新功能 / Added

- DataPilot 不在前台时，任务完成、出错、需要审批 / 回答 / 确认计划会弹出系统通知（设置 → 通用 可关闭） / While DataPilot is in the background, a system notification appears when a task finishes, fails, or needs your approval / answer / plan review (can be turned off in Settings → General)

### 问题修复 / Fixed

- Windows：「获取并添加模型」的模型列表里勾选框上下不对齐 / Windows: checkboxes in the “fetch & add models” list were not vertically aligned

### 改进 / Improved

- 首次打开的配置向导默认选第一个网关预设 / The first-run wizard now defaults to the first gateway preset

## 0.1.5 · 2026-11-21

新增「数据对比」skill：表格与表格、多个表格、表格与数据库对比并导出差异报告；添加项目前先确认信任文件夹；新对话引导页更新。

New Data Compare skill (tables, several tables, table vs database, Excel diff report); trust confirmation before adding a project folder; refreshed new-chat hint.

### 新功能 / Added

- 新增内置 skill「数据对比 / Data Compare」（/data-compare、/数据对比）：表格与表格、多个表格、表格与已连接数据库对比，列出只在一边的记录、逐列值差异、重复键和空键，大表可按维度对账，并导出 Excel 差异报告 / New built-in skill “Data Compare / 数据对比” (/data-compare): compare two tables, several tables, or a table with a connected database — rows on one side only, value differences per column, duplicate / empty keys, totals by dimension for big tables, plus an Excel diff report
- 新建 / 打开项目时，选好文件夹后先弹出「要信任这个文件夹吗？」，说明会读取文件、创建 .datapilot/ 和 outputs/；点「信任并添加」才添加，选错了可取消 / When creating / opening a project, a “Trust this folder?” dialog explains what DataPilot will read and create; the folder is added only after “Trust and add”, and Cancel undoes a wrong pick
- 新对话的引导页按现有功能重写：数据文件、PDF / Word / 扫描件、数据库连接、项目文件夹，以及清洗、统计、关联对比、图表看板、字段提取、报告导出等示例 / The new-chat hint now covers what DataPilot actually does: data files, PDF / Word / scans, database connections, project folders, with examples for profiling, metrics, matching files, charts and dashboards, field extraction and reports

## 0.1.4 · 2026-11-21

记忆改进：跨项目的个人偏好每次对话都会带上，记住时会提示，助手推断的内容不再当作你说的。

Memory: personal preferences now apply in every chat and project, saving is announced, and the assistant's own guesses are no longer stored as your words.

### 新功能 / Added

- 用户记忆（跨项目偏好）的索引每次对话都加载，并在每轮问题旁重复一遍偏好，例如「金额用万元、结论先行」在新项目里也会照做 / The user-memory index is loaded in every chat and preferences are repeated next to each question, so e.g. units and answer style carry over to new projects
- 记住一条内容时，对话里提示「🧠 已记住：…」，并注明是你说的还是助手推断的 / Saving a memory shows “🧠 Remembered: …” in the chat, saying whether it came from you or was inferred
- 记忆面板：每条记忆显示「已确认 / 待确认」和来源，待确认的可一键确认；用户记忆页可编辑「关于我」长期指令（DATAPILOT.md，所有项目都加载） / Memory panel: confirmed / unconfirmed badge and origin per entry, one-click confirm; edit the “About me” standing instructions (DATAPILOT.md) on the User tab
- 用户记忆也会定期自动整理（合并重复、处理矛盾）；普通对话里输入 /dream 可手动整理 / User memory is consolidated periodically too (duplicates, conflicts); /dream in a plain chat runs it now

### 改进 / Improved

- 只有能在你的原话里找到依据的内容才标为「已确认」；助手自己说的口径、你没反驳的，一律记为待确认，召回时也会标明 / Only entries backed by your own words are marked confirmed; the assistant's statements you did not contradict are stored as unconfirmed and labelled so when recalled
- 普通对话里助手推断的业务口径不再写入用户记忆（避免影响所有项目） / Business rules inferred in plain chats are no longer written to user memory (they would affect every project)
- 每次自动记忆抽取都写入日志（写了几条、跳过原因、失败原因，不含记忆正文） / Every automatic memory extraction is logged (counts, skip and failure reasons, no memory text)

## 0.1.3 · 2026-11-20

扫描版 PDF 和图片可以直接读取和导入表格（文档识别 / OCR），并可逐页对照原件；新增合同字段抽取 skill。

Scanned PDFs and images can be read and their tables imported (document parsing / OCR), with a page-by-page comparison view; new contract-extraction skill.

### 新功能 / Added

- 扫描版 PDF 中没有文字层的页面和图片（png/jpg/webp/tif/bmp）在读取、导入时自动送 MinerU 文档识别服务识别；只发送扫描页，不发送文件名 / Scanned PDF pages without a text layer and images are sent to the MinerU document-parse service when read or imported; only those pages, never the file name
- 默认 advanced 档为主（不编造文字、能读印章）、basic 档并行交叉核对（补回被印章遮住的行）；两档不一致的金额、日期、编号标为「需核对」 / advanced tier as primary (no invented text, reads seals), basic tier in parallel as cross-check (lines under seals); disagreeing amounts / dates / numbers flagged for review
- 工具卡显示哪些页经过识别、几处需核对；「原页对照」逐页并排显示原页图像和识别文字，需核对项高亮（文件面板也可打开） / Tool cards show which pages were recognized and how many items need checking; Compare with page shows the page image next to the recognized text (also from the Files panel)
- 设置 → 通用 →「文档识别（OCR）」：服务器地址、主档 / 核对档、交叉核对开关、可选 API Key（存钥匙串）、测试连接、恢复默认 / Settings → General → Document parsing: server, tiers, cross-check switch, optional API key (keychain), test connection, restore defaults
- 内置 skill「合同字段抽取」：双方、金额、日期、期限、付款、违约、盖章签字等，每项注明页码和核对状态，可汇总为合同台账 / Built-in contract-extraction skill: parties, amounts, dates, term, payment, breach, seals and signatures with page numbers and review status; can build a contract register

### 问题修复 / Fixed

- 设置 → 通用「本地模型同步的网关」点恢复默认后，列表偶尔仍显示旧值 / Sync gateway list occasionally kept the old value after Restore default
- 对话中点 📎 选择文件时只能选数据文件，现在也能选 PDF、Word、PPT 和图片 / The 📎 file picker only allowed data files; PDFs, Word, PowerPoint and images can now be chosen too
- 新上传文件时，Agent 只处理本条消息上传的文件，不再自动把之前上传的文件拉进来对比；对比对象不明确时会先询问 / With new uploads the agent works on the files of that message only and asks when the comparison target is unclear

### 改进 / Improved

- 识别结果按文件内容在本机缓存，同一文件再次读取不重复识别；服务不可用时明确提示原因（如需连 VPN），不会返回空结果 / Results cached locally by file content; when the service is unavailable the reason is stated (e.g. VPN needed) instead of an empty result
- 「仅本地模型」模式下只使用本机 / 内网地址的识别服务 / Local-only mode uses the document-parse service only on localhost / private addresses
- 原页对照：翻页按钮加大并带文字；新增「下载识别结果（docx）」，全部页面的识别文字（含表格和需核对项）导出为 Word / Page comparison: larger labelled page buttons; Download OCR (docx) exports all pages with tables and review items
- 原页对照：左侧原页可左转 / 右转 90° 或还原（扫描方向不正的页面），每页单独记住 / Page comparison: rotate the original page left / right 90° or reset, per page

## 0.1.2 · 2026-11-20

同步网关的本地模型每周自动同步；许可与第三方声明在应用内查看。

Weekly sync of gateway local models; licenses and third-party notices shown in-app.

### 新功能 / Added

- 同步网关的 local 模型每周自动同步：新上线的自动添加并标为「本地」，下线的自动删除；正在使用的模型下线时自动切换到剩下的本地模型并提示 / Weekly sync of gateway *local models: new ones added and tagged local, removed ones dropped; a role using a removed model switches to a remaining local model
- 设置 → 模型服务（同步网关）：「每周自动同步本地模型」开关、「立即检查」按钮和上次检查结果 / Settings → Providers (sync gateway): weekly-sync switch, Check now, and the last check result
- 设置 → 通用 →「本地模型同步的网关」：可配置多个网关（域名、IP、可带端口，*.域名 表示子域名），按主机名精确比对；默认网关来自内置默认值 / Settings → General → sync gateways: several hosts (names, IPs, optional port, *.domain), exact host match; default from the built-in defaults

### 问题修复 / Fixed

- 关于页「查看完整第三方声明」点击没反应（.md 默认程序是 IDE 时在后台打开） / About → View Third-Party Notices did nothing when .md files open in an IDE in the background

### 改进 / Improved

- 许可全文和第三方声明改为在应用内弹窗显示，可「在访达 / 资源管理器中显示」 / License texts and notices open in an in-app viewer, with Show in Finder / Explorer

## 0.1.1 · 2026-11-19

上下文压缩更稳定，自动识别模型的上下文上限，长工具参数不再断连。

More reliable context compaction, automatic context-window detection, no more cut-offs on long tool arguments.

### 新功能 / Added

- 内置模型表（约 1500 个常见 chat 模型），自动识别上下文上限；设置 → 模型角色 显示上限及来源 / Built-in model table (~1,500 chat models) for automatic context windows; Settings → Model roles shows the window and its source
- Agent 运行中也能点压缩：排队到当前这一步完成后执行 / Compact while a turn runs: queued until the current step ends
- 右侧检查器默认打开 / Inspector open by default
- 关于页新增「更新日志」，升级后首次打开自动显示 / Changelog in About, shown once after an upgrade

### 问题修复 / Fixed

- 压缩时网关报 HTTP 504、提示 a turn is running / Compaction failing with HTTP 504 / “a turn is running”
- glm 等始终思考的模型压缩时报 empty summary；半截草稿被当作摘要保存 / “empty summary” on always-thinking models (glm); half-written drafts saved as the summary
- 上下文窗口很大时点压缩没有任何反应 / Manual compaction doing nothing on large-window models
- 压缩过程中发送的消息没有被回答 / Messages sent during compaction were dropped
- 模型生成很长的工具参数时报 network error: Error while decoding chunks（自动重试，必要时改用 Hermes 工具调用） / “Error while decoding chunks” on long tool arguments (retries; switches to Hermes tool calls if needed)
- 压缩后上下文占用条不立即刷新 / Context meter not refreshing after compaction

### 改进 / Improved

- 网关错误显示可读原因（如“网关超时”），不再是空的 HTTP 504 / Readable gateway errors (e.g. “gateway timeout”) instead of a bare HTTP 504
- 摘要、笔记、标题等一次性请求改为流式，不再被网关 60 秒时限切断 / One-shot calls (summary, notes, titles) are streamed, so gateway timeouts no longer cut them
- 压缩失败时用会话笔记兜底 / Session notes as a fallback when the summary call fails
- 流式空闲超时 90 秒 → 300 秒 / Stream idle timeout 90 s → 300 s

## 0.1.0 · 2026-11-17

首个 Tauri 版：Mac 与 Windows 共用一套界面，核心全部改为 Rust。

First Tauri release: one UI for macOS and Windows on an all-Rust core.

### 新功能 / Added

- Tauri 统一界面，Mac 与 Windows 安装包（Windows 自带 Python，无需另装） / Unified Tauri UI with macOS and Windows installers (Windows bundles Python)
- 深色 / 浅色外观，可跟随系统 / Dark / light appearance, follows the system
- 新斜杠命令：/context /fork /review /doctor /remember /copy /permissions /settings /todos / New slash commands: /context /fork /review /doctor /remember /copy /permissions /settings /todos
- 看板配色：14 套模板 + 自定义配色 / Dashboard colours: 14 templates plus custom palettes
- 看板整体导出 PNG / Whole-dashboard PNG export
- 参照 Codex 的压缩：保留用户原话、超长时丢弃最早消息重试、可设压缩上限与摘要模型 / Codex-style compaction: keeps your own words, drops oldest messages on overflow, configurable limit and summary model

### 问题修复 / Fixed

- 上下文到 91% 仍未自动压缩 / Auto-compaction not triggering at 91%
- 「在访达中显示」打开的不是对话文件夹 / “Show in Finder” opening the wrong folder
- 保存的 skill 在 /skills 中找不到 / Saved skills missing from /skills
- /skills 列表用方向键选择时不跟随滚动 / /skills list not scrolling with the arrow keys
