import { useEffect, useState } from "react";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { core } from "../../transport";
import { Providers } from "./Providers";
import { Roles } from "./Roles";
import { General } from "./General";
import { Permissions } from "./Permissions";
import { Extensions } from "./Extensions";
import { Retrieval } from "./Retrieval";
import { FileChartColumn, Mail, X } from "lucide-react";
import { openPath } from "../../native";
import { openExternal } from "../links";
import { IconBtn } from "../ui";

function About() {
  const [a, setA] = useState<any>();
  const [v, setV] = useState<any>();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    core.request("app/about").then(setA).catch(() => {});
    core.request("initialize").then(setV).catch(() => {});
  }, []);
  const c = a?.credits;
  const ver = a?.version ?? v?.version ?? "…";
  const os = navigator.userAgent.includes("Windows") ? "Windows" : "macOS";
  const doc = (k: string) => a?.docs?.[k];
  const contact = c?.contact ?? "wanghualoong@126.com";
  const section = (title: string, items: any[]) => (
    <section>
      <h4>{title}</h4>
      {items.map((it) => (
        <div key={it.name} className="credit">
          <div className="row"><a href="#" onClick={(e) => { e.preventDefault(); openExternal(it.url); }}>{it.name}</a><span className="chip">{it.license}</span></div>
          <div className="muted small">{L(it.use_zh, it.use_en)}</div>
        </div>
      ))}
    </section>
  );
  return (
    <div className="pane about">
      <div className="row">
        <FileChartColumn size={56} strokeWidth={1.4} className="accent" />
        <div className="grow">
          <h2>DataPilot</h2>
          <div className="muted">{L("用自然语言分析数据、生成 BI 看板的桌面 Agent", "A desktop agent for natural-language data analysis and BI dashboards")}</div>
          <div className="row small">
            <span className="mono">{L("版本 ", "Version ")}{ver}</span>
            <button className="link" onClick={() => { navigator.clipboard.writeText(`DataPilot ${ver} · ${os}`); setCopied(true); }}>{copied ? L("已复制", "Copied") : L("复制", "Copy")}</button>
            <span className="muted">· {L("协议", "protocol")} {v?.protocol ?? "…"} · {v?.tools?.length ?? 0} {L("个工具", "tools")}</span>
          </div>
        </div>
      </div>
      <section>
        <h4>{L("许可", "License")}</h4>
        <div>{L("DataPilot 以 Apache License 2.0 发布。", "DataPilot is released under the Apache License 2.0.")}</div>
        {c && <div className="muted small">{L(c.usage_zh, c.usage_en)}</div>}
        <div className="row wrap">
          <button disabled={!doc("license")} onClick={() => openPath(doc("license"))}>{L("查看许可全文", "View License")}</button>
          <button disabled={!doc("notices")} onClick={() => openPath(doc("notices"))}>{L("查看完整第三方声明", "View Third-Party Notices")}</button>
          <button disabled={!doc("chat2db")} onClick={() => openPath(doc("chat2db"))}>{L("Chat2DB 许可", "Chat2DB License")}</button>
        </div>
      </section>
      {c?.code && section(L("致谢：包含或改编了以下项目的代码", "Thanks — includes or adapts code from"), c.code)}
      {c?.concepts && section(L("致谢：借鉴了以下项目的设计思路", "Thanks — design ideas from"), c.concepts)}
      <section>
        <h4>{L("联系我们", "Contact")}</h4>
        <div className="row"><span className="grow mono">{contact}</span>
          <button onClick={() => openExternal(`mailto:${contact}?subject=${encodeURIComponent(`DataPilot ${ver} 反馈 / Feedback`)}&body=${encodeURIComponent(`\n\n---\nDataPilot ${ver}\n${os}\n`)}`)}>
            <Mail size={14} /> {L("写邮件反馈", "Send Feedback")}</button></div>
      </section>
      {c?.copyright && <div className="muted small">{c.copyright}</div>}
    </div>
  );
}

const TABS: [string, string][] = [["providers", L("模型服务", "Providers")], ["roles", L("模型角色", "Model roles")], ["retrieval", L("索引与检索", "Retrieval")],
  ["general", L("通用", "General")], ["permissions", L("权限", "Permissions")], ["extensions", L("扩展", "Extensions")], ["about", L("关于", "About")]];

export function Settings({ tab = "providers", onTab, onClose }: { tab?: string; onTab?: (t: string) => void; onClose: () => void }) {
  const setTab = (t: string) => onTab?.(t);
  const loadConfig = useApp((s) => s.loadConfig);
  const config = useApp((s) => s.config);
  useEffect(() => { loadConfig().catch(() => {}); }, []);
  return (
    <div className="modal" onClick={onClose}>
      <div className="modalbox settings" onClick={(e) => e.stopPropagation()}>
        <div className="row"><h3 className="grow">{L("设置", "Settings")}</h3><IconBtn icon={X} title={L("关闭", "Close")} onClick={onClose} /></div>
        <div className="seg">{TABS.map(([k, t]) => <button key={k} className={tab === k ? "on" : ""} onClick={() => setTab(k)}>{t}</button>)}</div>
        <div className="settingsbody">
          {!config ? <div className="muted">…</div> : tab === "providers" ? <Providers /> : tab === "roles" ? <Roles /> : tab === "retrieval" ? <Retrieval /> : tab === "general" ? <General />
            : tab === "permissions" ? <Permissions /> : tab === "extensions" ? <Extensions /> : <About />}
        </div>
      </div>
    </div>
  );
}
