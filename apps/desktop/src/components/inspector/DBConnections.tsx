import { IconBtn } from "../ui";
import { Database, LoaderCircle, Plus } from "lucide-react";
import { ic, Status } from "../ui";
import { ask } from "../../native";
import { useState } from "react";
import { L } from "../../i18n";
import { core } from "../../transport";
import { useApp } from "../../store";
import { pickFile } from "../../native";
import { kcall, Menu, Section, sid, useLoad } from "./common";

type Field = { key: string; label: string; kind: string; required?: boolean; default?: string; group?: string };
type DBType = { type: string; label: string; default_port?: number; note?: string; fields: Field[] };

interface Form { id?: string; type: string; values: Record<string, string>; flags: Record<string, boolean>; live: boolean;
  password: string; passwordChanged: boolean; sshSecret: string; sshSecretChanged: boolean }

const blank = (type: string): Form => ({ type, values: {}, flags: {}, live: true, password: "", passwordChanged: false, sshSecret: "", sshSecretChanged: false });

function fromConn(c: any, t?: DBType): Form {
  const f = blank(c.type);
  f.id = c.id; f.live = c.live ?? true;
  for (const fd of t?.fields ?? []) {
    if (fd.kind === "bool") f.flags[fd.key] = !!c[fd.key];
    else if (fd.kind !== "secret" && c[fd.key] != null) f.values[fd.key] = String(c[fd.key]);
  }
  if (c.name) f.values.name = c.name;
  return f;
}

/** Connection JSON for the kernel (secrets never included; same shape as the SwiftUI form). */
function connJSON(f: Form, t?: DBType) {
  const c: Record<string, unknown> = { type: f.type, live: f.live };
  if (f.id) c.id = f.id;
  const ssh = !!f.flags.ssh;
  for (const [k, v] of Object.entries(f.values)) {
    if (!v.trim() || (k.startsWith("ssh_") && !ssh)) continue;
    c[k] = (k === "port" || k === "ssh_port") && /^\d+$/.test(v.trim()) ? Number(v.trim()) : v.trim();
  }
  for (const fd of t?.fields ?? []) if (fd.kind === "bool") c[fd.key] = !!f.flags[fd.key];
  return c;
}

