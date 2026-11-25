import { IconBtn, Status } from "../ui";
import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { L, UI_LANG_KEY, uiLanguage } from "../../i18n";
import { ask } from "../../native";
import { useApp } from "../../store";
import { core } from "../../transport";
import { reveal, saveDialog } from "../../native";
import { patch } from "./common";
import { PaletteSection } from "./Palette";
import { BackupSection } from "./Backup";
import { setThemePref, themePref, type ThemePref } from "../../theme";

/** Gateways whose "*local" models are synced weekly (settings.localModelSyncHosts; unset = the built-in defaults). */
function LocalSyncHosts() {
  const saved = useApp((x) => x.config?.settings?.localModelSyncHosts);
  const [defaults, setDefaults] = useState<string[]>([]);
  const [text, setText] = useState("");
  const [n, setN] = useState(0);   // bumped after a write finished (the store copy changes before the core has the file)
  useEffect(() => {
    core.request("models/sync_local", { dry: true }).then((r: any) => {
      setDefaults(r?.default_hosts ?? []);
      setText((r?.hosts ?? []).join("\n"));
    }).catch(() => {});
  }, [JSON.stringify(saved ?? null), n]);
  const save = () => {
    const list = text.split(/[\n,]+/).map((s) => s.trim()).filter(Boolean);
    const same = JSON.stringify(list) === JSON.stringify(saved ?? defaults);
    if (!same) patch({ settings: { localModelSyncHosts: list } }).then(() => setN((x) => x + 1));
  };
  return (
    <section className="synchosts">
      <h4>{L("本地模型同步的网关", "Gateways for local-model sync")}</h4>
      <div className="muted small">{L("这些网关上的模型服务每周同步一次名字以 local 结尾的模型（新增的自动添加，下线的自动删除）。每行一个：域名或 IP，可带端口（如 10.0.0.5:8000），*.example.com 表示所有子域名；按主机名精确比对。清空表示不同步。",
        "Providers on these gateways get their *local models synced weekly. One per line: host name or IP, optional port (10.0.0.5:8000), *.example.com for sub-domains; exact host match. Empty = no sync.")}
        {defaults.length ? L(`默认：${defaults.join("、")}。`, ` Default: ${defaults.join(", ")}.`) : L("默认不同步任何网关。", " Default: none.")}</div>
      <textarea rows={3} className="mono" value={text} onChange={(e) => setText(e.target.value)} onBlur={save} placeholder={defaults.join("\n")} />
      <div className="row">
        <span className="grow muted small">{saved === undefined ? L("当前使用默认设置", "Using the default") : ""}</span>
        <button disabled={saved === undefined} onClick={() => patch({ settings: { localModelSyncHosts: null } }).then(() => setN((x) => x + 1))}>{L("恢复默认", "Restore default")}</button>
      </div>
    </section>
  );
}

// server "" = the built-in default (app/defaults docParseServer; none in the open-source build)
const DOC_DEFAULT = { enabled: true, server: "", primaryTier: "advanced", crossCheck: true, checkTier: "basic" };
const TIERS = ["basic", "standard", "advanced", "flash"];

