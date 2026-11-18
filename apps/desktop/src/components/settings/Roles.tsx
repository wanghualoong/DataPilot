import { Status } from "../ui";
import { useEffect, useState } from "react";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { core } from "../../transport";
import { modelKind, patch } from "./common";

const ROLES: [string, string, string][] = [
  ["main", L("主对话模型", "Main model"), L("对话、工具调用、分析", "Chat, tools, analysis")],
  ["fast", L("快速模型（可选）", "Fast model (optional)"), L("标题、摘要、记忆整理；不设则用主模型", "Titles, summaries, memory; falls back to main")],
  ["vision", L("视觉模型（可选）", "Vision model (optional)"), L("看图（截图、图表）", "Images (screenshots, charts)")],
];

/** Where the context window came from (core `Capabilities.contextSource`). */
export function sourceLabel(s?: string): string {
  switch (s) {
    case "manual": return L("手动填写", "manual");
    case "server": return L("服务端报告", "reported by server");
    case "learned": return L("从报错中学到", "learned from an error");
    case "table": return L("内置模型表", "built-in model table");
    case "preset": return L("按模型系列估计", "model-family guess");
    default: return L("默认值，建议填写", "default — please set");
  }
}
const fmtK = (n?: number) => (n ? (n >= 1000 ? `${Math.round(n / 1024)}k` : String(n)) : "?");

function CapsEditor({ role, model, caps, onClose }: { role: string; model: string; caps: any; onClose: () => void }) {
  // resolved by the core: saved capabilities, else built-in model table / preset (one source of truth)
  const [c, setC] = useState<any>(caps ?? {});
  const [init, setInit] = useState<any>(caps ?? {});
  const [preset, setPreset] = useState<any>();
  useEffect(() => {
    core.request("model/preset", { model, capabilities: caps ?? null }).then((r: any) => { setC(r.capabilities); setInit(r.capabilities); setPreset(r.preset); }).catch(() => {});
  }, [model]);
  const [extra, setExtra] = useState(caps?.extraBody ? JSON.stringify(caps.extraBody) : "");
  const [err, setErr] = useState<string>();
  const num = (k: string, label: string) => <label className="fld"><span>{label}</span><input type="number" value={c[k] ?? ""} onChange={(e) => setC({ ...c, [k]: Number(e.target.value) })} /></label>;
  const sel = (k: string, label: string, opts: string[]) => (
    <label className="fld"><span>{label}</span><select value={c[k]} onChange={(e) => setC({ ...c, [k]: e.target.value })}>{opts.map((o) => <option key={o}>{o}</option>)}</select></label>);
  const chk = (k: string, label: string) => <label className="small row"><input type="checkbox" checked={!!c[k]} onChange={(e) => setC({ ...c, [k]: e.target.checked })} /> {label}</label>;
  const save = async () => {
    let extraBody: any = null;
    if (extra.trim()) {
      try { extraBody = JSON.parse(extra); } catch { /* reported below */ }
      if (!extraBody || typeof extraBody !== "object" || Array.isArray(extraBody)) return setErr(L("额外请求参数必须是 JSON 对象，如 {\"top_p\": 0.8}", "Extra body must be a JSON object, e.g. {\"top_p\": 0.8}"));
    }
    // config/set merges objects: drop the saved capabilities first so keys removed here (extraBody entries, temperature after 恢复预设) really go away
    if (caps && !(await patch({ roles: { [role]: { capabilities: null } } }))) return;
    // a window typed here is the user's own value: it wins over the table and is not replaced later
    const contextSource = c.contextWindow !== init.contextWindow ? "manual" : c.contextSource;
    if (await patch({ roles: { [role]: { capabilities: { ...c, extraBody, contextSource } } } })) onClose();
  };
  return (
    <div className="modal" onClick={onClose}>
      <div className="modalbox" onClick={(e) => e.stopPropagation()}>
        <h3>{L("模型能力", "Capabilities")} · {model}</h3>
        {num("contextWindow", L("上下文窗口（tokens）", "Context window"))}
        <div className="muted small">{L("来源：", "Source: ") + sourceLabel(c.contextSource)}</div>
        {num("maxOutputTokens", L("最大输出（tokens）", "Max output"))}
        {sel("toolCallMode", L("工具调用方式", "Tool calls"), ["native", "hermes"])}
        {sel("thinkingStyle", L("思考开关参数", "Thinking parameter"), ["none", "qwenChatTemplate", "deepseekChatTemplate", "enableThinkingTop", "thinkingObject"])}
        {sel("reasoningReplay", L("推理内容回传", "Reasoning replay"), ["never", "currentTurn"])}
        {chk("thinkingEnabled", L("默认开启思考", "Thinking on by default"))}
        {chk("supportsVision", L("支持图片输入（视觉）", "Vision input"))}
        {chk("supportsParallelToolCalls", L("支持并行工具调用", "Parallel tool calls"))}
        {chk("sendParallelToolCallsParam", L("发送 parallel_tool_calls 参数", "Send parallel_tool_calls param"))}
        <label className="fld"><span>{L("额外请求参数 JSON", "Extra body JSON")}</span>
          <input className="mono" value={extra} placeholder='{"top_p": 0.8}' onChange={(e) => { setExtra(e.target.value); setErr(undefined); }} /></label>
        {err && <div className="notice err small">{err}</div>}
        <div className="row">
          {/* like Swift: the form gets the model's preset; the extra-body text is kept; nothing is saved until 保存 */}
          <button title={L("按模型名称恢复内置预设（保存后生效）", "Restore the built-in preset for this model (applies on Save)")} disabled={!preset} onClick={() => setC(preset)}>{L("恢复预设", "Reset to preset")}</button>
          <span className="grow" />
          <button onClick={onClose}>{L("取消", "Cancel")}</button>
          <button className="primary" onClick={save}>{L("保存", "Save")}</button>
        </div>
      </div>
    </div>
  );
}

