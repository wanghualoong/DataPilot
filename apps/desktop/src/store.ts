// App state (zustand). All data comes from datapilot-core through `core`; the store only caches it for rendering.
import { create } from "zustand";
import { core } from "./transport";
import { addUser, answerCard, emptyChat, fromHistory, reduce, type ChatState } from "./reduce";
import type { AgentEvent, ProjectInfo, SessionInfo, Mode } from "./protocol/rpc";
import { L } from "./i18n";
import { chatModels, isLocalModel } from "./models";

interface AppState {
  sessions: SessionInfo[];
  projects: ProjectInfo[];
  selected?: string;
  chats: Record<string, ChatState>;
  open: Record<string, { mode: Mode; workspace: string }>;
  search: string;
  hits?: Set<string>;
  error?: string;
  notice?: string;
  ready: boolean;
  inspector?: { tab: string; id?: string };
  /** files dropped / picked for the next message (paths as the model sees them) */
  attached: string[];
  config?: any;
  /** bumped when a turn ends / files are imported: panels reload their data */
  dataVersion: number;
  /** settings window: open tab (undefined = closed) */
  settingsTab?: string;
  /** show the context blocks injected into each turn (eye toggle) */
  showContext: boolean;
  /** table names of the selected chat's kernel (for @ mentions) */
  tableNames: string[];
  sidebarHidden: boolean;
  /** first-run wizard (no provider or no main model) */
  onboarding: boolean;
  /** "Log in & choose key" sheet */
  quickLogin: boolean;

  init(): Promise<void>;
  reload(): Promise<void>;
  select(id: string): Promise<void>;
  newChat(projectId?: string): Promise<void>;
  send(text: string, skill?: string, hidden?: string): Promise<void>;
  cancel(): Promise<void>;
  respond(cardId: string, kind: "approval" | "question" | "plan", answer: string, feedback?: string): Promise<void>;
  withdraw(id: string): Promise<void>;
  setMode(m: Mode): Promise<void>;
  setNetwork(on: boolean): Promise<void>;
  rename(id: string, title: string): Promise<void>;
  flag(id: string, which: "pin" | "archive", on: boolean): Promise<void>;
  remove(id: string): Promise<void>;
  rewind(turnId: string): Promise<void>;
  openProject(path: string): Promise<void>;
  setSearch(q: string): Promise<void>;
  fail(e: unknown): void;
  note(t: string): void;
  openInspector(tab?: string, id?: string): void;
  attach(paths: string[]): Promise<void>;
  loadConfig(): Promise<any>;
  setMainModel(providerId: string, model: string): Promise<void>;
  setThinking(on: boolean): Promise<void>;
  setLocalOnly(on: boolean): Promise<void>;
  openSettings(tab?: string): void;
  compactNow(): Promise<void>;
  refreshTables(): Promise<void>;
  btw(question: string): Promise<void>;
  dismissBtw(id: string): void;
}

const msg = (e: unknown) => (e instanceof Error ? e.message : String(e));
let iid = 0;
let initialized = false;   // StrictMode runs effects twice in development: subscribe once

