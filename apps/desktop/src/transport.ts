// One request channel + one notification stream to datapilot-core.
// Tauri: `core_request` command / `core-event` event. Browser (development): dev-bridge.mjs over WebSocket.
export type Notification = { method: string; params: any };
type Listener = (n: Notification) => void;

export interface Transport {
  request<T = any>(method: string, params?: object): Promise<T>;
  subscribe(fn: Listener): () => void;
  kind: "tauri" | "bridge";
}

const listeners = new Set<Listener>();
const fanout = (n: Notification) => listeners.forEach((l) => l(n));

function tauriTransport(): Transport {
  let ready: Promise<void> | null = null;
  const init = () =>
    (ready ??= import("@tauri-apps/api/event").then(({ listen }) =>
      listen<any>("core-event", (e) => fanout({ method: e.payload.method, params: e.payload.params })).then(() => {}),
    ));
  return {
    kind: "tauri",
    async request(method, params = {}) {
      await init();
      const { invoke } = await import("@tauri-apps/api/core");
      return invoke("core_request", { method, params });
    },
    subscribe(fn) { init(); listeners.add(fn); return () => listeners.delete(fn); },
  };
}

function bridgeTransport(url: string): Transport {
  let ws: WebSocket | null = null;
  let opening: Promise<WebSocket> | null = null;
  let next = 0;
  const pending = new Map<number, { ok: (v: any) => void; err: (e: Error) => void }>();
  const connect = () =>
    (opening ??= new Promise<WebSocket>((resolve, reject) => {
      const s = new WebSocket(url);
      s.onopen = () => { ws = s; resolve(s); };
      s.onerror = () => reject(new Error("dev bridge not reachable: " + url));
      s.onclose = () => {
        ws = null; opening = null;
        pending.forEach((p) => p.err(new Error("bridge closed")));
        pending.clear();
      };
      s.onmessage = (m) => {
        const v = JSON.parse(m.data);
        if (typeof v.id === "number" && !v.method) {
          const p = pending.get(v.id);
          pending.delete(v.id);
          if (!p) return;
          v.error ? p.err(new Error(v.error.message ?? String(v.error))) : p.ok(v.result);
        } else if (v.method) fanout({ method: v.method, params: v.params });
      };
    }));
  return {
    kind: "bridge",
    async request(method, params = {}) {
      const s = ws ?? (await connect());
      const id = ++next;
      return new Promise((ok, err) => {
        pending.set(id, { ok, err });
        s.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
      });
    },
    subscribe(fn) { connect().catch(() => {}); listeners.add(fn); return () => listeners.delete(fn); },
  };
}

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
function bridgeUrl() {
  const q = new URLSearchParams(typeof location !== "undefined" ? location.search : "");
  return `ws://127.0.0.1:${q.get("bridgePort") ?? "5174"}/?token=${encodeURIComponent(q.get("bridge") ?? "")}`;
}
export const core: Transport = isTauri ? tauriTransport() : bridgeTransport(bridgeUrl());
