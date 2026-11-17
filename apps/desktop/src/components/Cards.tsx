import { CircleHelp, ClipboardList, Globe, Search, ShieldAlert } from "lucide-react";
import { ic } from "./ui";
import { useState } from "react";
import { useApp } from "../store";
import { L } from "../i18n";
import { Markdown } from "./Markdown";
import type { Card } from "../reduce";

const choiceText: Record<string, string> = {
  allowOnce: L("已允许一次", "Allowed once"), allowSession: L("本会话允许", "Allowed for this chat"),
  allowAlways: L("总是允许", "Always allowed"), deny: L("已拒绝", "Denied"), "—": L("已结束", "Closed"),
  approve: L("已批准", "Approved"), approve_auto: L("已批准（自动执行）", "Approved (auto)"),
};

const riskText: Record<string, string> = {
  network: L("该操作会访问网络。", "This action accesses the network."),
  external: L("该操作会影响沙箱之外的内容。", "This action affects things outside the sandbox."),
};

export function ApprovalCard({ c }: { c: Extract<Card, { kind: "approval" }> }) {
  const respond = useApp((s) => s.respond);
  const [fb, setFb] = useState("");
  const answered = c.answered && <div className="answered">{choiceText[c.answered] ?? c.answered}</div>;
  // network switch is off / Chrome missing for web_search: yes → allowOnce, no → deny (Swift ApprovalCard)
  if (c.netKind === "network" || c.netKind === "chrome_install") {
    const net = c.netKind === "network";
    return (
      <div className="card approval net">
        <div className="ctitle">{net ? <Globe {...ic} className="accent" /> : <Search {...ic} className="accent" />}
          {net ? L("此操作需要联网", "This action needs the internet") : L("联网搜索需要 Chrome", "Web search needs Chrome")}</div>
        {net && <div className="mono">{c.summary}</div>}
        <div className="muted small">{net
          ? L("当前「联网」开关是关闭的。开启后，本会话中的网页抓取、浏览器和 Python/bash 都可以访问网络。", "The Network switch is off. Turning it on lets web fetch, the browser and Python/bash reach the internet in this chat.")
          : L("本机没有找到 Chrome / Chromium / Edge。是否下载 Google 官方的 Chrome for Testing（约 150 MB，仅首次，保存在 DataPilot 数据目录，不影响系统）？不下载也可以，本次会改用 Bing 搜索。", "No Chrome / Chromium / Edge found. Download Google's Chrome for Testing (~150 MB, once, stored in DataPilot's data folder)? Otherwise Bing is used this time.")}</div>
        {answered || (
          <div className="row">
            <button className="primary" onClick={() => respond(c.id, "approval", "allowOnce")}>{net ? L("开启联网并继续", "Enable network & continue") : L("下载并安装", "Download & install")}</button>
            <span className="sp" />
            <button className={net ? "danger" : ""} onClick={() => respond(c.id, "approval", "deny")}>{net ? L("保持关闭", "Keep off") : L("暂不，用 Bing", "Not now, use Bing")}</button>
          </div>
        )}
      </div>
    );
  }
  // "Always allow" scoped like Claude Code: web_fetch(domain:x.com) / bash(git status *)
  const scope = c.host ? `${c.tool}(${c.host})` : c.tool;
  return (
    <div className="card approval">
      <div className="ctitle"><ShieldAlert {...ic} className="warn" />{L("需要确认", "Needs approval")} · <code>{c.tool}</code></div>
      <div>{c.summary}</div>
      <div className="risk">{riskText[c.risk] ?? L("该操作会修改工作区。", "This action modifies the workspace.")}</div>
      {answered || (
        <>
          <input placeholder={L("拒绝时可以说明原因（可选）", "Reason if denying (optional)")} value={fb} onChange={(e) => setFb(e.target.value)} />
          <div className="row wrap">
            <button className="primary" onClick={() => respond(c.id, "approval", "allowOnce")}>{L("允许一次", "Allow once")}</button>
            <button onClick={() => respond(c.id, "approval", "allowSession")}>{L("本会话允许", "Allow for chat")}</button>
            <button title={L("写入权限规则：", "Saved as rule: ") + scope} onClick={() => respond(c.id, "approval", "allowAlways")}>{L("总是允许 ", "Always allow ") + scope}</button>
            <button className="danger" onClick={() => respond(c.id, "approval", "deny", fb || undefined)}>{L("拒绝", "Deny")}</button>
          </div>
        </>
      )}
    </div>
  );
}

export function QuestionCard({ c }: { c: Extract<Card, { kind: "question" }> }) {
  const respond = useApp((s) => s.respond);
  const [text, setText] = useState("");
  return (
    <div className="card question">
      <div className="ctitle"><CircleHelp {...ic} className="accent" />{c.question}</div>
      {c.answered ? <div className="answered">{c.answered === "—" ? choiceText["—"] : c.answered}</div> : (
        <>
          <div className="row wrap">{c.options.map((o) => <button key={o} onClick={() => respond(c.id, "question", o)}>{o}</button>)}</div>
          <div className="row">
            <input placeholder={L("或者输入你的回答", "Or type an answer")} value={text} onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && text.trim()) respond(c.id, "question", text.trim()); }} />
            <button disabled={!text.trim()} onClick={() => respond(c.id, "question", text.trim())}>{L("回答", "Answer")}</button>
          </div>
        </>
      )}
    </div>
  );
}

export function PlanCard({ c }: { c: Extract<Card, { kind: "plan" }> }) {
  const respond = useApp((s) => s.respond);
  const [fb, setFb] = useState("");
  return (
    <div className="card plan">
      <div className="ctitle"><ClipboardList {...ic} className="accent" />{L("计划", "Plan")}</div>
      <Markdown text={c.plan} />
      {c.answered ? <div className="answered">{choiceText[c.answered] ?? L("已退回：", "Sent back: ") + c.answered}</div> : (
        <>
          <div className="row">
            <button className="primary" onClick={() => respond(c.id, "plan", "approve")}>{L("批准并执行（逐步确认）", "Approve (ask each step)")}</button>
            <button onClick={() => respond(c.id, "plan", "approve_auto")}>{L("批准并自动执行", "Approve (auto)")}</button>
          </div>
          <div className="row">
            <input placeholder={L("修改意见", "What to change")} value={fb} onChange={(e) => setFb(e.target.value)} />
            <button disabled={!fb.trim()} onClick={() => respond(c.id, "plan", fb.trim())}>{L("退回修改", "Send back")}</button>
          </div>
        </>
      )}
    </div>
  );
}
