// UI actions that go through the core (exports, SQL helpers, dashboards) — shared by cards and slash commands.
import { core } from "./transport";
import { useApp } from "./store";
import { reveal, saveDialog } from "./native";
import { L } from "./i18n";

const sid = () => useApp.getState().selected!;
const notice = (t: string) => useApp.getState().note(t);
const fail = (e: unknown) => useApp.getState().fail(e);

export async function kernel(op: string, params: object, timeout = 300) {
  const r = await core.request("kernel/call", { session: sid(), op, params, timeout });
  return r.result;
}

export async function exportResult(resultId: string, format: string) {
  try {
    const r = await kernel("export", { result_id: resultId, format, filename: `result_${resultId}` });
    notice(L(`已导出 ${r.rows ?? 0} 行：`, `Exported ${r.rows ?? 0} rows: `) + r.path);
    if (r.path) reveal(r.path);
  } catch (e) { fail(e); }
}

export async function saveText(name: string, ext: string, text: string, bom = false) {
  try {
    const r = await core.request("files/save", { session: sid(), name, ext, text, bom });
    notice(L("已导出：", "Exported: ") + r.path);
    reveal(r.path);
  } catch (e) { fail(e); }
}

export async function saveBase64(name: string, ext: string, base64: string) {
  try {
    const r = await core.request("files/save", { session: sid(), name, ext, base64 });
    notice(L("已导出：", "Exported: ") + r.path);
    reveal(r.path);
  } catch (e) { fail(e); }
}

const EXPLAIN_SYS = L(`你是数据分析审核员。用简体中文、面向业务人员解释下面这条 DuckDB SQL 的计算口径，帮助用户核对结果是否正确。
按以下要点输出 3–6 条短句（Markdown 列表），不要复述 SQL、不要客套：
- 数据来源：用了哪些表，怎么关联（关联键、是否会丢行或重复计数）
- 筛选条件：保留/剔除了哪些数据
- 计算方式：分组维度、聚合函数、比率的分子分母
- 排序与截取（如有 LIMIT）
- 风险提示：可能与用户问题不一致的地方（没有就不写）`,
`You review analyses. Explain this DuckDB SQL's logic for a business user in 3–6 short bullets (Markdown):
sources & joins (keys, row loss/duplication), filters, grouping & aggregation (ratio numerator/denominator),
ordering/limit, and risks vs. the user's question (omit if none). Do not repeat the SQL.`);

export async function explainSQL(sql: string, question?: string): Promise<string> {
  const user = (question ? L("用户问题：", "Question: ") + question + "\n" : "") + "SQL:\n" + sql;
  try {
    const r = await core.request("llm/fast", { system: EXPLAIN_SYS, user });
    return r.text || L("（模型没有返回内容）", "(empty response)");
  } catch (e) { return L("解释失败：", "Explain failed: ") + (e as Error).message; }
}

export async function formatSQL(sql: string): Promise<string> { return (await kernel("sql_format", { sql }, 30)).sql; }
export async function planSQL(sql: string): Promise<string> { return (await kernel("explain", { sql }, 60)).plan; }

export async function thumbsUp(question: string, sql: string) {
  const r = await core.request("qsql/feedback", { session: sid(), question, sql, up: true }).catch(() => ({ saved: false }));
  notice(r.saved ? L("已记为可信示例，之后类似问题会参考这条 SQL", "Saved as a verified example") : L("这条结果没有关联到数据集，未保存", "Not linked to a dataset; not saved"));
}

export async function pinChart(chart: object) {
  try {
    const r = await core.request("dashboard/pin", { session: sid(), chart, title: L("本会话看板", "Session dashboard") });
    notice(L("已加入看板：", "Added to dashboard: ") + r.id);
    useApp.getState().openInspector("dashboards", r.id);
  } catch (e) { fail(e); }
}

export async function exportChat(format: string) {
  const fmt = ["md", "markdown"].includes(format) ? "md" : format === "html" ? "html" : format === "jsonl" ? "jsonl" : "md";
  const st = useApp.getState();
  const title = st.chats[sid()]?.title || st.sessions.find((s) => s.id === sid())?.title || "chat";
  const path = await saveDialog(`${title}.${fmt}`, fmt);
  try {
    if (path) {
      await core.request("session/export", { session: sid(), format: fmt, path });
      notice(L("已导出：", "Exported: ") + path);
      reveal(path);
    } else if (path === undefined) {   // browser: download
      const r = await core.request("session/export", { session: sid(), format: fmt });
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([r.text], { type: "text/plain;charset=utf-8" }));
      a.download = `${title}.${fmt}`; a.click();
    }
  } catch (e) { fail(e); }
}
