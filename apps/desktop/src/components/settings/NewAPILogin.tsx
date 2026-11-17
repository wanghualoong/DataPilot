import { useEffect, useRef, useState } from "react";
import { KeyRound, LoaderCircle, Plus, RefreshCw, TriangleAlert, UserRoundCog } from "lucide-react";
import { Status, ic } from "../ui";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { core } from "../../transport";
import { inTauri, newapiLoginCancel, newapiLoginForce, newapiWebLogin } from "../../native";

type Stage = "checking" | "web" | "loading" | "keys" | "closed" | "failed";

/** Log in to a New API gateway and pick (or create) an API key — port of the SwiftUI NewAPILoginView.
 *  The gateway's own login page opens in a separate window, so every login method it offers works (password,
 *  Duke NetID / OIDC, GitHub …). The full key goes from the gateway straight into the keychain inside the core;
 *  this view only sees masked keys, and the login credential stays in memory. */
export function NewAPILogin({ provider, onClose, onUsed }: { provider: any; onClose: () => void; onUsed?: () => void }) {
  const origin = (() => { try { return new URL(provider.baseURL).origin; } catch { return ""; } })();
  const [stage, setStage] = useState<Stage>("checking");
  const [info, setInfo] = useState<any>({});
  const [text, setText] = useState("");
  const [err, setErr] = useState<string>();
  const [auth, setAuth] = useState<any>();
  const [tokens, setTokens] = useState<any[]>([]);
  const [sel, setSel] = useState<number>();
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  const infoRef = useRef<any>({});
  // browser / dev-bridge fallback: password or access token
  const [user, setUser] = useState(""); const [pw, setPw] = useState(""); const [token, setToken] = useState(""); const [uid, setUid] = useState("");

  const methods = (d: any) => {
    const m: string[] = [];
    if (d.password_login_enabled ?? true) m.push(L("账号密码", "Password"));
    if (d.oidc_enabled) m.push(d.oidc_display_name || "OIDC");
    if (d.github_oauth) m.push("GitHub");
    if (d.linuxdo_oauth) m.push("LinuxDO");
    if (d.wechat_login) m.push(L("微信", "WeChat"));
    if (d.telegram_oauth) m.push("Telegram");
    return m;
  };
  const loadTokens = async (a: any) => {
    const r = await core.request("newapi/tokens", { origin, auth: a });
    setTokens(r.tokens);
    return r.tokens as any[];
  };
  const afterLogin = async (a: any) => {
    setStage("loading"); setText(L("正在读取你的 Key…", "Reading your keys…"));
    try {
      setAuth(a);
      const ts = await loadTokens(a);
      if (!alive.current) return;
      setStage("keys");
      setText(ts.length ? L("选择一个 Key（列表中的 Key 已脱敏，确认后才会取回完整 Key 并存入钥匙串）。", "Pick a key (masked here; the full key is fetched only on confirm and stored in the keychain).")
                        : L("你还没有 Key，可以新建一个 DataPilot 专用 Key。", "You have no keys yet — create one for DataPilot."));
    } catch (e) { setErr((e as Error).message); setStage(inTauri ? "closed" : "web"); }
  };
  const webLogin = async (fresh = false) => {
    if (!inTauri) { setStage("web"); return; }
    setErr(undefined); setStage("web");
    setText(L(`请在弹出的网关窗口中登录（可用：${methods(infoRef.current).join(" / ")}）。登录成功后会自动列出你的 Key。`,
              `Log in in the gateway window (available: ${methods(infoRef.current).join(" / ")}). Your keys will be listed automatically.`));
    try { const a = await newapiWebLogin(origin, fresh); if (alive.current) await afterLogin(a); }
    catch (e) {
      if (!alive.current) return;
      const m = String((e as any)?.message ?? e);
      if (m !== "cancelled") setErr(m);
      setStage("closed");
    }
  };
  const start = async () => {
    setStage("checking"); setErr(undefined);
    try {
      const d = (await core.request("newapi/status", { origin })).status ?? {};
      if (!alive.current) return;
      setInfo(d); infoRef.current = d;
      await webLogin();
    } catch (e) { setErr((e as Error).message); setStage("failed"); }
  };
  useEffect(() => {
    alive.current = true;
    const off = core.subscribe((n) => { if (n.method === "newapi/login_status" && alive.current) setText(n.params.text); });
    if (origin) start(); else { setErr(L("Base URL 无效", "Invalid Base URL")); setStage("failed"); }
    return () => { alive.current = false; off(); newapiLoginCancel(); };
  }, []);

  const close = () => { newapiLoginCancel(); onClose(); };
  const use = async (t: any) => {
    if (t.status !== 1) { setErr(L(`这个 Key 当前不可用（${statusText(t.status)}）`, `This key is not usable (${statusText(t.status)})`)); return; }
    setBusy(true); setErr(undefined);
    try {
      const r = await core.request("newapi/use", { origin, auth, token_id: t.id, provider_id: provider.id });
      useApp.getState().note(L(`「${provider.name}」的 API Key 已更新：`, `API key for “${provider.name}” updated: `) + r.masked);
      if (onUsed) onUsed(); else onClose();
    } catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  };
  const create = async () => {
    setBusy(true); setErr(undefined);
    try { const r = await core.request("newapi/create", { origin, auth, name: "DataPilot" }); await loadTokens(auth); setSel(r.token?.id); }
    catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  };
  const switchAccount = async () => {
    await core.request("newapi/logout", { origin, auth }).catch(() => {});
    setAuth(undefined); setTokens([]); setSel(undefined);
    webLogin(true);
  };
  const passwordLogin = async () => {
    setBusy(true); setErr(undefined);
    try {
      const a = token.trim() ? { access_token: token.trim(), user_id: uid ? Number(uid) : undefined }
                             : (await core.request("newapi/login", { origin, username: user, password: pw })).auth;
      setPw(""); await afterLogin(a);
    } catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  };

  const qpu = Number(info.quota_per_unit) || 0;
  const quota = (t: any) => t.unlimited_quota ? L("不限", "Unlimited") : info.display_in_currency && qpu > 0 ? `$${(t.remain_quota / qpu).toFixed(2)}` : String(Math.round(t.remain_quota ?? 0));
  const expiry = (t: any) => (t.expired_time ?? -1) <= 0 ? L("永不过期", "Never") : new Date(t.expired_time * 1000).toLocaleDateString();
  const models = (t: any) => {
    if (!t.model_limits_enabled || !t.model_limits) return L("全部模型", "All models");
    const ms = String(t.model_limits).split(",");
    return ms.length <= 2 ? ms.join(", ") : `${ms.slice(0, 2).join(", ")} +${ms.length - 2}`;
  };
  const selected = tokens.find((t) => t.id === sel);

  return (
    <div className="modal" onClick={close}>
      <div className="modalbox newapi" onClick={(e) => e.stopPropagation()}>
        <div className="row">
          <KeyRound {...ic} size={22} className="accent" />
          <div className="grow">
            <h3>{L("登录并选择 Key", "Log in & choose a key")}</h3>
            <div className="muted small">{(info.system_name ? info.system_name + " · " : "") + (origin ? new URL(origin).host : provider.baseURL)}</div>
          </div>
          {stage === "web" && inTauri && <button title={L("已在网关窗口里登录、但这里没有继续时点这个", "Click if you are logged in but nothing happens")} onClick={() => newapiLoginForce()}>{L("我已登录，继续", "I'm logged in — continue")}</button>}
          {stage === "keys" && inTauri && <button onClick={switchAccount}><UserRoundCog {...ic} size={14} /> {L("切换账号", "Switch account")}</button>}
          <button onClick={close}>{L("取消", "Cancel")}</button>
        </div>
        {text && stage !== "failed" && <div className="muted small">{text}</div>}
        {err && <Status msg={"❌ " + err} />}

        {(stage === "checking" || stage === "loading") && <div className="center pad"><LoaderCircle size={28} className="spin muted" /></div>}
        {stage === "failed" && (
          <div className="center pad col">
            <TriangleAlert size={30} className="warn" />
            <button onClick={start}>{L("重试", "Retry")}</button>
          </div>
        )}
        {stage === "web" && inTauri && (
          <div className="center pad col">
            <LoaderCircle size={22} className="spin muted" />
            <div className="muted small">{L("等待你在网关窗口中完成登录…（关闭该窗口即取消）", "Waiting for you to log in in the gateway window… (close it to cancel)")}</div>
          </div>
        )}
        {stage === "closed" && (
          <div className="center pad col">
            <div className="muted small">{L("登录窗口已关闭。", "The login window was closed.")}</div>
            <div className="row"><button className="primary" onClick={() => webLogin()}>{L("重新打开登录窗口", "Open the login window again")}</button>
              <button onClick={() => webLogin(true)}>{L("换个账号登录", "Log in with another account")}</button></div>
          </div>
        )}
        {stage === "web" && !inTauri && (
          <>
            <label className="fld"><span>{L("用户名", "Username")}</span><input value={user} onChange={(e) => setUser(e.target.value)} /></label>
            <label className="fld"><span>{L("密码", "Password")}</span><input type="password" value={pw} onChange={(e) => setPw(e.target.value)} onKeyDown={(e) => e.key === "Enter" && passwordLogin()} /></label>
            <details><summary className="small">{L("开启了两步验证 / 第三方登录？改用访问令牌", "2FA / OAuth? Use an access token")}</summary>
              <label className="fld"><span>{L("访问令牌（个人设置 → 生成令牌）", "Access token (Settings → generate)")}</span><input type="password" value={token} onChange={(e) => setToken(e.target.value)} /></label>
              <label className="fld"><span>{L("用户 ID", "User ID")}</span><input value={uid} onChange={(e) => setUid(e.target.value)} /></label>
            </details>
            <div className="row end"><button className="primary" disabled={busy || (!token && (!user || !pw))} onClick={passwordLogin}>{L("登录", "Log in")}</button></div>
          </>
        )}
        {stage === "keys" && (
          <>
            <div className="tablewrap keytable">
              <table>
                <thead><tr><th>{L("名称", "Name")}</th><th>{L("状态", "Status")}</th><th>{L("剩余额度", "Remaining")}</th><th>{L("过期时间", "Expires")}</th>
                  <th>{L("可用模型", "Models")}</th><th>{L("分组", "Group")}</th><th>Key</th></tr></thead>
                <tbody>
                  {tokens.map((t) => (
                    <tr key={t.id} className={(sel === t.id ? "on " : "") + (t.status === 1 ? "" : "dim")} onClick={() => setSel(t.id)} onDoubleClick={() => use(t)}>
                      <td>{t.name || "—"}</td>
                      <td className={t.status === 1 ? "ok" : "warn"}>{statusText(t.status)}</td>
                      <td className="num">{quota(t)}</td><td>{expiry(t)}</td>
                      <td title={t.model_limits || ""}>{models(t)}</td><td>{t.group || L("默认", "default")}</td>
                      <td className="mono muted small">{t.key}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="row">
              <button disabled={busy} onClick={create}><Plus {...ic} size={14} /> {L("新建 DataPilot 专用 Key", "Create a DataPilot key")}</button>
              <button disabled={busy} onClick={async () => { setBusy(true); try { await loadTokens(auth); } catch (e) { setErr((e as Error).message); } finally { setBusy(false); } }}>
                <RefreshCw {...ic} size={14} className={busy ? "spin" : undefined} /> {L("刷新", "Refresh")}</button>
              <span className="grow muted small">{L("完整 Key 只保存在系统钥匙串；登录凭据只在内存中。", "The full key is stored only in the keychain; the login credential stays in memory.")}</span>
              <button className="primary" disabled={busy || !selected || selected.status !== 1} onClick={() => selected && use(selected)}>{L("使用所选 Key", "Use selected key")}</button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

const statusText = (s: number) => ({ 1: L("可用", "Enabled"), 2: L("已禁用", "Disabled"), 3: L("已过期", "Expired"), 4: L("额度用尽", "Exhausted") } as any)[s] ?? String(s);