/** Document parsing (OCR) of scanned PDFs / images through a MinerU server (settings.docParse). */
function DocParse() {
  const saved = useApp((x) => x.config?.settings?.docParse);
  const defServer = useApp((x) => x.defaults?.docParseServer) ?? "";
  const d = { ...DOC_DEFAULT, server: defServer, ...(saved ?? {}) };
  const [server, setServer] = useState(d.server);
  const [key, setKey] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [msg, setMsg] = useState<string>();
  const [busy, setBusy] = useState(false);
  useEffect(() => { setServer(d.server); }, [d.server]);
  useEffect(() => { core.request("secret/has", { kind: "api_key", id: "docparse" }).then((r: any) => setHasKey(!!r?.has)).catch(() => {}); }, []);
  const set = (p: object) => patch({ settings: { docParse: { ...d, ...p } } });
  const test = async () => {
    setBusy(true); setMsg("…");
    try {
      const r: any = await core.request("docparse/test", { server: server.trim() });
      setMsg(`✅ MinerU ${r.version ?? ""} · ${r.device ?? ""} · ${r.ms}ms` + (r.tiers?.length ? L(` · 可用档位：${r.tiers.join(", ")}`, ` · tiers: ${r.tiers.join(", ")}`) : ""));
    } catch (e: any) { setMsg("❌ " + (e?.message ?? String(e))); }
    setBusy(false);
  };
  return (
    <section className="docparse">
      <h4>{L("文档识别（OCR）", "Document parsing (OCR)")}</h4>
      <div className="muted small">{L("扫描版 PDF 和图片没有文字层，读取或导入时只把这些页发给 MinerU 文档识别服务识别（有文字层的页不发送）。默认用 advanced 档为主（不编造文字、能读印章），basic 档并行核对（补回被印章遮住的行）；两档不一致的金额、日期、编号会标成「需核对」。",
        "Scanned PDF pages and images have no text layer; only those pages are sent to the MinerU service. advanced is the primary tier (no invented text, reads seals), basic runs in parallel as a cross-check; disagreeing amounts / dates / numbers are flagged for review.")}</div>
      <label className="small row"><input type="checkbox" checked={d.enabled} onChange={(e) => set({ enabled: e.target.checked })} /> {L("启用文档识别", "Enable document parsing")}</label>
      <label className="fld"><span>{L("服务器地址", "Server")}</span>
        <input className="dpserver" value={server} placeholder={defServer || "http://mineru.example.com:8010"} onChange={(e) => setServer(e.target.value)} onBlur={() => server.trim() !== d.server && set({ server: server.trim() })} /></label>
      {d.enabled && !d.server.trim() && <div className="muted small">{L("还没有填写服务器地址：扫描件和图片暂不识别，读取时会提示。", "No server yet: scanned pages and images are not recognized until one is set.")}</div>}
      <label className="fld"><span>{L("主识别档位", "Primary tier")}</span>
        <select value={d.primaryTier} onChange={(e) => set({ primaryTier: e.target.value })}>{TIERS.map((t) => <option key={t} value={t}>{t}</option>)}</select></label>
      <label className="small row"><input type="checkbox" checked={d.crossCheck} onChange={(e) => set({ crossCheck: e.target.checked })} /> {L("并行用另一档交叉核对（推荐，识别时间不变，服务器负载加倍）", "Cross-check with a second tier in parallel (recommended)")}</label>
      {d.crossCheck && <label className="fld"><span>{L("核对档位", "Check tier")}</span>
        <select value={d.checkTier} onChange={(e) => set({ checkTier: e.target.value })}>{TIERS.filter((t) => t !== d.primaryTier).map((t) => <option key={t} value={t}>{t}</option>)}</select></label>}
      <label className="fld"><span>{L("API Key（可选，存钥匙串）", "API key (optional, keychain)")}</span>
        <input type="password" value={key} placeholder={hasKey ? L("已保存（留空不修改）", "saved (leave empty to keep)") : L("服务器未开启鉴权时留空", "empty if the server has no auth")}
          onChange={(e) => setKey(e.target.value)}
          onBlur={async () => { if (key.trim()) { await core.request("secret/set", { kind: "api_key", id: "docparse", value: key.trim() }).catch(useApp.getState().fail); setKey(""); setHasKey(true); } }} /></label>
      <div className="row">
        <button disabled={busy} onClick={test}>{L("测试连接", "Test connection")}</button>
        {hasKey && <button onClick={async () => { await core.request("secret/set", { kind: "api_key", id: "docparse", value: null }).catch(() => {}); setHasKey(false); }}>{L("清除 Key", "Clear key")}</button>}
        <span className="grow" />
        <button disabled={saved === undefined} onClick={() => patch({ settings: { docParse: null } })}>{L("恢复默认", "Restore defaults")}</button>
      </div>
      <Status msg={msg} />
      <div className="muted xs">{L("说明：文件会上传到该服务器识别（不发送文件名），识别结果在本机缓存；MinerU 没有删除接口，服务器端按其保留期清理。「仅本地模型」模式下只允许本机 / 内网地址。",
        "Files are uploaded to this server (without the file name); results are cached locally. MinerU has no delete API: the server keeps uploads for its retention period. In local-only mode only localhost / private addresses are used.")}</div>
    </section>
  );
}

