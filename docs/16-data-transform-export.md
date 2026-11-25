# 16 · 对话式计算、派生表与导出

## 计算放在哪一层
| 需求 | 工具 | 说明 |
|---|---|---|
| 一次性问答（筛选、加减乘除、条件统计） | `sql_query` | 只读；结果存成 `artifacts/<result_id>.parquet` |
| 需要复用的口径 / 清洗 / 标记 | `data_transform` | 生成派生表或视图；原始导入表只读 |
| 交互筛选、切换维度、比率 | 看板 `calcFields` / `measures` / `params` | 见 07 |
| “导出 / 下载 / 存成 CSV” | `data_export` | 写到项目 `outputs/` 或工作区 `exports/` |

Python 中的 DataFrame 可在 `sql_query` 里按变量名查询（只注册到本对话的游标）。

## data_transform（`sqlguard.check_transform`）
- 允许：`CREATE [OR REPLACE] TABLE|VIEW x AS SELECT …`；仅对派生表：`ALTER`、`UPDATE`、`DROP`。
- 禁止：改动导入表、`COPY/ATTACH/INSTALL/LOAD/INSERT/DELETE/MERGE/PRAGMA/SET`、读写文件的函数；多语句。
- 风险：普通为 `sandboxed`；`DROP` 为 `external`（询问模式下出审批卡）；计划模式禁用。
- 血缘 `data/derived.json`：SQL、依赖、后续 ALTER/UPDATE 步骤。源文件重新导入后按依赖顺序重建（`op_rebuild_derived`），失败的在下轮对话开头提示。
- 数据面板显示「派生」标记，悬停看 SQL。

## data_export（`op_export`）
- 来源：`sql` / `table` / `result_id`；格式：csv / xlsx / parquet / json；`columns` 选列或改名；`limit`。
- CSV 默认带 UTF-8 BOM（Excel 打开中文不乱码）。
- xlsx：XlsxWriter `constant_memory` 分批写（每批 2 万行），进度显示在内核任务列表；超过 1,048,575 行拒绝并建议 CSV。
- 单元格转换 `export_cells.py`（`convert(v)` 按值类型分发，`XlsxCells` 写入）：空值 / NaN / ±inf → 空单元格；日期时间去时区；≥15 位整数和超过 15 位有效数字的小数转文本；bytes 转 hex；列表 / 字典转 JSON；文本截到 32,767 字。
- 同名文件自动加 `_2`、`_3`，从不覆盖。
- 界面入口：SQL 结果卡「导出」、数据面板右键「导出表…」、看板导出菜单「导出数据集明细」、看板卡片 ⤓（当前筛选后的聚合数据）。

## 数据字典（`kind:"dictionary"` → `op_export_doc`）
xlsx / docx / pdf / md / html / csv；内容：表、列、类型、空值率、唯一、示例、catalog 业务描述、已确认关系。PDF 用 reportlab 内置中文字体 STSong-Light。

## SQL「解释」
SQL 结果卡的「解释」按钮由快速模型用中文列出口径（来源与关联、筛选、聚合、排序、风险），只读。
