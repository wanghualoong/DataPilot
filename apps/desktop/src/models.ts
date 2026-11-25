// Model helpers shared by the composer, the store and settings (no imports: avoids cycles).

/** Same rule as Swift ModelCapabilities.kind(of:). */
export function modelKind(m: string): string {
  const l = m.toLowerCase();
  if (l.includes("rerank")) return "rerank";
  if (l.includes("embed") || l.startsWith("bge-") || l.includes("bge-m3") || l.includes("text-embedding")) return "embedding";
  if (["whisper", "tts", "speech", "image", "seedream", "seedance", "video", "t2v", "i2v", "r2v", "dall-e", "sora", "midjourney"].some((k) => l.includes(k))) return "other";
  return "chat";
}

/** Same rule as the core's is_local_model / Swift LocalModels: explicit mark wins, else model name or LAN endpoint. */
export function isLocalModel(config: any, providerId: string, model: string): boolean {
  const mark = config?.settings?.localMarks?.[`${providerId}|${model}`];
  if (typeof mark === "boolean") return mark;
  const m = model.toLowerCase();
  if (m.includes("local") || m.includes("ollama") || m.startsWith("lmstudio")) return true;
  return isLocalHost((config?.providers ?? []).find((p: any) => p.id === providerId)?.baseURL ?? "");
}

/** localhost / *.local / private LAN address (Swift LocalModels.isLocalHost). */
export function isLocalHost(base: string): boolean {
  let h = "";
  try { h = new URL(base).hostname.toLowerCase().replace(/^\[|\]$/g, ""); } catch { return false; }
  if (h === "localhost" || h === "::1" || h.endsWith(".local") || h.startsWith("127.")) return true;
  const p = h.split(".").map(Number);
  return p.length === 4 && p.every((x) => !isNaN(x)) && (p[0] === 10 || (p[0] === 192 && p[1] === 168) || (p[0] === 172 && p[1] >= 16 && p[1] <= 31));
}

export type ChatModel = { providerId: string; provider: string; model: string };

/** Chat models the user added (config.modelCache); in local-only mode only local ones (same as Swift allModels). */
export function chatModels(config: any, ignoreLocalOnly = false): ChatModel[] {
  const local = !ignoreLocalOnly && !!config?.settings?.localOnly;
  const out: ChatModel[] = [];
  for (const p of config?.providers ?? [])
    for (const m of config?.modelCache?.[p.id] ?? [])
      if (modelKind(m) === "chat" && (!local || isLocalModel(config, p.id, m))) out.push({ providerId: p.id, provider: p.name || p.id, model: m });
  return out;
}

/** Permission modes: same titles / details / order as the SwiftUI ModeUI. */
export const MODES = [
  { m: "plan", zh: "计划", en: "Plan", dzh: "只读探查数据，提交计划，批准后再执行", den: "Explore read-only, propose a plan, run after approval" },
  { m: "ask", zh: "询问", en: "Ask", dzh: "沙箱内自动；联网与沙箱外操作需要确认（默认）", den: "Sandboxed work automatic; network and external actions ask (default)" },
  { m: "auto", zh: "自动", en: "Auto", dzh: "沙箱内与联网自动；桌面控制、表单提交等仍需确认", den: "Sandboxed + network automatic; desktop control / form submits still ask" },
  { m: "full", zh: "完全访问", en: "Full access", dzh: "所有操作都不再询问（仅在你信任当前任务时使用）", den: "Never ask (only when you trust the task)" },
] as const;
/** ⇧⇥ cycle (full access excluded, like Claude Code's bypass mode). */
export const nextMode = (m: string) => (m === "ask" ? "auto" : m === "auto" ? "plan" : "ask");

/** "@表名" mentions that match known tables (same rule as Swift mentionedTables). */
export function mentionedTables(text: string, known: string[]): string[] {
  const lower = new Map(known.map((k) => [k.toLowerCase(), k]));
  const out: string[] = [];
  for (const m of text.matchAll(/@([^\s@，。,；;：:！!？?、()（）]+)/g)) {
    const t = lower.get(m[1].toLowerCase());
    if (t && !out.includes(t)) out.push(t);
  }
  return out;
}

/** Provider preset of the first-run wizard / quick login. `login`: New API gateway (log in & pick a key);
 *  `loginHint`: text next to the login button (e.g. which account to use). */
export type Preset = { id: string; zh: string; en: string; providerName: string; baseURL: string; proxyMode: string; login?: boolean; loginHint?: { zh: string; en: string } };

/** Neutral presets, used until the core's `app/defaults` (Resources/defaults.json + optional defaults.local.json) arrives. */
export const FALLBACK_PRESETS: Preset[] = [
  { id: "custom", zh: "自定义 / New API / vLLM", en: "Custom / New API / vLLM", providerName: "New API", baseURL: "", proxyMode: "global", login: true },
  { id: "ollama", zh: "本地 Ollama", en: "Local Ollama", providerName: "Ollama", baseURL: "http://localhost:11434/v1", proxyMode: "none" },
];

export function presets(defaults?: { providerPresets?: Preset[] }): Preset[] {
  const p = defaults?.providerPresets;
  return Array.isArray(p) && p.length ? p : FALLBACK_PRESETS;
}

/** The chat model added (and selected) by default: id ends with "local", prefer qwen3 (Swift AppModel.defaultLocalModel). */
export function defaultLocalModel(models: string[]): string | undefined {
  const c = models.filter((m) => modelKind(m) === "chat" && m.toLowerCase().endsWith("local"));
  return c.find((m) => m.toLowerCase().includes("qwen3")) ?? [...c].sort()[0];
}

/** "*local" models get an explicit local mark unless the user already set one (Swift markSuffixLocal). */
export function suffixLocalMarks(config: any, providerId: string, models: string[]): Record<string, boolean> {
  const out: Record<string, boolean> = {};
  for (const m of models) {
    const k = `${providerId}|${m}`;
    if (m.toLowerCase().endsWith("local") && config?.settings?.localMarks?.[k] == null) out[k] = true;
  }
  return out;
}

export const normURL = (u: string) => u.trim().replace(/[/ ]+$/, "").toLowerCase();
