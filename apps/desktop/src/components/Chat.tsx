import { useEffect, useRef, useState } from "react";
import {
  Brain, ChevronDown, ChevronRight, Circle, CircleCheck, CircleDot, Eye, EyeOff, PanelLeft, PanelRight, Share, ShieldAlert, Shrink, Undo2, X,
  LoaderCircle, TriangleAlert, Info, MessagesSquare,
} from "lucide-react";
import { ask, onFileDrop } from "../native";
import { exportChat } from "../actions";
import { Composer } from "./Composer";
import { useApp } from "../store";
import { L } from "../i18n";
import { Markdown } from "./Markdown";
import { ToolCard } from "./ToolCard";
import { ApprovalCard, PlanCard, QuestionCard } from "./Cards";
import { CopyBtn, IconBtn, MenuItem, Popover, ic } from "./ui";
import { stripCitations } from "./links";
import type { Item } from "../reduce";

export function TodoIcon({ status }: { status: string }) {
  if (status === "completed") return <CircleCheck {...ic} size={14} className="ok" />;
  if (status === "in_progress") return <CircleDot {...ic} size={14} className="accent" />;
  return <Circle {...ic} size={14} className="muted" />;
}

function Row({ it, running, showContext }: { it: Item; running: boolean; showContext: boolean }) {
  const rewind = useApp((s) => s.rewind);
  const [showR, setShowR] = useState(false);
  const [open, setOpen] = useState(false);
  const x = it as any;
  switch (it.kind) {
    case "user":
      return (
        <div className="msg user">
          {x.turn_id && !running && (
            <IconBtn icon={Undo2} size={13} className="rewind" title={L("回到这条消息之前（对话和数据一起回退）", "Rewind to before this message")}
              onClick={async () => { if (await ask(L("回退到这条消息之前？之后的对话会被移除。", "Rewind to before this message? Later messages are removed."))) rewind(x.turn_id); }} />
          )}
          <CopyBtn className="copy" text={() => x.text ?? ""} />
          <div className="bubble">{x.text}</div>
        </div>
      );
    case "assistant":
      return (
        <div className="msg assistant">
          {x.reasoning && (
            <div className="reasoning">
              <span className="rtoggle" onClick={() => setShowR(!showR)}>
                <Brain {...ic} size={13} />{x.streaming && !x.text ? L("思考中…", "Thinking…") : L("思考过程", "Reasoning")}
                {showR ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
              </span>
              {showR && <pre>{x.reasoning}</pre>}
            </div>
          )}
          {x.text && <Markdown text={stripCitations(x.text)} />}
          {x.text && !x.streaming && <div className="mfoot"><CopyBtn className="copy" text={() => stripCitations(x.text)} /></div>}
        </div>
      );
    case "context":
      if (!showContext) return null;
      return (
        <div className="ctxblock">
          <span className="rtoggle" onClick={() => setOpen(!open)}>
            <Eye {...ic} size={13} />{L("注入的上下文", "Injected context")}{open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </span>
          {open && <pre>{x.text}</pre>}
        </div>
      );
    case "btw": return <BtwCard x={x} />;
    case "tool": return <ToolCard it={x} />;
    case "approval": return <ApprovalCard c={x} />;
    case "question": return <QuestionCard c={x} />;
    case "plan": return <PlanCard c={x} />;
    case "compacted":
      return (
        <div className="notice compacted">
          <span className="rtoggle" onClick={() => setOpen(!open)}>
            <Shrink {...ic} size={13} /> {L("上下文已压缩", "Context compacted")}{x.text && (open ? <ChevronDown size={12} /> : <ChevronRight size={12} />)}
          </span>
          {open && x.text && <pre>{x.text}</pre>}
        </div>
      );
    case "error": return <div className="notice err"><TriangleAlert {...ic} size={13} /> {x.text}</div>;
    case "notice": return <div className="notice"><Info {...ic} size={13} className="nicon" /><Markdown text={x.text ?? ""} /></div>;
    default: return null;
  }
}

/** /btw side question: answered from the chat context without tools; not part of the conversation (Swift BtwCard). */
function BtwCard({ x }: { x: any }) {
  const dismiss = useApp((s) => s.dismissBtw);
  return (
    <div className="btw">
      <div className="row">
        <MessagesSquare {...ic} size={14} className="purple" /><b className="mono purple small">/btw</b>
        <span className="grow ell btwq">{x.text}</span>
        <span className="muted small">{L("旁问 · 不计入对话", "Side question · not in conversation")}</span>
        <IconBtn icon={X} size={13} title={L("关闭", "Dismiss")} onClick={() => dismiss(x.id)} />
      </div>
      {x.running ? <div className="muted small row"><LoaderCircle {...ic} size={13} className="spin" />{L("思考中…", "Thinking…")}</div>
        : x.is_error ? <div className="err small">{x.output}</div>
        : <><Markdown text={x.output ?? ""} />
            <div><CopyBtn text={() => x.output ?? ""} /></div></>}
    </div>
  );
}

function EmptyHint() {
  const main = useApp((s) => s.config?.roles?.main);
  return (
    <div className="emptyhint">
      <div className="row"><h2 className="grow">{L("开始分析", "Get started")}</h2>
        {!main && <button className="primary" onClick={() => useApp.setState({ quickLogin: true })}>{L("登录并选择 Key", "Log in & choose key")}</button>}</div>
      <p className="muted">{L("用自然语言描述你想做的事，DataPilot 会自己写 SQL / Python、跑出结果并解释。", "Describe what you want in plain language; DataPilot writes the SQL / Python, runs it and explains the result.")}</p>
      <h4>{L("可以给它的材料", "What you can give it")}</h4>
      {[L("数据文件：Excel、CSV、Parquet、JSON — 拖进窗口或点回形针上传", "Data files: Excel, CSV, Parquet, JSON — drag in or click the paperclip"),
        L("文档与图片：PDF、Word、PPT、扫描件和照片（自动识别文字，可「原页对照」核对）", "Documents & images: PDF, Word, PPT, scans and photos (text is recognized; check it with “Compare with page”)"),
        L("数据库：在右侧「数据」面板添加连接（支持 SSH 隧道），直接查询库里的表", "Databases: add a connection in the Data panel on the right (SSH tunnel supported) and query tables directly"),
        L("项目文件夹：在左侧打开一个文件夹作为项目，文件、对话、数据和记忆都留在项目里", "Project folders: open a folder on the left as a project; files, chats, data and memory stay with it")].map((t) => <div key={t} className="muted">• {t}</div>)}
      <h4>{L("可以让它做的事（示例）", "What it can do (examples)")}</h4>
      {[L("看下这份数据有哪些字段、有没有空值和异常值", "What columns does this data have? Any nulls or outliers?"),
        L("按月统计各类别的指标，并算同比 / 环比", "Monthly metrics by category, with YoY / MoM"),
        L("把这几个文件按编号关联起来，找出对不上的记录", "Join these files by ID and find records that don't match"),
        L("画一张趋势图，再做一个可以筛选的看板", "Draw a trend chart, then build a filterable dashboard"),
        L("从这些合同 / 发票里提取关键字段，整理成表格", "Extract key fields from these contracts / invoices into a table"),
        L("把分析结论写成 Word 报告或 PPT，导出到 outputs", "Write the findings as a Word report or PPT into outputs")].map((t) => <div key={t} className="muted">• {t}</div>)}
      <p className="muted small">{L("输入 / 查看命令和 skill（如 /plan 先出计划、/review 复核、/export 导出）；输入 @ 引用已导入的表；说「记住…」可保存偏好。",
        "Type / for commands and skills (e.g. /plan, /review, /export); @ to mention imported tables; say “remember …” to save a preference.")}</p>
    </div>
  );
}

export function Chat({ id }: { id: string }) {
  const chat = useApp((s) => s.chats[id]);
  const open = useApp((s) => s.open[id]);
  const session = useApp((s) => s.sessions.find((x) => x.id === id));
  const model = useApp((s) => s.config?.roles?.main?.model);
  const showContext = useApp((s) => s.showContext);
  const inspector = useApp((s) => !!s.inspector);
  const sidebarHidden = useApp((s) => s.sidebarHidden);
  const { withdraw, attach, setMode, compactNow } = useApp();
  const [drop, setDrop] = useState(false);
  useEffect(() => {
    let off = () => {};
    onFileDrop((paths) => attach(paths), setDrop).then((f) => (off = f));
    return () => off();
  }, [id]);
  const end = useRef<HTMLDivElement>(null);
  const items = chat?.items ?? [];
  const ctxCount = items.filter((i) => i.kind === "context").length;
  useEffect(() => { end.current?.scrollIntoView({ block: "end" }); }, [items.length, (items[items.length - 1] as any)?.text]);
  return (
    <div className="chat">
      <header className="chead">
        {sidebarHidden && <IconBtn icon={PanelLeft} title={L("显示侧栏", "Show sidebar")} onClick={() => useApp.setState({ sidebarHidden: false })} />}
        <div className="ctitles">
          <h2>{chat?.title || session?.title || L("新会话", "New chat")}</h2>
          {model && <span className="muted small">{model}</span>}
        </div>
        <div className="tools">
          <IconBtn icon={showContext ? EyeOff : Eye} on={showContext}
            title={L(`显示 / 隐藏每轮自动注入给模型的上下文（日期、记忆、会话笔记、数据表状态）。本会话已注入 ${ctxCount} 次。`, `Show / hide the context injected into each turn. Injected ${ctxCount}× in this chat.`)}
            onClick={() => {
              const on = !showContext;
              useApp.setState({ showContext: on });
              if (on && !ctxCount) useApp.getState().note(L("本会话还没有注入过上下文：发出第一条消息后，每轮注入的记忆 / 笔记 / 数据表状态会显示在对应消息上方。", "No context injected yet — it appears above each message after you send one."));
            }} />
          <IconBtn icon={Shrink} title={L("压缩上下文：把较早的对话总结成摘要，释放模型上下文（同 /compact，⌘⇧K）；运行中点击会在当前这一步完成后压缩", "Compact: summarize earlier turns (same as /compact, ⌘⇧K); while running, it happens after the current step")} onClick={() => compactNow()} />
          <Popover align="right" caret={false} title={L("导出对话（也可输入 /export）", "Export chat (or type /export)")} label={<Share {...ic} />}>
            <MenuItem onClick={() => exportChat("md")}>Markdown (.md)</MenuItem>
            <MenuItem onClick={() => exportChat("html")}>{L("网页 (.html)", "HTML (.html)")}</MenuItem>
            <MenuItem onClick={() => exportChat("jsonl")}>{L("原始记录 (.jsonl)", "Raw transcript (.jsonl)")}</MenuItem>
          </Popover>
          <IconBtn icon={PanelRight} on={inspector} title={L("检查器（数据 / 看板 / 记忆 / 任务 / 文件）", "Inspector (data / dashboards / memory / tasks / files)")}
            onClick={() => useApp.getState().openInspector(inspector ? undefined : "data")} />
        </div>
      </header>
      {drop && <div className="dropzone">{L("松开以上传到工作区", "Drop to upload to workspace")}</div>}
      <div className="stream">
        {!items.length && <EmptyHint />}
        {items.map((it, i) => <Row key={it.id + ":" + i} it={it} running={!!chat?.running} showContext={showContext} />)}
        {chat?.todos?.length ? (
          <div className="todos">{chat.todos.map((t: any) => <div key={t.id} className="row"><TodoIcon status={t.status} /> {t.content}</div>)}</div>
        ) : null}
        {chat?.pending.map((p) => (
          <div key={p.id} className="msg user pending">
            <IconBtn icon={X} size={13} title={L("撤回（未送达前可撤回；输入框为空时按 ↑ 可取回编辑）", "Withdraw (before delivery; ↑ in an empty composer to edit)")} onClick={() => withdraw(p.id)} />
            <div className="bubble">{p.text}<div className="small muted">{L("排队中", "Queued")}</div></div>
          </div>
        ))}
        {chat?.running && <div className="working"><LoaderCircle {...ic} size={14} className="spin" /> {L("正在处理…", "Working…")}</div>}
        <div ref={end} />
      </div>
      {open?.mode === "full" && (
        <div className="banner full">
          <ShieldAlert {...ic} size={14} /><span className="grow">{L("完全访问模式：所有操作都不再询问（包括桌面控制、表单提交）", "Full access: nothing asks for approval (incl. desktop control and form submits)")}</span>
          <button onClick={() => setMode("ask")}>{L("切回询问模式", "Back to Ask")}</button>
        </div>
      )}
      <Composer running={!!chat?.running} />
    </div>
  );
}