function DBForm({ types, initial, onDone }: { types: DBType[]; initial: Form; onDone: (saved: boolean) => void }) {
  const projectRoot = useApp((s) => s.sessions.find((x) => x.id === s.selected)?.project_root);
  const [f, setF] = useState(initial);
  const [msg, setMsg] = useState<string>();
  const [busy, setBusy] = useState(false);
  const t = types.find((x) => x.type === f.type);
  const ssh = !!f.flags.ssh;
  const set = (p: Partial<Form>) => setF((x) => ({ ...x, ...p }));
  const run = async (testOnly: boolean) => {
    setBusy(true); setMsg(undefined);
    try {
      const isFile = ["sqlite", "duckdb"].includes(f.type), path = f.values.path?.trim() ?? "";
      if (isFile && path.startsWith("/") && (!projectRoot || !path.startsWith(projectRoot))) {
        setMsg("❌ " + L("数据库文件需要放在项目文件夹内（内核沙箱只能读取项目文件夹）。", "The database file must be inside the project folder (the kernel sandbox can only read the project folder)."));
        return;
      }
      if (testOnly) {
        // Test needs the port open in the sandbox: a new network port must be saved first (the kernel restarts to allow it).
        const port = Number(f.values.port?.trim() || t?.default_port || 0);
        if (!isFile && !ssh && port) {
          const k = await core.request("kernel/status", { session: sid() }).catch(() => null);
          if (k?.running && !k.network && !(k.launched_ports ?? []).includes(port)) {
            setMsg(L("新端口需要先保存连接（内核会重启以放行该端口），然后在列表里「测试连接」。", "A new port must be saved first (the kernel restarts to allow it); then use “Test” in the list."));
            return;
          }
        }
        const p: any = { conn: connJSON(f, t), timeout: 15 };
        if (f.passwordChanged && f.password) p.password = f.password;
        else if (f.id) p.connection = f.id;   // saved password is filled in by the core
        if (ssh && f.sshSecretChanged && f.sshSecret) p.ssh_secret = f.sshSecret;
        const r = await kcall("db_test", p, 60);
        setMsg("✅ " + L("连接成功", "Connected") + ` · ${r.version ?? ""} · ${r.tables ?? 0} ` + L("张表", "tables") + ` · ${r.ms ?? 0} ms`);
        return;
      }
      const r = await kcall("db_save", { conn: connJSON(f, t) }, 30);
      const id = r.connection?.id;
      if (!id) return;
      if (f.passwordChanged || !f.id) await core.request("secret/set", { kind: "db_password", id, value: f.password || null });
      if (!ssh) await core.request("secret/set", { kind: "ssh_secret", id, value: null });
      else if (f.sshSecretChanged || !f.id) await core.request("secret/set", { kind: "ssh_secret", id, value: f.sshSecret || null });
      onDone(true);
    } catch (e) { setMsg("❌ " + String((e as Error).message).split("\ntraceback")[0]); }
    finally { setBusy(false); }
  };
  const field = (fd: Field) => {
    if (fd.group === "ssh" && !ssh) return null;
    if (fd.kind === "bool") return (
      <label key={fd.key} className="small row"><input type="checkbox" checked={!!f.flags[fd.key]} onChange={(e) => set({ flags: { ...f.flags, [fd.key]: e.target.checked } })} /> {fd.label}</label>
    );
    if (fd.kind === "secret") {
      const isSSH = fd.key === "ssh_secret";
      return (
        <label key={fd.key} className="fld"><span>{fd.label}</span>
          <input type="password" autoComplete="off" placeholder={f.id ? L("已保存（留空不修改）", "saved (leave empty to keep)") : ""}
            value={isSSH ? f.sshSecret : f.password}
            onChange={(e) => set(isSSH ? { sshSecret: e.target.value, sshSecretChanged: true } : { password: e.target.value, passwordChanged: true })} />
        </label>
      );
    }
    const picker = fd.kind === "file" || fd.kind === "path";
    return (
      <label key={fd.key} className="fld"><span>{fd.label}{fd.required ? " *" : ""}</span>
        <span className="row">
          <input value={f.values[fd.key] ?? ""} placeholder={fd.default ?? (fd.key === "port" && t?.default_port ? String(t.default_port) : "")}
            onChange={(e) => set({ values: { ...f.values, [fd.key]: e.target.value } })} />
          {picker && <button onClick={async () => { const p = await pickFile(fd.kind === "path" ? "~/.ssh" : undefined); if (p) set({ values: { ...f.values, [fd.key]: p } }); }}>…</button>}
        </span>
      </label>
    );
  };
  return (
    <div className="modal" onClick={() => onDone(false)}>
      <div className="modalbox" onClick={(e) => e.stopPropagation()}>
        <h3>{f.id ? L("编辑数据库连接", "Edit connection") : L("新建数据库连接", "New connection")}</h3>
        <label className="fld"><span>{L("类型", "Type")}</span>
          <select value={f.type} disabled={!!f.id} onChange={(e) => set({ type: e.target.value })}>{types.map((x) => <option key={x.type} value={x.type}>{x.label}</option>)}</select>
        </label>
        {t?.note && <div className="muted small">{t.note}</div>}
        <label className="fld"><span>{L("名称", "Name")}</span><input placeholder={L("例如：销售库（只读）", "e.g. Sales (read-only)")} value={f.values.name ?? ""} onChange={(e) => set({ values: { ...f.values, name: e.target.value } })} /></label>
        {t?.fields.map(field)}
        <label className="small row"><input type="checkbox" checked={f.live} onChange={(e) => set({ live: e.target.checked })} /> {L("允许实时查询（db_query）", "Allow live queries (db_query)")}</label>
        <div className="muted small">{L("密码只保存在系统钥匙串 / 凭据管理器，不会写入文件，也不会发给模型。", "Passwords are kept in the system keychain only — never in files or sent to the model.")}</div>
        <Status msg={msg} />
        <div className="row end">
          <button disabled={busy} onClick={() => run(true)}>{L("测试连接", "Test")}</button>
          <button onClick={() => onDone(false)}>{L("取消", "Cancel")}</button>
          <button className="primary" disabled={busy} onClick={() => run(false)}>{L("保存", "Save")}</button>
        </div>
      </div>
    </div>
  );
}

