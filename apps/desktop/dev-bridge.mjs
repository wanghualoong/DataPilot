// Development bridge: runs the real datapilot-core and relays JSON-RPC over a local WebSocket, so the React UI can
// run in a normal browser (and in Playwright). Loopback only, random token required. Not shipped in the app.
//   node dev-bridge.mjs [--port 5174] [--app-support DIR]
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createWriteStream, mkdirSync } from "node:fs";
import { createInterface } from "node:readline";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";
import { WebSocketServer } from "ws";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../..");
const arg = (k, d) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : d; };
const port = Number(arg("--port", process.env.DP_BRIDGE_PORT ?? "5174"));
const support = arg("--app-support", process.env.DP_APP_SUPPORT ?? join(homedir(), "Library/Application Support/DataPilot"));
const token = process.env.DP_BRIDGE_TOKEN ?? randomBytes(12).toString("hex");
mkdirSync(support, { recursive: true });

const env = { ...process.env, DP_RESOURCES: join(repo, "Resources"), DP_PYTHON_ROOT: join(repo, "build/vendor/python"), DP_APP_SUPPORT: support };
if (!process.env.DP_BRIDGE_KEEP_API_KEY) delete env.DP_API_KEY;
const core = spawn(process.env.DP_CORE ?? join(repo, "datapilot-core/target/release/datapilot-core"), [], { env, stdio: ["pipe", "pipe", "pipe"] });
core.stderr.pipe(createWriteStream(join(support, "core.log"), { flags: "a" }));
core.on("exit", (c) => { console.error("core exited", c); process.exit(1); });

// browser request id → {socket, id}; core ids are our own counter
let next = 0;
const pending = new Map();
const clients = new Set();
createInterface({ input: core.stdout }).on("line", (line) => {
  let v; try { v = JSON.parse(line); } catch { return; }
  if (typeof v.id === "number" && !v.method) {
    const p = pending.get(v.id); pending.delete(v.id);
    if (p && p.ws.readyState === 1) p.ws.send(JSON.stringify({ ...v, id: p.id }));
  } else for (const c of clients) if (c.readyState === 1) c.send(line);
});

const wss = new WebSocketServer({ host: "127.0.0.1", port });
wss.on("connection", (ws, req) => {
  if (new URL(req.url, "http://x").searchParams.get("token") !== token) return ws.close(1008, "bad token");
  clients.add(ws);
  ws.on("close", () => clients.delete(ws));
  ws.on("message", (data) => {
    let m; try { m = JSON.parse(String(data)); } catch { return; }
    if (m.method === "shutdown") return ws.send(JSON.stringify({ jsonrpc: "2.0", id: m.id, error: { message: "not allowed" } }));
    const id = ++next;
    pending.set(id, { ws, id: m.id });
    core.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method: m.method, params: m.params ?? {} }) + "\n");
  });
});
const stop = () => { core.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: 0, method: "shutdown", params: {} }) + "\n"); setTimeout(() => process.exit(0), 3000); };
process.on("SIGINT", stop); process.on("SIGTERM", stop);
console.log(`bridge ready: ws://127.0.0.1:${port}  open http://localhost:5173/?bridge=${token}&bridgePort=${port}`);
