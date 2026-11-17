---
name: data-cleaning
description: 数据清洗与质量检查：缺失值、重复行、异常值、类型错误、编码/日期格式问题。导入新数据后或结果可疑时使用。Data quality checks and cleaning.
---

# 数据清洗流程

1. **概览**：`data_schema` 查看各列空值率、取值范围、低基数取值。
2. **重复**：`SELECT count(*) - count(DISTINCT 主键) FROM t`；无主键时按全部列判重。
3. **缺失**：空值率 > 20% 的列向用户说明；数值列用中位数、类别列用"未知"填充前先询问。
4. **异常值**：数值列用 IQR（Q1-1.5IQR, Q3+1.5IQR）检测，列出前 10 条，不要自动删除。
5. **类型**：数字被识别为 VARCHAR 时检查千分位、货币符号、百分号；日期列统一为 DATE。
6. **一致性**：同一实体的多种写法（"北京"/"北京市"）用 `SELECT col, count(*) GROUP BY 1` 找出并与用户确认映射。
7. 清洗用 python（pandas）生成新表，保留原表：`con.execute("CREATE OR REPLACE TABLE t_clean AS SELECT * FROM df")`。
8. 输出清洗报告：每一步发现的问题数、处理方式、清洗后行数。把确认过的映射/规则用 `memory_write`（scope=dataset, kind=rule）保存。