function RoleRow({ role, title, hint }: { role: string; title: string; hint: string }) {
  const config = useApp((s) => s.config);
  const cur = config?.roles?.[role];
  const [msg, setMsg] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [edit, setEdit] = useState(false);
  const local = config?.settings?.localOnly;
  const [win, setWin] = useState<any>();
  useEffect(() => {
    if (!cur) return setWin(undefined);
    core.request("model/preset", { model: cur.model, capabilities: cur.capabilities ?? null }).then((r: any) => setWin(r.capabilities)).catch(() => setWin(undefined));
  }, [cur?.model, JSON.stringify(cur?.capabilities ?? null)]);
  const marks = config?.settings?.localMarks ?? {};
  const models: [string, string, string][] = (config?.providers ?? []).flatMap((p: any) =>
    (config?.modelCache?.[p.id] ?? []).filter((m: string) => modelKind(m) === "chat" && (!local || marks[`${p.id}|${m}`] !== false)).map((m: string) => [p.id, p.name || p.id, m]));
  return (
    <div className="pane bordered">
      <div className="row"><b className="grow">{title}</b><span className="muted small">{hint}</span></div>
      <div className="row">
        <select className="grow" value={cur ? `${cur.providerId}|${cur.model}` : ""} onChange={async (e) => {
          setMsg(undefined);
          const v = e.target.value;
          if (!v) return patch({ roles: { [role]: null } });
          const [p, ...m] = v.split("|");
          await patch({ roles: { [role]: { providerId: p, model: m.join("|"), capabilities: null } } });
        }}>
          <option value="">{role === "main" ? L("选择模型", "Pick a model") : L("（不设置）", "(none)")}</option>
          {models.map(([p, pn, m]) => <option key={p + m} value={`${p}|${m}`}>{m} · {pn}</option>)}
        </select>
        <button disabled={!cur || busy} title={L("测试工具调用、推理内容和图片输入，并写回能力配置", "Probe tools / reasoning / vision and save capabilities")}
          onClick={async () => {
            setBusy(true); setMsg(undefined);
            try {
              const r = await core.request("provider/probe", { role });
              await useApp.getState().loadConfig();
              setMsg("✅ " + [`${r.ms}ms`, r.native_tools ? L("原生工具调用 ✓", "native tools ✓") : L("未返回原生工具调用 → 已切换为 Hermes 模式", "no native tool call → Hermes mode"),
                r.reasoning ? L("返回推理内容 ✓", "reasoning ✓") : "", r.vision ? L("图片输入 ✓", "vision ✓") : L("不支持图片输入", "no vision"),
                r.context_window ? L(`服务端报告上下文 ${r.context_window} tokens（已写入）`, `server context ${r.context_window} tokens (saved)`) : "",
                r.usage ? `tokens ${r.usage.prompt}+${r.usage.completion}` : ""].filter(Boolean).join(" · "));
            } catch (e) { setMsg("❌ " + (e as Error).message); } finally { setBusy(false); }
          }}>{busy ? "…" : L("测试", "Test")}</button>
        <button disabled={!cur} onClick={() => setEdit(true)}>{L("能力…", "Capabilities…")}</button>
      </div>
      {win && <div className="muted small">{L(`上下文 ${fmtK(win.contextWindow)} tokens（${sourceLabel(win.contextSource)}）· 最大输出 ${fmtK(win.maxOutputTokens)}`,
        `Context ${fmtK(win.contextWindow)} tokens (${sourceLabel(win.contextSource)}) · max output ${fmtK(win.maxOutputTokens)}`)}</div>}
      <Status msg={msg} />
      {edit && cur && <CapsEditor role={role} model={cur.model} caps={cur.capabilities} onClose={() => setEdit(false)} />}
    </div>
  );
}

export function Roles() {
  return (
    <div className="pane">
      <div className="muted small">{L("模型来自「模型服务 → 获取并添加模型」。检索只用关键词和 grep，不需要 embedding / rerank 模型。「测试」会检查工具调用、推理内容和图片输入并更新能力配置。",
        "Models come from Providers → Fetch models. Retrieval is keyword + grep only — no embedding / rerank needed. “Test” probes tools, reasoning and vision.")}</div>
      {ROLES.map(([k, t, h]) => <RoleRow key={k} role={k} title={t} hint={h} />)}
    </div>
  );
}
