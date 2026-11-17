import { IconBtn } from "../ui";
import { CircleCheck, CircleX, Pencil, Plus, RefreshCw, X } from "lucide-react";
import { ask } from "../../native";
import { useEffect, useState } from "react";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { core } from "../../transport";
import { reveal } from "../../native";
import { patch, rid } from "./common";

function MCPEditor({ s, onSave, onClose }: { s: any; onSave: (s: any) => void; onClose: () => void }) {
  const [v, setV] = useState({ transport: "stdio", enabled: true, args: [], env: {}, ...s });
  const [args, setArgs] = useState((v.args ?? []).join(" "));
  const [env, setEnv] = useState(Object.entries(v.env ?? {}).map(([k, x]) => `${k}=${x}`).join("\n"));
  return (
    <div className="modal" onClick={onClose}>
      <div className="modalbox" onClick={(e) => e.stopPropagation()}>
        <h3>{s.name ? L("编辑 MCP 服务器", "Edit MCP server") : L("添加 MCP 服务器", "Add MCP server")}</h3>
        <label className="fld"><span>{L("名称", "Name")}</span><input value={v.name ?? ""} onChange={(e) => setV({ ...v, name: e.target.value })} /></label>
        <label className="fld"><span>{L("连接方式", "Transport")}</span>
          <select value={v.transport} onChange={(e) => setV({ ...v, transport: e.target.value })}><option value="stdio">stdio</option><option value="http">Streamable HTTP</option></select></label>
        {v.transport === "http" ? (
          <label className="fld"><span>URL</span><input value={v.url ?? ""} placeholder="https://…/mcp" onChange={(e) => setV({ ...v, url: e.target.value })} /></label>
        ) : (
          <>
            <label className="fld"><span>{L("命令", "Command")}</span><input value={v.command ?? ""} placeholder="npx" onChange={(e) => setV({ ...v, command: e.target.value })} /></label>
            <label className="fld"><span>{L("参数（空格分隔）", "Arguments (space separated)")}</span><input value={args} placeholder="-y @playwright/mcp@latest" onChange={(e) => setArgs(e.target.value)} /></label>
            <label className="fld"><span>{L("环境变量（每行 KEY=VALUE）", "Environment (KEY=VALUE per line)")}</span><textarea rows={3} value={env} onChange={(e) => setEnv(e.target.value)} /></label>
          </>
        )}
        <div className="row end">
          <button onClick={onClose}>{L("取消", "Cancel")}</button>
          <button className="primary" disabled={!v.name?.trim()} onClick={() => onSave({
            ...v, name: v.name.trim(), args: args.split(/\s+/).filter(Boolean),
            env: Object.fromEntries(env.split("\n").map((l: string) => l.split("=")).filter((p: string[]) => p[0]?.trim()).map((p: string[]) => [p[0].trim(), p.slice(1).join("=")])),
          })}>{L("保存", "Save")}</button>
        </div>
      </div>
    </div>
  );
}

