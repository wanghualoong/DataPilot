import { useEffect } from "react";
import { FolderOpen, ShieldCheck } from "lucide-react";
import { L } from "../i18n";
import { useApp } from "../store";

/** Shown after a folder is picked for a new project: the user must trust it before it is added (or cancel a wrong pick). */
export function TrustFolder() {
  const path = useApp((s) => s.trustFolder);
  const cancel = () => useApp.setState({ trustFolder: undefined });
  const trust = () => { if (!path) return; useApp.setState({ trustFolder: undefined }); useApp.getState().openProject(path); };
  useEffect(() => {
    if (!path) return;
    const k = (e: KeyboardEvent) => { if (e.key === "Escape") cancel(); };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [path]);
  if (!path) return null;
  const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
  return (
    <div className="modal" onClick={cancel}>
      <div className="modalbox trustbox" role="dialog" aria-modal="true" onClick={(e) => e.stopPropagation()}>
        <div className="row"><ShieldCheck size={18} className="accent" /><h3 className="grow">{L("要信任这个文件夹吗？", "Trust this folder?")}</h3></div>
        <div className="trustpath row"><FolderOpen size={15} className="muted" /><b>{name}</b><span className="muted small ell grow" title={path}>{path}</span></div>
        <p className="muted small">{L("添加为项目后，DataPilot 会：", "Once added as a project, DataPilot will:")}</p>
        <ul className="small">
          <li>{L("读取这个文件夹里的文件（含子文件夹），用于分析和回答问题；", "read files in this folder (and subfolders) to analyze and answer questions;")}</li>
          <li>{L("在文件夹里创建 .datapilot/（对话、导入的数据、项目记忆）和 outputs/（导出的报告、图表）；", "create .datapilot/ (chats, imported data, project memory) and outputs/ (exported reports, charts) inside it;")}</li>
          <li>{L("按你的权限设置，在沙箱里运行 SQL / Python 处理这些文件。", "run SQL / Python on these files in the sandbox, as your permission settings allow.")}</li>
        </ul>
        <p className="muted small">{L("只添加你自己的、内容可信的文件夹。选错了请点「取消」，不会做任何改动。", "Only add folders you own and trust. Picked the wrong one? Cancel — nothing will be changed.")}</p>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button onClick={cancel}>{L("取消", "Cancel")}</button>
          <button className="primary" autoFocus onClick={trust}>{L("信任并添加", "Trust and add")}</button>
        </div>
      </div>
    </div>
  );
}
