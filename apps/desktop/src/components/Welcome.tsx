import { FileChartColumn, FolderOpen, KeyRound, SlidersHorizontal, SquarePen } from "lucide-react";
import { useApp } from "../store";
import { pickFolder } from "../native";
import { L } from "../i18n";

export function Welcome() {
  const { newChat } = useApp();
  const hasMain = useApp((s) => !!s.config?.roles?.main);
  return (
    <div className="welcome">
      <FileChartColumn size={56} strokeWidth={1.4} className="accent" />
      <h1>DataPilot</h1>
      <p className="muted">{L("用自然语言分析表格、数据库和文档，生成图表、看板与报告", "Analyze spreadsheets, databases and documents in plain language; get charts, dashboards and reports")}</p>
      <div className="row">
        <button className={hasMain ? "primary big" : "big"} onClick={() => newChat()}><SquarePen size={15} /> {L("新建会话", "New Chat")}</button>
        <button className="big" onClick={async () => { const p = await pickFolder(); if (p) useApp.getState().requestProject(p); }}><FolderOpen size={15} /> {L("打开项目文件夹…", "Open project folder…")}</button>
      </div>
      {!hasMain && (
        <div className="row">
          <button className="primary big" title={L("用网关账号（如 SSO / OIDC）登录，直接从你的 Key 列表里选", "Log in to the gateway (e.g. SSO / OIDC) and pick one of your keys")}
            onClick={() => useApp.setState({ quickLogin: true })}><KeyRound size={15} /> {L("登录并选择 Key", "Log in & choose key")}</button>
          <button className="big" onClick={() => useApp.setState({ onboarding: true })}><SlidersHorizontal size={15} /> {L("配置模型", "Configure Models")}</button>
        </div>
      )}
    </div>
  );
}