function MCP() {
  const servers: any[] = useApp((s) => s.config?.settings?.mcpServers) ?? [];
  const [status, setStatus] = useState<Record<string, any>>({});
  const [edit, setEdit] = useState<any>();
  const load = () => core.request("mcp/status").then((r) => setStatus(r.status ?? {})).catch(() => {});
  useEffect(() => { load(); const t = setInterval(load, 3000); return () => clearInterval(t); }, []);
  const save = async (list: any[]) => { await patch({ settings: { mcpServers: list } }); setTimeout(load, 1500); };
  const [busy, setBusy] = useState(false);
  const reconnect = async () => { setBusy(true); await core.request("mcp/sync").then((r) => setStatus(r.status ?? {})).catch(useApp.getState().fail); setBusy(false); };
  const hasPw = servers.some((s) => s.name === "playwright");
  return (
    <section>
      <div className="row"><h4 className="grow">{L("MCP 服务器", "MCP servers")}</h4><IconBtn icon={Plus} title={L("添加 MCP 服务器", "Add MCP server")} onClick={() => setEdit({ id: rid() })} /></div>
      {!servers.length && <div className="muted small">{L("连接外部工具（Playwright、文件系统、数据库…）；工具名以 mcp__服务器__ 开头，可在权限里单独设置。", "Connect external tools; names start with mcp__server__.")}</div>}
      {servers.map((s) => (
        <div key={s.id || s.name} className="irow">
          <input type="checkbox" checked={s.enabled !== false} onChange={(e) => save(servers.map((x) => (x === s ? { ...x, enabled: e.target.checked } : x)))} />
          <span className="grow">{s.name} <span className="muted small mono">{s.transport === "http" ? s.url : [s.command, ...(s.args ?? [])].join(" ")}</span></span>
          <span className="small muted">{typeof status[s.name] === "string" ? status[s.name] : status[s.name] ? JSON.stringify(status[s.name]) : ""}</span>
          <IconBtn icon={Pencil} size={13} title={L("编辑", "Edit")} onClick={() => setEdit(s)} />
          <IconBtn icon={X} size={13} className="danger" title={L("删除", "Remove")} onClick={async () => { if (await ask(L(`删除「${s.name}」？`, `Delete “${s.name}”?`))) save(servers.filter((x) => x !== s)); }} />
        </div>
      ))}
      <div className="row wrap">
        {/* same server definition as Swift: npx @playwright/mcp@latest */}
        <button disabled={hasPw} onClick={() => save([...servers, { id: rid(), name: "playwright", transport: "stdio", command: "npx", args: ["@playwright/mcp@latest"], env: {}, url: "", enabled: true }])}>
          {L("添加 Playwright MCP（本机 Chrome）", "Add Playwright MCP (local Chrome)")}</button>
        <button onClick={() => setEdit({ id: rid() })}>{L("添加自定义", "Add custom")}</button>
        <button disabled={busy} onClick={reconnect}><RefreshCw size={13} className={busy ? "spin" : undefined} /> {L("重新连接", "Reconnect")}</button>
      </div>
      <div className="muted small">{L("MCP 服务器在沙箱外运行，其工具每次调用都需要审批（可设为“总是允许”）。", "MCP servers run outside the sandbox; their tools require approval (or “always allow”).")}</div>
      {edit && <MCPEditor s={edit} onClose={() => setEdit(undefined)} onSave={(v) => { setEdit(undefined); save([...servers.filter((x) => x !== edit && (x.id ? x.id !== v.id : x.name !== edit.name)), v]); }} />}
    </section>
  );
}

function Skills() {
  const s = useApp((x) => x.config?.settings) ?? {};
  const [r, setR] = useState<any>();
  const [dir, setDir] = useState("");
  useEffect(() => { core.request("skill/list").then(setR).catch(() => {}); }, [s.externalSkillsEnabled, (s.externalSkillDirs ?? []).join()]);
  const dirs: string[] = s.externalSkillDirs ?? ["~/.codex/skills", "~/.codex/skills/.system", "~/.claude/skills"];
  const skills: any[] = r?.skills ?? [];
  const parent = (d: string) => String(d).replace(/[\\/]+$/, "").replace(/[\\/][^\\/]+$/, "");
  // the core reports expanded dirs (1:1 with settings while enabled); a dir counts as found when skills were loaded from it
  const found = (i: number): boolean | undefined => r?.external_exists?.[i] ?? (r?.external_dirs?.[i] ? skills.some((k) => parent(k.dir) === String(r.external_dirs[i]).replace(/[\\/]+$/, "")) : undefined);
  const bySource = Object.entries(skills.reduce((m: any, k) => ({ ...m, [k.source]: (m[k.source] ?? 0) + 1 }), {})).map(([k, n]) => `${k} ${n}`).sort().join(" · ");
  return (
    <section>
      <h4>Skills</h4>
      <label className="small row"><input type="checkbox" checked={s.externalSkillsEnabled ?? true} onChange={(e) => patch({ settings: { externalSkillsEnabled: e.target.checked } })} /> {L("加载 Codex / Claude Code 的 skills（只读）", "Load Codex / Claude Code skills (read-only)")}</label>
      {dirs.map((d, i) => <div key={d} className="irow small"><span className="grow mono">{d}</span>
        {found(i) !== undefined && <span className="muted" >{found(i) ? L("存在", "found") : L("不存在", "missing")}</span>}<IconBtn icon={X} size={13} className="danger" title={L("删除", "Remove")} onClick={() => patch({ settings: { externalSkillDirs: dirs.filter((x) => x !== d) } })} /></div>)}
      <div className="row"><input placeholder="~/path/to/skills" value={dir} onChange={(e) => setDir(e.target.value)} /><button disabled={!dir.trim()} onClick={() => { patch({ settings: { externalSkillDirs: [...dirs, dir.trim()] } }); setDir(""); }}>{L("添加目录", "Add folder")}</button></div>
      <div className="muted small">{L(`共 ${skills.length} 个 skills：`, `${skills.length} skills: `)}{bySource}{L("；在输入框里用 /名称 调用。", "; invoke with /name.")}</div>
      <div className="row small">{r?.user_dir && <button onClick={() => reveal(r.user_dir).catch(useApp.getState().fail)}>{L("打开我的 skills 文件夹", "Open my skills folder")}</button>}
        <span className="muted">{L("在对话中输入 /skill-new 用 skill-creator 创建", "Type /skill-new in chat to create one")}</span></div>
      <div className="chips">{skills.map((k: any) => <span key={k.name} className="chip" title={k.description}>{k.name}{k.source !== "builtin" ? ` · ${k.source}` : ""}</span>)}</div>
    </section>
  );
}

