import { IconBtn } from "../ui";
import { X } from "lucide-react";
import { useState } from "react";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { patch } from "./common";

type Rule = { tool: string; pattern?: string | null; behavior: "allow" | "ask" | "deny" };
const show = (r: Rule) => (r.pattern ? `${r.tool}(${r.pattern})` : r.tool);
/** "web_fetch(domain:x.com)" → {tool, pattern} */
export function parseRule(t: string): { tool: string; pattern?: string } | null {
  const m = t.trim().match(/^([A-Za-z0-9_*:-]+)(?:\((.*)\))?$/);
  return m ? { tool: m[1], ...(m[2] ? { pattern: m[2] } : {}) } : null;
}
const label = (b: string) => ({ allow: L("允许", "Allow"), ask: L("询问", "Ask"), deny: L("拒绝", "Deny") } as any)[b] ?? b;

export function Permissions() {
  const rules: Rule[] = useApp((s) => s.config?.rules) ?? [];
  const [text, setText] = useState("");
  const [beh, setBeh] = useState<Rule["behavior"]>("allow");
  const [err, setErr] = useState<string>();
  const add = async () => {
    const r = parseRule(text);
    if (!r) return setErr(L("格式：工具名 或 工具名(模式)", "Format: tool or tool(pattern)"));
    setErr(undefined);
    if (await patch({ rules: [...rules, { ...r, behavior: beh }] })) setText("");
  };
  return (
    <div className="pane">
      <div className="muted small">{L("规则按顺序匹配，拒绝优先。审批卡上的「始终允许」也会写到这里。", "Rules match in order; deny wins. “Always allow” on approval cards adds rules here.")}</div>
      {!rules.length && <div className="muted small">{L("还没有规则", "No rules yet")}</div>}
      {rules.map((r, i) => (
        <div key={i} className="irow">
          <span className={"chip " + r.behavior}>{label(r.behavior)}</span>
          <span className="grow mono">{show(r)}</span>
          <IconBtn icon={X} size={13} className="danger" title={L("删除", "Remove")} onClick={() => patch({ rules: rules.filter((_, j) => j !== i) })} />
        </div>
      ))}
      <h4>{L("添加规则", "Add rule")}</h4>
      <div className="row">
        <select style={{ flex: "none" }} value={beh} onChange={(e) => setBeh(e.target.value as any)}>{["allow", "ask", "deny"].map((b) => <option key={b} value={b}>{label(b)}</option>)}</select>
        <input placeholder="web_fetch(domain:weather.com.cn)  ·  bash(git status *)  ·  mcp__playwright__*" value={text} onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.key === "Enter" && add()} />
        <button disabled={!text.trim()} onClick={add}>{L("添加", "Add")}</button>
      </div>
      {err && <div className="notice err small">{err}</div>}
      <div className="muted small">{L("优先级：拒绝 > 询问 > 允许。说明符：domain:主机（含子域名）、前缀*（命令/路径前缀）、或子串。计划模式与联网开关仍然生效。",
        "Precedence: deny > ask > allow. Specifiers: domain:host (incl. subdomains), prefix* (command/path prefix), or substring. Plan mode and the Network switch still apply.")}</div>
    </div>
  );
}