function Proxy() {
  const s = useApp((x) => x.config?.settings) ?? {};
  const [host, setHost] = useState(s.proxyHost ?? "");
  const [port, setPort] = useState(String(s.proxyPort || ""));
  const [msg, setMsg] = useState<string>();
  const [busy, setBusy] = useState(false);
  const mode = s.proxyMode ?? "system";
  return (
    <section>
      <h4>{L("网络代理", "Network proxy")}</h4>
      <div className="seg">{[["system", L("跟随系统（含 PAC）", "System (incl. PAC)")], ["none", L("直连", "Direct")], ["custom", L("自定义", "Custom")]].map(([k, t]) =>
        <button key={k} className={mode === k ? "on" : ""} onClick={() => patch({ settings: { proxyMode: k } })}>{t}</button>)}</div>
      {mode === "custom" && (
        <div className="row">
          <input placeholder="proxy.example.com" value={host} onChange={(e) => setHost(e.target.value)} onBlur={() => patch({ settings: { proxyHost: host.trim() } })} />
          <input style={{ width: 90, flex: "none" }} placeholder="3128" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} onBlur={() => patch({ settings: { proxyPort: Number(port) || 0 } })} />
        </div>
      )}
      <div className="row">
        <button disabled={busy} onClick={async () => {
          setBusy(true);
          const r = await core.request("net/test", { url: "https://www.apple.com/library/test/success.html" }).catch((e) => ({ ok: false, error: e.message }));
          setMsg(r.ok ? `✅ HTTP ${r.status} · ${r.ms}ms${r.proxy ? " · " + L("经代理 ", "via ") + r.proxy : ""}` : "❌ " + (r.error ?? `HTTP ${r.status}`));
          setBusy(false);
        }}>{L("测试连接", "Test connection")}</button>
        <Status msg={msg} />
      </div>
      <div className="muted small">{L("用于网页抓取、联网搜索、HTTP MCP，以及选择「跟随全局」的模型服务。是否允许联网由输入框上方的「联网」开关决定。",
        "Used by web fetch / search, HTTP MCP and providers set to “global”. Whether tools may go online is set by the Network switch.")}</div>
    </section>
  );
}

function Chrome() {
  const [st, setSt] = useState<any>();
  const [busy, setBusy] = useState(false);
  const notice = useApp((s) => s.notice);
  const load = () => core.request("chrome/status").then(setSt).catch(() => {});
  useEffect(() => { load(); }, []);
  return (
    <section>
      <h4>Chrome</h4>
      <div className="row small">
        <span className="grow">{st?.path ? `✓ ${st.managed ? "Chrome for Testing" : "Chrome"} ${st.version ?? ""} · ` : L("未找到 Chrome（网页渲染、Google 搜索、browser 工具需要）", "Chrome not found (needed for rendering, Google search, browser tool)")}
          {st?.path && <span className="muted">{st.path}</span>}</span>
        <button disabled={busy} onClick={async () => { setBusy(true); await core.request("chrome/install").catch((e) => useApp.getState().fail(e)); setBusy(false); load(); }}>
          {busy ? "…" : st?.path ? L("重新安装 / 更新", "Reinstall / update") : L("下载 Chrome for Testing", "Install Chrome for Testing")}</button>
      </div>
      {busy && notice && <div className="muted small">{notice}</div>}
    </section>
  );
}

