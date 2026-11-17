# ADR 0003：DuckDB + 语义化 DashboardSpec
- 决策：所有导入表进入每会话 `data.duckdb`；小表（≤5000 行）额外暴露为 pandas DataFrame。看板用自定义语义 JSON（数据集 SQL + 列映射 + 聚合），由前端确定性编译为 ECharts。
- 理由：一条一致的 Text-to-SQL 路径；模型只选列而不写图表配置，出错率低、可校验字段；预聚合 cube 让前端筛选即时。
- 后果：渲染器需维护图表编译规则；超过 10 万行的数据集被截断（提示用户进一步聚合）。
