import { useEffect, useState } from "react";
import { FolderOpen } from "lucide-react";
import { L } from "../../i18n";
import { ask, pickFile, pickFolder, reveal, saveDialog } from "../../native";
import { useApp } from "../../store";
import { core } from "../../transport";
import { IconBtn } from "../ui";
import { patch } from "./common";

const size = (n: number) => (n >= 1e9 ? `${(n / 1e9).toFixed(1)} GB` : n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1e3))} KB`);
const when = (t: number) => new Date(t * 1000).toLocaleString();
const KIND: Record<string, [string, string]> = { auto: ["自动", "auto"], manual: ["手动", "manual"], "pre-restore": ["恢复前", "pre-restore"] };
const SECRET: Record<string, [string, string]> = {
  api_key: ["API Key", "API key"], db_password: ["数据库密码", "database password"], ssh_secret: ["SSH 密码 / 私钥口令", "SSH password / passphrase"],
  header: ["请求头", "header"], mcp_env: ["MCP 环境变量", "MCP env"],
};

function secretsText(list: any[] | undefined): string {
  if (!list?.length) return "";
  return list.map((s) => `${L(...(SECRET[s.kind] ?? [s.kind, s.kind]))}：${s.label || s.id}${s.kind === "header" || s.kind === "mcp_env" ? ` (${s.id})` : ""}`).join("；");
}

/** After a restore the open chats / kernels in the UI are stale: forget them and reload the lists. */
async function refreshAfterRestore() {
  useApp.setState({ chats: {}, open: {}, selected: undefined });
  await useApp.getState().reload().catch(() => {});
  await useApp.getState().loadConfig().catch(() => {});
}

/** Settings → General → 备份与恢复 / Backup & restore. */
export function BackupSection() {
  const auto = useApp((x) => x.config?.settings?.autoBackup) ?? true;
  const projects = useApp((x) => x.projects);
  const [list, setList] = useState<any>();
  const [exp, setExp] = useState<Record<string, { on: boolean; files: boolean }> | null>(null);
  const [imp, setImp] = useState<{ path: string; pv: any; mode: "merge" | "replace" } | null>(null);
  const [result, setResult] = useState<any>();
  const [busy, setBusy] = useState("");
  const [msg, setMsg] = useState("");
  const load = () => core.request("backup/list").then(setList).catch(() => {});
  useEffect(() => { load(); }, []);

  const startExport = () => setExp(Object.fromEntries(projects.map((p) => [p.id, { on: true, files: false }])));
  const doExport = async () => {
    const path = await saveDialog(list?.default_name ?? "DataPilot-备份.zip", "zip");
    if (!path) return;
    const sel = Object.entries(exp ?? {}).filter(([, v]) => v.on).map(([id, v]) => ({ id, files: v.files }));
    setExp(null); setBusy(L("正在备份…", "Backing up…")); setMsg("");
    try {
      const r: any = await core.request("backup/export", { path, projects: sel });
      setMsg(L(`已备份：${r.path}（${size(r.size)}，${r.files} 个文件）`, `Backed up: ${r.path} (${size(r.size)}, ${r.files} files)`));
      reveal(r.path); load();
    } catch (e: any) { setMsg(L(`备份失败：${e?.message ?? e}`, `Backup failed: ${e?.message ?? e}`)); }
    setBusy("");
  };

  const startImport = async (p?: string) => {
    const path = p ?? (await pickFile(list?.dir));
    if (!path) return;
    setBusy(L("正在校验备份…", "Checking backup…")); setMsg(""); setResult(undefined);
    try { const pv = await core.request("backup/preview", { path }); setImp({ path, pv, mode: "merge" }); }
    catch (e: any) { setMsg(L(`无法恢复：${e?.message ?? e}`, `Cannot restore: ${e?.message ?? e}`)); }
    setBusy("");
  };
  const doImport = async () => {
    if (!imp) return;
    if (imp.mode === "replace" && !(await ask(L("「替换」会用备份覆盖本机的对话、记忆、设置和所选项目数据（会先自动备份当前数据到 backups/）。继续？",
      "Replace overwrites this computer's chats, memory, settings and the projects' data with the backup (the current data is backed up to backups/ first). Continue?")))) return;
    const { path, mode } = imp;
    setImp(null); setBusy(L("正在恢复…（会先停止正在运行的数据内核）", "Restoring… (data kernels are stopped first)"));
    try {
      const r: any = await core.request("backup/import", { path, mode });
      setResult({ ...r, path });
      await refreshAfterRestore(); load();
    } catch (e: any) { setMsg(L(`恢复失败：${e?.message ?? e}`, `Restore failed: ${e?.message ?? e}`)); }
    setBusy("");
  };
  /** A project folder of the backup does not exist here: pick where to put it (after trusting that folder). */
  const relink = async (p: any) => {
    const dir = await pickFolder();
    if (!dir || !result) return;
    if (!(await ask(L(`要信任这个文件夹吗？\n${dir}\n\n项目「${p.name}」会恢复到这里：DataPilot 会读取其中的文件，并在里面创建 .datapilot/ 和 outputs/。只选择你自己的、内容可信的文件夹。`,
      `Trust this folder?\n${dir}\n\nProject "${p.name}" will be restored here: DataPilot reads its files and creates .datapilot/ and outputs/ inside it. Only pick folders you own and trust.`)))) return;
    setBusy(L("正在恢复项目…", "Restoring project…"));
    try {
      const r: any = await core.request("backup/import", { path: result.path, mode: "merge", projects_only: true, relink: { [p.id]: dir } });
      setResult({ ...result, needs_relink: result.needs_relink.filter((x: any) => x.id !== p.id),
                  projects_restored: [...(result.projects_restored ?? []), ...(r.projects_restored ?? [])] });
      await refreshAfterRestore();
    } catch (e: any) { setMsg(L(`恢复项目失败：${e?.message ?? e}`, `Project restore failed: ${e?.message ?? e}`)); }
    setBusy("");
  };

  const backups: any[] = list?.backups ?? [];
  return (
    <section>
      <h4>{L("备份与恢复", "Backup & restore")}</h4>
      <div className="muted small">{L("备份对话、记忆、技能、设置和项目的 .datapilot/ 数据为一个 zip，可在另一台电脑上恢复。API Key 和密码只存在钥匙串 / 凭据管理器里，不会写进备份，恢复后需要重新填写。",
        "Back up chats, memory, skills, settings and projects' .datapilot/ data as one zip, to restore on another computer. API keys and passwords stay in the keychain / credential manager and are never in the backup: enter them again after restoring.")}</div>
      <div className="row small" style={{ gap: 8, marginTop: 6 }}>
        <button disabled={!!busy} onClick={startExport}>{L("导出备份…", "Export backup…")}</button>
        <button disabled={!!busy} onClick={() => startImport()}>{L("从备份恢复…", "Restore from backup…")}</button>
        {list?.dir && <IconBtn icon={FolderOpen} size={13} title={L("打开备份文件夹", "Open backup folder")} onClick={() => reveal(list.dir)} />}
      </div>
      <label className="small row"><input type="checkbox" checked={auto} onChange={(e) => patch({ settings: { autoBackup: e.target.checked } })} />
        {L("每周自动备份到 backups/（保留最近 4 份，不含项目数据）", "Weekly automatic backup to backups/ (keeps 4, without project data)")}</label>
      {busy && <div className="small">{busy}</div>}
      {msg && <div className="small" style={{ whiteSpace: "pre-wrap" }}>{msg}</div>}
      {result && <RestoreReport r={result} onRelink={relink} onClose={() => setResult(undefined)} />}
      {backups.length > 0 && (
        <details className="small"><summary>{L(`本机备份（${backups.length}）`, `Backups on this computer (${backups.length})`)}</summary>
          {backups.map((b) => (
            <div key={b.path} className="row" style={{ gap: 8 }}>
              <span className="grow ell" title={b.path}>{b.name}</span>
              <span className="muted">{L(...(KIND[b.kind] ?? [b.kind, b.kind]))} · {size(b.size)} · {when(b.mtime)}</span>
              <button disabled={!!busy} onClick={() => startImport(b.path)}>{L("恢复", "Restore")}</button>
            </div>
          ))}
        </details>
      )}

      {exp && (
        <div className="modal" onClick={() => setExp(null)}>
          <div className="modalbox trustbox" role="dialog" aria-modal="true" onClick={(e) => e.stopPropagation()}>
            <h3>{L("导出备份", "Export backup")}</h3>
            <p className="muted small">{L("总是包含：对话、记忆、技能、DATAPILOT.md、设置（不含 Key / 密码）。不包含：浏览器配置、日志、缓存。", "Always included: chats, memory, skills, DATAPILOT.md, settings (no keys / passwords). Never: browser profile, logs, caches.")}</p>
            {projects.length > 0 ? <>
              <p className="small">{L("项目（.datapilot/：项目对话、导入的数据、项目记忆）", "Projects (.datapilot/: project chats, imported data, project memory)")}</p>
              {projects.map((p) => (
                <div key={p.id} className="row small" style={{ gap: 12 }}>
                  <label className="row grow ell"><input type="checkbox" checked={exp[p.id]?.on ?? false} onChange={(e) => setExp({ ...exp, [p.id]: { ...exp[p.id], on: e.target.checked } })} /> <b>{p.name}</b> <span className="muted ell" title={p.path}>{p.path}</span></label>
                  <label className="row muted"><input type="checkbox" disabled={!exp[p.id]?.on} checked={exp[p.id]?.files ?? false} onChange={(e) => setExp({ ...exp, [p.id]: { ...exp[p.id], files: e.target.checked } })} /> {L("含项目原始文件", "include project files")}</label>
                </div>
              ))}
            </> : <p className="muted small">{L("还没有项目。", "No projects yet.")}</p>}
            <div className="row" style={{ justifyContent: "flex-end", gap: 8 }}>
              <button onClick={() => setExp(null)}>{L("取消", "Cancel")}</button>
              <button className="primary" autoFocus onClick={doExport}>{L("选择位置并导出", "Choose location & export")}</button>
            </div>
          </div>
        </div>
      )}

      {imp && (
        <div className="modal" onClick={() => setImp(null)}>
          <div className="modalbox trustbox" role="dialog" aria-modal="true" onClick={(e) => e.stopPropagation()}>
            <h3>{L("从备份恢复", "Restore from backup")}</h3>
            <p className="small">✅ {L("备份完整（校验通过）", "Backup verified")} · DataPilot {imp.pv.version} · {when(imp.pv.created)} · {size(imp.pv.bytes)}</p>
            <ul className="small">
              <li>{L(`对话 ${imp.pv.chats} 个（本机还没有的 ${imp.pv.chats_new} 个）`, `${imp.pv.chats} chats (${imp.pv.chats_new} not on this computer)`)}</li>
              <li>{L(`记忆 ${imp.pv.memory} 条 · 技能 ${imp.pv.skills} 个`, `${imp.pv.memory} memory entries · ${imp.pv.skills} skills`)}</li>
              {(imp.pv.projects ?? []).map((p: any) => (
                <li key={p.id}>{L("项目", "Project")} <b>{p.name}</b>{p.files ? L("（含原始文件）", " (with files)") : ""} — {p.exists ? <span className="muted">{p.path}</span> : <span className="err">{L("本机没有这个文件夹，恢复后可选择新位置", "folder not on this computer; choose a new location after restoring")}</span>}</li>
              ))}
            </ul>
            <label className="small row"><input type="radio" checked={imp.mode === "merge"} onChange={() => setImp({ ...imp, mode: "merge" })} />
              <span><b>{L("合并（推荐）", "Merge (recommended)")}</b> — {L("添加本机没有的对话；记忆和技能以较新的为准；本机设置不变，只补上缺少的模型服务", "add missing chats; newer memory / skills win; local settings kept, missing model services added")}</span></label>
            <label className="small row"><input type="radio" checked={imp.mode === "replace"} onChange={() => setImp({ ...imp, mode: "replace" })} />
              <span><b>{L("替换", "Replace")}</b> — {L("用备份覆盖本机数据（先自动备份当前数据到 backups/）", "overwrite this computer's data with the backup (current data is backed up to backups/ first)")}</span></label>
            {imp.pv.secrets_needed?.length > 0 && <p className="muted small">🔑 {L("恢复后需要重新填写：", "To enter again after restoring: ")}{secretsText(imp.pv.secrets_needed)}</p>}
            <p className="muted small">{L("恢复时会停止正在运行的数据内核；有任务正在运行时请先等它完成。", "Running data kernels are stopped; wait for running tasks to finish first.")}</p>
            <div className="row" style={{ justifyContent: "flex-end", gap: 8 }}>
              <button onClick={() => setImp(null)}>{L("取消", "Cancel")}</button>
              <button className="primary" autoFocus onClick={doImport}>{L("开始恢复", "Restore")}</button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

function RestoreReport({ r, onRelink, onClose }: { r: any; onRelink: (p: any) => void; onClose: () => void }) {
  return (
    <div className="small" style={{ border: "1px solid var(--line)", borderRadius: 8, padding: "8px 10px", marginTop: 6 }}>
      <div className="row"><b className="grow">✅ {L("恢复完成", "Restore finished")}</b><button onClick={onClose}>{L("关闭", "Close")}</button></div>
      <div>{r.replaced ? L(`已替换为备份数据（共 ${r.chats_added} 个对话）`, `Replaced with the backup (${r.chats_added} chats)`) : L(`新增对话 ${r.chats_added} 个`, `${r.chats_added} chats added`)}
        {r.memory_added != null && L(`，记忆新增 ${r.memory_added} / 更新 ${r.memory_updated}`, `, memory +${r.memory_added} / updated ${r.memory_updated}`)}
        {r.skills_changed ? L(`，技能 ${r.skills_changed} 个文件`, `, ${r.skills_changed} skill files`) : ""}
        {r.providers_added?.length ? L(`，新增模型服务：${r.providers_added.join("、")}`, `, model services added: ${r.providers_added.join(", ")}`) : ""}</div>
      {r.projects_restored?.length > 0 && <div>{L("已恢复项目：", "Projects restored: ")}{r.projects_restored.map((p: any) => p.name).join("、")}</div>}
      {r.kept_local?.length > 0 && <div className="muted">{L("保留了本机的：", "Kept local: ")}{r.kept_local.join("、")}</div>}
      {r.pre_backup && <div className="muted">{L("恢复前的数据已备份到：", "Previous data backed up to: ")}<span>{r.pre_backup}</span></div>}
      {r.needs_relink?.map((p: any) => (
        <div key={p.id} className="row" style={{ gap: 8 }}>
          <span className="grow">⚠️ {L(`项目「${p.name}」的文件夹在本机不存在（${p.path}）`, `Project "${p.name}": folder not found (${p.path})`)}</span>
          <button onClick={() => onRelink(p)}>{L("选择新位置…", "Choose location…")}</button>
        </div>
      ))}
      {r.secrets_needed?.length > 0 && <div>🔑 {L("请在 设置 → 模型服务 / 数据库连接 中重新填写：", "Enter again in Settings → Providers / database connections: ")}{secretsText(r.secrets_needed)}</div>}
    </div>
  );
}