export function General() {
  const s = useApp((x) => x.config?.settings) ?? {};
  const [info, setInfo] = useState<any>();
  useEffect(() => { import("@tauri-apps/api/core").then(({ invoke }) => invoke("core_info")).then(setInfo).catch(() => {}); }, []);
  return (
    <div className="pane">
      <section>
        <label className="fld"><span>{L("界面语言", "Language")}</span>
          <select defaultValue={uiLanguage()} onChange={async (e) => {
            localStorage.setItem(UI_LANG_KEY, e.target.value);
            if (await ask(L("界面语言已更改，需要重新加载窗口才能生效。现在重新加载？", "Language changed. Reload the window now to apply it?"))) window.location.reload();
          }}>
            <option value="system">{L("跟随系统", "System")}</option><option value="zh-Hans">简体中文</option><option value="en">English</option>
          </select></label>
        <label className="fld"><span>{L("外观", "Appearance")}</span>
          <select defaultValue={themePref()} onChange={(e) => setThemePref(e.target.value as ThemePref)}>
            <option value="system">{L("跟随系统", "System")}</option><option value="light">{L("浅色", "Light")}</option><option value="dark">{L("深色", "Dark")}</option>
          </select></label>
        <label className="fld"><span>{L("回复语言", "Reply language")}</span>
          <select value={s.responseLanguage ?? "auto"} onChange={(e) => patch({ settings: { responseLanguage: e.target.value } })}>
            <option value="auto">{L("跟随用户", "Match user")}</option><option value="zh-Hans">简体中文</option><option value="en">English</option>
          </select></label>
        <label className="fld"><span>{L("小表阈值（行）", "Small-table threshold (rows)")}</span>
          <input type="number" step={1000} defaultValue={s.smallRowThreshold ?? 5000} onBlur={(e) => patch({ settings: { smallRowThreshold: Number(e.target.value) || 5000 } })} /></label>
        <label className="fld"><span>{L("最大轮数", "Max turns")}</span>
          <input type="number" min={5} max={300} step={5} defaultValue={s.maxTurns ?? 120} onBlur={(e) => patch({ settings: { maxTurns: Math.min(300, Math.max(5, Number(e.target.value) || 120)) } })} /></label>
        <label className="fld"><span>{L(`自动压缩阈值 ${Math.round((s.compactThreshold ?? 0.8) * 100)}%`, `Auto-compact at ${Math.round((s.compactThreshold ?? 0.8) * 100)}%`)}</span>
          <input type="range" min={0.5} max={0.95} step={0.05} value={s.compactThreshold ?? 0.8} onChange={(e) => patch({ settings: { compactThreshold: Number(e.target.value) } })} /></label>
        <label className="fld"><span title={L("可选。大上下文模型（如 256k）建议设置，例如 160000：取「窗口 × 阈值」和这个数中较小的", "Optional. For large-context models, e.g. 160000: the smaller of window × threshold and this wins")}>{L("压缩上限（tokens，0 = 不限）", "Compaction cap (tokens, 0 = none)")}</span>
          <input type="number" min={0} step={10000} defaultValue={s.compactTokenLimit ?? 0} onBlur={(e) => patch({ settings: { compactTokenLimit: Math.max(0, Number(e.target.value) || 0) } })} /></label>
        <label className="fld"><span>{L("压缩摘要由谁写", "Compaction summary by")}</span>
          <select value={s.compactModel ?? "fast"} onChange={(e) => patch({ settings: { compactModel: e.target.value } })}>
            <option value="fast">{L("快速模型（省时间）", "Fast model (quicker)")}</option><option value="main">{L("主模型（质量更好）", "Main model (better quality)")}</option>
          </select></label>
        <label className="fld"><span>{L("新会话默认模式", "Default mode for new chats")}</span>
          <select value={s.defaultMode ?? "ask"} onChange={(e) => patch({ settings: { defaultMode: e.target.value } })}>
            <option value="plan">{L("计划", "Plan")}</option><option value="ask">{L("逐步确认", "Ask")}</option><option value="auto">{L("自动", "Auto")}</option>
          </select></label>
        <label className="small row"><input type="checkbox" checked={s.memoryEnabled ?? true} onChange={(e) => patch({ settings: { memoryEnabled: e.target.checked } })} /> {L("启用长期记忆", "Enable long-term memory")}</label>
        <label className="small row"><input type="checkbox" checked={!!s.localOnly} onChange={(e) => useApp.getState().setLocalOnly(e.target.checked)} /> {L("本地模式（只用本地模型，禁用联网工具）", "Local-only mode (local models only, no network tools)")}</label>
        <NotifyToggle />
      </section>
      <PaletteSection />
      <LocalSyncHosts />
      <DocParse />
      <Proxy />
      <Chrome />
      <BackupSection />
      {info && (
        <section>
          <h4>{L("数据位置", "Data location")}</h4>
          <div className="row small"><span className="grow">{info.app_support}</span><IconBtn icon={FolderOpen} size={13} title={L("在访达 / 资源管理器中显示", "Reveal")}onClick={() => reveal(info.app_support)} /></div>
          <div className="muted small">{L("日志：", "Logs: ")}<span>{info.app_support}/logs</span>{info.restarts ? L(` · 核心已自动重启 ${info.restarts} 次`, ` · core restarted ${info.restarts}×`) : ""}</div>
          <Diagnostics dir={info.app_support} />
        </section>
      )}
    </div>
  );
}

