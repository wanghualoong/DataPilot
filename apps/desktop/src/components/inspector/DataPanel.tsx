import { IconBtn } from "../ui";
import { FlaskConical, LoaderCircle, RefreshCw, Square, Table2, Users } from "lucide-react";
import { ic } from "../ui";
import { useEffect, useState } from "react";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { core } from "../../transport";
import { reveal } from "../../native";
import { saveBase64 } from "../../actions";
import { DataTable } from "../DataTable";
import { WebFrame } from "../WebFrame";
import { kcall, Menu, Section, sid, useLoad } from "./common";
import { DBConnections } from "./DBConnections";
import { KernelError } from "./KernelError";

export async function exportTo(params: object) {
  const st = useApp.getState();
  try {
    const r = await kcall("export", params, 900);
    st.note(L("已导出：", "Exported: ") + r.path);
    if (r.path) reveal(r.path);
  } catch (e) { st.fail(e); }
}

/** ER diagram: Resources/web/er.html; its DPHost.call(op) goes to the kernel through the core's UI whitelist. */
function ERDiagram({ onChange }: { onChange: () => void }) {
  return (
    <WebFrame page="er.html" height="70vh" render={(w) => w.DPER?.load()}
      onMessage={async (m, w) => {
        if (m.type === "call") {
          try { w.DPHost._resolve(m.id, await kcall(m.op, m.params ?? {}, 300), null); }
          catch (e) { w.DPHost._resolve(m.id, null, (e as Error).message); }
        } else if (m.type === "relationsChanged") onChange();
        else if (m.type === "exportPNG" && typeof m.data === "string") saveBase64(m.name || "ER", "png", m.data.slice(m.data.indexOf(",") + 1));
      }} />
  );
}

/** Long-running kernel steps (this chat's and — in a project — other chats'), polled every 1.5 s while shown. */
function TaskList() {
  const st = useApp();
  const [tasks, setTasks] = useState<any[]>([]);
  const [n, setN] = useState(0);
  useEffect(() => {
    let live = true;
    const tick = async () => {
      const r = await core.request("kernel/tasks", { session: sid() }).catch(() => ({ tasks: [] }));
      if (live) setTasks(((r.tasks ?? []) as any[]).filter((t) => t.secs >= 1.5 && t.state === "running"));
    };
    tick();
    const h = setInterval(tick, 1500);
    return () => { live = false; clearInterval(h); };
  }, [st.selected, n]);
  const project = st.sessions.find((s) => s.id === st.selected)?.project_id;
  const label = (t: any) => {
    const who = t.ns === st.selected || (t.ns === "_default" && !project) ? L("本对话", "This chat")
      : t.ns === "_default" ? L("检查器", "Inspector") : "「" + (st.sessions.find((s) => s.id === t.ns)?.title ?? t.ns) + "」";
    const what = t.op === "import" ? L("正在导入", "importing") : t.op === "exec" ? L("正在执行代码", "running code")
      : ["profile", "schema", "relations"].includes(t.op) ? L("正在分析表结构", "profiling") : L("正在查询", "querying");
    return `${who} ${what}${t.detail ? " · " + t.detail : ""}（${Math.floor(t.secs)}s）`;
  };
  if (!tasks.length) return <div className="muted small">{L("没有正在运行的任务", "Nothing running")}</div>;
  return <>{tasks.map((t) => (
    <div key={t.id} className="irow">
      <LoaderCircle {...ic} size={12} className="spin muted" />
      <span className="grow small muted ell2" title={t.detail}>{label(t)}</span>
      <IconBtn icon={Square} size={12} title={L("停止这一步（只停这一个任务，不重启内核）", "Stop this step (only this task; the kernel is not restarted)")}
        onClick={async () => { await core.request("kernel/cancel_task", { session: sid(), id: t.id }).catch(() => {}); setN((x) => x + 1); }} />
    </div>
  ))}</>;
}