function Extract({ conn, onDone }: { conn: any; onDone: (msg?: string) => void }) {
  const [tables, , , busy] = useLoad(async () => ((await kcall("db_schema", { connection: conn.id, columns: false }, 120)).tables ?? []) as any[]);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [running, setRunning] = useState(false);
  const [err, setErr] = useState<string>();
  const [filter, setFilter] = useState("");
  const go = async () => {
    setRunning(true);
    try {
      const r = await kcall("db_extract", { connection: conn.id, tables: [...picked].sort() }, 3600);
      const rels = (r.relations ?? []).length;
      onDone((r.tables ?? []).map((t: any) => `${t.as}：${t.rows} ` + L("行", "rows")).join(L("；", "; ")) + (rels > 0 ? L(`；登记外键关系 ${rels} 条`, `; ${rels} FK relations`) : ""));
    } catch (e) { setErr((e as Error).message.split("\ntraceback")[0]); setRunning(false); }
  };
  const name = (t: any) => (t.schema ? `${t.schema}.${t.table}` : t.table);
  const shown = (tables ?? []).filter((t) => !filter || name(t).toLowerCase().includes(filter.toLowerCase()));
  return (
    <div className="modal" onClick={() => onDone()}>
      <div className="modalbox" onClick={(e) => e.stopPropagation()}>
        <h3>{L("抽取到本地：", "Extract: ")}{conn.name}</h3>
        <div className="muted small">{L("抽取的是明细数据（不聚合），保存为本地表；两表间的外键会自动登记为已确认关系。之后可在列表中「刷新」重新拉取。",
          "Full detail rows are copied into local tables; foreign keys between them become confirmed relations. Use “Refresh” later to re-pull.")}</div>
        <input placeholder={L("筛选表名", "Filter")} value={filter} onChange={(e) => setFilter(e.target.value)} />
        <div className="picklist">
          {busy && <LoaderCircle {...ic} size={14} className="spin muted" />}
          {shown.map((t) => (
            <label key={name(t)} className="row small"><input type="checkbox" checked={picked.has(name(t))}
              onChange={(e) => { const s = new Set(picked); e.target.checked ? s.add(name(t)) : s.delete(name(t)); setPicked(s); }} /> {name(t)} <span className="muted">{t.type}</span></label>
          ))}
        </div>
        {err && <div className="notice err small">{err}</div>}
        <div className="row end">
          {running && <span className="grow row muted small"><LoaderCircle {...ic} size={13} className="spin" />{L("正在抽取…（进度见「内核任务」）", "Extracting… (see kernel tasks)")}</span>}
          <button disabled={running} onClick={() => setPicked(new Set(shown.map(name)))}>{L("全选", "All")}</button>
          <button onClick={() => onDone()}>{L("取消", "Cancel")}</button>
          <button className="primary" disabled={!picked.size || running} onClick={go}>{L(`抽取 ${picked.size} 张表`, `Extract ${picked.size}`)}</button>
        </div>
      </div>
    </div>
  );
}

