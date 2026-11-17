# ADR 0004：四层记忆 + 证据累积
- 决策：L0 上下文 / L1 会话笔记 / L2 事件·情景·Q-SQL / L3 偏好·习惯·数据集口径 / L4 Skills；显式与纠正立即生效，推断类需证据 ≥3 或用户确认；数据集指纹（含 Jaccard≥0.7 的同类报表）继承口径。存储统一在 SQLite（FTS5 trigram + 可选向量）。
- 理由：本地模型窗口小，需按预算注入；避免把一次性推断当成长期偏好；可解释、可编辑。
- 参考：codex（压缩/交接摘要）、公开分析中的 session memory / extract memories / auto dream 设计思路。
