import { CircleCheck, Database, Folder, Globe, LayoutDashboard, Brain, X, type LucideIcon } from "lucide-react";
import { L } from "../i18n";
import { useApp } from "../store";
import { DataPanel } from "./inspector/DataPanel";
import { DashboardPanel } from "./inspector/DashboardPanel";
import { MemoryPanel } from "./inspector/MemoryPanel";
import { BrowserPanel, FilesPanel, TodoPanel } from "./inspector/OtherPanels";
import { IconBtn } from "./ui";

// same tabs / icons as the SwiftUI InspectorTab
const TABS: [string, string, LucideIcon][] = [
  ["data", L("数据", "Data"), Database], ["dashboards", L("看板", "Dashboards"), LayoutDashboard], ["browser", L("浏览器", "Browser"), Globe],
  ["memory", L("记忆", "Memory"), Brain], ["todos", L("任务", "Tasks"), CircleCheck], ["files", L("文件", "Files"), Folder],
];

export function Inspector({ width }: { width: number }) {
  const { inspector, openInspector, selected } = useApp();
  const usedBrowser = useApp((s) => (s.selected ? (s.chats[s.selected]?.items ?? []).some((i: any) => i.tool_name === "browser") : false));
  if (!inspector || !selected) return null;
  const tabs = TABS.filter(([k]) => k !== "browser" || usedBrowser || inspector.tab === "browser");
  return (
    <aside className="inspector" style={{ width }}>
      <div className="itabs">
        <div className="segbar">
          {tabs.map(([k, t, Icon]) => (
            <button key={k} title={t} className={inspector.tab === k ? "on" : ""} onClick={() => openInspector(k)}>
              <Icon size={14} strokeWidth={1.8} /><span>{t}</span>
            </button>
          ))}
        </div>
        <IconBtn icon={X} title={L("关闭检查器", "Close inspector")} onClick={() => openInspector()} />
      </div>
      <div className="ibodywrap">
        {inspector.tab === "data" && <DataPanel />}
        {inspector.tab === "dashboards" && <DashboardPanel />}
        {inspector.tab === "browser" && <BrowserPanel />}
        {inspector.tab === "memory" && <MemoryPanel />}
        {inspector.tab === "todos" && <TodoPanel />}
        {inspector.tab === "files" && <FilesPanel />}
      </div>
    </aside>
  );
}
