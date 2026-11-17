# ADR-0006 · 检索只用关键词 + grep，不接 Embedding / Rerank

日期：2026-11-08　状态：已采纳（取代计划 3.5 节“可选 Embedding / Rerank”）

## 背景
早期设计为记忆、Q-SQL、schema linking 叠加向量检索（Embedding）与精排（Rerank）。用户要求与 Codex / Claude Code 保持一致。
Codex 与 Claude Code 都没有向量索引：Claude Code 用 Grep / Glob 工具让模型自己搜代码，记忆（CLAUDE.md / MEMORY.md）直接全文注入；Codex 用 rg 和 AGENTS.md 分层拼接。

## 决策
- 删除 Embedding / Rerank 角色、向量索引与 RRF 融合。旧配置加载时自动清除这两个角色。
- 记忆 / 情景 / Q-SQL：FTS5 trigram BM25，无命中退化为中文二元组重叠。
- 新增 `grep`（正则搜内容，支持 glob 过滤、上下文行、files_with_matches / count）与 `glob`（按文件名，按修改时间倒序）工具，只读、可并行、Plan 模式可用；跳过 .git/.datapilot/checkpoints 与二进制文件（duckdb/parquet/xlsx…）。
- 表内数据由 `sql_query` 检索；取值对齐沿用 Vanna 做法（先查 distinct 值），不靠向量。

## 理由
- 少配两个模型，部署和排障都更简单；检索结果可解释（命中哪个词一目了然）。
- 记忆规模小（几十到几百条），画像与口径本就按置信度全量注入，向量带来的收益有限。
- 模型主动 grep 的效果依赖模型能力；qwen3.8-27b 的工具调用足够稳定。

## 代价
同义词召回变弱（如“营收”≠“销售额”）。缓解：catalog 同义词表、模型可多次 grep 不同关键词。
