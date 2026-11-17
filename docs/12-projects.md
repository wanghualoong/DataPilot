# 12 项目（Projects）

参考 Codex 的“项目”概念：一个项目 = 用户电脑上的一个文件夹。对话、数据库、看板和产出都放在这个文件夹里，便于整体备份、迁移和共享。

## 目录布局
```
<项目文件夹>/
  DATAPILOT.md            # 项目说明（/init 生成，可手改）；每轮自动注入上下文（≤6000 字符）
  outputs/                # Agent 的产出（报告、导出、CSV），可写
  <你的原始文件…>          # 只读：沙箱内只能读，不能改/删
  .datapilot/
    project.json          # {id, name, version}
    .gitignore            # 默认忽略整个 .datapilot
    data/                 # 共享数据工作区：data.duckdb、artifacts/、dashboards/、catalog.json、checkpoints/<会话>/、
                          #   relations.json、derived.json、connections.json（数据库连接，无密码）、queries.json（我的查询）
    sessions/<id>/        # 每个会话：transcript.jsonl、session.json、notes.md
```
不属于任何项目的对话（侧栏“未归档对话”）仍在 `~/Library/Application Support/DataPilot/sessions/<id>/`。

## 沙箱与权限
| 路径 | 内核 / bash 权限 |
|---|---|
| 项目文件夹（原始文件） | 只读（READ_ROOT） |
| `outputs/` | 读写（WRITE_ROOT） |
| `.datapilot/data/` | 读写（WORKSPACE） |
| 其他位置 | 拒绝；默认断网 |

- `data_import` 直接读取原始文件（相对项目根的路径，如 `data/sales.xlsx`），**不复制**。
- 拖入项目内的文件只引用相对路径；拖入项目外的文件复制到 `.datapilot/data/uploads/`。
- `write_file` 在项目模式下写入 `outputs/`；`DATAPILOT.md` 例外，写在项目根。
- Python 内核里可用 `PROJECT_ROOT`（只读）和 `OUTPUT_DIR`（outputs/）。

## 一个项目一个内核
- 核心为每个项目维护一个沙箱进程（`kernel::KernelProcess`）；同项目的会话共用这一个 DuckDB。
- 每个会话通过 `KernelClient(process:namespace:)` 访问，请求自动带上 `ns=<会话 id>`。内核为每个 namespace 保持独立的 Python 变量，DataFrame 检查点也按会话分开（`checkpoints/<会话>/`），所以会话之间的变量互不干扰，但导入的表是共享的。
- 空闲回收：项目中没有会话在运行时，按设置的空闲时间回收项目内核；退出 App 时全部关闭。

## 项目内各对话的共享范围
| 内容 | 范围 | 说明 |
|---|---|---|
| 数据（DuckDB 表） | 项目内共享 | 一个项目一个数据库；数据页顶部提示「项目内共享，N 个对话可见」 |
| Python 变量 / DataFrame | 每个对话独立 | 内核按 namespace 隔离，检查点在 `checkpoints/<会话>/` |
| 看板 | 每个对话自己的 + 项目共享 | 「本会话看板」id 为 `db_s_<会话id>`；`dashboard_create` 记录 `owner`。看板页默认显示本对话的看板和无 owner 的旧看板（标「项目共享」，含旧的 `db_session`）；点「全部」显示其他对话的看板 |
| 文件页（项目文件夹、outputs/） | 项目内共享 | 项目即文件夹 |
| 任务（todo）、会话笔记 notes.md、对话记录 | 每个对话独立 | 存在 `sessions/<id>/` |
| 长期记忆（app.sqlite） | 全局 | 按数据集指纹关联，跨项目复用；`DATAPILOT.md` 是项目级 |
| 图表产物 artifacts/、语义层 catalog.json | 项目内共享 | |

