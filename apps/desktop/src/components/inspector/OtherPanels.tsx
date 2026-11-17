import { IconBtn } from "../ui";
import { FileText, FolderOpen, RefreshCw, Sparkles, Table2 } from "lucide-react";
import { TodoIcon } from "../Chat";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { core } from "../../transport";
import { openPath, reveal } from "../../native";
import { Menu, sid, useLoad } from "./common";
import { useState } from "react";

export function TodoPanel() {
  const todos = useApp((s) => (s.selected ? s.chats[s.selected]?.todos : undefined)) ?? [];
  if (!todos.length) return <div className="muted small pad">{L("Agent 处理多步任务时会在这里列出进度。", "Multi-step tasks show their progress here.")}</div>;
  const done = todos.filter((t: any) => t.status === "completed").length;
  return (
    <div className="ipanel">
      <div className="muted small pad">{done}/{todos.length} {L("完成", "done")}</div>
      {todos.map((t: any) => (
        <div key={t.id} className={"irow" + (t.status === "completed" ? " muted" : "")}>
          <TodoIcon status={t.status} /> <span className="grow">{t.content}</span>
        </div>
      ))}
    </div>
  );
}

const DATA_EXT = /\.(csv|tsv|xlsx|xls|parquet|json|txt)$/i;

export function FilesPanel() {
  const send = useApp((s) => s.send);
  const running = useApp((s) => (s.selected ? !!s.chats[s.selected]?.running : false));
  const [f, , reload] = useLoad(async () => core.request("files/list", { session: sid() }));
  const [filter, setFilter] = useState("");
  const analyze = (rel: string) => send(L(`请导入项目文件 ${rel} 并概览这份数据（行数、字段含义、数据质量、可以做的分析）。`, `Import the project file ${rel} and give an overview (rows, field meanings, data quality, possible analyses).`));
  const row = (label: string, abs: string, rel?: string) => (
    <div key={abs} className="irow click" onDoubleClick={() => openPath(abs)} title={abs}>
      <span className="grow small">{DATA_EXT.test(label) ? <Table2 size={13} className="muted" /> : <FileText size={13} className="muted" />} {label}</span>
      {rel && DATA_EXT.test(rel) && <IconBtn icon={Sparkles} size={13} title={L("在对话中分析", "Analyze in chat")} disabled={running} onClick={() => analyze(rel)} />}
      <Menu label="⋯" items={[
        [L("打开", "Open"), () => openPath(abs)],
        [L("在访达 / 资源管理器中显示", "Reveal"), () => reveal(abs)],
        [L("复制路径", "Copy path"), () => navigator.clipboard?.writeText(rel ?? abs)],
      ]} />
    </div>
  );
  if (!f) return null;
  const root: string | null = f.project_root;
  const proj: string[] = (f.project ?? []).filter((p: string) => !filter || p.toLowerCase().includes(filter.toLowerCase()));
  const outputs = proj.filter((p) => p.startsWith("outputs/"));
  const data = proj.filter((p) => !p.startsWith("outputs/") && DATA_EXT.test(p));
  const other = proj.filter((p) => !p.startsWith("outputs/") && !DATA_EXT.test(p));
  const base = (p: string) => p.split(/[\\/]/).pop() ?? p;
  return (
    <div className="ipanel">
      <div className="row pad small">
        <span className="grow muted mono">{root ?? f.workspace_dir}</span>
        <IconBtn icon={FolderOpen} size={13} title={L("在访达 / 资源管理器中显示", "Reveal")}className="link" onClick={() => reveal(root ?? f.workspace_dir)} />
        <IconBtn icon={RefreshCw} size={13} title={L("刷新", "Refresh")} onClick={reload} />
      </div>
      <div className="muted xs pad">{root ? L("原始文件只读；Agent 的产出写入 outputs/；数据库与对话记录在 .datapilot/。", "Originals are read-only; outputs go to outputs/; DB and chats live in .datapilot/.")
        : L("提示：在侧栏“新建项目”选择一个文件夹，即可像 Codex 一样在项目里对话，文件与产出都保存在该文件夹。", "Tip: create a project (sidebar) to work inside a folder, like Codex.")}</div>
      {root && <div className="pad"><input className="wide" placeholder={L("筛选文件", "Filter files")} value={filter} onChange={(e) => setFilter(e.target.value)} /></div>}
      {root && (
        <>
          {data.length > 0 && <div className="section"><span>{L("数据文件", "Data files")}</span></div>}
          {data.map((p) => row(p, `${root}/${p}`, p))}
          {outputs.length > 0 && <div className="section"><span>outputs/</span></div>}
          {outputs.map((p) => row(p.slice(8), `${root}/${p}`, p))}
          {other.length > 0 && <div className="section"><span>{L("其他文件", "Other files")}</span></div>}
          {other.slice(0, 300).map((p) => row(p, `${root}/${p}`, p))}
        </>
      )}
      <div className="section"><span>{L("工作区", "Workspace")}</span></div>
      {(f.workspace ?? []).map((p: string) => row(p.slice((f.workspace_dir?.length ?? 0) + 1) || base(p), p, root ? undefined : p.slice((f.workspace_dir?.length ?? 0) + 1)))}
      {!(f.workspace ?? []).length && !(f.project ?? []).length && <div className="muted small pad">{L("还没有文件", "No files yet")}</div>}
    </div>
  );
}

export function BrowserPanel() {
  const last = useApp((s) => {
    const items = s.selected ? s.chats[s.selected]?.items ?? [] : [];
    for (let i = items.length - 1; i >= 0; i--) { const d = (items[i] as any).display; if ((items[i] as any).tool_name === "browser" && d?.screenshot) return d; }
    return undefined;
  });
  const [img] = useLoad(async () => (last ? (await core.request("files/image", { session: sid(), path: last.screenshot })).data_url as string : undefined), [last?.screenshot, last?.ts]);
  if (!last) return <div className="muted small pad">{L("Agent 使用 browser 工具后，这里会显示页面截图。", "Page screenshots appear here after the agent uses the browser tool.")}</div>;
  return (
    <div className="ipanel">
      <div className="row pad small">
        <span className="grow mono">{last.url}</span>
        {last.url && <a href={last.url} target="_blank" rel="noreferrer noopener">{L("在浏览器中打开", "Open in browser")}</a>}
      </div>
      <div className="muted small pad">{L("Agent 在后台浏览器中操作；这里显示每一步后的页面截图。需要登录时请在浏览器中打开。", "The agent works in a background browser; this shows the page after each step.")}</div>
      {img && <img className="shot" src={img} alt="browser" />}
    </div>
  );
}
