import { BookOpen, Brain, ChartColumn, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, CircleCheck, CirclePause, CircleX, Code, Database, Download, Ellipsis, FileSearch, FileText, Folder, Globe, LayoutDashboard, ListChecks, ListTree, LoaderCircle, Pin, Save, Search, SquareFunction, Table2, Terminal, ThumbsUp, Upload, Users, Wrench, X, type LucideIcon } from "lucide-react";
import { IconBtn, MenuItem, Popover, ic } from "./ui";
import { useEffect, useState } from "react";
import { L } from "../i18n";
import { DataTable } from "./DataTable";
import { ChartCard } from "./ChartCard";
import { Markdown } from "./Markdown";
import { explainSQL, exportResult, formatSQL, kernel, pinChart, planSQL, thumbsUp } from "../actions";
import { useApp } from "../store";
import { core } from "../transport";
import { openPath, reveal } from "../native";
import { openExternal } from "./links";
import { OcrInfo } from "./PageCompare";
import "./cards.css";

// same icons as the SwiftUI ToolCallCard
const icons: Record<string, LucideIcon> = { sql_query: Table2, python: Code, data_import: Download, data_schema: ListTree, chart_create: ChartColumn,
  dashboard_create: LayoutDashboard, dashboard_update: LayoutDashboard, memory_write: Brain, memory_search: Brain, memory_forget: Brain, web_fetch: Globe,
  web_search: Search, agent: Users, subagent: Users, bash: Terminal, todo_write: ListChecks, skill: BookOpen, data_transform: SquareFunction,
  data_export: Upload, db_list: Database, db_schema: Database, db_query: Database, db_extract: Database, db_refresh: Database, query_save: FileSearch, browser: Globe };

