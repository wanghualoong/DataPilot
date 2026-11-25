import { useEffect, useMemo, useRef, useState } from "react";
import { imeEnter } from "../ime";
import { ArrowUp, FileText, MessageSquareText, Paperclip, Square, Table2, X } from "lucide-react";
import { ask, pickFiles } from "../native";
import { useApp } from "../store";
import { core } from "../transport";
import { L } from "../i18n";
import { exportChat } from "../actions";
import { builtins, canonicalCmd, dashboardPrompt, filterCmds, initPrompt, initTarget, parseSlash, reviewPrompt, type Cmd } from "../slash";
import { contextReport, doctorReport } from "../reports";
import { chatModels, mentionedTables, nextMode, MODES } from "../models";
import { ControlBar } from "./ChatControls";
import { IconBtn, ic } from "./ui";
import { stripCitations } from "./links";
import type { Mode } from "../protocol/rpc";

const exportOpts: Cmd[] = [
  { name: "md", desc: L("Markdown 文本（推荐）", "Markdown text (recommended)") },
  { name: "html", desc: L("带样式的网页，可直接分享", "Styled web page") },
  { name: "jsonl", desc: L("原始转录（含工具调用）", "Raw transcript incl. tool calls") },
];

type MenuItem = Cmd & { fill: string; mention?: boolean; provider?: string; back?: boolean };

/** Hidden hint for the model: scope this turn to the @-mentioned tables (same text as the SwiftUI mentionNote). */
function mentionNote(text: string, known: string[]) {
  const ts = mentionedTables(text, known);
  if (!ts.length) return "";
  const list = ts.join("、");
  return L(`\n\n[用户用 @ 指定了表：${list}。先用 data_schema(tables: [...]) 只查看这些表，分析时只用这些表；确需其他表时先说明原因。]`,
           `\n\n[The user @-mentioned tables: ${list}. Call data_schema with only these tables and analyze with them; explain before using other tables.]`);
}