function Tasks() {
  return <Section title={L("内核任务", "Kernel tasks")} open={false}><TaskList /></Section>;
}

function SavedQueries() {
  const send = useApp((s) => s.send);
  const [qs, , reload] = useLoad(async () => ((await kcall("queries", {}, 30)).queries ?? []) as any[]);
  if (!qs?.length) return null;
  return (
    <Section title={L("我的查询", "Saved queries")}>
      {qs.map((q) => (
        <div key={q.id} className="irow">
          <span className="grow" title={q.sql}>{q.name}{q.connection && <span className="muted"> · {q.connection}</span>}</span>
          <Menu label="⋯" items={[
            [L("运行", "Run"), () => send(L(`运行我保存的查询「${q.name}」`, `Run my saved query “${q.name}”`) + (q.connection ? L(`（在数据库连接 ${q.connection} 上用 db_query）`, ` (on database connection ${q.connection} via db_query)`) : "") + "：\n```sql\n" + q.sql + "\n```")],
            [L("复制 SQL", "Copy SQL"), () => navigator.clipboard?.writeText(q.sql)],
            [L("删除", "Delete"), async () => { await kcall("queries", { action: "delete", query_id: q.id }, 30).catch(() => {}); reload(); }],
          ]} />
        </div>
      ))}
    </Section>
  );
}

const relBadge = (s: string) => s === "confirmed" ? [L("已确认", "confirmed"), "ok"] : s === "suspect" ? [L("疑似", "suspect"), "warn"]
  : s === "rejected" ? [L("已否决", "rejected"), "muted"] : [L("推断", "inferred"), "muted"];

