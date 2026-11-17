---
name: skill-creator
description: 创建或更新一个 DataPilot skill：把用户描述的或本会话里做过的分析流程整理成可复用的 SKILL.md 并保存。用户说"创建 skill / 保存成技能 / 以后都这样分析"时使用。Create or update a reusable skill.
---

# 创建 Skill

目标：产出一个简洁、可复用、可被模型按需加载的 `SKILL.md`。

1. **弄清用途**（必要时用 ask_user 问 1–2 个问题）：
   - 这个 skill 解决什么分析任务？什么时候应该触发（关键词、数据类型）？
   - 输入是什么数据（必需的列/口径）？输出是什么（表、图表、看板、报告）？
   - 如果用户说"把刚才的流程保存下来"，就从本会话的工具调用（SQL、python、chart_create、dashboard_create）中提炼步骤。
2. **命名**：小写短横线，如 `monthly-sales-review`；不与已有 skill 重名（已有列表见系统提示的 Skills 目录）。
3. **写 description**（≤ 250 字，中英文均可）：第一句说做什么，第二句说何时使用。这是模型决定是否加载它的唯一依据，要包含触发词。
4. **写正文 instructions**（Markdown，建议 ≤ 80 行）：
   - 编号步骤，每步写清使用的工具（data_schema / sql_query / python / chart_create / dashboard_create / write_file）。
   - 关键 SQL 用模板给出，列名用占位符并说明如何从 schema 中匹配。
   - 写明口径与常见坑（如剔除退款、去重、空值处理、count_distinct 不可再加总）。
   - 写明产出格式（结论先行、表格、建议的图表类型）。
   - 不要写入具体数据值或一次性的信息。
5. **向用户展示草稿**，确认后调用 `save_skill`（name, description, instructions）。保存后告诉用户可以用 `/skill名` 直接调用。
6. 更新已有 skill：用同名再次 `save_skill` 覆盖（用户 skill 目录中），并说明改了什么。
