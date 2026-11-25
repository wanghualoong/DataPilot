# 19 · 数据库直连（只读分析）

连接表单字段的组织参考了 Chat2DB 的思路（未使用其代码）；元数据查询按各数据库官方文档的系统视图编写；只读执行策略为自行设计。Java 服务层和 JDBC 驱动没有引入，改用随 App 内置的纯 Python 驱动。

## 支持的数据库
| 类型 | 驱动 | 备注 |
|---|---|---|
| PostgreSQL | pg8000 | 金仓 KingbaseES、openGauss 等 PG 协议数据库也可用 |
| MySQL / MariaDB | PyMySQL | TiDB、Doris、StarRocks 等 MySQL 协议数据库也可用 |
| SQL Server | pymssql | |
| Oracle | python-oracledb（thin 模式，不需要 Oracle 客户端） | 「数据库」一栏填服务名 |
| ClickHouse | clickhouse-connect（HTTP） | 会话设置为 `readonly=2` |
| SQLite 文件 | sqlite3，以只读 URI 打开 | 文件须放在项目文件夹内 |
| DuckDB 文件 | duckdb，`read_only=True` | 同上 |

达梦（需要厂商驱动）留作后续。SSH 隧道见下文「SSH 隧道」。

## 两种用法
1. **抽取快照（推荐）**：`db_extract` 把选中的表或一条只读查询拉成本地 DuckDB 表。
   - 保留明细，不聚合；数据按批流式写入，进度显示在「内核任务」。
   - 两张抽取表之间的数据库外键自动登记为**已确认关系**（`source=db`）。
   - 抽取清单保存在 `connections.json`，`db_refresh` 按清单重拉，然后重建派生表、复核关系。
   - 抽取出的表名如果和本地已有的非抽取表重名，会拒绝执行，不会覆盖。
2. **实时查询**：`db_query` 在远端执行只读 SQL。
   - 默认最多 1000 行（上限 10 万）、超时 60 秒。完整结果保存为 `result_id`，可以导出或画图。
   - `dialect:"duckdb"` 表示 SQL 按 DuckDB 语法书写，执行前用 sqlglot 转写成目标方言。
   - 每个连接都有「允许实时查询」开关，关闭后只能抽取。

## 只读保证（多层）
- **会话层**：
  - PG：`default_transaction_read_only=on` + `statement_timeout`
  - MySQL：`SET SESSION TRANSACTION READ ONLY` + `max_execution_time`
  - Oracle：`SET TRANSACTION READ ONLY` + `call_timeout`
  - ClickHouse：`readonly=2`
  - SQLite / DuckDB：只读打开
- **语句层**（`connectors.check_readonly`）：
  - 只接受 SELECT / WITH / SHOW / DESCRIBE / EXPLAIN（拒绝 `EXPLAIN ANALYZE`）。
  - 用 sqlglot 按目标方言解析，单语句；拒绝任何 DML/DDL 节点、`SELECT … INTO`、`INTO OUTFILE`、`FOR UPDATE`。
  - 拒绝读写服务器文件或访问外部资源的函数：`pg_read_file`、`dblink`、`load_file`、`xp_*`、`openrowset`、`url()`、`s3()`、`read_csv` 等。
- 仍然**建议使用只读账号**（连接表单里有提示）。

## 密码与沙箱
- **密码存放**：只存 macOS 钥匙串（service `io.github.datapilot.dbpasswords`，account = 连接 id）。
  - `connections.json` 里不存密码，写入前会强制删除密码字段。
  - Swift 在每次调用内核时把密码临时放进请求参数，用完即弃。
  - 密码不进入模型上下文，也不出现在工具结果里；内核的错误信息会先抹掉密码再返回。
- **模型的权限**：模型不能新建连接，也看不到密码。连接只能由用户在数据面板「数据库连接」中新建。
- **沙箱放行**：联网总开关保持关闭时，内核沙箱只放行 `connections.json` 中各连接的 TCP 端口，外加 DNS（`SandboxPolicy.dbPortRules`）。
  - 新增端口后，下一次数据库调用前内核会自动重启（`KernelClient.syncDBPorts`）。
  - 实测：放行 45431 后，45431 可以连接，45432 和外网都被拒绝。

