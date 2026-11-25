---
name: data-compare
description: 数据对比 / Data Compare：两个或多个表格（Excel、CSV、已导入的表）之间、表格与已连接数据库之间逐行逐列对比——按键找出只在一边的记录、哪一列从什么变成什么、重复键与空键，或按维度对账（行数、金额合计），并导出 Excel 差异报告。用户说"对比/比对/核对/对账/差异/两个版本有什么不同/和数据库对不对得上"时使用。Compare tables, files and database data row by row or by totals.
---

# 数据对比 / Data Compare

对比逻辑固定在内核模块 `dataagent.compare` 里（DuckDB SQL，大表也能跑），**不要自己临场写 JOIN / EXCEPT 去对比**。用 `python` 工具调用：

```python
from dataagent.compare import compare, compare_many, reconcile, suggest_keys, schema_diff
r = compare(con, "左表", "右表", keys=["订单号"])
print(r["text"])          # 概要 + 差异样例 + 结果表名 + Excel 报告路径 + ⚠ 提示
```

## 1. 确定对比对象（只用本轮上传 / 用户点名的文件和表）
| 场景 | 做法 |
|---|---|
| 表格 vs 表格 | 本轮上传的文件、项目文件夹里的文件先 `data_import`（一个工作簿多个 sheet 会变成多张表）；已导入的表直接用表名 |
| 多个表格 | 同上导入后用 `compare_many`：默认都和第一个（基准）比；按时间排列的版本（1 月、2 月、3 月…）用 `chain=True` 依次相邻比 |
| 表格 vs 数据库 | `db_list` → `db_schema` 找到对应表 → 先确认规模（`db_query` 跑 `SELECT count(*) …`），再按下面「数据库」一节抽取 |
| 用户没说清楚和什么比 | 用 `ask_user` 问，给出候选（文件名 / 表名 / 连接名），不要猜 |

也可以直接对比一条 SELECT（例如只比某个月）：`compare(con, "SELECT * FROM 订单 WHERE 月份='2026-10'", "db_orders")`。

## 2. 确定对比键（最关键）
- 用户说了键（订单号、客户编码、工号…）就用；两边列名不同写成 `keys=["订单号=order_id"]`。
- 没说时先 `suggest_keys(con, 左, 右)` 看候选（两边唯一率、键匹配率）。只有一个候选且两边都唯一、匹配率高才直接用，**并在结论里说明用了哪个键**；否则用 `ask_user` 让用户选。
- 没有任何键（流水、日志）时 `keys=[]` 做整行对比：只能看出哪些行只在一边出现（重复行按次数计），看不出具体哪列变了，要向用户说明。
- 键格式不一致：前导零用 `key_steps=["strip_zeros"]`（"007"="7"），大小写用 `ignore_case=True`，带前缀 / 分隔符不同的先用 `relation_test` 探测转换，再用 SELECT 生成规范化的键列后对比。

## 3. 调用
```python
# 两表：值列名不同用 columns 映射；不关心的列 ignore；金额容差 tol（数字或 {列: 容差, "*": 默认}）
r = compare(con, "财务_10月", "系统_10月", keys=["单据号"], columns={"含税金额": "amount"}, ignore=["更新时间"], tol=0.01)

# 多表
m = compare_many(con, ["v1", "v2", "v3"], keys=["客户编码"], chain=True)   # 或 base="v1"
print(m["text"])

# 汇总对账：按维度比行数和金额合计（数据库大表、或键会重复的明细行）
a = reconcile(con, "本地订单", "db_orders_agg", by=["大区", "月份"], measures=["金额"], tol=0.01)
print(a["text"])
```
- 自动处理：文本首尾空格、Excel 把编号读成 1.0、"1,234.50" / "¥12" 这类文本数字、日期文本与日期类型、数值容差（默认 1e-6，金额建议 0.01）。
- 结果表（可继续 `sql_query` 钻取、`data_export` 导出）：`cmp_<名>_summary / _columns / _diff / _left_only / _right_only / _bad_keys`，汇总对账是 `cmp_<名>_reconcile`。`_diff` 是长表：键、列、左值、右值、差额。
- 默认在 OUTPUT_DIR（项目的 outputs/）生成 Excel 报告（概要、列对照、值不同、仅在左表、仅在右表、重复或空键）；不需要时 `report=False`。

## 4. 数据库
- **小表 / 有限范围**（几十万行以内）：`db_extract` 抽成本地表再 `compare`。只比一部分时带 `where`，例如 `{"table":"orders","as":"db_orders","where":"order_date >= '2026-10-01'"}`，范围要和本地表一致。
- **大表**：先对账，再钻取：
  1. `db_extract` 用 `sql` 在数据库端汇总：`SELECT region, month, count(*) AS 行数, sum(amount) AS 金额 FROM orders GROUP BY 1,2`（`as`: `db_orders_agg`）；
  2. `reconcile(con, "SELECT 大区 AS region, 月份 AS month, count(*) AS 行数, sum(金额) AS 金额 FROM 本地订单 GROUP BY 1,2", "db_orders_agg", by=["region","month"], measures=["行数","金额"], counts=False)`；
  3. 只把不一致的组用 `where` 抽明细，再 `compare` 找到具体记录。
- 不要为了快去掉明细或粗化口径；抽取前告诉用户范围和大概行数。

## 5. 汇报（先结论，后细节）
1. 一句话结论：两边是否一致；不一致时多少行、主要差在哪几列。
2. 一张小表：左行数、右行数、一致、有差异、仅左、仅右、重复键、空键。
3. 差异样例（前 10–20 条，列出键、列、左值、右值、差额）；按列统计差异最多的前几列。
4. ⚠ 提示原样转述：自动选的键、重复键没参与逐值比较、类型不一致按文本比较、键完全对不上等。
5. 给出 Excel 报告路径和结果表名，并建议下一步（例如「要不要按大区看金额差异？」「重复的 37 个单据号要不要先去重？」）。

## 要求
- 只陈述对比结果里的数字，不推测差异原因；可以给"可能原因"但要标明是推测（例如四舍五入、时间口径不同、退款未同步）。
- 键或范围不确定时先问，不要把错误的键得出的"差异"当结论。
- 用户确认过的对比口径（键、容差、忽略的列、映射）可以用 `memory_write` 记下，下次同类对比直接用。
