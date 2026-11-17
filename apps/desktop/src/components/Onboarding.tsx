// First-run wizard (SwiftUI OnboardingView) and the "Log in & choose key" sheet (SwiftUI QuickLoginSheet).
import { useState } from "react";
import { KeyRound, LoaderCircle } from "lucide-react";
import { useApp } from "../store";
import { core } from "../transport";
import { L } from "../i18n";
import { PRESETS, defaultLocalModel, isLocalHost, modelKind, normURL, suffixLocalMarks } from "../models";
import { patch, rid } from "./settings/common";
import { NewAPILogin } from "./settings/NewAPILogin";
import { ModelPicker } from "./settings/Providers";
import { Status, ic } from "./ui";

/** Pick a main model from the provider's added models when none is usable yet (Swift ensureMainModel). */
async function ensureMainModel(pid: string) {
  const cfg = await useApp.getState().loadConfig();
  const added: string[] = (cfg?.modelCache?.[pid] ?? []).filter((m: string) => modelKind(m) === "chat");
  const m = cfg?.roles?.main;
  const valid = m && (cfg.providers ?? []).some((p: any) => p.id === m.providerId) && (m.providerId !== pid || added.includes(m.model));
  if (!valid) {
    const main = defaultLocalModel(added) ?? added.find((x) => x.toLowerCase().includes("qwen3")) ?? added[0];
    if (main) await patch({ roles: { main: { providerId: pid, model: main, capabilities: null } } });
  }
  useApp.setState({ onboarding: false });
}

export function Onboarding() {
  const st = useApp();
  const [preset, setPreset] = useState("custom");
  const [baseURL, setBaseURL] = useState("");
  const [key, setKey] = useState("");
  const [models, setModels] = useState<string[]>([]);
  const [main, setMain] = useState("");
  const [fast, setFast] = useState("");
  const [status, setStatus] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [pid] = useState(rid);
  const [login, setLogin] = useState(false);
  const pr = PRESETS.find((p) => p.id === preset)!;
  const provider = () => ({ id: pid, name: pr.providerName, baseURL: baseURL.trim(), proxyMode: pr.proxyMode, extraHeaders: {} });
  const close = () => useApp.setState({ onboarding: false });

  const saveProvider = async () => {
    const cfg = st.config ?? (await st.loadConfig());
    await patch({ providers: [...(cfg?.providers ?? []).filter((p: any) => p.id !== pid), provider()] });
  };
  const connect = async () => {
    setBusy(true); setStatus(undefined);
    try {
      await saveProvider();
      if (key.trim()) await core.request("secret/set", { kind: "api_key", id: pid, value: key.trim() });
      const ids: string[] = (await core.request("provider/models", { provider: provider() })).models.map((m: any) => m.id);
      setModels(ids);
      await patch({ availableModels: { [pid]: ids } });
      setStatus(L(`✅ ${ids.length} 个模型`, `✅ ${ids.length} models`));
      // auto-assign (same order as Swift autoAssign)
      const chat = ids.filter((m) => modelKind(m) === "chat"), lo = (m: string) => m.toLowerCase();
      setMain(defaultLocalModel(ids) ?? chat.find((m) => lo(m).includes("qwen3") && lo(m).includes("local")) ?? chat.find((m) => lo(m).includes("qwen3") && !lo(m).includes("vl"))
        ?? chat.find((m) => lo(m).includes("deepseek")) ?? chat[0] ?? "");
      setFast(chat.find((m) => lo(m).includes("qwen") && ["7b", "8b", "flash", "turbo"].some((k) => lo(m).includes(k))) ?? "");
    } catch (e) { setStatus("❌ " + (e as Error).message); }
    finally { setBusy(false); }
  };
  const finish = async () => {
    const added = [...new Set([main, fast, defaultLocalModel(models) ?? ""].filter(Boolean))].sort();
    const cfg = st.config;
    await patch({
      roles: { main: main ? { providerId: pid, model: main, capabilities: null } : null, fast: fast ? { providerId: pid, model: fast, capabilities: null } : null, embedding: null, rerank: null },
      modelCache: { [pid]: added }, settings: { localMarks: suffixLocalMarks(cfg, pid, added) },
    });
    close();
    if (!useApp.getState().sessions.length) st.newChat();
  };
  const chatModels = models.filter((m) => modelKind(m) === "chat");
  const pick = (label: string, v: string, set: (s: string) => void) => (
    <label className="fld"><span>{label}</span>
      <select value={v} onChange={(e) => set(e.target.value)}>
        <option value="">{L("无", "None")}</option>
        {chatModels.map((m) => <option key={m} value={m}>{m}</option>)}
      </select>
    </label>
  );
  return (
    <div className="modal">
      <div className="modalbox onboard" onClick={(e) => e.stopPropagation()}>
        <h2>{L("欢迎使用 DataPilot", "Welcome to DataPilot")}</h2>
        <p className="muted">{L("连接你的 New API（或 vLLM / Ollama 等 OpenAI 兼容服务）。", "Connect your New API (or any OpenAI-compatible endpoint such as vLLM / Ollama).")}</p>
        <label className="fld"><span>{L("预设", "Preset")}</span>
          <select value={preset} onChange={(e) => { setPreset(e.target.value); setBaseURL(PRESETS.find((p) => p.id === e.target.value)!.baseURL); }}>
            {PRESETS.map((p) => <option key={p.id} value={p.id}>{L(p.zh, p.en)}</option>)}
          </select>
        </label>
        <label className="fld"><span>Base URL</span><input value={baseURL} placeholder="https://your-newapi.example.com/v1" onChange={(e) => setBaseURL(e.target.value)} /></label>
        <label className="fld"><span>API Key</span><input type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)} /></label>
        {(preset === "dku" || preset === "custom") && (
          <div className="row">
            <button disabled={busy || !baseURL.trim()} onClick={async () => { await saveProvider(); setLogin(true); }}><KeyRound {...ic} size={13} /> {L("登录并选择 Key…", "Log in & choose key…")}</button>
            <span className="muted small">{preset === "dku" ? L("用 Duke NetID 登录，直接从你的 Key 列表里选。", "Log in with Duke NetID and pick from your keys.") : L("New API 网关可直接登录选 Key。", "New API gateways: log in and pick a key.")}</span>
          </div>
        )}
        <div className="row">
          <button className="primary" disabled={busy || !baseURL.trim()} onClick={connect}>{L("连接并获取模型", "Connect & fetch models")}</button>
          {busy && <LoaderCircle {...ic} size={14} className="spin muted" />}
          <Status msg={status} />
        </div>
        {models.length > 0 && <>
          {pick(L("主对话模型", "Main model"), main, setMain)}
          {pick(L("快速模型（可选）", "Fast model (optional)"), fast, setFast)}
          <div className="muted small">{L("完成后会把所选模型和一个结尾为 local 的本地模型加入列表；之后可在 设置 → 模型服务 中增删。", "Your picks and one *local model will be added; manage them later in Settings → Providers.")}</div>
        </>}
        <div className="row end">
          <button onClick={close}>{L("稍后设置", "Later")}</button>
          <span className="grow" />
          <button className="primary" disabled={!main} onClick={finish}>{L("完成", "Done")}</button>
        </div>
        {login && <NewAPILogin provider={provider()} onClose={() => setLogin(false)} onUsed={() => { setLogin(false); connect(); }} />}
      </div>
    </div>
  );
}