## 工具
| 工具 | 风险 | 计划模式 |
|---|---|---|
| `db_list` 列出连接 | readOnly | 允许 |
| `db_schema` 表、列、主键、外键、方言 | readOnly | 允许 |
| `db_query` 实时只读查询 | readOnly | 允许 |
| `db_extract` / `db_refresh` | sandboxed | 禁止 |
| `query_save`（save / list / delete「我的查询」） | sandboxed | 禁止 |

## 内核 op（`dataagent/dbops.py`，混入 Engine）
- 连接管理：`db_types`（表单字段定义，各平台 UI 共用）、`db_connections`、`db_save`、`db_delete`、`db_test`
- 读取与抽取：`db_schema`、`db_query`、`db_extract`、`db_refresh`
- 其他：`queries`（我的查询）、`sql_format`（sqlglot pretty）、`explain`（本地 DuckDB，或 PG / MySQL / SQLite / DuckDB / ClickHouse 连接）、`result_page`（结果分页、排序）

## SSH 隧道
- **谁来建**：Rust 核心（`datapilot-core/src/ssh_tunnel.rs`）在内核沙箱外运行 `/usr/bin/ssh -N -L 127.0.0.1:<本地端口>:<数据库主机>:<数据库端口>`，内核再连 `127.0.0.1:<本地端口>`。
- **本地端口**：由连接 id 哈希得到（40000–59999），重启后不变，内核启动前就能确定。沙箱只放行这个端口（`kernel::db_ports` 对开了 SSH 的连接返回隧道端口，而不是数据库端口）。
- **连接字段**：`ssh`、`ssh_host`、`ssh_port`（默认 22）、`ssh_user`（默认当前用户）、`ssh_key`（可选，支持 `~/`）。表单字段来自 `connectors._SSH`，打开 SSH 开关后才显示。
- **口令**：SSH 密码或私钥口令存在钥匙串 `io.github.datapilot.sshsecrets`（账户 = 连接 id），只有核心读取。通过 `SSH_ASKPASS` 加环境变量交给 ssh，不进 argv、文件或内核。没有口令时使用 `BatchMode=yes`（私钥或 ssh-agent）。
- **主机校验**：使用 `StrictHostKeyChecking=accept-new`，首次连接时写入 `~/.ssh/known_hosts`，之后主机指纹变化会拒绝连接。主机名和用户名不能以 `-` 开头，也不能含空白或 shell 字符。
- **复用与清理**：同一连接的隧道会复用，参数变化或进程退出后重建；删除连接时关掉对应隧道，核心退出时全部关闭。
- **使用流程**：开启 SSH 的连接要先保存，再测试。未保存的表单测试会提示先保存，因为需要沙箱先放行隧道端口。内核 `_open` 会拒绝没有 `port_override` 的 SSH 连接，所以不会绕过隧道直连。
- **测试**：
  - `ssh_tunnel::tests::live_tunnel` 启动一个临时 sshd 加 echo 服务器，覆盖无口令密钥、有口令密钥、错误口令三种情况；
  - 另外经由核心 `kernel/call` 做了端到端测试：未保存时被拒绝、保存后建隧道、内核经隧道连到数据库、删除时关闭隧道、退出时清理。

## 界面
- **数据面板「数据库连接」**：可以新建、编辑、测试连接，选表抽取、刷新、删除。
  - 表单字段由 `db_types` 生成；密码用 SecureField 输入，直接写入钥匙串。
- **数据面板「我的查询」**：可以运行（交给 agent 执行并解释）、复制、删除。
- **SQL 卡片「更多」**：保存到「我的查询」、格式化 SQL、执行计划、查看全部行（分页 500 行，可排序）。

## 测试
- `tests/python/test_db_connect.py`：用 SQLite 和 DuckDB 文件做端到端测试，覆盖：
  - 连接、元数据、外键转为关系；
  - 抽取、刷新、派生表重建；
  - 只读拦截、行数上限、方言转写；
  - 密码不落盘、错误信息里抹掉密码；
  - 我的查询、格式化、EXPLAIN、分页。
- **未测试的驱动**：PostgreSQL、MySQL、SQL Server、Oracle、ClickHouse 本机没有服务器，只验证了驱动能导入、连接错误处理正确、SQL 生成和方言转写正确，**没有对真实服务器做端到端测试**。
- Swift：`PermissionMatrixTests` 覆盖新工具在 4 种权限模式下的行为；`dbPortSandboxRules` 检查端口规则和 `connections.json` 端口解析。