export function DataPanel() {
  const st = useApp();
  const session = st.sessions.find((s) => s.id === st.selected);
  const project = st.projects.find((p) => p.id === session?.project_id);
  const [refreshRel, setRefreshRel] = useState(0);
  const [data, err, reload, busy] = useLoad(async () => {
    const [t, d] = await Promise.all([kcall("tables", {}, 60), kcall("derived", {}, 30).catch(() => ({ tables: [] }))]);
    // derived table (lowercased) → SQL (+ follow-up steps), shown as the badge tooltip
    const derived = new Map<string, string>(((d.tables ?? []) as any[]).map((x) => [String(x.name).toLowerCase(), [x.sql ?? "", ...(x.steps ?? [])].join(";\n")]));
    return { tables: (t.tables ?? []) as any[], derived };
  });
  const [rels, , reloadRels] = useLoad(async () => ((await kcall("relations", refreshRel ? { refresh: true } : {}, 300)).relations ?? []) as any[], [refreshRel]);
  const [open, setOpen] = useState<string>();
  const [preview, setPreview] = useState<{ columns: string[]; rows: unknown[][] }>();
  const [er, setER] = useState(false);
  const show = async (t: string) => {
    if (open === t) { setOpen(undefined); return; }
    setOpen(t); setPreview(undefined);
    const r = await kcall("preview", { table: t, limit: 100 }, 60).catch(() => null);
    if (r) setPreview({ columns: r.columns, rows: r.rows });
  };
  const tables = data?.tables ?? [];
  return (
    <div className="ipanel">
      <DBConnections onChange={reload} />
      <Section title={L(`表（${tables.length}）`, `Tables (${tables.length})`)} right={
        <>
          <IconBtn icon={RefreshCw} size={13} title={L("刷新", "Refresh")} spin={busy} onClick={reload} />
          {tables.length > 0 && <Menu label={L("数据字典…", "Dictionary…")} items={["xlsx", "docx", "pdf", "md", "html"].map((f) => [f === "docx" ? "Word (.docx)" : f.toUpperCase(), () => exportTo({ kind: "dictionary", format: f })])} />}
        </>}>
        {project && <div className="muted small row ishare"><Users {...ic} size={12} />{L(`项目内共享：「${project.name}」的 ${st.sessions.filter((s) => s.project_id === project.id).length} 个对话都能看到这些表`, `Shared in project “${project.name}”: visible to its ${st.sessions.filter((s) => s.project_id === project.id).length} chats`)}</div>}
        {err && <KernelError raw={err} reload={reload} />}
        {!tables.length && !err && <div className="muted small">{L("还没有导入数据。拖入 CSV / Excel，或在对话里让 Agent 导入。", "No data yet. Drop a CSV / Excel file or ask the agent to import one.")}</div>}
        {tables.map((t) => (
          <div key={t.table}>
            <div className="irow click" onClick={() => show(t.table)}>
              <span className="grow row">{data?.derived.has(t.table.toLowerCase()) ? <FlaskConical {...ic} size={13} className="muted" /> : <Table2 {...ic} size={13} className="muted" />}{t.table}
                {data?.derived.has(t.table.toLowerCase()) && <span className="ibadge accent" title={data.derived.get(t.table.toLowerCase())}>{L("派生", "derived")}</span>}</span>
              <span className="muted small">{Number(t.rows).toLocaleString()} {L("行", "rows")} · {t.columns?.length ?? 0} {L("列", "cols")}</span>
              <span onClick={(e) => e.stopPropagation()}>
                <Menu label="⋯" items={["csv", "xlsx", "parquet"].map((f) => [L("导出 ", "Export ") + f.toUpperCase(), () => exportTo({ table: t.table, format: f, filename: t.table })])} />
              </span>
            </div>
            {open === t.table && (
              <div className="ipreview">
                <div className="muted small">{(t.columns ?? []).map((c: any) => `${c.name}: ${c.type}`).join(" · ")}</div>
                {preview && <DataTable columns={preview.columns} rows={preview.rows} max={100} />}
              </div>
            )}
          </div>
        ))}
      </Section>
      {(tables.length > 1 || (rels?.length ?? 0) > 0) && (
        <Section title={L(`表关系（${rels?.length ?? 0}）`, `Relations (${rels?.length ?? 0})`)} right={
          <>
            <button className="link" onClick={() => setER(true)}>{L("关系图", "Diagram")}</button>
            <button className="link" onClick={() => setRefreshRel((x) => x + 1)}>{L("重新推断", "Re-infer")}</button>
          </>}>
          {(rels ?? []).map((r) => {
            const [b, cls] = relBadge(r.status);
            const act = async (action: string) => { await kcall("relation_define", { id: r.id, action }, 60).catch(st.fail); reloadRels(); };
            const meta = [r.cardinality, r.match_rate != null ? L("匹配 ", "match ") + Math.round(r.match_rate * 100) + "%" : null, r.transform ? L("转换：", "transform: ") + r.transform : null].filter(Boolean);
            return (
              <div key={r.id} className="irel">
                <div className="irow">
                  <span className={"ibadge " + cls}>{b}</span>
                  <span className="grow small ell">{r.from_table}.{r.from_column} → {r.to_table}.{r.to_column}</span>
                  <Menu items={[
                    ...(r.status !== "confirmed" ? [[L("确认", "Confirm"), () => act("confirm")] as [string, () => void]] : []),
                    [L("否决（不再提示）", "Reject (never suggest again)"), () => act("reject")],
                    ...(r.source !== "auto" ? [[L("删除", "Delete"), () => act("delete")] as [string, () => void]] : []),
                  ]} />
                </div>
                {meta.length > 0 && <div className="muted small">{meta.join(" · ")}</div>}
                {r.note && <div className="muted small">{r.note}</div>}
              </div>
            );
          })}
        </Section>
      )}
      <SavedQueries />
      <Tasks />
      {er && (
        <div className="modal" onClick={() => setER(false)}>
          <div className="modalbox wide" onClick={(e) => e.stopPropagation()}>
            <ERDiagram onChange={reloadRels} />
            <div className="row end"><button onClick={() => setER(false)}>{L("关闭", "Close")}</button></div>
          </div>
        </div>
      )}
    </div>
  );
}
