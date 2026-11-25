// Slash commands (same set and prompts as the SwiftUI SlashCommands).
import { L } from "./i18n";

export type Cmd = { name: string; desc: string; skill?: boolean; source?: string };

export const builtins: Cmd[] = [
  { name: "plan", desc: L("计划模式：只读探查数据，提交计划，确认后再执行", "Plan mode: explore read-only, propose a plan") },
  { name: "ask", desc: L("询问模式：联网 / 沙箱外操作需确认（默认）", "Ask mode: confirm network / external actions") },
  { name: "auto", desc: L("自动模式：沙箱内与联网操作自动执行", "Auto mode: sandboxed + network automatic") },
  { name: "full", desc: L("完全访问：所有操作都不再询问（需确认）", "Full access: never ask (confirm first)") },
  { name: "init", desc: L("扫描数据，生成 DATAPILOT.md", "Scan data and write DATAPILOT.md") },
  { name: "dashboard", desc: L("把本会话的分析结果组合成交互式看板", "Turn this chat's analysis into a dashboard") },
  { name: "status", desc: L("模型、上下文占用、内核、已导入的表", "Model, context, kernel, tables") },
  { name: "btw", desc: L("/btw 问题：带上下文快速问一句，不调用工具（运行中也能用）", "/btw question: side question, no tools") },
  { name: "compact", desc: L("总结早先对话，释放上下文", "Summarize earlier turns") },
  { name: "rewind", desc: L("撤销上一轮（恢复工作区与上下文）", "Undo the last turn") },
  { name: "new", desc: L("开始一个新会话", "New chat") },
  { name: "clear", desc: L("开始新会话，当前会话保留在侧栏", "Start fresh; this chat stays") },
  { name: "model", desc: L("/model 名称：切换主模型", "/model name: switch the main model") },
  { name: "skills", desc: L("列出可用的 skill", "List skills") },
  { name: "skill-new", desc: L("用 skill-creator 把一套流程保存成 skill", "Create a skill with skill-creator") },
  { name: "export", desc: L("/export md|html|jsonl：导出对话", "/export md|html|jsonl") },
  { name: "dream", desc: L("整理项目记忆；/dream undo 撤销上次整理", "Consolidate memory; /dream undo") },
  { name: "memory", desc: L("打开记忆面板", "Open the memory panel") },
  { name: "browser", desc: L("打开浏览器截图面板", "Open the browser panel") },
  { name: "mcp", desc: L("查看 MCP 服务器状态", "MCP server status") },
  { name: "context", desc: L("上下文占用明细：系统说明、工具、记忆、对话各占多少", "Context breakdown by part") },
  { name: "fork", desc: L("复制当前会话（对话 + 工作区），试另一种分析思路", "Copy this chat (conversation + workspace) to try another direction") },
  { name: "review", desc: L("复核上一轮分析：口径、空值、重复计数、关联放大、结论与数据是否一致", "Review the last analysis: definitions, nulls, double counting, join fan-out") },
  { name: "doctor", desc: L("一键自检：Python 内核、模型、Key、代理、Chrome、磁盘", "Self-check: Python, model, key, proxy, Chrome, disk") },
  { name: "remember", desc: L("/remember 内容：写一条记忆（项目会话写入项目记忆）", "/remember text: save a memory (project memory in project chats)") },
  { name: "copy", desc: L("复制上一条回答", "Copy the last answer") },
  { name: "permissions", desc: L("打开 设置 → 权限", "Open Settings → Permissions") },
  { name: "settings", desc: L("打开设置（通用）", "Open Settings (General)") },
  { name: "todos", desc: L("打开任务面板", "Open the tasks panel") },
  { name: "help", desc: L("列出所有 / 命令", "List all / commands") },
];

export const reviewPrompt = L(`/review：请复核你上一轮（或本会话最近一次）的分析结论，不要重复做一遍，而是逐项检查：
1. 指标口径：分子分母、去重口径（COUNT vs COUNT DISTINCT）、含税 / 不含税、时间范围是否和问题一致；
2. 空值与异常值：NULL、0、负数、重复行是否被正确处理；
3. 关联：JOIN 是否让行数放大（多对多 / 一对多导致重复计数），必要时用行数对比验证；
4. 过滤与分组：WHERE / GROUP BY 是否遗漏或多算了类别；
5. 结论与数据：文字结论、图表与查询结果是否一致，数字是否抄错。
需要时用 sql_query 做小的核对查询。最后给出：✅ 没问题的项、⚠️ 有疑问的项（附核对 SQL 和结果）、需要修正的结论。`,
`/review: review your last analysis (or the most recent one in this chat). Don't redo it; check item by item:
1. metric definitions: numerator/denominator, COUNT vs COUNT DISTINCT, tax, time range vs the question;
2. nulls and outliers: NULL, 0, negatives, duplicate rows;
3. joins: did a JOIN fan out rows (one-to-many / many-to-many double counting)? verify with row counts if needed;
4. filters and grouping: missing or extra categories in WHERE / GROUP BY;
5. conclusions vs data: do the text, charts and query results agree?
Run small check queries with sql_query when needed. Finish with ✅ OK items, ⚠️ doubtful items (with the check SQL and result), and conclusions to correct.`);

