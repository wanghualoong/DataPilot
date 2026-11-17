# 06 · 数据引擎与 Text-to-SQL

## 阈值（默认 5,000 行，可在设置修改）
| 规模 | 处理 |
|---|---|
| ≤ 阈值 | DuckDB 表 + 同名 pandas DataFrame（两种方式都可用） |
| > 阈值、多表、多 sheet | DuckDB 表，走 Text-to-SQL；pandas 只处理 SQL 已缩减后的结果 |
理由：关键不是 pandas 能否处理（百万行也可以），而是模型能否“看清”数据。几千行以上必须依靠 schema/画像/SQL；多表关联 SQL 更准确。所有表都进 DuckDB，保证一条一致的路径。**任何数字都必须来自执行结果**。

## 导入（`dataagent/importer.py`）
- 编码：BOM → UTF-8 → charset-normalizer；GB2312/GBK 统一放宽为 GB18030。
- 分隔符：csv.Sniffer，失败则计数投票。
- 表头行：扫描前 15 行，取非空多且字符串比例高的一行（跳过 Excel 标题行）。
- Excel：calamine 引擎（快），失败退回 openpyxl/xlrd；每个非空 sheet 一张表 `<文件>_<sheet>`。
- 清洗：去全空行列；列名去空白、去重、空名为 `col_n`（保留中文）；去千分位后 ≥98% 可解析的转数值；`YYYY-MM(-DD)`/`年月日` 转日期。
- 返回每表画像与数据集指纹。

## 画像（`profile.py`）
每列：类型、空值率、approx distinct、数值/时间 min/max(/avg)、低基数（≤50）全部取值、否则 3–5 个样例、是否唯一。渲染为 **M-Schema**（XiYan）文本：
```
# Table: orders  (20000 rows)
[
(customer_id:BIGINT, Range [1 ~ 500]),
(状态:VARCHAR, Values: [paid, refund]),
...
]
【Foreign keys】
orders.customer_id = master_customers.customer_id  (N:1, confidence 0.99)
```

## 关系推断
候选：父列唯一 + 名称规则（同名 `*_id`/`编号`/`代码`、`<父表>id`↔`id`、后缀包含）+ 类型兼容 → SQL 计算子列值在父列的包含率（采样 20 万）。包含率 ≥0.95（名称强匹配时 ≥0.8）入选；置信度 = 0.6×包含率 + 0.4×名称分；每个子列保留最佳。检查器“数据”页展示，`catalog_update` 可确认。

## 语义层 `catalog.json`
表/列描述、指标（name/expr/description）、确认的关系、同义词。`data_schema` 输出时合并进 M-Schema 与【Metrics】段。

## SQL 执行与守卫（`sqlguard.py` + `op_sql`）
- 只允许 SELECT/WITH/SUMMARIZE/DESCRIBE/SHOW/EXPLAIN/PIVOT；sqlglot 解析拒绝 DDL/DML/COPY/多语句。
- 返回前 50 行预览 + 总行数 + 耗时；完整结果存 `artifacts/<result_id>.parquet`。
- 空结果附提示：先查 DISTINCT 取值核对过滤条件。
- 错误原样回传，模型自纠（系统提示要求 ≤3 次）。

## 数据库直连
`connectors.py`（驱动、只读守卫、元数据）+ `dbops.py`（op_db_*、我的查询、格式化、EXPLAIN、结果分页）。抽取得到的是普通 DuckDB 表，之后与导入表一样用 `sql_query` / `data_transform` / 看板分析；外键自动成为已确认关系。详见 19-db-connections.md。

## 结果分页
`result_page {result_id, offset, limit≤5000, sort?, desc?}` 从 `artifacts/<result_id>.parquet` 读取任意一页，SQL 卡片「查看全部」使用。

## 提示词规则（系统提示）
标识符逐字匹配 schema；中文列双引号；不确定取值先 intermediate SQL；复杂问题多步 SQL；默认应用数据集口径；结果为空先核查；数字只来自执行结果。

## 示例积累
`sql_query.question` + 有结果 → 存 Q-SQL（未验证）；👍 → verified；同数据集（指纹）新问题检索 top-3 作为示例注入。

## 评测
`tests/eval/text2sql_eval.json`（基于 `make_fixtures.py` 数据集的问题与标准 SQL）+ `scripts/run_eval.py`：用真实模型跑 Agent，比较结果集，输出执行准确率。