## 内核进程的生命周期（防止 DuckDB 锁冲突）
DuckDB 同一时间只允许一个进程打开数据库文件。`kernel::KernelProcess`（`datapilot-core/src/kernel.rs`，Swift 版已删除）保证同一个工作区只有一个内核进程。下面几条中提到的 Swift 细节（GCD、协作线程池）是 Swift 时期踩过的坑，Rust 版用独立线程读管道，不存在这些问题：
- 每次启动有一个编号（generation）；旧进程迟到的退出回调 / 输出不会清空新进程的状态（以前会“遗忘”仍在运行的进程，导致新进程报 `Could not set lock on file … Conflicting lock`）。
- `restart()` / `shutdown()`（切换联网、停止、超时、空闲回收时调用）等旧进程真正退出：正常关闭最多约 1.5 s，再 SIGTERM，约 3 s 后 SIGKILL。
- 兜底：启动遇到锁冲突时，从报错中取出占锁 PID；仅当它是本 App 启动的内置 python `kernel.py` 子进程时结束它并重试一次。
- 检查器数据页 / 看板页遇到内核错误时显示简短说明和「重启内核」「复制详情」按钮。
- 启动是**单飞**的：检查器刷新和 agent 回合同时触发首次启动时，共用同一次启动，不会再启动第二个进程。以前第二次启动会覆盖等待"就绪"的回调，第一个调用方永远挂起，表现为对话一直转圈、用户消息没写入记录、DuckDB 面板也一直转。等待就绪超过 90 秒会报错并结束该进程，不会无限等待。
- 内核 stdout 用 `readabilityHandler`（GCD 事件）按行读取，**不用** `FileHandle.bytes.lines`。后者在 Swift 协作线程池上做阻塞读：每个空闲内核占住一个池线程，同时打开 3–4 个项目内核后线程池被占满，新内核其实已启动并发出了 `_ready`，App 却收不到，90 秒后报 "kernel did not become ready"（曾导致同时开两个项目导入 CSV 全部失败、内核反复重启）。MCP 服务器的读取同样改了。测试：`idleKernelsDoNotBlockNewKernel`。
- 内核 stderr 管道在进程退出后会不断返回空数据，读回调必须在 EOF 时注销，否则每个退出过的内核都会占满一个 CPU 核（曾导致 App 占用约 780% CPU、界面卡死）。
- 测试：`kernelRestartNeverOrphans`、`kernelLockHolderParsing`、`kernelConcurrentStartSingleFlight`。

## 内核内的并发（多个对话共用一个项目内核）
一个项目只有一个内核进程（DuckDB 只允许一个进程写库），但进程内部**并发**处理请求，详见 ADR-0011。

**执行规则**
| 请求 | 同一对话内 | 不同对话之间 |
|---|---|---|
| 查表、预览、`sql`、`query_data`、画像、关系推断（检查器 / 看板刷新也走这些） | 并发 | 并发 |
| 导入 `import`、`checkpoint`、`vars` | 按提交顺序串行 | 并发 |
| 执行 Python `exec` | 按提交顺序串行 | 串行（全局一把锁：matplotlib 等绘图状态是进程级的） |

**实现（`Resources/kernel/kernel.py`、`dataagent/engine.py`）**
- 主线程只读请求；每个请求在自己的线程里执行，并使用**自己的 DuckDB 游标**（同一个连接派生）。
  - 一个游标在跑长查询时，其他游标照常查询（实测 0.001 s 返回）。
  - 两个游标可以同时写不同的表。
  - 临时注册的 DataFrame（导入用的 `_dp_import`）只对本游标可见，所以两个对话同时导入不会撞名。
- 每个对话有一个专属游标，用户代码里的 `con` / `sql()` 用的就是它。
- "当前对话"和"当前游标"是线程局部变量（`Engine.bind`），不再是全局 `ns_name`。
- `print` 输出按线程收集：A 请求的输出不会混进 B 的结果；用户代码自己起的线程输出到 stderr，不会污染协议流。
- 同一对话的有序请求用每对话一个 FIFO 队列保证顺序：入队发生在主线程读到请求时，而不是工作线程开始时。

**控制请求**（主线程立即应答，不排队）
- `{"op":"cancel","target":"<id>"}` / `{"op":"cancel","ns":"<对话>"}`：
  - 排队中的请求直接退出，返回 `cancelled` + `state:"queued"`；
  - 正在执行的请求：先中断它的 DuckDB 查询（`cursor.interrupt()`，0.5 s 内生效），再向该线程抛 `Cancelled` 异常（纯 Python 循环下一条字节码就停）。
- `{"op":"running"}`：列出排队 / 执行中的请求（对话、操作、摘要、已用时）。

**核心侧（`kernel.rs`，原 Swift `KernelProcess` 的行为）**
- **超时**：先发 `cancel` 只停这一个请求，内核不重启。
  - 排队中被取消 → `KernelError.busy`（"本对话的上一步还在执行"）；
  - 执行中被取消 → `KernelError.timeout(restarted: false)`。
- **停止按钮**（`turn/cancel` → `KernelClient::interrupt`）：只取消**本对话**的请求，同项目其他对话的导入 / 查询照常进行。
- **宽限期**：取消后 `cancelGrace`（10 s）内仍未停止（例如卡在 pandas 读大文件这类 C 代码里），才退回重启内核，并返回 `timeout(restarted: true)`。表和已检查点的 DataFrame 会保留。
- **其他接口**：`isBusy(ns:)` 只看本对话；`runningTasks()` 供检查器显示。

**检查器**
- 数据页 / 看板页不再因"内核忙"跳过刷新：刷新是并发执行的，直接返回。
- 数据页每 1.5 s 查询一次 `running`，列出运行超过 1.5 s 的任务，例如：
  - 「本对话 正在导入 · Sales.csv（32s）」
  - 「「对话 X」 正在执行代码 …」
- 每个任务带「停止」按钮，只停这一个任务，不重启内核。