export const dashboardPrompt = L(`请把本会话到目前为止的分析整理成一个交互式 BI 看板（调用 dashboard_create）：
- 先用 data_schema 确认表结构，datasets 的 SQL 按所有可选维度 GROUP BY 预聚合；
- 包含 2–4 个 KPI 卡、主要维度的对比图（配 dimensionOptions / measureOptions / typeOptions 以便切换）、趋势图（如有时间列）和一个洞察文字组件；
- 为常用维度加全局筛选器。完成后用一两句话说明看板内容。`,
`Turn this session's analysis so far into an interactive BI dashboard (call dashboard_create):
- check the schema first; dataset SQL should be pre-aggregated by every selectable dimension;
- include 2–4 KPI cards, comparison charts for the main dimensions (with dimensionOptions / measureOptions / typeOptions), a trend chart if there is a time column, and an insight text widget;
- add global filters for common dimensions. Summarize the dashboard in one or two sentences.`);

export const initPrompt = (target: string) => L(
`/init：请扫描当前可用的数据（已导入的表，以及 ${target} 下的数据文件），为后续分析生成项目说明文件 DATAPILOT.md，写入 ${target}。
内容包括：1) 数据文件 / 表清单与行数；2) 每张表的用途和关键列（中文含义、单位）；3) 表之间的关联键；4) 常用指标口径（若能推断）；5) 已知的数据质量问题；6) 建议的分析方向。
需要时先用 data_import 导入未导入的文件。保持简洁（≤150 行）。完成后告诉我写了什么。`,
`/init: scan the available data (imported tables and data files under ${target}) and write a project guide DATAPILOT.md into ${target}.
Include: 1) files/tables with row counts; 2) purpose and key columns of each table (meaning, units); 3) join keys between tables; 4) common metric definitions (if inferable); 5) known data-quality issues; 6) suggested analyses.
Import files that are not imported yet if needed. Keep it concise (≤150 lines). Tell me what you wrote when done.`);

export const initTarget = (projectRoot?: string | null) => projectRoot
  ? L(`项目根目录 ${projectRoot}（write_file 路径用 DATAPILOT.md）`, `the project root ${projectRoot} (write_file path: DATAPILOT.md)`)
  : L("工作区根目录（write_file 路径用 DATAPILOT.md）", "the workspace root (write_file path: DATAPILOT.md)");

/** Prefix matches, then substring, then subsequence (same ranking as the Swift filter). */
export function filterCmds(cmds: Cmd[], q: string): Cmd[] {
  q = q.toLowerCase();
  if (!q) return cmds;
  const sub = (s: string) => { let i = 0; for (const ch of s) if (ch === q[i]) i++; return i === q.length; };
  const pre = cmds.filter((c) => c.name.toLowerCase().startsWith(q));
  const con = cmds.filter((c) => !pre.includes(c) && c.name.toLowerCase().includes(q));
  const fz = cmds.filter((c) => !pre.includes(c) && !con.includes(c) && sub(c.name.toLowerCase()));
  // last: the description mentions it (e.g. "/对比" finds data-compare, whose description starts with 数据对比)
  const desc = cmds.filter((c) => !pre.includes(c) && !con.includes(c) && !fz.includes(c) && (c.desc ?? "").toLowerCase().includes(q));
  return [...pre, ...con, ...fz, ...desc];
}

export function parseSlash(t: string): { name: string; args: string } | null {
  t = t.trim();
  if (!t.startsWith("/") || t.length < 2) return null;
  const m = t.slice(1).match(/^(\S+)\s*([\s\S]*)$/);
  return m ? { name: m[1], args: m[2].trim() } : null;
}

const ALIASES: Record<string, string> = { "skill-save": "skill-new", "save-skill": "skill-new", "new-skill": "skill-new", "skill-create": "skill-new", "create-skill": "skill-new",
  // Chinese names of built-in skills
  "数据对比": "data-compare", "对比": "data-compare", "比对": "data-compare", "对账": "data-compare" };

/** Typed name → built-in command name: skills keep their exact name; otherwise `_` = `-`, case-insensitive, plus aliases. */
export function canonicalCmd(name: string, skills: string[] = []): string {
  if (skills.includes(name)) return name;
  const n = name.toLowerCase().replace(/_/g, "-");
  if (ALIASES[n] && (builtins.some((c) => c.name === ALIASES[n]) || skills.includes(ALIASES[n]) || !skills.length)) return ALIASES[n];
  return builtins.some((c) => c.name === n) ? n : name;
}
