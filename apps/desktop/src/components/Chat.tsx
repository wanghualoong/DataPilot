import { useEffect, useRef, useState } from "react";
import {
  Brain, ChevronDown, ChevronRight, Circle, CircleCheck, CircleDot, Eye, EyeOff, PanelLeft, PanelRight, Share, ShieldAlert, Shrink, Undo2, X,
  LoaderCircle, TriangleAlert, Info, MessagesSquare, Copy,
} from "lucide-react";
import { ask, onFileDrop } from "../native";
import { exportChat } from "../actions";
import { Composer } from "./Composer";
import { useApp } from "../store";
import { L } from "../i18n";
import { Markdown } from "./Markdown";
import { ToolCard } from "./ToolCard";
import { ApprovalCard, PlanCard, QuestionCard } from "./Cards";
import { IconBtn, MenuItem, Popover, ic } from "./ui";
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
          <IconBtn icon={Copy} size={13} className="copy" title={L("复制", "Copy")} onClick={() => navigator.clipboard.writeText(x.text ?? "")} />
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
          {x.text && !x.streaming && <div className="mfoot"><IconBtn icon={Copy} size={13} className="copy" title={L("复制", "Copy")} onClick={() => navigator.clipboard.writeText(stripCitations(x.text))} /></div>}
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
            <div><IconBtn icon={Copy} size={13} title={L("复制", "Copy")} onClick={() => navigator.clipboard.writeText(x.output ?? "")} /></div></>}
    </div>
  );
}

function EmptyHint() {
  const main = useApp((s) => s.config?.roles?.main);
  return (
    <div className="emptyhint">
      <div className="row"><h2 className="grow">{L("开始分析", "Get started")}</h2>
        {!main && <button className="primary" onClick={() => useApp.setState({ quickLogin: true })}>{L("登录并选择 Key", "Log in & choose key")}</button>}</div>
      <p className="muted">{L("把 Excel / CSV 拖进窗口，或点击回形针上传。然后直接提问，例如：", "Drag Excel/CSV files in, or click the paperclip. Then ask, e.g.:")}</p>
      {[L("各大区本季度销售额和同比是多少？", "What are sales by region this quarter, and YoY?"),
        L("找出复购率最高的客户群，并说明原因", "Which customer segment has the highest repeat rate, and why?"),
        L("做一个销售看板，可以按地区和品类筛选", "Build a sales dashboard filterable by region and category")].map((t) => <div key={t} className="muted">• {t}</div>)}
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