export function QuickLogin() {
  const config = useApp((s) => s.config);
  const close = () => useApp.setState({ quickLogin: false });
  // existing remote providers first, then presets with a URL (no local ones)
  const choices: { name: string; url: string }[] = [];
  for (const p of config?.providers ?? []) if (!isLocalHost(p.baseURL) && !choices.some((c) => normURL(c.url) === normURL(p.baseURL))) choices.push({ name: p.name, url: p.baseURL });
  for (const p of PRESETS) if (p.baseURL && !isLocalHost(p.baseURL) && !choices.some((c) => normURL(c.url) === normURL(p.baseURL))) choices.push({ name: p.providerName, url: p.baseURL });
  const [baseURL, setBaseURL] = useState(choices[0]?.url ?? "");
  const [prov, setProv] = useState<any>();
  const [step, setStep] = useState<"url" | "login" | "pick">("url");
  const valid = (() => { try { return !!new URL(baseURL.trim()).host; } catch { return false; } })();

  const next = () => {
    const url = baseURL.trim();
    const ex = (config?.providers ?? []).find((p: any) => normURL(p.baseURL) === normURL(url));
    const pr = PRESETS.find((p) => p.baseURL && normURL(p.baseURL) === normURL(url));
    setProv(ex ? { ...ex, name: ex.name === "New API" ? "NetID" : ex.name }
               : { id: rid(), name: pr?.providerName ?? "NetID", baseURL: url, proxyMode: pr?.proxyMode ?? "global", extraHeaders: {} });
    setStep("login");
  };
  const used = async () => {
    const cfg = useApp.getState().config;
    await patch({ providers: [...(cfg?.providers ?? []).filter((p: any) => p.id !== prov.id), prov] });
    setStep("pick");
  };
  if (step === "login") return <NewAPILogin provider={prov} onClose={close} onUsed={used} />;
  if (step === "pick") return <ModelPicker provider={prov} fresh onClose={async (saved) => { if (saved) await ensureMainModel(prov.id); close(); }} />;
  return (
    <div className="modal" onClick={close}>
      <div className="modalbox" onClick={(e) => e.stopPropagation()}>
        <h3>{L("登录并选择 Key", "Log in & choose a key")}</h3>
        <div className="muted small">{L("选择要登录的 New API 网关（DKU 用户选 DKU AI Gateway，用 Duke NetID 登录）。", "Choose the New API gateway to log in to (DKU users: DKU AI Gateway, log in with Duke NetID).")}</div>
        {choices.length > 0 && (
          <label className="fld"><span>{L("网关", "Gateway")}</span>
            <select value={choices.some((c) => c.url === baseURL) ? baseURL : ""} onChange={(e) => setBaseURL(e.target.value)}>
              {!choices.some((c) => c.url === baseURL) && <option value="">{baseURL || "—"}</option>}
              {choices.map((c) => <option key={c.url} value={c.url}>{c.name} — {c.url}</option>)}
            </select>
          </label>
        )}
        <label className="fld"><span>Base URL</span><input value={baseURL} placeholder="https://your-newapi.example.com/v1" onChange={(e) => setBaseURL(e.target.value)} /></label>
        <div className="row end">
          <button onClick={close}>{L("取消", "Cancel")}</button>
          <button className="primary" disabled={!valid} onClick={next}>{L("下一步：登录", "Next: log in")}</button>
        </div>
      </div>
    </div>
  );
}
