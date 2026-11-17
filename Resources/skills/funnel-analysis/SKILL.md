---
name: funnel-analysis
description: 转化漏斗分析：各步骤人数、步骤转化率、整体转化率、流失最大环节，可按维度拆分。用户问转化、漏斗、流失环节时使用。Conversion funnel analysis.
---

# 漏斗分析

1. 与用户确认步骤顺序与事件定义（如 浏览→加购→下单→支付），以及时间窗口（如 7 天内）。
2. 每个用户取各步骤首次时间，要求后一步时间 ≥ 前一步：
```sql
WITH u AS (
  SELECT user_id,
    min(CASE WHEN event='view' THEN ts END) AS s1,
    min(CASE WHEN event='cart' THEN ts END) AS s2,
    min(CASE WHEN event='order' THEN ts END) AS s3
  FROM events GROUP BY 1
)
SELECT count(s1) AS 浏览,
       count(CASE WHEN s2>=s1 THEN 1 END) AS 加购,
       count(CASE WHEN s3>=s2 AND s2>=s1 THEN 1 END) AS 下单
FROM u;
```
3. 计算步骤转化率与整体转化率，指出流失最大的环节。
4. 图表用 funnel；按维度对比时用 bar（x=步骤，series=维度）。
