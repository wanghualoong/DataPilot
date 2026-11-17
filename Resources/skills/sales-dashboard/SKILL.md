---
name: sales-dashboard
description: 快速搭建销售/经营类 BI 看板：KPI 卡、趋势、维度对比、占比、明细，带筛选与维度切换。用户要"看板/仪表盘/dashboard"时使用。Build a sales/business dashboard.
---

# 销售看板搭建

1. `data_schema` 确认：时间列、金额/数量指标、可选维度（地区、品类、渠道、客户等级等低基数列）。
2. 写一个**预聚合 dataset**：按 月份 + 所有候选维度 GROUP BY，指标用 sum/count。行数控制在 10 万以内。
```sql
SELECT date_trunc('month', 日期) AS 月份, 大区, 品类, 渠道,
       sum(金额) AS 销售额, count(*) AS 订单数, count(DISTINCT 客户ID) AS 客户数
FROM ... WHERE <口径过滤> GROUP BY ALL
```
   注意：count_distinct 类指标在二次聚合时不可加，看板中避免对其按维度再求和，或单独建 dataset。
3. 组件建议：
   - 第一行 3-4 个 kpi（总销售额、订单数、客户数、客单价）
   - bar：维度对比，带 dimensionOptions（所有候选维度）与 measureOptions
   - line：月度趋势，可加 series=品类（≤ 8 个系列）
   - donut：占比（≤ 8 类）
   - radar：多指标对比（≥ 3 个指标时效果最好）
   - table：明细
4. filters：每个低基数维度一个 multiselect，时间列一个 daterange。
5. 创建后简要说明看板包含什么、如何筛选/切换，以及可导出 HTML。
