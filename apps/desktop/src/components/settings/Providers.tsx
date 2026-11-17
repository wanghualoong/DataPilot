import { IconBtn, Status } from "../ui";
import { X, Plus } from "lucide-react";
import { ask } from "../../native";
import { useMemo, useState } from "react";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { core } from "../../transport";
import { modelKind, patch, rid } from "./common";
import { defaultLocalModel, isLocalModel, suffixLocalMarks } from "../../models";
import { NewAPILogin } from "./NewAPILogin";

/** Qwen / DeepSeek chat models, no VL (Swift ModelPicker.recommended; retrieval is keyword-only). */
export const recommended = (models: string[]) =>
  models.filter((m) => { const l = m.toLowerCase(); return modelKind(m) === "chat" && (l.includes("qwen") || l.includes("deepseek")) && !l.includes("vl"); });

export function ModelPicker({ provider, onClose, fresh }: { provider: any; onClose: (saved: boolean) => void; fresh?: boolean }) {
  const config = useApp((s) => s.config);
  const [list, setList] = useState<{ id: string; local: boolean }[]>();
  const [err, setErr] = useState<string>();
  const [q, setQ] = useState("");
  const [kind, setKind] = useState("chat");
  const [sel, setSel] = useState<Set<string>>(new Set(config?.modelCache?.[provider.id] ?? []));
  const [name, setName] = useState(provider.name ?? "");
  useMemo(() => {
    core.request("provider/models", { provider }).then((r) => {
      setList(r.models);
      if (fresh && !sel.size) { const d = defaultLocalModel(r.models.map((m: any) => m.id)); if (d) setSel(new Set([d])); }
    }).catch((e) => setErr(e.message));
  }, []);
  const shown = (list ?? []).filter((m) => (kind === "all" || modelKind(m.id) === kind) && m.id.toLowerCase().includes(q.toLowerCase()));
  const save = async () => {
    const added = [...sel].sort();
    const n = name.trim();
    // Swift ensureMainModel: keep the main model unless it is unset / gone; else pick *local → qwen3 → first
    const chat = added.filter((m) => modelKind(m) === "chat"), cur = config?.roles?.main;
    const keep = cur && (config?.providers ?? []).some((x: any) => x.id === cur.providerId) && (cur.providerId !== provider.id || chat.includes(cur.model));
    const main = keep ? undefined : defaultLocalModel(chat) ?? chat.find((m) => m.toLowerCase().includes("qwen3")) ?? chat[0];
    const renamed = n && n !== provider.name && (config?.providers ?? []).some((x: any) => x.id === provider.id);
    await patch({ ...(renamed ? { providers: (config?.providers ?? []).map((x: any) => (x.id === provider.id ? { ...x, name: n } : x)) } : {}), modelCache: { [provider.id]: added }, availableModels: { [provider.id]: (list ?? []).map((m) => m.id) },
                  settings: { localMarks: suffixLocalMarks(config, provider.id, added) },
                  ...(main ? { roles: { main: { providerId: provider.id, model: main, capabilities: null } } } : {}) });
    onClose(true);
  };
  return (
    <div className="modal" onClick={() => onClose(false)}>
      <div className="modalbox" onClick={(e) => e.stopPropagation()}>
        <h3>{L("选择要添加的模型：", "Add models: ")}{provider.name}</h3>
        {err && <div className="notice err small">{err}</div>}
        {!list && !err && <div className="muted">{L("正在获取模型列表…", "Fetching models…")}</div>}
        {list && (
          <>
            <label className="fld"><span>{L("服务名称", "Provider name")}</span><input value={name} onChange={(e) => setName(e.target.value)} /></label>
            <div className="row">
              <input placeholder={L("搜索", "Search")} value={q} onChange={(e) => setQ(e.target.value)} />
              <select value={kind} onChange={(e) => setKind(e.target.value)}>
                {[["chat", L("对话", "Chat")], ["embedding", "Embedding"], ["rerank", "Rerank"], ["other", L("其他", "Other")], ["all", L("全部", "All")]].map(([k, t]) => <option key={k} value={k}>{t}</option>)}
              </select>
            </div>
            <div className="picklist">
              {shown.map((m) => (
                <label key={m.id} className="row small">
                  <input type="checkbox" checked={sel.has(m.id)} onChange={(e) => { const s = new Set(sel); e.target.checked ? s.add(m.id) : s.delete(m.id); setSel(s); }} />
                  <span className="mono">{m.id}</span>{m.local && <span className="chip">{L("本地", "local")}</span>}
                </label>
              ))}
            </div>
            <div className="row small">
              <button onClick={() => setSel(new Set([...sel, ...shown.map((m) => m.id)]))}>{L("全选当前", "Select shown")}</button>
              <button onClick={() => setSel(new Set())}>{L("清空", "Clear")}</button>
              <button title={L("Qwen / DeepSeek 对话模型（不含 VL）", "Qwen / DeepSeek chat models (no VL)")} onClick={() => setSel(new Set([...sel, ...recommended(list.map((m) => m.id))]))}>{L("推荐", "Recommended")}</button>
              <span className="muted grow">{sel.size} {L("个已选", "selected")}</span>
            </div>
          </>
        )}
        <div className="row end"><button onClick={() => onClose(false)}>{L("取消", "Cancel")}</button><button className="primary" disabled={!list || !sel.size} onClick={save}>{L("添加", "Add")}</button></div>
      </div>
    </div>
  );
}