**测试**
- `tests/python/test_concurrency.py`（13 项）：
  - A 跑慢查询时，B 查表和检查器 SQL 在 1 s 内返回；
  - `running()` 能列出 A；
  - 取消排队中 / 执行中 SQL / 纯 Python 循环；
  - 取消后 A 仍可用；
  - 同一对话保持顺序；
  - 输出隔离；
  - 跨对话 `exec` 不重叠；
  - 命名空间隔离；
  - 两个对话并行导入。
- Swift：
  - `queuedTimeoutDoesNotKillLongImport`：A 执行时 B 立即返回，A 的第二步排队超时为 `.busy`，进程号不变；
  - `kernelCancelIsPerRequest`：超时只取消该请求、不重启；A 按停止时 B 的查询照常完成，进程号不变。

## 源文件变化检测（数据源同步）
每个项目对话的回合开始时（`source_sync.rs`，在注入上下文之前）检查项目文件夹里的数据文件。只读文件属性，不读内容，开销是毫秒级。
- **记录**：内核每次从项目文件夹导入时写 `.datapilot/data/sources.json`，内容是文件相对路径 → 表名、修改时间、大小、导入参数（table / sheet / header_row）、mode。`drop_tables` 会同步更新它。
- **基线**：`source_state.json` 记录"已见过的文件"和"用户选择保留表的已删除文件"。第一次检查时，当前所有文件都作为基线，不会把已有文件当成新增。
- **规则**（用户决定）：
  | 变化 | 处理 |
  |---|---|
  | 已导入的同名文件修改时间或大小变了 | **自动重新导入替换**：同表名、同 sheet / 标题行参数，聊天里显示导入卡片 |
  | 以追加（append）方式导入的文件变了 | 不自动处理，提示模型向用户确认 |
  | 新增数据文件 | 弹卡片询问「导入新文件 / 暂不导入」。暂不导入的文件以后不再询问 |
  | 已导入的文件被删除 | 弹卡片询问「删除对应的表 / 保留这些表」。保留则以后不再询问 |
- **结果告知模型**：处理结果以一条 meta 消息（`[数据源同步]`）放在用户消息之前，模型据此知道哪些表已刷新、哪些已删除。
- **约束**：同一项目同时只运行一次同步（两个对话同时开始回合时，只有一个执行）。如果用户在询问卡片打开时按了停止，不记为已处理，下一轮会重新询问。`outputs/` 里的文件不参与检测。
- **测试**：`projectSourceSync`，覆盖修改后自动重导、行数更新、新增后导入、删除后删表、已处理后无待办，以及"暂不导入"之后不再询问。

## 删除项目数据
侧栏项目右键「删除项目数据（不保留，可重新开始）…」→ 确认框列出将移到废纸篓的内容：`.datapilot/`、`outputs/`、`DATAPILOT.md`（`AppModel.generatedItems`）。执行顺序：关闭该项目所有对话的内核 → 从 `AgentServices` 注销并关闭项目内核（释放 DuckDB 文件锁）→ 从索引移除 → `trashItem`。用户原始文件不动。「从侧栏移除」同样会注销项目内核，避免残留进程占锁。

## 回退的差异
项目会话**不做工作区快照**（数据库是共享的，回退会影响其它会话）。“回退到此消息之前”只回退对话上下文，界面会提示：已导入的表和 outputs/ 文件不会回退。

## 侧栏与操作
- 侧栏底部“新建项目（选择文件夹）”，或工具栏“新建”菜单。选择已有 `.datapilot/` 的文件夹时会恢复其中的会话（换电脑也可以接着用）。
- 项目标题右侧的 “+” 新建会话；在项目会话中按 ⌘N，新会话默认归入同一项目。
- 右键项目：在 Finder 中显示、重新定位文件夹（文件夹被移动后会显示“文件夹不存在”）、从侧栏移除（只移除索引，文件保留）。
- 检查器“文件”页显示项目文件树：数据文件 / outputs/ / 其他；点 🔍 即“在对话中分析”，也可以拖进输入框。

## 记忆
- `DATAPILOT.md` 是项目级的说明，相当于 Codex 的 AGENTS.md。
- 结构化长期记忆（偏好、口径、Q-SQL）仍在 `app.sqlite`，按数据集指纹关联，跨项目复用。

## 实现位置
`datapilot-core/src/ui.rs`（项目列表、打开 / 重新关联 / 移除 / 删除数据，沿用 SwiftUI 时期的 app.sqlite 表结构，旧数据直接可用）、`kernel.rs`（KernelProcess + 会话句柄）、`sandbox/`（写入根 outputs/）、`source_sync.rs`、`agent.rs`（project_block 注入）、`Resources/kernel/dataagent/engine.py`（namespace、resolve）、Tauri 界面 `apps/desktop/src/components/Sidebar.tsx`、`inspector/`（文件页）。
