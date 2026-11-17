---
name: yoy-mom-analysis
description: 同比（YoY）、环比（MoM/QoQ）与增长贡献分析。用户问"增长/下降/同比/环比/比去年"时使用。Year-over-year and period-over-period growth analysis.
---

# 同比环比分析

1. 确认时间列与指标口径（先检查"数据集口径与规则"记忆）。确认数据覆盖的完整周期，不完整的月份要提示。
2. 用窗口函数计算，DuckDB 示例：
```sql
WITH m AS (
  SELECT date_trunc('month', 日期) AS 月份, sum(金额) AS 值 FROM t GROUP BY 1
)
SELECT 月份, 值,
  lag(值, 1)  OVER (ORDER BY 月份) AS 上月,
  lag(值, 12) OVER (ORDER BY 月份) AS 去年同月,
  值 / NULLIF(lag(值, 1)  OVER (ORDER BY 月份), 0) - 1 AS 环比,
  值 / NULLIF(lag(值, 12) OVER (ORDER BY 月份), 0) - 1 AS 同比
FROM m ORDER BY 月份;
```
3. **增长贡献拆解**：按维度（大区/品类）计算 `本期-上期`，贡献度 = 维度增量 / 总增量，找出主要驱动项。
4. 图表：趋势用 line（x=月份），增长率不要和金额放同一张图（禁止双轴），另画一张。
5. 结论先给总体同比/环比数字，再列前 3 个驱动维度。