function ProviderEditor({ p, isNew, onDone }: { p: any; isNew: boolean; onDone: () => void }) {
  const config = useApp((s) => s.config);
  const [v, setV] = useState({ ...p });
  const [key, setKey] = useState("");
  const [msg, setMsg] = useState<string>();
  const [pick, setPick] = useState(false);
  const [login, setLogin] = useState(false);
  const added: string[] = config?.modelCache?.[p.id] ?? [];
  const marks = config?.settings?.localMarks ?? {};
  const save = async () => {
    if (!v.name?.trim() || !v.baseURL?.trim()) { setMsg(L("名称和 Base URL 必填", "Name and Base URL are required")); return false; }
    const providers = [...(config?.providers ?? []).filter((x: any) => x.id !== v.id), { ...v, name: v.name.trim(), baseURL: v.baseURL.trim() }];
    if (!(await patch({ providers }))) return false;
    if (key.trim()) await core.request("secret/set", { kind: "api_key", id: v.id, value: key.trim() }).catch(useApp.getState().fail);
    setKey("");
    setMsg("✅ " + L("已保存", "Saved"));
    return true;
  };
  const remove = async () => {
    if (!(await ask(L(`删除模型服务「${p.name}」？已添加的模型和对应的角色设置也会移除。`, `Delete “${p.name}”?`)))) return;
    const roles = config?.roles ?? {};
    const r: any = {};
    for (const k of ["main", "fast", "vision"]) if (roles[k]?.providerId === p.id) r[k] = null;
    const lm: any = {};
    for (const k of Object.keys(marks)) if (k.startsWith(p.id + "|")) lm[k] = null;
    await patch({ providers: (config?.providers ?? []).filter((x: any) => x.id !== p.id), modelCache: { [p.id]: null }, availableModels: { [p.id]: null }, roles: r, settings: { localMarks: lm } });
    await core.request("secret/set", { kind: "api_key", id: p.id, value: null }).catch(() => {});
    onDone();
  };
  return (
    <div className="pane">
      <label className="fld"><span>{L("名称", "Name")}</span><input value={v.name ?? ""} onChange={(e) => setV({ ...v, name: e.target.value })} /></label>
      <label className="fld"><span>Base URL</span><input value={v.baseURL ?? ""} placeholder="https://your-newapi.example.com/v1" onChange={(e) => setV({ ...v, baseURL: e.target.value })} /></label>
      <label className="fld"><span>API Key</span>
        <input type="password" autoComplete="off" value={key} placeholder={isNew ? "" : L("已保存在钥匙串（留空不修改）", "stored in the keychain (leave empty to keep)")} onChange={(e) => setKey(e.target.value)} />
      </label>
      <label className="fld"><span>{L("网络代理", "Proxy")}</span>
        <select value={v.proxyMode ?? "global"} onChange={(e) => setV({ ...v, proxyMode: e.target.value })}>
          <option value="global">{L("跟随全局设置", "Follow global")}</option><option value="none">{L("直连", "Direct")}</option><option value="custom">{L("自定义", "Custom")}</option>
        </select>
      </label>
      {v.proxyMode === "custom" && <label className="fld"><span>{L("代理地址", "Proxy")}</span><input value={v.proxyURL ?? ""} placeholder="host:port" onChange={(e) => setV({ ...v, proxyURL: e.target.value })} /></label>}
      <div className="muted small">{L("API Key 保存在 macOS 钥匙串中（通过系统 security 工具读写，App 更新后不会反复弹窗）。本地/内网模型建议选“直连”。", "The API key is stored in the macOS Keychain. Choose “Direct” for local/LAN models.")}</div>
      <div className="row wrap">
        <button className="primary" onClick={save}>{L("保存", "Save")}</button>
        <button disabled={!v.baseURL} onClick={async () => { if (await save()) setPick(true); }}>{L("获取并添加模型…", "Fetch & add models…")}</button>
        <button onClick={async () => { if (await save()) setLogin(true); }}>{L("登录并选择 Key…", "Log in & choose key…")}</button>
        {!isNew && <button className="danger" onClick={remove}>{L("删除", "Delete")}</button>}
        <Status msg={msg} />
      </div>
      <h4>{L(`已添加的模型（${added.length}）`, `Added models (${added.length})`)}</h4>
      <div className="muted small">{added.length
        ? L("点击模型右侧的「本地 / 云端」标签切换：本地模式下只会使用标为「本地」的模型。名称含 local、ollama，或服务地址是 localhost / 内网 IP 的模型默认标为本地。",
            "Click the “Local / Cloud” tag to switch. Local-only mode uses only models tagged Local. Names with local/ollama or localhost/LAN endpoints default to Local.")
        : L("尚未添加。点击“获取并添加模型…”从服务端自动拉取列表后勾选。", "None yet. Click “Fetch & add models…” to pull the list from the server and pick.")}</div>
      {added.map((m) => {
        const k = `${p.id}|${m}`;
        return (
          <div key={m} className="irow small">
            <span className="grow mono">{m}</span><span className="chip">{modelKind(m)}</span>
            <label title={L("本地模式下只能用标记为本地的模型", "Local-only mode uses only local models")}>
              <input type="checkbox" checked={isLocalModel(config, p.id, m)} onChange={(e) => patch({ settings: { localMarks: { [k]: e.target.checked } } })} /> {L("本地", "local")}
            </label>
            <IconBtn icon={X} size={13} className="danger" title={L("删除", "Remove")} onClick={() => patch({ modelCache: { [p.id]: added.filter((x) => x !== m) } })} />
          </div>
        );
      })}
      {pick && <ModelPicker provider={v} onClose={() => setPick(false)} />}
      {login && <NewAPILogin provider={v} onClose={() => setLogin(false)} />}
    </div>
  );
}

export function Providers() {
  const providers: any[] = useApp((s) => s.config?.providers) ?? [];
  const [sel, setSel] = useState<string | undefined>(providers[0]?.id);
  const [draft, setDraft] = useState<any>();
  const cur = draft ?? providers.find((p) => p.id === sel) ?? providers[0];
  return (
    <div className="split">
      <div className="list">
        {providers.map((p) => <div key={p.id} className={"irow click" + (cur?.id === p.id && !draft ? " on" : "")} onClick={() => { setDraft(undefined); setSel(p.id); }}>{p.name || p.id}</div>)}
        <button className="link row" onClick={() => setDraft({ id: rid(), name: "", baseURL: "", proxyMode: "global", extraHeaders: {} })}><Plus size={13} /> {L("添加模型服务", "Add provider")}</button>
      </div>
      {cur ? <ProviderEditor key={cur.id} p={cur} isNew={!!draft} onDone={() => { setDraft(undefined); setSel(undefined); }} />
        : <div className="muted pad">{L("添加一个 OpenAI 兼容的模型服务（New API、DeepSeek、Ollama、vLLM…）", "Add an OpenAI-compatible provider")}</div>}
    </div>
  );
}