export function Composer({ running }: { running: boolean }) {
  const st = useApp();
  const { send, cancel, newChat, selected, attached, config, tableNames } = st;
  const [text, setText] = useState("");
  const [skills, setSkills] = useState<Cmd[]>([]);
  const [hi, setHi] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const [modelProvider, setModelProvider] = useState<string>();   // /model level 2 (Swift: provider list → models)
  const ta = useRef<HTMLTextAreaElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  // keyboard selection in the / menu: keep the highlighted row visible
  useEffect(() => { (menuRef.current?.children[hi] as HTMLElement | undefined)?.scrollIntoView({ block: "nearest" }); }, [hi]);
  const loadSkills = () => core.request("skill/list").then((r) => { const l: Cmd[] = r.skills.map((s: any) => ({ name: s.name, desc: s.description, skill: true, source: s.source })); setSkills(l); return l; }).catch(() => skills);
  useEffect(() => { loadSkills(); if (!config) st.loadConfig().catch(() => {}); }, []);
  useEffect(() => { if (text === "/") loadSkills(); }, [text === "/"]);
  const savedSkills = useApp((s) => (selected ? (s.chats[selected]?.items ?? []).filter((i: any) => i.kind === "tool" && i.tool_name === "save_skill" && i.output).length : 0));
  useEffect(() => { if (savedSkills) loadSkills(); }, [savedSkills]);
  useEffect(() => { ta.current?.focus(); }, [selected]);
  // grow with the text (1–10 lines)
  useEffect(() => {
    const t = ta.current;
    if (!t) return;
    t.style.height = "auto";
    t.style.height = Math.min(t.scrollHeight, 220) + "px";
  }, [text]);

  // popup: commands, the options of /export · /model · /skills, or @ table mentions
  const menu: MenuItem[] = useMemo(() => {
    if (dismissed) return [];
    const at = text.match(/(^|\s)@([^\s@]*)$/);
    if (at && !text.startsWith("/")) {
      const q = at[2].toLowerCase();
      return tableNames.filter((t) => !q || t.toLowerCase().includes(q))
        .sort((a, b) => Number(!a.toLowerCase().startsWith(q)) - Number(!b.toLowerCase().startsWith(q)) || a.localeCompare(b))
        .slice(0, 10).map((t) => ({ name: t, desc: L("只用这张表分析", "scope this turn to the table"), mention: true, fill: text.slice(0, text.length - at[2].length) + t + " " }));
    }
    const m = text.match(/^\/(\S*)(\s+(\S*))?$/);
    if (!m) return [];
    if (m[2] !== undefined) {
      const q = m[3] ?? "";
      if (m[1] === "export") return filterCmds(exportOpts, q).map((c) => ({ ...c, fill: `/export ${c.name}` }));
      if (m[1] === "model") {
        const ms = chatModels(config), cur = config?.roles?.main;
        const opts = (xs: typeof ms) => xs.map((x) => ({ name: x.model, desc: x.provider + (cur?.model === x.model && cur?.providerId === x.providerId ? L(" · ✓ 当前", " · ✓ current") : ""), fill: `/model ${x.providerId}|${x.model}` }));
        const provs: { id: string; name: string; n: number }[] = [];
        for (const x of ms) { const g = provs.find((p) => p.id === x.providerId); if (g) g.n++; else provs.push({ id: x.providerId, name: x.provider, n: 1 }); }
        const f = (xs: MenuItem[]) => (q ? xs.filter((c) => c.name.toLowerCase().includes(q.toLowerCase())) : xs);
        if (modelProvider) {
          const mine = opts(ms.filter((x) => x.providerId === modelProvider));
          return q ? f(mine) : [{ name: L("‹ 返回服务商列表", "‹ Back to providers"), desc: L("⌫ 也可返回", "or press ⌫"), fill: "/model ", back: true }, ...mine];
        }
        if (provs.length <= 1) return f(opts(ms));
        const pl: MenuItem[] = provs.map((p) => ({ name: p.name, desc: (p.id === cur?.providerId ? L("✓ 当前 · ", "✓ current · ") : "") + L(`${p.n} 个模型 ›`, `${p.n} models ›`), fill: "/model ", provider: p.id }));
        return q ? [...f(pl), ...f(opts(ms))] : pl;
      }
      if (m[1] === "skills") return filterCmds(skills, q).map((c) => ({ ...c, fill: `/${c.name} ` }));
      return [];
    }
    return filterCmds([...builtins, ...skills], m[1]).slice(0, 12).map((c) => ({ ...c, fill: `/${c.name} ` }));
  }, [text, skills, config, tableNames, dismissed, modelProvider]);

  const note = (t: string) => st.note(t);
  const setMode = async (m: Mode, msg: string) => { await st.setMode(m); note(msg); };

  const run = async (raw: string) => {
    const t = raw.trim();
    if (!t && !attached.length) return;
    setText("");
    const cmd = parseSlash(t);
    if (!cmd && running && attached.length) { setText(t); return note(L("Agent 运行中只能插话文字；附件请等本轮结束后再发送", "While the agent runs you can only add text; send files after this turn")); }
    if (!cmd) return send(t, undefined, mentionNote(t, tableNames));
    const { args } = cmd;
    // aliases: /skill_save, /save_skill, /skill_new … → /skill-new (underscores accepted for every built-in)
    const name = canonicalCmd(cmd.name, skills.map((x) => x.name));
    const agent = (prompt: string) => (running ? note(L("Agent 正在运行，请稍后再试", "The agent is busy; try again later")) : send(prompt));
    const chat = selected ? useApp.getState().chats[selected] : undefined;
    switch (name) {
      case "plan": await setMode("plan", L("已切换到计划模式：先只读探查，再提交计划供你确认", "Plan mode: explore read-only, then propose a plan")); if (args) agent(args); return;
      case "ask": return setMode("ask", L("已切换到询问模式", "Ask mode"));
      case "auto": return setMode("auto", L("已切换到自动模式：沙箱内与联网操作自动执行；桌面控制等仍需确认", "Auto mode: sandboxed and network actions run automatically"));
      case "full":
        if (await ask(L("完全访问：所有操作（含沙箱外命令、桌面控制）都不再询问。确定开启？", "Full access: nothing asks for approval any more (incl. external commands). Enable?")))
          await setMode("full", L("已开启完全访问", "Full access enabled"));
        return;
      case "new": case "clear": return newChat();
      case "btw": {
        if (!args) return setText("/btw ");
        return st.btw(args);
      }
      case "compact":
        return st.compactNow();
      case "rewind": {
        const last = [...(chat?.items ?? [])].reverse().find((i: any) => i.kind === "user" && i.turn_id) as any;
        if (!last || running) return note(L("没有可回退的消息", "Nothing to rewind"));
        return st.rewind(last.turn_id);
      }
      case "init": {
        const pid = st.sessions.find((s) => s.id === selected)?.project_id;
        const root = pid ? st.projects.find((p) => p.id === pid)?.path : null;
        return agent(initPrompt(initTarget(root)) + (args ? "\n\n" + args : ""));
      }
      case "dashboard": return agent(dashboardPrompt + (args ? "\n\n" + L("补充要求：", "Extra requirements: ") + args : ""));
      case "dream": return agent(args === "undo" ? "/dream undo" : "/dream");
      case "memory": return st.openInspector("memory");
      case "browser": return st.openInspector("browser");
      case "mcp": {
        const r = await core.request("mcp/status").catch(() => ({ status: {} }));
        const e = Object.entries(r.status ?? {});
        return note(e.length ? "MCP:\n" + e.map(([k, v]) => `- ${k}: ${typeof v === "string" ? v : JSON.stringify(v)}`).join("\n") : L("未配置 MCP 服务器（设置 → 扩展）", "No MCP servers configured (Settings → Extensions)"));
      }
      case "skills": {
        if (args && skills.some((s) => s.name === args)) return send(L(`使用 ${args}`, `Use ${args}`), args);
        return setText("/skills ");
      }
      case "skill-new":
        return send(args || L("我想创建一个新的 skill，请先问我它的用途；如果本会话已有可复用的分析流程，也可以建议把它保存下来。", "I want to create a new skill. Ask me what it is for, or suggest saving a reusable workflow from this chat."), "skill-creator");
      case "export": return args ? exportChat(args) : setText("/export ");
      case "model": {
        const ms = chatModels(config ?? (await st.loadConfig()));
        if (!args) return ms.length ? setText("/model ") : note(L("还没有可用的对话模型，请先在 设置 → 模型服务 中添加。", "No chat models yet — add one in Settings → Providers."));
        setModelProvider(undefined);
        const [pid, mid] = args.includes("|") ? args.split("|", 2) : [undefined, args];
        const m = ms.find((x) => x.model === mid && (!pid || x.providerId === pid)) ?? ms.find((x) => x.model.toLowerCase().includes(mid.toLowerCase()));
        if (m && config?.roles?.main?.model === m.model && config?.roles?.main?.providerId === m.providerId) return note(L("已经是当前模型：", "Already using ") + m.model);
        return m ? st.setMainModel(m.providerId, m.model) : note(L(`没有找到模型「${args}」。可用：`, `No model “${args}”. Available: `) + ms.map((x) => x.model).join(", "));
      }
      case "status": {
        if (!selected) return;
        const k = await core.request("kernel/status", { session: selected }).catch(() => ({ running: false }));
        let tables = "";
        if (k.running) {
          const r = await core.request("kernel/call", { session: selected, op: "tables", timeout: 20 }).catch(() => null);
          tables = (r?.result?.tables ?? []).map((t: any) => `${t.table}${t.rows != null ? ` (${t.rows})` : ""}`).join(", ") || "-";
        }
        const u = chat?.usage;
        const s = st.sessions.find((x) => x.id === selected);
        return note([
          L("会话：", "Session: ") + `${chat?.title || s?.title || ""} (${selected})`,
          L("模型：", "Model: ") + (config?.roles?.main?.model ?? "-") + (chat?.thinking ? L("（思考开）", " (thinking on)") : ""),
          L("模式：", "Mode: ") + (st.open[selected]?.mode ?? "-") + L("；联网：", "; network: ") + (chat?.network ? "on" : "off") + (config?.settings?.localOnly ? L("；本地模式", "; local-only") : ""),
          u ? L("上下文：", "Context: ") + `${u.context_tokens} / ${u.context_window} tokens (${Math.round((u.context_tokens / Math.max(1, u.context_window)) * 100)}%)` : "",
          L("数据内核：", "Kernel: ") + (k.running ? L("运行中", "running") : L("未启动（发消息时自动启动）", "idle (starts on demand)")),
          k.running ? L("已导入的表：", "Tables: ") + tables : "",
          L("数据目录：", "Data dir: ") + (st.open[selected]?.workspace ?? ""),
        ].filter(Boolean).join("\n\n"));
      }
      case "context": {
        if (!selected) return;
        if (running) return note(L("Agent 正在运行，请本轮结束后再查看", "A turn is running; try after it finishes"));
        const r = await core.request("session/context", { session: selected }).catch((e) => { st.fail(e); return null; });
        return r && note(contextReport(r));
      }
      case "fork": {
        if (!selected) return;
        if (running) return note(L("Agent 正在运行，请本轮结束后再复制", "A turn is running; fork after it finishes"));
        const r = await core.request("session/fork", { session: selected }).catch((e) => { st.fail(e); return null; });
        if (!r?.id) return;
        await st.reload(); await st.select(r.id);
        return st.note(L(`已复制为新会话「${r.title}」：对话和数据都已带过来，在这里继续不会影响原会话。`, `Forked into “${r.title}”: conversation and data copied; the original chat is unchanged.`));
      }
      case "review": return agent(reviewPrompt + (args ? "\n\n" + L("重点检查：", "Focus on: ") + args : ""));
      case "doctor": {
        note(L("正在自检…", "Running checks…"));
        const r = await core.request("app/doctor", selected ? { session: selected } : {}).catch((e) => { st.fail(e); return null; });
        return r && note(doctorReport(r));
      }
      case "remember": {
        if (!args) return setText("/remember ");
        const pid = st.sessions.find((s) => s.id === selected)?.project_id;
        const scope = pid ? "project" : "user";
        const name = args.replace(/\s+/g, " ").slice(0, 40);
        try {
          await core.request("memory/topics", { scope, session: selected, action: "write", name, description: args.slice(0, 120), type: pid ? "project" : "user", body: args });
          return note(pid ? L("已写入项目记忆：", "Saved to project memory: ") + name : L("已写入个人记忆（所有会话可用）：", "Saved to your memory (all chats): ") + name);
        } catch (e) { return st.fail(e); }
      }
      case "copy": {
        const last = [...(chat?.items ?? [])].reverse().find((i: any) => i.kind === "assistant" && i.text) as any;
        if (!last) return note(L("还没有可复制的回答", "No answer to copy yet"));
        await navigator.clipboard.writeText(stripCitations(last.text));
        return note(L("已复制上一条回答", "Copied the last answer"));
      }
      case "permissions": return st.openSettings("permissions");
      case "settings": return st.openSettings("general");
      case "todos": return st.openInspector("todos");
      case "help":
        return note(builtins.map((c) => `- /${c.name} — ${c.desc}`).join("\n") + (skills.length ? "\n\n" + L("Skills（直接 /名称 调用）：", "Skills (invoke with /name): ") + skills.map((s) => s.name).join(", ") : ""));
      default: {
        const sk = skills.find((s) => s.name === name) ?? (await loadSkills()).find((s) => s.name === name || s.name === cmd.name || s.name === canonicalCmd(cmd.name, [s.name]));
        if (sk) return send(args || L(`使用 ${sk.name}`, `Use ${sk.name}`), sk.name);
        const near = filterCmds(builtins, name).slice(0, 3).map((c) => "/" + c.name);
        return note(L("未知命令 /", "Unknown command /") + name + (near.length ? L("。你是不是想用：", ". Did you mean: ") + near.join("、") : L("。输入 /help 查看全部命令", ". Type /help for all commands")));
      }
    }
  };

  const pick = (c: MenuItem) => {
    if (c.back) { setModelProvider(undefined); setText("/model "); setHi(0); return; }
    if (c.provider) { setModelProvider(c.provider); setText("/model "); setHi(0); ta.current?.focus(); return; }
    setText(c.fill);
    ta.current?.focus();
    // options of /export and /model run at once (like the SwiftUI picker)
    if (/^\/(export|model) \S+$/.test(c.fill)) run(c.fill);
  };

  const canSend = !!text.trim() || attached.length > 0;
  return (
    <div className="composer">
      {menu.length > 0 && (
        <div className="slash" ref={menuRef}>
          {menu.map((c, i) => (
            <div key={c.fill} className={i === hi ? "on" : ""} onMouseEnter={() => setHi(i)} onMouseDown={(e) => { e.preventDefault(); pick(c); }}>
              {c.mention ? <Table2 {...ic} size={13} /> : null}
              <b>{c.mention || c.provider || c.back || c.fill.startsWith("/model ") ? c.name : c.fill.trim()}</b> <span className="muted">{c.skill ? `skill${c.source && c.source !== "builtin" ? "·" + c.source : ""} · ` : ""}{c.desc.slice(0, 80)}</span>
            </div>
          ))}
        </div>
      )}
      {attached.length > 0 && (
        <div className="chips">
          {attached.map((f) => (
            <span key={f} className="chip"><FileText {...ic} size={12} />{f.split(/[\\/]/).pop()}
              <button title={L("移除", "Remove")} onClick={() => useApp.setState({ attached: attached.filter((x) => x !== f) })}><X size={11} /></button></span>
          ))}
        </div>
      )}
      <div className="inputbox">
        <IconBtn icon={Paperclip} title={L("上传文件", "Upload files")} onClick={async () => st.attach(await pickFiles())} />
        <textarea ref={ta} rows={1} value={text}
          placeholder={running ? L("Agent 运行中：输入可插话（⏎），/btw 顺便问一句，↑ 撤回排队消息", "Agent running: type to interject (⏎), /btw for a side question, ↑ to withdraw")
                               : L("问点什么… (⏎ 发送，⇧⏎ 换行，@ 指定表，/ 命令)", "Ask anything… (⏎ send, ⇧⏎ newline, @ a table, / commands)")}
          onChange={(e) => {
            const v = e.target.value;
            if (!v.startsWith("/") && !v.includes("@")) setDismissed(false);
            if (!v.startsWith("/model")) setModelProvider(undefined);
            if (v.endsWith("@") && v.length > text.length) { setDismissed(false); st.refreshTables(); }
            if (v.length <= 1) setDismissed(false);
            setText(v); setHi(0);
          }}
          onKeyDown={(e) => {
            if (imeEnter(e)) return;
            if (e.key === "Tab" && e.shiftKey) {   // ⇧⇥ cycles approval modes (Claude Code style)
              e.preventDefault();
              const cur = (selected && st.open[selected]?.mode) || "ask";
              const n = nextMode(cur);
              const info = MODES.find((x) => x.m === n)!;
              st.setMode(n as Mode).then(() => note(L("模式：", "Mode: ") + L(info.zh, info.en) + " — " + L(info.dzh, info.den)));
              return;
            }
            if (menu.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) { e.preventDefault(); setHi((hi + (e.key === "ArrowDown" ? 1 : menu.length - 1)) % menu.length); return; }
            if (!menu.length && e.key === "ArrowUp" && !text && selected) {   // take back the last queued interjection
              const p = useApp.getState().chats[selected]?.pending ?? [];
              if (p.length) { e.preventDefault(); const last = p[p.length - 1]; st.withdraw(last.id).then(() => setText(last.text)); }
              return;
            }
            if (menu.length && e.key === "Tab") { e.preventDefault(); pick(menu[hi]); return; }
            if (menu.length && e.key === "Enter" && !e.shiftKey && menu[hi].fill.trim() !== text.trim()) { e.preventDefault(); pick(menu[hi]); return; }
            if (modelProvider && ((e.key === "Backspace" && text === "/model ") || e.key === "Escape")) { e.preventDefault(); setModelProvider(undefined); setText("/model "); return; }
            if (e.key === "Escape" && menu.length) { e.preventDefault(); setDismissed(true); return; }
            if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); run(text); }
          }} />
        {running && canSend && (
          <button className="roundbtn" title={L("插话 (⏎)：Agent 执行完当前工具后会读到这条消息并调整做法", "Interject (⏎): the agent reads it after the current tool and adapts")} onClick={() => run(text)}>
            <MessageSquareText size={16} />
          </button>
        )}
        {running ? (
          <button className="roundbtn stop" title={L("停止 (⌘.)", "Stop (⌘.)")} aria-label={L("停止", "Stop")} onClick={() => cancel()}><Square size={12} fill="currentColor" /></button>
        ) : (
          <button className="roundbtn send" disabled={!canSend} title={L("发送 (⏎)", "Send (⏎)")} aria-label={L("发送", "Send")} onClick={() => run(text)}><ArrowUp size={17} strokeWidth={2.4} /></button>
        )}
      </div>
      <ControlBar />
    </div>
  );
}
