# 05 · 沙箱与会话恢复

## Seatbelt 沙箱
- 调用：`/usr/bin/sandbox-exec -p <policy> -DWORKSPACE=… -DTMPDIR=… -DPYROOT=… -DRESROOT=… -- <cmd>`（绝对路径，不经 PATH）。
- 策略 `Resources/sandbox/base.sbpl`（改编自 openai/codex，Apache-2.0）：`(deny default)`；系统路径、内置 Python、Resources 只读；仅 `WORKSPACE` 与会话 `tmp/` 可写；禁止删除/替换工作区根；禁止 XPC 与写绕过 fcntl。
- 网络：默认关闭；输入框“联网”开关打开后追加 `network.sbpl` 并重启内核。
- **路径必须是 realpath**：macOS 的 `/var`、`/tmp` 是到 `/private/...` 的符号链接，而 Foundation 的 `resolvingSymlinksInPath()` 会去掉 `/private`，导致规则不匹配、内核无法启动。统一用 `URL.realPathURL`（realpath(3)）。
- App 不启用 App Sandbox（被沙箱的进程无法再套 sandbox-exec），保留 Hardened Runtime。
- 实测：工作区可写；写家目录、`/Users/Shared` 被拒；无网络时 urlopen 失败；进程启动约 0.4–0.6s。

## Windows：AppContainer + Job Object（M3）
- 实现在 `datapilot-core/src/sandbox/appcontainer.rs`，与 Seatbelt 共用 `sandbox::{Policy, spawn, shell}` 接口（内核与 bash 工具都走它）。
- 文件：容器 SID 在工作区 / tmp / outputs 上有完全控制，在项目目录 / Python / Resources 上只读执行；其余位置按系统默认（AppContainer 读不到用户目录）。
- 网络：默认没有网络能力；联网开关或数据库连接 → `internetClient` + `privateNetworkClientServer`。回环需安装时豁免（`--loopback-exempt`）。
- 进程：CREATE_SUSPENDED → 放进 KILL_ON_JOB_CLOSE Job → 恢复；只继承三个管道句柄；停止 = TerminateJobObject（整棵进程树）。
- bash 工具在 Windows 上是 PowerShell（`-EncodedCommand`，UTF-8 输出），系统提示会告诉模型。

## 持久内核
`KernelClient`（actor）↔ `Resources/kernel/kernel.py`，JSON-lines 协议：
`exec / import / profile / schema / relations / sql / query_data / preview / tables / vars / checkpoint / drop_tables / ping / shutdown`，以及主线程直接应答的控制请求 `cancel`（取消一个请求或一个对话的请求）、`running`（列出排队 / 执行中的请求）。
- 懒启动：第一次需要时启动，启动是单飞的（并发调用共用一次启动），等待就绪最多 90 s；空闲 15 分钟（可配）回收。
- **并发**：每个请求一个线程、一个 DuckDB 游标；同一对话的 exec/import 等按提交顺序执行，`exec` 跨对话全局串行。详见 `12-projects.md`「内核内的并发」和 ADR-0011。
- 超时（默认 300s，python 600s）或用户停止 → 只取消该请求（中断 DuckDB 查询 + 向线程抛异常），内核不重启；10 s 内停不下来才重启内核，下次调用透明恢复。
- App 读取内核 stdout 用 `readabilityHandler` 事件按行切分（线性扫描，只扫新到的字节），不用 `FileHandle.bytes.lines`（会占住协作线程，多个内核时线程池耗尽）。
- `query_data` 加 `to_file: true` 时把结果行写到 `tmp/qd_*.json`，App 原样拼进看板脚本，不在 Swift 里解析（见 `07-bi-dashboard-spec.md`「加载性能」）。
- DuckDB 线程数 `min(8, CPU 核数)`。
- 内核命名空间预置 `pd`、`duckdb`、`con`、`sql(q)`、`WORKSPACE`；小表同时作为同名 DataFrame。

## 历史保留（每会话工作区）
| 文件 | 作用 |
|---|---|
| `uploads/` | 原始上传文件 |
| `data.duckdb` | 导入的表永久保存，恢复无需重导 |
| `checkpoints/*.parquet` | 每次 exec 后自动保存变化的 DataFrame（≤200MB） |
| `cells.jsonl` | 执行过的代码（审计 / 必要时重放） |
| `artifacts/` | 查询结果 parquet、图片 |
| `dashboards/*.json` | 看板规格（数据实时重查） |
| `notes.md` / `catalog.json` | 会话笔记 / 语义层 |
| `../transcript.jsonl` | 会话记录 |
| `../snapshots/<turn>/` | 每轮开始前的工作区快照（APFS clone） |

## 快速恢复流程
1. 打开会话：从 `transcript.jsonl` 渲染历史（不启动内核，毫秒级）；上下文 = 最近 `compacted` 摘要 + 保留尾部 + 之后的消息。
2. 第一次发消息/打开数据面板：内核启动（~0.4s）→ 连接 `data.duckdb` → 从 checkpoints 恢复 DataFrame。
3. 注入“工作区状态”（表与行数、Python 变量），模型直接接着用。
测试：`tests/python/test_kernel.py` 验证重启后表与 `top` DataFrame 可用。

## 快照与回退
每个用户回合开始前用 `cp -c`（APFS clonefile，几乎零拷贝）快照工作区到 `snapshots/<turnId>`，保留最近 20 个。聊天中用户消息右键“回退到此处之前” → 恢复工作区 + 截断上下文（写入 `rewind` 记录）。


## 项目模式
项目会话共享一个内核进程和 `.datapilot/data/data.duckdb`；Python 变量按会话 namespace 隔离，检查点在 `checkpoints/<会话>/`。项目会话不做 APFS 工作区快照，回退只回退上下文。详见 `12-projects.md`。