function Browser() {
  const s = useApp((x) => x.config?.settings) ?? {};
  const [t, setT] = useState((s.browserAllowedDomains ?? []).join(", "));
  return (
    <section>
      <h4>{L("浏览器工具", "Browser tool")}</h4>
      <label className="fld"><span>{L("允许访问的域名（逗号分隔；留空表示不限）", "Allowed domains (comma separated; empty = any)")}</span>
        <input value={t} placeholder="example.com, *.gov.cn" onChange={(e) => setT(e.target.value)}
          onBlur={() => patch({ settings: { browserAllowedDomains: t.split(/[,\s]+/).map((x: string) => x.trim()).filter(Boolean) } })} /></label>
      <div className="muted small">{L("浏览器使用独立的数据存储（与日常使用的 Chrome 隔离），登录状态会保留。表单输入需要审批。", "The browser uses an isolated data store (separate from your everyday Chrome); logins persist. Typing into forms requires approval.")}</div>
    </section>
  );
}

function ComputerUse() {
  const s = useApp((x) => x.config?.settings) ?? {};
  const [p, setP] = useState<any>();
  const load = () => core.request("computer/permissions").then(setP).catch(() => {});
  useEffect(() => { load(); }, []);
  if (p && !p.supported) return null;   // Windows: hidden in the first version
  const req = async (kind: string) => { await core.request("computer/request", { kind }).catch(() => {}); setTimeout(load, 1500); };
  return (
    <section>
      <h4>{L("Computer Use（控制电脑）", "Computer Use")}</h4>
      <label className="small row"><input type="checkbox" checked={!!s.computerUseEnabled} onChange={(e) => patch({ settings: { computerUseEnabled: e.target.checked } })} /> {L("允许 Agent 控制鼠标键盘（每一步都需要确认）", "Let the agent control mouse & keyboard (each step asks)")}</label>
      <label className="fld"><span>{L("每轮最多操作步数", "Max actions per turn")}</span>
        <input type="number" min={1} max={200} defaultValue={s.computerMaxStepsPerTurn ?? 40} onBlur={(e) => patch({ settings: { computerMaxStepsPerTurn: Number(e.target.value) || 40 } })} /></label>
      <div className="row small">
        <span>{p?.accessibility ? <CircleCheck size={13} className="ok" /> : <CircleX size={13} className="errc" />} {L("辅助功能", "Accessibility")}</span>{!p?.accessibility && <button onClick={() => req("accessibility")}>{L("授权", "Grant")}</button>}
        <span>{p?.screen_recording ? <CircleCheck size={13} className="ok" /> : <CircleX size={13} className="errc" />} {L("屏幕录制", "Screen recording")}</span>{!p?.screen_recording && <button onClick={() => req("screen_recording")}>{L("授权", "Grant")}</button>}
        <IconBtn icon={RefreshCw} size={13} title={L("刷新", "Refresh")} onClick={load} />
      </div>
    </section>
  );
}

export function Extensions() {
  return <div className="pane"><Skills /><MCP /><Browser /><ComputerUse /></div>;
}