function SqlActions({ d }: { d: any }) {
  const [extra, setExtra] = useState<{ title: string; text: string; md?: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [voted, setVoted] = useState(false);
  const run = async (title: string, f: () => Promise<string>, md = false) => {
    setBusy(true);
    try { setExtra({ title, text: await f(), md }); } catch (e) { setExtra({ title, text: "⚠️ " + (e as Error).message }); }
    finally { setBusy(false); }
  };
  const total: number = d.row_count ?? d.rows?.length ?? 0;
  const [pager, setPager] = useState(false);
  const save = async () => {
    const name = window.prompt(L("保存查询", "Save query") + " — " + L("名称", "Name"), d.question ?? "")?.trim();
    if (!name) return;
    try {
      await kernel("queries", { action: "save", name, sql: d.sql, ...(d.connection ? { connection: d.connection } : {}) }, 30);
      useApp.getState().note(L("已保存到「我的查询」：", "Saved query: ") + name);
      useApp.setState((s) => ({ dataVersion: s.dataVersion + 1 }));
    } catch (e) { useApp.getState().fail(e); }
  };
  return (
    <>
      <div className="row wrap small actions">
        <span className="rowcount">{L(`共 ${total.toLocaleString()} 行`, `${total.toLocaleString()} rows`)}</span>
        {d.result_id && (
          <select defaultValue="" onChange={(e) => { if (e.target.value) exportResult(d.result_id, e.target.value); e.target.value = ""; }}>
            <option value="" disabled>{L("导出…", "Export…")}</option>
            <option value="csv">CSV</option><option value="xlsx">Excel (.xlsx)</option><option value="parquet">Parquet</option>
          </select>
        )}
        <button disabled={busy} onClick={() => run(L("口径解释", "Explanation"), () => explainSQL(d.sql, d.question ?? undefined), true)}>{L("解释", "Explain")}</button>
        <button disabled={busy} onClick={() => run(L("格式化后的 SQL", "Formatted SQL"), () => formatSQL(d.sql))}>{L("格式化", "Format")}</button>
        <button disabled={busy} onClick={() => run(L("执行计划", "Query plan"), () => planSQL(d.sql))}>{L("执行计划", "Plan")}</button>
        {d.question && <button disabled={voted} title={L("结果正确：记为可信示例", "Correct: save as verified example")} onClick={() => { setVoted(true); thumbsUp(d.question, d.sql); }}><ThumbsUp {...ic} size={13} /></button>}
        <Popover caret={false} title={L("更多", "More")} label={<><Ellipsis {...ic} size={13} />{L("更多", "More")}</>}>
          <MenuItem icon={Save} onClick={save}>{L("保存到「我的查询」…", "Save to queries…")}</MenuItem>
          {d.result_id && total > (d.rows?.length ?? 0) && <MenuItem icon={Table2} onClick={() => setPager(true)}>{L(`查看全部 ${total.toLocaleString()} 行…`, `View all ${total.toLocaleString()} rows…`)}</MenuItem>}
        </Popover>
        {busy && <span className="muted">…</span>}
      </div>
      {pager && <ResultPager resultId={d.result_id} close={() => setPager(false)} />}
      {extra && (
        <div className="extra">
          <div className="row"><b className="small">{extra.title}</b><IconBtn icon={X} size={12} title={L("关闭", "Close")} onClick={() => setExtra(null)} /></div>
          {extra.md ? <Markdown text={extra.text} /> : <pre className="code">{extra.text}</pre>}
        </div>
      )}
    </>
  );
}

/** Paged viewer for a saved result (artifacts/<id>.parquet) — the whole result, 500 rows a page (Swift ResultPagerView). */
function ResultPager({ resultId, close }: { resultId: string; close: () => void }) {
  const page = 500;
  const [q, setQ] = useState({ offset: 0, sort: "", desc: false });
  const [r, setR] = useState<{ columns: string[]; rows: unknown[][]; total: number }>({ columns: [], rows: [], total: 0 });
  const [err, setErr] = useState<string>();
  useEffect(() => {
    let live = true;
    kernel("result_page", { result_id: resultId, offset: q.offset, limit: page, ...(q.sort ? { sort: q.sort, desc: q.desc } : {}) }, 60)
      .then((v) => { if (live) { setR({ columns: v.columns ?? [], rows: v.rows ?? [], total: v.total ?? 0 }); setErr(undefined); } })
      .catch((e) => live && setErr((e as Error).message));
    return () => { live = false; };
  }, [resultId, q]);
  useEffect(() => { const k = (e: KeyboardEvent) => e.key === "Escape" && close(); document.addEventListener("keydown", k); return () => document.removeEventListener("keydown", k); }, []);
  return (
    <div className="modal" onClick={close}>
      <div className="modalbox wide pager" onClick={(e) => e.stopPropagation()}>
        <div className="row">
          <h3>{L("完整结果", "Full result")}</h3>
          <span className="muted small grow">{Math.min(q.offset + 1, r.total)}–{Math.min(q.offset + page, r.total)} / {r.total.toLocaleString()}</span>
          <select value={q.sort} title={L("排序", "Sort")} onChange={(e) => setQ({ ...q, sort: e.target.value, offset: 0 })}>
            <option value="">{L("原顺序", "Original")}</option>
            {r.columns.map((c) => <option key={c} value={c}>{c}</option>)}
          </select>
          <label className="row small"><input type="checkbox" checked={q.desc} disabled={!q.sort} onChange={(e) => setQ({ ...q, desc: e.target.checked, offset: 0 })} />{L("降序", "Desc")}</label>
          <IconBtn icon={ChevronLeft} title={L("上一页", "Previous")} disabled={q.offset === 0} onClick={() => setQ({ ...q, offset: Math.max(0, q.offset - page) })} />
          <IconBtn icon={ChevronRight} title={L("下一页", "Next")} disabled={q.offset + page >= r.total} onClick={() => setQ({ ...q, offset: q.offset + page })} />
        </div>
        {err && <div className="err small">{err}</div>}
        {r.columns.length > 0 && <DataTable columns={r.columns} rows={r.rows} max={page} />}
        <div className="row end"><button onClick={close}>{L("关闭", "Close")}</button></div>
      </div>
    </div>
  );
}

/** Python figure (workspace PNG) loaded through the core as a data URL. */
function PyImage({ path }: { path: string }) {
  const [src, setSrc] = useState<string>();
  useEffect(() => {
    const session = useApp.getState().selected;
    if (session) core.request("files/image", { session, path }).then((r) => setSrc(r.data_url)).catch(() => {});
  }, [path]);
  return src ? <img className="pyimg" src={src} alt={path.split(/[\\/]/).pop()} /> : null;
}

/** Clickable web_search results (Swift SearchResultsView). */
function SearchResults({ d }: { d: any }) {
  const items: any[] = d.results ?? [];
  const host = (u: string) => { try { return new URL(u).host; } catch { return ""; } };
  return (
    <div className="cbox">
      <div className="muted small">{(d.backend === "google" ? "Google" : "Bing") + " · " + L(`${items.length} 条结果`, `${items.length} results`)}</div>
      {items.slice(0, 8).map((r, i) => (
        <div key={i} className="sres">
          {r.url && <a href={r.url} title={r.url} onClick={(e) => { e.preventDefault(); openExternal(r.url); }}>{r.title || r.url}</a>}
          {r.url && <div className="muted small">{host(r.url) || r.site}</div>}
          {r.snippet && <div className="muted small clamp2">{r.snippet}</div>}
        </div>
      ))}
    </div>
  );
}

const fmtBytes = (b: number) => (b < 1000 ? `${b} bytes` : b < 1e6 ? `${Math.round(b / 1e3)} KB` : b < 1e9 ? `${(b / 1e6).toFixed(1)} MB` : `${(b / 1e9).toFixed(2)} GB`);

function FileOut({ d }: { d: any }) {
  const meta = [d.rows != null ? L(`${d.rows} 行`, `${d.rows} rows`) : "", d.bytes != null ? fmtBytes(d.bytes) : ""].filter(Boolean).join(" · ");
  return (
    <div className="cbox row">
      <FileText {...ic} size={16} className="muted" />
      <div className="grow"><div className="ell" title={d.path}>{String(d.path).split(/[\\/]/).pop()}</div>{meta && <div className="muted small">{meta}</div>}</div>
      <button className="small" onClick={() => openPath(d.path)}>{L("打开", "Open")}</button>
      <IconBtn icon={Folder} size={14} title={L("在 Finder 中显示", "Show in Finder")} onClick={() => reveal(d.path)} />
    </div>
  );
}

export function ToolCard({ it }: { it: any }) {
  const [open, setOpen] = useState(false);
  const d = it.display ?? {};
  const isChart = d.kind === "chart" && d.data;
  const cols: string[] = isChart ? [] : d.columns ?? d.data?.columns ?? d.table?.columns ?? [];
  const rows: unknown[][] = isChart ? [] : d.rows ?? d.data?.rows ?? d.table?.rows ?? [];
  const Icon = icons[it.tool_name] ?? Wrench;
  const code: string | undefined = d.code ?? (d.kind !== "sql" ? d.sql : undefined) ?? d.command;
  if (d.kind === "blocked")   // plan mode refused this tool: compact label only (Swift)
    return <div className="tool blocked" title={d.reason}><CirclePause {...ic} size={13} /><span className="tsum">{L("计划模式下暂不执行：", "Skipped in plan mode: ") + it.tool_name}</span></div>;
  const tools: string[] = d.kind === "subagent" ? [...new Set<string>(d.tools ?? [])] : [];
  const status = it.running ? <LoaderCircle {...ic} size={13} className="spin" /> : it.is_error ? <CircleX {...ic} size={14} className="errc" /> : <CircleCheck {...ic} size={14} className="ok" />;
  return (
    <div className={"tool" + (it.is_error ? " err" : "")}>
      <div className="thead" onClick={() => setOpen(!open)}>
        {status}<Icon {...ic} size={14} className="muted" />
        <span className="tsum">{it.summary || it.tool_name}</span>
        <span className="tstat">{it.progress ?? (it.ms ? (it.ms > 1000 ? `${(it.ms / 1000).toFixed(1)}s` : `${it.ms}ms`) : "")}{open ? <ChevronUp size={12} /> : <ChevronDown size={12} />}</span>
      </div>
      {d.kind === "sql" && d.sql && <pre className="code">{d.sql}</pre>}
      {d.kind === "transform" && d.purpose && cols.length > 0 && <div className="cap">{d.purpose}</div>}
      {cols.length > 0 && rows.length > 0 && <DataTable columns={cols} rows={rows} max={open ? 2000 : d.kind === "transform" ? 10 : 12} />}
      {d.kind === "transform" && cols.length > 0 && <div className="cap">{L(`派生表 ${d.target ?? ""} · 共 ${d.row_count ?? 0} 行（源文件更新后自动重建）`, `Derived table ${d.target ?? ""} · ${d.row_count ?? 0} rows (rebuilt when sources change)`)}</div>}
      {d.kind === "python" && (d.images ?? []).map((p: string) => <PyImage key={p} path={p} />)}
      {d.kind === "search" && <SearchResults d={d} />}
      {d.kind === "file" && d.path && <FileOut d={d} />}
      {d.ocr && <OcrInfo o={d.ocr} />}
      {d.kind === "subagent" && <div className="cap">{L(`${d.turns ?? 0} 轮 · ${(d.tools ?? []).length} 次工具调用`, `${d.turns ?? 0} turns · ${(d.tools ?? []).length} tool calls`) + (tools.length ? "：" + tools.join(", ") : "")}</div>}
      {d.kind === "dashboard" && d.id && (
        <button className="dashbtn small" onClick={() => useApp.getState().openInspector("dashboards", d.id)}><LayoutDashboard {...ic} size={13} />{L("打开看板：", "Open dashboard: ") + (d.title ?? "")}</button>
      )}
      {d.kind === "sql" && d.sql && !it.is_error && <SqlActions d={d} />}
      {isChart && (
        <>
          <ChartCard display={d} />
          {d.sql && <div className="row small actions"><button className="ibtn" onClick={() => pinChart(d)}><Pin {...ic} size={13} />{L("加入看板", "Add to dashboard")}</button></div>}
        </>
      )}
      {open && (
        <div className="tbody">
          {code && <pre className="code">{code}</pre>}
          {!cols.length && it.output && <pre className="out">{it.output}</pre>}
          {it.arguments && <details><summary>{L("参数", "Arguments")}</summary><pre className="out">{it.arguments}</pre></details>}
        </div>
      )}
    </div>
  );
}