export const useApp = create<AppState>((set, get) => {
  const patchChat = (id: string, f: (c: ChatState) => ChatState) =>
    set((s) => ({ chats: { ...s.chats, [id]: f(s.chats[id] ?? emptyChat()) } }));

  // one session/open per session, even when select() and send() race
  const opening = new Map<string, Promise<void>>();
  const ensureOpen = (id: string): Promise<void> => {
    if (get().open[id]) return Promise.resolve();
    let p = opening.get(id);
    if (!p) {
      p = core.request("session/open", { id })
        .then((r) => {
          set((s) => ({ open: { ...s.open, [id]: { mode: r.mode, workspace: r.workspace } } }));
          if (r.context_tokens && r.context_window && !get().chats[id]?.usage) patchChat(id, (c) => ({ ...c, usage: { context_tokens: r.context_tokens!, context_window: r.context_window! } }));
        })
        .finally(() => opening.delete(id));
      opening.set(id, p);
    }
    return p;
  };

  return {
    sessions: [], projects: [], chats: {}, open: {}, search: "", ready: false, attached: [], dataVersion: 0, showContext: false, tableNames: [], sidebarHidden: false, onboarding: false, quickLogin: false,

    async init() {
      if (initialized) return;
      initialized = true;
      core.subscribe((n) => {
        if (n.method === "agent/event") {
          const { session, event } = n.params as { session: string; event: AgentEvent };
          patchChat(session, (c) => reduce(c, event));
          if (event.type === "title_changed") get().reload();
          // data changed mid-turn (import / python / semantic layer / derived table): panels reload at once (Swift dataVersion)
          if (event.type === "tool_finished" && ["data_import", "python", "catalog_update", "data_transform", "db_extract"].includes((event as any).message?.name ?? "")) {
            set((s) => ({ dataVersion: s.dataVersion + 1 }));
            if (session === get().selected) get().refreshTables().catch(() => {});
          }
          if (event.type === "turn_finished") {
            set((s) => ({ dataVersion: s.dataVersion + 1 }));
            get().send("", undefined).catch(() => {}); // flush pending interjections
          }
        } else if (n.method === "core/notice") set({ notice: n.params.text });
      });
      try {
        await core.request("initialize"); await get().reload(); set({ ready: true });
        const c = await get().loadConfig().catch(() => null);
        if (c && (!c.providers?.length || !c.roles?.main)) set({ onboarding: true });
      }
      catch (e) { get().fail(e); }
    },

    async reload() {
      const r = await core.request("session/list");
      set({ sessions: r.sessions, projects: r.projects });
    },

    async select(id) {
      set({ selected: id, error: undefined });
      try {
        if (!get().chats[id]) {
          const h = await core.request("session/history", { session: id });
          patchChat(id, () => fromHistory(h));
        }
        await ensureOpen(id);
      } catch (e) { get().fail(e); }
    },

    async newChat(projectId) {
      try {
        const s = await core.request("session/create", { title: L("新会话", "New chat"), project_id: projectId ?? null });
        await get().reload();
        patchChat(s.id, () => emptyChat());
        await get().select(s.id);
      } catch (e) { get().fail(e); }
    },

    async send(text, skill, hidden) {
      const id = get().selected;
      if (!id) return;
      const c = get().chats[id] ?? emptyChat();
      // empty text = called after turn_finished: send what was typed meanwhile as the next turn
      if (!text.trim() && !get().attached.length) {
        if (c.running) return;
        const r = await core.request("turn/take_pending", { session: id }).catch(() => ({ pending: [] }));
        const left = [...(r.pending ?? []), ...c.pending.filter((p) => !(r.pending ?? []).some((x: any) => x.id === p.id))];
        patchChat(id, (x) => ({ ...x, pending: [] }));
        if (!left.length) return;
        text = left.map((p: any) => p.text).join("\n");
      }
      const q = c.items.find((i: any) => i.kind === "question" && !i.answered);
      if (q && c.running && !get().attached.length) return get().respond(q.id, "question", text.trim());
      try {
        await ensureOpen(id);
        if (c.running) {   // typed while the agent works → interjection
          const pid = `i${Date.now()}-${++iid}`;
          patchChat(id, (x) => ({ ...x, pending: [...x.pending, { id: pid, text }] }));
          await core.request("turn/interject", { session: id, text, id: pid });
          return;
        }
        const files = get().attached;
        if (files.length) {
          text = L("[已上传文件] ", "[Uploaded files] ") + files.join(", ") + "\n\n" + (text.trim() ? text : L("请导入并概览这些数据。", "Please import and summarize these files."));
          set({ attached: [] });
        }
        patchChat(id, (x) => ({ ...addUser(x, text), running: true }));
        await core.request("turn/start", { session: id, text: text + (hidden ?? ""), ...(skill ? { skill } : {}) });
      } catch (e) {
        patchChat(id, (x) => ({ ...x, running: false }));
        get().fail(e);
      }
    },

    async cancel() {
      const id = get().selected;
      if (id) await core.request("turn/cancel", { session: id }).catch(get().fail);
    },

    async respond(cardId, kind, answer, feedback) {
      const id = get().selected;
      if (!id) return;
      patchChat(id, (c) => answerCard(c, cardId, answer));
      const method = kind === "approval" ? "approval/respond" : "question/answer";
      const p = kind === "approval" ? { session: id, id: cardId, choice: answer, feedback } : { session: id, id: cardId, answer };
      await core.request(method, p).catch(get().fail);
    },

    async withdraw(pid) {
      const id = get().selected;
      if (!id) return;
      const r = await core.request("turn/withdraw", { session: id, id: pid }).catch(() => ({ withdrawn: false }));
      if (r.withdrawn) patchChat(id, (c) => ({ ...c, pending: c.pending.filter((p) => p.id !== pid) }));
    },

    async setMode(m) {
      const id = get().selected;
      if (!id) return;
      await core.request("mode/set", { session: id, mode: m }).catch(get().fail);
      set((s) => ({ open: { ...s.open, [id]: { ...s.open[id], mode: m } } }));
    },

    async setNetwork(on) {
      const id = get().selected;
      if (!id) return;
      await core.request("network/set", { session: id, enabled: on }).catch(get().fail);
      patchChat(id, (c) => ({ ...c, network: on }));
    },

    async rename(id, title) { await core.request("session/rename", { session: id, title }).catch(get().fail); await get().reload(); },
    async flag(id, which, on) { await core.request(`session/${which}`, { session: id, on }).catch(get().fail); await get().reload(); },

    async remove(id) {
      await core.request("session/delete", { session: id }).catch(get().fail);
      set((s) => {
        const chats = { ...s.chats }; delete chats[id];
        const open = { ...s.open }; delete open[id];
        return { chats, open, selected: s.selected === id ? undefined : s.selected };
      });
      await get().reload();
    },

    async rewind(turnId) {
      const id = get().selected;
      if (!id) return;
      try {
        const r = await core.request("session/rewind", { session: id, turn_id: turnId });
        const h = await core.request("session/history", { session: id });
        patchChat(id, () => fromHistory(h));
        set({ notice: r.workspace_restored ? L("已回退到这条消息之前（数据也已恢复）", "Rewound to before this message (data restored too)") : L("已回退到这条消息之前", "Rewound to before this message") });
      } catch (e) { get().fail(e); }
    },

    async openProject(path) {
      try { const p = await core.request("project/open", { path }); await get().reload(); await get().newChat(p.id); }
      catch (e) { get().fail(e); }
    },

    async setSearch(q) {
      set({ search: q });
      if (!q.trim()) return set({ hits: undefined });
      const r = await core.request("session/search", { q }).catch(() => ({ hits: [] }));
      if (get().search === q) set({ hits: new Set(r.hits.map((h: any) => h.session_id)) });
    },

    fail(e) { set({ error: msg(e) }); },
    note(t) { const id = get().selected; if (id) patchChat(id, (c) => reduce(c, { type: "notice", text: t })); else set({ notice: t }); },
    openInspector(tab, id) { set({ inspector: tab ? { tab, id } : undefined }); },

    async attach(paths) {
      const id = get().selected;
      if (!id || !paths.length) return;
      try {
        const r = await core.request("files/import", { session: id, paths });
        set((s) => ({ attached: [...s.attached, ...r.files.filter((f: string) => !s.attached.includes(f))], dataVersion: s.dataVersion + 1 }));
        if (r.failed?.length) get().fail(L("无法导入：", "Could not import: ") + r.failed.join(", "));
      } catch (e) { get().fail(e); }
    },

    async loadConfig() { const c = await core.request("config/get"); set({ config: c }); return c; },

    // main model for new turns: config.json roles.main (same as the SwiftUI menu) + every open chat
    async setMainModel(providerId, model) {
      try {
        await core.request("config/set", { patch: { roles: { main: { providerId, model, capabilities: null } } } });
        for (const sid of Object.keys(get().open)) await core.request("model/set", { session: sid }).catch(() => {});
        await get().loadConfig();
        get().note(L("主模型已切换为 ", "Main model switched to ") + model);
      } catch (e) { get().fail(e); }
    },
    // same rules as the SwiftUI AppModel.setLocalOnly: needs a local chat model; main / fast switched to local ones; network off
    async setLocalOnly(on) {
      const cfg = get().config ?? (await get().loadConfig());
      const patch: any = { settings: { localOnly: on } };
      if (on) {
        const locals = chatModels(cfg, true).filter((m) => isLocalModel(cfg, m.providerId, m.model));
        if (!locals.length) { get().note("⚠️ " + L("没有标记为本地的模型。请在 设置 → 模型服务 的“已添加的模型”中勾选“本地”。", "No local models yet. Mark models as local in Settings → Providers.")); return; }
        const main = cfg?.roles?.main, fast = cfg?.roles?.fast;
        if (!main || !isLocalModel(cfg, main.providerId, main.model)) patch.roles = { main: { providerId: locals[0].providerId, model: locals[0].model, capabilities: null } };
        if (fast && !isLocalModel(cfg, fast.providerId, fast.model)) patch.roles = { ...(patch.roles ?? {}), fast: null };
      }
      try {
        await core.request("config/set", { patch });
        await get().loadConfig();
        for (const sid of Object.keys(get().open)) {
          await core.request("model/set", { session: sid }).catch(() => {});
          if (on) { await core.request("network/set", { session: sid, enabled: false }).catch(() => {}); patchChat(sid, (c) => ({ ...c, network: false })); }
        }
        get().note(on ? L("🔒 已开启本地模式：只使用本地模型，联网工具已禁用", "🔒 Local-only mode on: local models only, network tools disabled") : L("已关闭本地模式", "Local-only mode off"));
      } catch (e) { get().fail(e); }
    },
    // /btw: side question answered from the chat context without tools; shown as a card, not saved (Swift BtwCard)
    async btw(question) {
      const sid = get().selected;
      if (!sid) return;
      const id = "btw" + ++iid;
      patchChat(sid, (c) => ({ ...c, items: [...c.items, { kind: "btw", id, text: question, running: true }] }));
      let output = "", is_error = false;
      try { output = (await core.request("session/btw", { session: sid, question })).text; } catch (e) { output = msg(e); is_error = true; }
      patchChat(sid, (c) => ({ ...c, items: c.items.map((it) => (it.id === id ? ({ ...it, output, is_error, running: false } as any) : it)) }));
    },
    dismissBtw(id) {
      const sid = get().selected;
      if (sid) patchChat(sid, (c) => ({ ...c, items: c.items.filter((it) => !(it.kind === "btw" && it.id === id)) }));
    },
    openSettings(tab) { set({ settingsTab: tab ?? "providers" }); },
    async compactNow() {
      const id = get().selected;
      if (!id || get().chats[id]?.running) return;
      await core.request("compact", { session: id }).catch(get().fail);
    },
    async refreshTables() {
      const id = get().selected;
      if (!id) return;
      const r = await core.request("kernel/call", { session: id, op: "tables", timeout: 20 }).catch(() => null);
      set({ tableNames: (r?.result?.tables ?? []).map((t: any) => t.table) });
    },
    async setThinking(on) {
      const id = get().selected;
      if (!id) return;
      await core.request("model/set", { session: id, thinking: on }).catch(get().fail);
      patchChat(id, (c) => ({ ...c, thinking: on }));
    },
  };
});
