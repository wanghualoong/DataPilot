---
name: rfm-segmentation
description: RFM 客户分层（最近购买 Recency、频次 Frequency、金额 Monetary），识别高价值、流失风险客户。用户问客户分层、价值、复购、流失时使用。RFM customer segmentation.
---

# RFM 分层

1. 需要：客户 ID、订单日期、订单金额。确认是否剔除退款/测试订单（查口径记忆）。
2. 计算（参考日期 = 数据最大日期 + 1 天）：
```sql
WITH base AS (
  SELECT 客户ID, max(日期) AS last_dt, count(DISTINCT 订单ID) AS F, sum(金额) AS M FROM orders GROUP BY 1
), s AS (
  SELECT *, date_diff('day', last_dt, (SELECT max(日期) + 1 FROM orders)) AS R,
    ntile(5) OVER (ORDER BY date_diff('day', last_dt, (SELECT max(日期) + 1 FROM orders)) DESC) AS r_score,
    ntile(5) OVER (ORDER BY F) AS f_score, ntile(5) OVER (ORDER BY M) AS m_score
  FROM base
)
SELECT *, CASE
  WHEN r_score>=4 AND f_score>=4 AND m_score>=4 THEN '重要价值'
  WHEN r_score<=2 AND f_score>=4 THEN '重要挽留'
  WHEN r_score>=4 AND f_score<=2 THEN '新客/潜力'
  WHEN r_score<=2 AND f_score<=2 THEN '流失'
  ELSE '一般' END AS 分层
FROM s;
```
3. 汇总各层客户数、金额占比；用 donut 展示人数占比、bar 展示金额。
4. 给出每层的运营建议（1 句）。
