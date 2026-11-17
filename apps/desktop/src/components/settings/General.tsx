import { IconBtn, Status } from "../ui";
import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { L, UI_LANG_KEY, uiLanguage } from "../../i18n";
import { ask } from "../../native";
import { useApp } from "../../store";
import { core } from "../../transport";
import { reveal } from "../../native";
import { patch } from "./common";
import { PaletteSection } from "./Palette";
import { setThemePref, themePref, type ThemePref } from "../../theme";

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
          {st?.path && <span className="mono muted">{st.path}</span>}</span>
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
          <input type="number" min={5} max={300} step={5} defaultValue={s.maxTurns ?? 60} onBlur={(e) => patch({ settings: { maxTurns: Math.min(300, Math.max(5, Number(e.target.value) || 60)) } })} /></label>
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
      </section>
      <PaletteSection />
      <Proxy />
      <Chrome />
      {info && (
        <section>
          <h4>{L("数据位置", "Data location")}</h4>
          <div className="row small"><span className="grow mono">{info.app_support}</span><IconBtn icon={FolderOpen} size={13} title={L("在访达 / 资源管理器中显示", "Reveal")}onClick={() => reveal(info.app_support)} /></div>
          <div className="muted small">{L("日志：", "Log: ")}<span className="mono">{info.app_support}/core.log</span>{info.restarts ? L(` · 核心已自动重启 ${info.restarts} 次`, ` · core restarted ${info.restarts}×`) : ""}</div>
        </section>
      )}
    </div>
  );
}
