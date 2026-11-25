// Markdown for /context and /doctor notices.
import { L } from "./i18n";

const PART: Record<string, [string, string]> = {
  system: ["系统说明", "System prompt"], skills: ["Skill 目录", "Skill catalog"], instructions: ["DATAPILOT.md 指令", "DATAPILOT.md"],
  memory: ["项目记忆索引", "Project memory"], tools: ["内置工具定义", "Built-in tools"], mcp: ["MCP 工具定义", "MCP tools"],
  injected: ["每轮注入的上下文", "Injected context"], summary: ["压缩摘要", "Compaction summary"], user: ["你的消息", "Your messages"],
  assistant: ["模型回答", "Model replies"], tool_results: ["工具结果", "Tool results"], images: ["图片", "Images"],
};
const k = (n: number) => (n >= 1000 ? (n / 1000).toFixed(1) + "k" : String(n));

export function contextReport(r: any): string {
  const win = Math.max(1, r.window || 1);
  const used = Math.max(r.estimated || 0, r.last_actual || 0);
  const bar = (n: number) => "█".repeat(Math.round((n / win) * 40)) || (n > 0 ? "▏" : "");
  const rows = (r.parts as any[]).filter((p) => p.tokens > 0).sort((a, b) => b.tokens - a.tokens)
    .map((p) => `| ${L(...PART[p.key] ?? [p.key, p.key])}${p.count ? ` (${p.count})` : ""} | ${k(p.tokens)} | ${((p.tokens / win) * 100).toFixed(1)}% | \`${bar(p.tokens)}\` |`);
  const left = Math.max(0, win - used);
  return [
    L(`**上下文占用**（${r.model}，窗口 ${k(win)} tokens）`, `**Context usage** (${r.model}, window ${k(win)} tokens)`),
    "", `| ${L("部分", "Part")} | tokens | ${L("占窗口", "of window")} | |`, "|---|---:|---:|---|", ...rows,
    `| **${L("合计（估算）", "Total (estimate)")}** | **${k(r.estimated)}** | **${((r.estimated / win) * 100).toFixed(1)}%** | |`, "",
    (r.last_actual ? L(`上一轮模型实际报告：${k(r.last_actual)} tokens。`, `Last turn as reported by the model: ${k(r.last_actual)} tokens. `) : "")
      + L(`剩余约 ${k(left)}；达到 ${Math.round((r.compact_threshold ?? 0.8) * 100)}% 时自动压缩，也可以用 /compact 手动压缩。`, `About ${k(left)} left; auto-compacts at ${Math.round((r.compact_threshold ?? 0.8) * 100)}%, or run /compact.`),
    L("（估算按中文 1 字≈1 token、英文 4 字符≈1 token，和模型实际计数会有偏差。）", "(Estimated: ~1 token per CJK character, ~4 characters per token otherwise.)"),
  ].join("\n");
}

const ITEM: Record<string, [string, string]> = {
  python: ["Python 运行时", "Python runtime"], kernel: ["数据内核", "Data kernel"], key: ["API Key", "API key"], model: ["主模型", "Main model"],
  network: ["网络 / 代理", "Network / proxy"], chrome: ["Chrome（联网搜索 / 浏览器）", "Chrome (web search / browser)"],
  storage: ["数据目录", "Data folder"], disk: ["磁盘空间", "Disk space"], mcp: ["MCP 服务器", "MCP servers"], core: ["核心版本", "Core"],
  recent: ["最近 10 次慢或失败的任务", "Last 10 slow or failed tasks"],
};

export function doctorReport(r: any): string {
  const items = (r.items ?? []) as { key: string; ok: boolean | null; detail: string }[];
  const icon = (ok: boolean | null) => (ok === true ? "✅" : ok === false ? "❌" : "⚠️");
  const bad = items.filter((i) => i.ok === false).length, warn = items.filter((i) => i.ok === null).length;
  return [
    `**${L("自检结果", "Self-check")}**：` + (bad ? L(`${bad} 项失败`, `${bad} failed`) : L("没有失败项", "nothing failed")) + (warn ? L(`，${warn} 项需要注意`, `, ${warn} warnings`) : ""),
    "", ...items.map((i) => `- ${icon(i.ok)} **${L(...ITEM[i.key] ?? [i.key, i.key])}**：${i.detail}`),
  ].join("\n");
}
