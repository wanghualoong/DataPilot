// End-to-end: golden scripted model + real datapilot-core (via dev-bridge) + Vite + system Chrome (playwright-core).
//   node e2e/run.mjs [case-name]     e.g. 04-interactive-cards
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const here = dirname(fileURLToPath(import.meta.url));
const app = join(here, "..");
const repo = join(app, "../..");
const caseName = process.argv[2] ?? "04-interactive-cards";
const casePath = [join(here, "cases", caseName + ".json"), join(repo, "tests/golden/cases", caseName + ".json")].find(existsSync);
const py = join(repo, "build/vendor/python/bin/python3");
const chrome = ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"].find(existsSync);
const llmPort = 22000 + (process.pid % 500), bridgePort = llmPort + 1, vitePort = 5199;
const procs = [];
const run = (cmd, args, opts = {}) => { const p = spawn(cmd, args, { stdio: ["ignore", "pipe", "pipe"], ...opts }); procs.push(p); return p; };
const waitLine = (p, re, ms = 30000) => new Promise((ok, err) => {
  const t = setTimeout(() => err(new Error("timeout waiting for " + re)), ms);
  const on = (d) => { if (re.test(String(d))) { clearTimeout(t); ok(); } };
  p.stdout.on("data", on); p.stderr.on("data", on);
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const cleanup = () => procs.forEach((p) => { try { p.kill("SIGTERM"); } catch {} });
process.on("exit", cleanup);

const support = mkdtempSync(join(tmpdir(), "dp-e2e-"));
const c = JSON.parse(await (await import("node:fs/promises")).readFile(casePath, "utf8"));
writeFileSync(join(support, "config.json"), JSON.stringify({
  providers: [{ id: "golden", name: "golden", baseURL: `http://127.0.0.1:${llmPort}`, proxyMode: "none" }],
  roles: { main: { providerId: "golden", model: "golden-model" } },
  modelCache: { golden: ["golden-model"] },
  settings: { memoryEnabled: false, defaultMode: c.mode ?? "ask", externalSkillsEnabled: false },
}));

run(py, ["-I", join(repo, "scripts/golden_llm_server.py"), String(llmPort), casePath]);
const token = "e2e" + Date.now();
const bridge = run("node", [join(app, "dev-bridge.mjs"), "--port", String(bridgePort), "--app-support", support], { env: { ...process.env, DP_BRIDGE_TOKEN: token } });
await waitLine(bridge, /bridge ready/);
const vite = run(join(app, "node_modules/.bin/vite"), ["--port", String(vitePort), "--strictPort"], { cwd: app });
await waitLine(vite, /Local:/);

const browser = await chromium.launch({ executablePath: chrome, headless: true });
const page = await (await browser.newContext({ locale: "zh-CN", viewport: { width: 1280, height: 860 } })).newPage();
const logs = [];
page.on("console", (m) => logs.push(m.type() + ": " + m.text()));
page.on("pageerror", (e) => logs.push("pageerror: " + e.message));
let ok = true;
const newChat = async () => { await page.getByRole("button", { name: /新建会话/ }).first().click(); await page.locator(".popmenu .mi").filter({ hasText: "新建对话" }).click(); };
const check = (cond, what) => { console.log((cond ? "✓ " : "✗ ") + what); if (!cond) ok = false; };
try {
  await page.goto(`http://localhost:${vitePort}/?bridge=${token}&bridgePort=${bridgePort}`);
  await newChat();
  if (c.fixtures) {   // same fixtures as the Rust golden test, into the new session's uploads/
    await page.locator(".chead").waitFor();
    const { readdirSync, statSync } = await import("node:fs");
    const dirs = readdirSync(join(support, "sessions")).map((d) => join(support, "sessions", d)).sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
    const { execFileSync } = await import("node:child_process");
    execFileSync(py, ["-I", join(repo, "tests/python/make_fixtures.py"), join(dirs[0], "workspace/uploads")]);
  }
  await page.locator("textarea").fill(c.user);
  await page.locator("textarea").press("Enter");
  const act = c.interact ?? {};
  if (act.plan) {
    await page.locator(".card.plan").first().waitFor({ timeout: 30000 });
    check((await page.locator(".card.plan").count()) === 1, `计划卡数量 ${await page.locator(".card.plan").count()}`);
    check(true, "计划卡出现");
    await page.locator(".card.plan").getByRole("button", { name: act.plan === "approve_auto" ? "批准并自动执行" : "批准并执行（逐步确认）" }).click();
  }
  if (act.question) {
    await page.locator(".card.question").waitFor({ timeout: 30000 });
    check(true, "提问卡出现");
    await page.locator(".card.question").getByRole("button", { name: act.question }).click();
  }
  if (act.approval) {
    await page.locator(".card.approval").waitFor({ timeout: 30000 });
    check(true, "审批卡出现");
    await page.locator(".card.approval").getByRole("button", { name: act.approval === "deny" ? "拒绝" : "允许一次" }).click();
  }
  // turn finished: no "正在处理" and the final text visible
  // the turn really ran (a tool card or an answer exists), then it ended
  await page.locator(".tool, .msg.assistant").first().waitFor({ timeout: 60000 });
  await page.getByText("正在处理…").waitFor({ state: "detached", timeout: 60000 });
  const want = c.expect?.text_contains;
  if (want) check(await page.locator(".msg.assistant").filter({ hasText: want }).count() > 0, `最终回答包含「${want}」`);
  const tools = await page.locator(".tool .tsum").allInnerTexts();
  check(tools.length === (c.expect?.results?.length ?? tools.length), `工具卡片数 ${tools.length}`);
  const errs = await page.locator(".tool.err").count();
  check(errs === (c.expect?.results ?? []).filter((r) => r.error).length, `出错的工具卡片数 ${errs}`);
  // title arrives, sidebar shows it; history reload after switching chats
  await sleep(1500);
  const title = await page.locator(".chead h2").innerText();
  check(title !== "新会话", `标题已更新：${title}`);
  await newChat();
  await page.locator(".srow").filter({ hasText: title }).click();
  await page.locator(".tool").first().waitFor({ timeout: 10000 });
  check((await page.locator(".tool").count()) === tools.length, "切回会话后历史完整");
  if (c.ui === "b2") {
    // chart card: the iframe renderer drew a canvas
    const fr = page.frameLocator("iframe.webframe").first();
    await fr.locator("canvas").first().waitFor({ timeout: 15000 });
    check(true, "图表卡片（iframe）已渲染");
    // SQL card actions through the kernel
    await page.getByRole("button", { name: "格式化" }).first().click();
    await page.locator(".extra pre").first().waitFor({ timeout: 20000 });
    check((await page.locator(".extra pre").first().innerText()).toUpperCase().includes("GROUP BY"), "格式化 SQL");
    await page.getByRole("button", { name: "执行计划" }).first().click();
    await page.getByText("执行计划").nth(1).waitFor({ timeout: 20000 });
    check((await page.locator(".extra pre").first().innerText()).length > 10, "执行计划");
    // no dashboard yet: the inspector lists this chat's charts → "一键生成看板" builds the session dashboard
    if (!(await page.locator(".inspector").count())) await page.getByTitle(/检查器/).click();
    await page.locator(".inspector .itabs button").filter({ hasText: "看板" }).click();
    await page.locator(".inspector").getByText("本会话图表").waitFor({ timeout: 10000 });
    await page.locator(".inspector").getByRole("button", { name: /一键生成看板/ }).click();
    await page.frameLocator(".inspector iframe.webframe").locator("canvas").first().waitFor({ timeout: 20000 });
    check(await page.locator(".inspector").getByText("本会话图表").count() === 0, "一键生成看板：生成并显示看板");
    // pin to dashboard → spec file in the workspace (same chart again: deduplicated, still confirms)
    await page.locator(".stream").getByRole("button", { name: /加入看板/ }).click();
    await page.locator(".notice").filter({ hasText: "已加入看板" }).waitFor({ timeout: 10000 });
    check(true, "加入看板");
    // slash commands
    await page.locator("textarea").fill("/sta");
    check(await page.locator(".slash").getByText("/status").count() > 0, "斜杠菜单过滤");
    await page.locator("textarea").fill("/status");
    await page.locator("textarea").press("Enter");
    await page.locator(".notice").filter({ hasText: "数据内核" }).waitFor({ timeout: 15000 });
    check(true, "/status");
    await page.locator("textarea").fill("/help");
    await page.locator("textarea").press("Enter");
    await page.locator(".notice").filter({ hasText: "/dashboard" }).waitFor({ timeout: 5000 });
    check(true, "/help");
    // new commands: /context /doctor /remember /copy /review (prompt only checked via menu)
    await page.locator("textarea").fill("/context");
    await page.locator("textarea").press("Enter");
    // (the /help notice also mentions 上下文占用, so wait for a line only the report has)
    const ctxOk = await page.locator(".notice").filter({ hasText: "内置工具定义" }).waitFor({ timeout: 15000 }).then(() => true, () => false);
    check(ctxOk, "/context 明细");
    await page.locator("textarea").fill("/doctor");
    await page.locator("textarea").press("Enter");
    await page.locator(".notice").filter({ hasText: "自检结果" }).waitFor({ timeout: 60000 });
    const doc = await page.locator(".notice").filter({ hasText: "自检结果" }).last().innerText();
    check(doc.includes("Python 运行时") && doc.includes("数据目录") && doc.includes("核心版本"), "/doctor 自检");
    await page.locator("textarea").fill("/remember 销售额口径含税");
    await page.locator("textarea").press("Enter");
    await page.locator(".notice").filter({ hasText: "已写入" }).waitFor({ timeout: 10000 });
    const { readdirSync: rd, readFileSync: rf } = await import("node:fs");
    const memFiles = rd(join(support, "memory")).filter((f) => f.endsWith(".md") && f !== "MEMORY.md");
    check(memFiles.some((f) => rf(join(support, "memory", f), "utf8").includes("销售额口径含税")), "/remember 写入记忆");
    await page.locator("textarea").fill("/rev");
    check(await page.locator(".slash").getByText("/review").count() > 0, "/review 出现在菜单");
    await page.locator("textarea").fill("");
    // theme: gear menu → dark, UI + dashboard iframes follow; back to system
    await page.getByRole("button", { name: "设置与账号" }).click();
    await page.locator(".popmenu .mi").filter({ hasText: "深色" }).click();
    await sleep(300);
    check(await page.evaluate(() => document.documentElement.dataset.theme) === "dark", "手动切换深色");
    await page.getByRole("button", { name: "设置与账号" }).click();
    await page.locator(".popmenu .mi").filter({ hasText: "浅色" }).click();
    await sleep(300);
    check(await page.evaluate(() => document.documentElement.dataset.theme) === "light", "手动切换浅色");
    await page.getByRole("button", { name: "设置与账号" }).click();
    await page.locator(".popmenu .mi").filter({ hasText: "跟随系统" }).click();
    // export (browser: download)
    const dl = page.waitForEvent("download", { timeout: 10000 });
    await page.locator("textarea").fill("/export md");
    await page.locator("textarea").press("Enter");
    const d = await dl;
    const txt = (await (await import("node:fs/promises")).readFile(await d.path(), "utf8"));
    check(txt.includes("华东区") && txt.includes("```sql"), "导出 Markdown");
    // B3 inspector: data / dashboards / memory / files / tasks
    // "加入看板" already opened the inspector (dashboards tab); open it only if closed, then go to the data tab
    if (!(await page.locator(".inspector").count())) await page.getByTitle(/检查器/).click();
    check(await page.locator(".inspector .itabs button.on").filter({ hasText: "看板" }).count() > 0, "加入看板后打开看板页");
    await page.locator(".inspector .itabs button").filter({ hasText: "数据" }).click();
    await page.locator(".inspector .irow").filter({ hasText: "master_customers" }).first().waitFor({ timeout: 20000 });
    check(true, "检查器·数据：表列表");
    await page.locator(".inspector .irow").filter({ hasText: "master_customers" }).first().click();
    await page.locator(".inspector .ipreview .dt").first().waitFor({ timeout: 20000 });
    check(true, "检查器·数据：预览");
    await page.locator(".inspector .itabs button").filter({ hasText: "看板" }).click();
    await page.frameLocator(".inspector iframe.webframe").locator("canvas").first().waitFor({ timeout: 20000 });
    check(true, "检查器·看板：iframe 渲染（dashboard/query inline）");
    // per-dashboard palette: toolbar menu → spec.palette, renderer picks it up
    await page.locator(".inspector").getByTitle(/看板配色/).click();
    await page.locator(".popmenu .mi").filter({ hasText: "Tableau" }).click();
    await sleep(1500);
    const pal = await page.frameLocator(".inspector iframe.webframe").locator("body").evaluate(() => window.DP._state.spec?.palette);
    check(pal === "tableau", `看板单独配色生效 (${pal})`);
    const png = await page.frameLocator(".inspector iframe.webframe").locator("body").evaluate(async () => {
      const u = await window.DP.exportPNG();
      const im = await new Promise((ok) => { const i = new Image(); i.onload = () => ok(i); i.src = u; });
      const root = document.getElementById("root");
      const cards = document.querySelectorAll(".dp-card").length, charts = Object.keys(window.DP._state.charts).length;
      const first = Object.values(window.DP._state.charts)[0]?.getDom().getBoundingClientRect();
      return { w: im.width, h: im.height, rw: root.scrollWidth, rh: root.scrollHeight, cards, charts, fh: first?.height ?? 0 };
    });
    check(png.w === png.rw * 2 && png.h === png.rh * 2 && png.h > png.fh * 2 + 60, `看板整体导出 PNG（${png.w}×${png.h}，页面 ${png.rw}×${png.rh}，图表高 ${png.fh}，${png.cards} 张卡片 / ${png.charts} 个图表）`);
    await page.locator(".inspector .itabs button").filter({ hasText: "记忆" }).click();
    await page.locator(".inspector .seg").waitFor({ timeout: 5000 });
    check(true, "检查器·记忆");
    await page.locator(".inspector .itabs button").filter({ hasText: "文件" }).click();
    await page.locator(".inspector .irow").filter({ hasText: "master.xlsx" }).first().waitFor({ timeout: 10000 });
    check(true, "检查器·文件");
    await page.locator(".inspector .itabs button").filter({ hasText: "任务" }).click();
    check(await page.locator(".inspector").getByText("Agent 处理多步任务").count() > 0, "检查器·任务");
    // B4 settings: add a permission rule → config.json; general setting; providers list
    await page.getByRole("button", { name: "设置与账号" }).click();
    await page.locator(".popmenu .mi").filter({ hasText: "设置…" }).click();
    await page.locator(".modalbox.settings").waitFor({ timeout: 5000 });
    check(await page.locator(".settings .split .irow").filter({ hasText: "golden" }).count() > 0, "设置·模型服务列表");
    await page.locator(".settings .seg button").filter({ hasText: "权限" }).click();
    await page.locator(".settings input[placeholder^='web_fetch']").fill("web_fetch(domain:example.com)");
    await page.locator(".settings").getByRole("button", { name: "添加" }).click();
    await page.locator(".settings .irow").filter({ hasText: "web_fetch(domain:example.com)" }).waitFor({ timeout: 5000 });
    await page.locator(".settings .seg button").filter({ hasText: "通用" }).click();
    await page.locator(".settings label").filter({ hasText: "本地模式" }).locator("input").click();
    await sleep(800);
    const cfg = JSON.parse((await import("node:fs")).readFileSync(join(support, "config.json"), "utf8"));
    check(cfg.rules?.some((r) => r.tool === "web_fetch" && r.pattern === "domain:example.com" && r.behavior === "allow"), "设置·权限规则写入 config.json");
    check(cfg.settings?.localOnly === true && cfg.providers?.[0]?.id === "golden", "设置·通用写入且保留其他字段");
    await page.locator(".settings .palopt").filter({ hasText: "Okabe" }).click();
    await sleep(800);
    const cfg2 = JSON.parse((await import("node:fs")).readFileSync(join(support, "config.json"), "utf8"));
    check(cfg2.settings?.dashboardPalette === "okabe", "设置·看板配色写入 config.json");
    await page.locator(".settings .palopt").filter({ hasText: "莫兰迪" }).click();
    await sleep(800);
    const cfg3 = JSON.parse((await import("node:fs")).readFileSync(join(support, "config.json"), "utf8"));
    check(cfg3.settings?.dashboardPalette === "morandi", "设置·新模板（莫兰迪）写入 config.json");
    await page.locator(".settings .palopt").filter({ hasText: "自定义" }).click();
    await page.locator(".settings .palcustom select").selectOption("warm");
    await page.locator(".settings .palchip input[type=color]").first().evaluate((el) => {
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
      set.call(el, "#123456"); el.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await page.locator(".settings .palcustom").getByRole("button", { name: "应用自定义" }).click();
    await sleep(800);
    const cfg4 = JSON.parse((await import("node:fs")).readFileSync(join(support, "config.json"), "utf8"));
    const cp = cfg4.settings?.dashboardPalette;
    check(Array.isArray(cp) && cp[0] === "#123456" && cp[1] === "#e67e22" && cp.length === 6, `设置·自定义配色（取色器）写入 config.json (${JSON.stringify(cp)})`);
    await page.locator(".settings .seg button").filter({ hasText: "扩展" }).click();
    await page.locator(".settings").getByText("MCP 服务器").first().waitFor({ timeout: 5000 });
    await page.locator(".settings .seg button").filter({ hasText: "关于" }).click();
    await page.locator(".settings").getByText(/写邮件反馈/).waitFor({ timeout: 5000 });
    check(true, "设置·扩展 / 关于");
    await page.locator(".modalbox.settings").getByRole("button", { name: "关闭" }).click();
    // /fork: copy of this chat becomes the selected session with the same history
    const before = await page.locator(".srow").count();
    await page.locator("textarea").fill("/fork");
    await page.locator("textarea").press("Enter");
    await page.locator(".chead h2").filter({ hasText: "（分支）" }).waitFor({ timeout: 15000 });
    check(await page.locator(".srow").count() === before + 1 && await page.locator(".msg.user").count() > 0, "/fork 复制会话（含历史）");
  }
  await page.screenshot({ path: join(support, "e2e.png"), fullPage: false });
  console.log("screenshot", join(support, "e2e.png"));
  // narrow window: the composer control bar must not wrap or squash
  await page.keyboard.press("Escape").catch(() => {});
  await page.locator(".modal").first().click({ position: { x: 5, y: 5 } }).catch(() => {});
  for (const w of [1000, 900]) {
    await page.setViewportSize({ width: w, height: 700 }); await sleep(300);
    const h = await page.locator(".cbar").first().evaluate((el) => [el.getBoundingClientRect().height, el.scrollWidth - el.clientWidth, el.clientWidth, el.className,
      [...el.children].map((c) => c.className + ":" + Math.round(c.getBoundingClientRect().width)).join(" "), !!document.querySelector(".inspector")]).catch(() => null);
    if (h && h[1] > 1) console.log("  cbar", JSON.stringify(h.slice(2)));
    if (h) check(h[0] < 34 && h[1] <= 1, `窄窗口 ${w}px 控制条不换行不溢出 (h=${Math.round(h[0])}, overflow=${h[1]})`);
    await page.screenshot({ path: join(support, `e2e-${w}.png`) });
  }
  console.log("screenshot", join(support, "e2e-900.png"));
  // inspector open → the chat column is at its narrowest
  await page.setViewportSize({ width: 1100, height: 700 }); await sleep(200);
  if (!(await page.locator(".inspector").count())) await page.locator(".chead .tools button").last().click();
  await sleep(500);
  const hi = await page.locator(".cbar").first().evaluate((el) => [el.getBoundingClientRect().height, el.scrollWidth - el.clientWidth, el.clientWidth]).catch(() => null);
  if (hi) check(hi[0] < 34 && hi[1] <= 1, `检查器打开时控制条不变形 (w=${Math.round(hi[2])}, h=${Math.round(hi[0])}, overflow=${hi[1]})`);
  await page.screenshot({ path: join(support, "e2e-insp.png") });
  console.log("screenshot", join(support, "e2e-insp.png"));
} catch (e) {
  ok = false;
  console.log("✗ " + e.message.split("\n")[0]);
  await page.screenshot({ path: join(support, "e2e-fail.png") }).catch(() => {});
  console.log("screenshot", join(support, "e2e-fail.png"));
}
const bad = logs.filter((l) => l.startsWith("error") || l.startsWith("pageerror"));
if (bad.length) { console.log("browser errors:\n  " + bad.slice(0, 10).join("\n  ")); }
if (!ok) console.log("browser log tail:\n  " + logs.slice(-15).join("\n  "));
await browser.close();
cleanup();
process.exit(ok ? 0 : 1);