/** 日志文件夹 + 诊断包 (logs + version / self-check; no chats, data or keys). */
function Diagnostics({ dir }: { dir: string }) {
  const [msg, setMsg] = useState("");
  const sep = dir.includes("\\") ? "\\" : "/";
  const exportZip = async () => {
    const d = new Date(), pad = (n: number) => String(n).padStart(2, "0");
    const name = `DataPilot-诊断包-${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}.zip`;
    const path = await saveDialog(name, "zip");
    if (!path) return;
    setMsg(L("正在生成…", "Creating…"));
    try { const r: any = await core.request("app/diagnostics", { path }); setMsg(L(`已导出：${r.path}`, `Exported: ${r.path}`)); reveal(r.path); }
    catch (e: any) { setMsg(L(`导出失败：${e?.message ?? e}`, `Export failed: ${e?.message ?? e}`)); }
  };
  return (
    <>
      <div className="row small" style={{ gap: 8, marginTop: 6 }}>
        <button onClick={() => reveal(`${dir}${sep}logs${sep}datapilot.log`)}>{L("打开日志文件夹", "Open log folder")}</button>
        <button onClick={exportZip}>{L("导出诊断包", "Export diagnostics")}</button>
      </div>
      <div className="muted small">{msg || L("任务很慢或失败时，把诊断包发给开发者。只含日志（耗时、状态、错误信息）和版本 / 自检结果，不含对话内容、数据或密钥。", "When a task is slow or fails, send the diagnostics zip to the developer. It holds only logs (timings, status, errors) and version / self-check — no chats, data or keys.")}</div>
    </>
  );
}

/** Background notifications (done / error / approval) — stored on this machine only. */
function NotifyToggle() {
  const [on, setOn] = useState(() => localStorage.getItem("dp.notify") !== "off");
  return <label className="small row"><input type="checkbox" checked={on} onChange={(e) => { setOn(e.target.checked); localStorage.setItem("dp.notify", e.target.checked ? "on" : "off"); }} /> {L("切到其他窗口时，完成、出错或需要审批 / 回答时弹出系统通知", "System notification when a task finishes, fails or needs your approval / answer while DataPilot is in the background")}</label>;
}
