// Composer bottom bar: approval mode, model, thinking, network, local-only, context meter (same row as the SwiftUI app).
import { useEffect, useRef, useState } from "react";
import { ClipboardList, Cpu, Hand, Lock, LockOpen, ShieldAlert, Zap, type LucideIcon } from "lucide-react";
import { useApp } from "../store";
import { L } from "../i18n";
import { ask } from "../native";
import { MODES, chatModels } from "../models";
import { MenuHead, MenuItem, MenuSep, Popover, Switch, ic } from "./ui";
import type { Mode } from "../protocol/rpc";

export const MODE_ICONS: Record<string, LucideIcon> = { plan: ClipboardList, ask: Hand, auto: Zap, full: ShieldAlert };
export const modeInfo = (m: string) => MODES.find((x) => x.m === m) ?? MODES[1];

export function ModeMenu() {
  const id = useApp((s) => s.selected);
  const mode = useApp((s) => (id ? s.open[id]?.mode : undefined)) ?? "ask";
  const setMode = useApp((s) => s.setMode);
  const cur = modeInfo(mode);
  const Icon = MODE_ICONS[mode] ?? Hand;
  return (
    <Popover up className={"modepop mode-" + mode} title={L("审批模式（⇧⇥ 切换）：", "Approval mode (⇧⇥ to cycle): ") + L(cur.dzh, cur.den)}
      label={<><Icon {...ic} size={13} /><span className="mlabel">{L(cur.zh, cur.en)}</span></>}>
      {MODES.filter((x) => x.m !== "full").map((x) => (
        <MenuItem key={x.m} icon={MODE_ICONS[x.m]} checked={mode === x.m} hint={L(x.dzh, x.den)} onClick={() => setMode(x.m as Mode)}>{L(x.zh, x.en)}</MenuItem>
      ))}
      <MenuSep />
      <MenuItem icon={ShieldAlert} danger checked={mode === "full"} hint={L(MODES[3].dzh, MODES[3].den)}
        onClick={async () => { if (await ask(L("完全访问：所有操作（含沙箱外命令、桌面控制）都不再询问。确定开启？", "Full access: nothing asks for approval any more (incl. external commands). Enable?"))) setMode("full"); }}>
        {L("完全访问", "Full access")}
      </MenuItem>
    </Popover>
  );
}

export function ModelMenu() {
  const config = useApp((s) => s.config);
  const setMainModel = useApp((s) => s.setMainModel);
  const openSettings = useApp((s) => s.openSettings);
  const models = chatModels(config);
  const cur = config?.roles?.main;
  const groups: { id: string; name: string; models: string[] }[] = [];
  for (const m of models) {
    const g = groups.find((x) => x.id === m.providerId);
    if (g) g.models.push(m.model); else groups.push({ id: m.providerId, name: m.provider, models: [m.model] });
  }
  return (
    <Popover up className="modelpop" title={L("主模型（下一轮生效）", "Main model (from the next turn)")}
      label={<><Cpu {...ic} size={13} /><span className="ell">{cur?.model ?? L("选择模型", "Pick a model")}</span></>}>
      {groups.map((g) => (
        <div key={g.id}>
          <MenuHead>{g.name}</MenuHead>
          {g.models.map((m) => (
            <MenuItem key={m} checked={cur?.providerId === g.id && cur?.model === m} onClick={() => setMainModel(g.id, m)}>{m}</MenuItem>
          ))}
        </div>
      ))}
      {!groups.length && <MenuHead>{config?.settings?.localOnly ? L("没有标记为本地的模型", "No local models") : L("请先在设置中添加模型服务", "Add a provider in Settings first")}</MenuHead>}
      <MenuSep />
      <MenuItem onClick={() => openSettings("providers")}>{L("管理模型服务…", "Manage providers…")}</MenuItem>
    </Popover>
  );
}

export function ControlBar() {
  const id = useApp((s) => s.selected);
  const chat = useApp((s) => (id ? s.chats[id] : undefined));
  const local = useApp((s) => !!s.config?.settings?.localOnly);
  const { setThinking, setNetwork, setLocalOnly } = useApp();
  const u = chat?.usage;
  const pct = u ? u.context_tokens / Math.max(1, u.context_window) : 0;
  // narrow column: keep every control intact, shed text instead (mode label, meter bar, then switch labels)
  const ref = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(999);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setW(e.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const size = w < 290 ? " tiny narrow" : w < 540 ? " narrow" : "";
  return (
    <div className={"cbar" + size} ref={ref}>
      <ModeMenu />
      <ModelMenu />
      <Switch on={!!chat?.thinking} onChange={setThinking} label={L("思考", "Thinking")} title={L("思考模式：让模型先推理再回答（更准但更慢）", "Thinking: the model reasons before answering (more accurate, slower)")} />
      <Switch on={!!chat?.network} disabled={local} onChange={setNetwork} label={L("联网", "Network")}
        title={L("联网总开关：关闭时任何工具都不能访问互联网（网页抓取、浏览器、Python/bash）；调用模型不受影响。", "Master network switch: when off, no tool can reach the internet (web fetch, browser, Python/bash). Model API calls are unaffected.")} />
      <button className={"pill" + (local ? " on" : "")} onClick={() => setLocalOnly(!local)}
        title={L("本地模式：只允许调用标记为本地的模型，数据不出本机；联网工具禁用", "Local-only: only models marked local are called; network tools disabled")}>
        {local ? <Lock {...ic} size={12} /> : <LockOpen {...ic} size={12} />}<span className="plabel">{L("本地模式", "Local mode")}</span>
      </button>
      <span className="grow" />
      {u && u.context_window > 0 && (
        <span className="meter" title={L(`上下文占用：${u.context_tokens} / ${u.context_window} tokens`, `Context: ${u.context_tokens} / ${u.context_window} tokens`)}>
          <span className="bar"><span style={{ width: `${Math.min(100, pct * 100)}%` }} className={pct > 0.8 ? "hot" : ""} /></span>{Math.round(pct * 100)}%
        </span>
      )}
    </div>
  );
}