export function DBConnections({ onChange }: { onChange: () => void }) {
  const [types] = useLoad(async () => ((await kcall("db_types", {}, 90)).types ?? []) as DBType[]);
  const [conns, , reload] = useLoad(async () => ((await kcall("db_connections", {}, 30)).connections ?? []) as any[]);
  const [editing, setEditing] = useState<Form>();
  const [extracting, setExtracting] = useState<any>();
  const [status, setStatus] = useState<Record<string, string>>({});
  const st = (id: string, s: string) => setStatus((x) => ({ ...x, [id]: s }));
  const act = async (id: string, f: () => Promise<string>) => { st(id, "…"); try { st(id, await f()); } catch (e) { st(id, "❌ " + (e as Error).message.split("\ntraceback")[0]); } };
  const tdef = (t: string) => types?.find((x) => x.type === t);
  return (
    <Section title={L("数据库连接", "Databases")} open={!!conns?.length} right={<IconBtn icon={Plus} size={13} title={L("添加数据库连接", "Add a database connection")} disabled={!types?.length} onClick={() => setEditing(blank("postgres"))} />}>
      {!conns?.length && <div className="muted small">{L("可直连 PostgreSQL / MySQL / SQL Server / Oracle / ClickHouse / SQLite / DuckDB，只读分析", "Connect PostgreSQL / MySQL / SQL Server / Oracle / ClickHouse / SQLite / DuckDB (read-only)")}</div>}
      {(conns ?? []).map((c) => (
        <div key={c.id} className="iconn">
          <div className="irow">
            <span className="grow row"><Database {...ic} size={13} className="muted" />{c.name} <span className="muted small">{tdef(c.type)?.label ?? c.type}{c.ssh ? " · SSH" : ""}</span></span>
            <Menu label="⋯" items={[
              [L("测试连接", "Test"), () => act(c.id, async () => { const r = await kcall("db_test", { connection: c.id, timeout: 15 }, 60); return "✅ " + L("连接成功", "Connected") + ` · ${r.version ?? ""} · ${r.tables ?? 0} ` + L("张表", "tables") + ` · ${r.ms ?? 0} ms`; })],
              [L("选择表抽取到本地…", "Extract tables…"), () => setExtracting(c)],
              ...(c.extracts?.length ? [[L("刷新已抽取的表", "Refresh extracted tables"), () => act(c.id, async () => { await kcall("db_refresh", { connection: c.id }, 3600); onChange(); return "✅ " + L("已刷新", "Refreshed"); })] as [string, () => void]] : []),
              [L("编辑…", "Edit…"), () => setEditing(fromConn(c, tdef(c.type)))],
              [L("删除（已抽取的本地表保留）", "Delete (extracted tables are kept)"), async () => {
                if (!(await ask(L(`删除连接「${c.name}」？`, `Delete “${c.name}”?`)))) return;
                await kcall("db_delete", { connection: c.id }, 30).catch(() => {});
                await core.request("secret/set", { kind: "db_password", id: c.id, value: null }).catch(() => {});
                await core.request("secret/set", { kind: "ssh_secret", id: c.id, value: null }).catch(() => {});
                reload();
              }],
            ]} />
          </div>
          <div className="muted small mono">{c.path ?? `${c.user ?? ""}@${c.host ?? ""}:${c.port_effective ?? ""}/${c.database ?? ""}`}</div>
          {c.extracts?.length > 0 && <div className="muted small">{L("已抽取：", "Extracted: ")}{c.extracts.map((e: any) => e.as).join(", ")}</div>}
          <Status msg={status[c.id]} />
        </div>
      ))}
      {editing && types && <DBForm types={types} initial={editing} onDone={(saved) => { setEditing(undefined); if (saved) { reload(); onChange(); } }} />}
      {extracting && <Extract conn={extracting} onDone={(m) => { const c = extracting; setExtracting(undefined); if (m) { st(c.id, "✅ " + m); reload(); onChange(); } }} />}
    </Section>
  );
}
