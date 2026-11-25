import { useEffect, useMemo, useRef, useState } from "react";
import { imeEnter } from "../ime";
import {
  Archive, ChevronDown, ChevronRight, Folder, FolderOpen, FolderPlus, FolderSync, FolderX, Info, KeyRound, LoaderCircle, Lock,
  MessageSquare, Monitor, Moon, PanelLeftClose, Sun, Plus, Pencil, Pin, PinOff, Search, Settings as Gear, SquarePen, Trash2, X,
} from "lucide-react";
import { ask, pickFolder, reveal } from "../native";
import { useApp } from "../store";
import { core } from "../transport";
import { L } from "../i18n";
import type { ProjectInfo, SessionInfo } from "../protocol/rpc";
import { Settings } from "./settings/Settings";
import { IconBtn, MenuHead, MenuItem, MenuSep, Popover, ic } from "./ui";
import { onTheme, setThemePref, themePref } from "../theme";

/** Right-click / ⋯ menu at a fixed position. */
function ContextMenu({ at, onClose, children }: { at: { x: number; y: number }; onClose: () => void; children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const h = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) onClose(); };
    const k = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("mousedown", h); document.addEventListener("keydown", k);
    return () => { document.removeEventListener("mousedown", h); document.removeEventListener("keydown", k); };
  }, []);
  const style = { left: Math.min(at.x, window.innerWidth - 240), top: Math.min(at.y, window.innerHeight - 260) };
  return <div ref={ref} className="popmenu" style={style} onClick={(e) => { e.stopPropagation(); onClose(); }}>{children}</div>;
}

function dayLabel(t: number) {
  if (!t) return "";
  const d = new Date(t * 1000), now = new Date();
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const y = new Date(now); y.setDate(now.getDate() - 1);
  if (d.toDateString() === now.toDateString()) return L("今天 ", "Today ") + time;
  if (d.toDateString() === y.toDateString()) return L("昨天 ", "Yesterday ") + time;
  return d.toLocaleDateString([], { month: "short", day: "numeric" }) + " " + time;
}

function Row({ s, indent }: { s: SessionInfo; indent: boolean }) {
  const { selected, select, rename, flag, remove } = useApp();
  const running = useApp((x) => x.chats[s.id]?.running);
  const title = useApp((x) => x.chats[s.id]?.title) || s.title || L("新会话", "New chat");
  const [editing, setEditing] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number }>();
  return (
    <div className={"srow" + (selected === s.id ? " sel" : "") + (indent ? " indent" : "")} title={title}
      onClick={() => select(s.id)} onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY }); }}>
      {!indent && <MessageSquare {...ic} size={14} className="muted sicon" />}
      <div className="stext">
        {editing ? (
          <input autoFocus defaultValue={title} onClick={(e) => e.stopPropagation()}
            onBlur={(e) => { setEditing(false); if (e.target.value.trim()) rename(s.id, e.target.value.trim()); }}
            onKeyDown={(e) => { if (imeEnter(e)) return; if (e.key === "Enter") (e.target as HTMLInputElement).blur(); if (e.key === "Escape") setEditing(false); }} />
        ) : (
          <span className="stitle">{s.pinned && <Pin size={10} className="muted pinmark" />}{title}</span>
        )}
        <span className="sdate">{dayLabel(s.updated)}</span>
      </div>
      {running && <LoaderCircle {...ic} size={13} className="spin muted" />}
      {menu && (
        <ContextMenu at={menu} onClose={() => setMenu(undefined)}>
          <MenuItem icon={Pencil} onClick={() => setEditing(true)}>{L("重命名", "Rename")}</MenuItem>
          <MenuItem icon={s.pinned ? PinOff : Pin} onClick={() => flag(s.id, "pin", !s.pinned)}>{s.pinned ? L("取消置顶", "Unpin") : L("置顶", "Pin")}</MenuItem>
          <MenuItem icon={FolderOpen} onClick={async () => { const r = await core.request("session/open", { id: s.id }).catch(() => null); const d = r?.dir ?? r?.workspace; if (d) reveal(d); }}>
            {L("在访达 / 资源管理器中显示", "Show in Finder / Explorer")}</MenuItem>
          <MenuSep />
          <MenuItem icon={Archive} onClick={() => flag(s.id, "archive", !s.archived)}>{s.archived ? L("取消归档", "Unarchive") : L("归档", "Archive")}</MenuItem>
          <MenuItem icon={Trash2} danger onClick={async () => { if (await ask(L("删除这个对话？", "Delete this chat?"))) remove(s.id); }}>{L("删除", "Delete")}</MenuItem>
        </ContextMenu>
      )}
    </div>
  );
}

function ProjectGroup({ p, items, collapsed, toggle }: { p: ProjectInfo; items: SessionInfo[]; collapsed: boolean; toggle: () => void }) {
  const { newChat, reload, fail } = useApp();
  const [menu, setMenu] = useState<{ x: number; y: number }>();
  const relink = async () => {
    const path = await pickFolder();
    if (path) { await core.request("project/relink", { id: p.id, path }).catch(fail); await reload(); }
  };
  return (
    <div>
      <div className={"prow" + (p.exists ? "" : " missing")} title={p.path} onClick={toggle}
        onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY }); }}>
        {collapsed ? <ChevronRight size={12} className="muted" /> : <ChevronDown size={12} className="muted" />}
        {collapsed ? <Folder {...ic} size={14} /> : <FolderOpen {...ic} size={14} />}
        <span className="grow">{p.name}</span>
        {!p.exists && <IconBtn icon={FolderSync} size={13} title={L("重新定位项目文件夹", "Relink the project folder")} onClick={relink} />}
        <IconBtn icon={SquarePen} size={13} className="hov" title={L("在此项目中新建会话", "New chat in this project")} onClick={() => newChat(p.id)} />
      </div>
      {!collapsed && items.map((s) => <Row key={s.id} s={s} indent />)}
      {!collapsed && !items.length && p.exists && (
        <div className="srow indent muted small" onClick={() => newChat(p.id)} title={L("在此项目中新建会话", "New chat in this project")}><Plus size={12} /> {L("新建会话", "New chat")}</div>
      )}
      {menu && (
        <ContextMenu at={menu} onClose={() => setMenu(undefined)}>
          <MenuItem icon={SquarePen} onClick={() => newChat(p.id)}>{L("新建会话", "New chat")}</MenuItem>
          <MenuItem icon={FolderOpen} onClick={() => reveal(p.path)}>{L("在访达 / 资源管理器中显示", "Show in Finder / Explorer")}</MenuItem>
          <MenuItem icon={FolderSync} onClick={relink}>{L("重新定位文件夹…", "Relink folder…")}</MenuItem>
          <MenuSep />
          <MenuItem icon={X} onClick={async () => { await core.request("project/remove", { id: p.id }).catch(fail); await reload(); }}>{L("从侧栏移除（保留文件）", "Remove from sidebar (keep files)")}</MenuItem>
          <MenuItem icon={FolderX} danger onClick={async () => {
            if (!(await ask(L(`删除项目「${p.name}」的 DataPilot 数据（.datapilot、outputs、DATAPILOT.md 移到废纸篓 / 回收站）并从侧栏移除？原始数据文件不动。`,
              `Move DataPilot data of “${p.name}” (.datapilot, outputs, DATAPILOT.md) to the Trash and remove it? Your source files stay.`)))) return;
            const r = await core.request("project/delete_data", { id: p.id }).catch((e) => { fail(e); return null; });
            if (r?.failed?.length) fail(r.failed.join("\n"));
            await reload();
          }}>{L("删除项目数据（不保留，可重新开始）…", "Delete project data (start over)…")}</MenuItem>
        </ContextMenu>
      )}
    </div>
  );
}

function Footer({ openSettings }: { openSettings: (tab?: string) => void }) {
  const r = useApp((s) => s.config?.roles) ?? {};
  const local = useApp((s) => !!s.config?.settings?.localOnly);
  const [theme, setTheme] = useState(themePref());
  useEffect(() => onTheme(() => setTheme(themePref())), []);
  const item = (label: string, v?: string | null, fallback = "—") => (
    <div className="fitem"><span className={"dot" + (v ? " ok" : "")} /><span className="muted">{label}</span><span className="ell">{v ?? fallback}</span></div>
  );
  return (
    <div className="sbbottom">
      <div className="fstatus" onClick={() => openSettings("providers")} title={L("点击打开设置", "Click to open Settings")}>
        {item(L("对话", "Chat"), r.main?.model)}
        {local && item(L("模式", "Mode"), L("本地模式", "Local only"))}
        {item(L("图片", "Vision"), r.main?.capabilities?.supportsVision ? L("支持", "yes") : null, L("不支持", "no"))}
        {item(L("检索", "Search"), L("关键词 + grep", "keyword + grep"))}
      </div>
      <div className="row">
        <Popover up caret={false} title={L("设置与账号", "Settings & account")} label={<Gear {...ic} size={17} />}>
          <MenuItem icon={KeyRound} onClick={() => useApp.setState({ quickLogin: true })}>{L("登录并选择 Key…", "Log in & choose key…")}</MenuItem>
          <MenuSep />
          <MenuItem icon={Gear} onClick={() => openSettings("providers")}>{L("设置…", "Settings…")}</MenuItem>
          <MenuSep />
          <MenuHead>{L("外观", "Appearance")}</MenuHead>
          {([["system", L("跟随系统", "System"), Monitor], ["light", L("浅色", "Light"), Sun], ["dark", L("深色", "Dark"), Moon]] as const).map(([k, t, I]) => (
            <MenuItem key={k} icon={I} checked={theme === k} onClick={() => { setThemePref(k); setTheme(k); }}>{t}</MenuItem>
          ))}
          <MenuSep />
          <MenuItem icon={Info} onClick={() => openSettings("about")}>{L("关于 DataPilot…", "About DataPilot…")}</MenuItem>
        </Popover>
        {local && <Lock {...ic} size={13} className="ok" />}
        {!r.main && <button className="link" onClick={() => useApp.setState({ quickLogin: true })}>{L("登录并选择 Key", "Log in & choose key")}</button>}
      </div>
    </div>
  );
}

export function Sidebar({ width }: { width: number }) {
  const { sessions, projects, newChat, requestProject, search, setSearch, hits, settingsTab, openSettings } = useApp();
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(() => JSON.parse(localStorage.getItem("dp.collapsed") || "{}"));
  useEffect(() => { localStorage.setItem("dp.collapsed", JSON.stringify(collapsed)); }, [collapsed]);
  const visible = useMemo(() => (hits ? sessions.filter((s) => hits.has(s.id)) : sessions), [sessions, hits]);
  const loose = visible.filter((s) => !s.project_id);
  const newProject = async () => { const p = await pickFolder(); if (p) requestProject(p); };
  const selected = useApp((s) => s.selected);
  const curPid = sessions.find((s) => s.id === selected)?.project_id;
  const curProject = curPid ? projects.find((p) => p.id === curPid) : undefined;
  return (
    <aside className="sidebar" style={{ width }}>
      <div className="sbtop">
        <div className="searchbox"><Search {...ic} size={13} className="muted" />
          <input className="search" placeholder={L("搜索会话内容", "Search chats")} value={search} onChange={(e) => setSearch(e.target.value)} />
          {search && <IconBtn icon={X} size={12} title={L("清除", "Clear")} onClick={() => setSearch("")} />}
        </div>
        <Popover align="right" caret={false} title={L("新建会话（⌘N）；也可新建项目", "New chat (⌘N); or a new project")} label={<SquarePen {...ic} size={16} />}>
          <MenuItem icon={MessageSquare} onClick={() => newChat()}>{L("新建对话", "New chat")}</MenuItem>
          {curProject && <MenuItem icon={SquarePen} onClick={() => newChat(curProject.id)}>{L(`在「${curProject.name}」中新建会话`, `New chat in “${curProject.name}”`)}</MenuItem>}
          <MenuItem icon={FolderPlus} onClick={newProject}>{L("新建 / 打开项目文件夹…", "New / open project folder…")}</MenuItem>
        </Popover>
        <IconBtn icon={PanelLeftClose} title={L("隐藏侧栏", "Hide sidebar")} onClick={() => useApp.setState({ sidebarHidden: true })} />
      </div>
      <div className="slist">
        <div className="section"><span>{L("项目", "Projects")}</span>
          <IconBtn icon={FolderPlus} size={13} title={L("新建 / 打开项目文件夹", "New / open project folder")} onClick={newProject} /></div>
        {projects.map((p) => {
          const items = visible.filter((s) => s.project_id === p.id);
          if (hits && !items.length) return null;
          return <ProjectGroup key={p.id} p={p} items={items} collapsed={!!collapsed[p.id]} toggle={() => setCollapsed({ ...collapsed, [p.id]: !collapsed[p.id] })} />;
        })}
        {!projects.length && (
          <div className="srow muted" onClick={newProject} title={L("选择一个文件夹作为项目：对话、数据库和产出都保存在该文件夹中", "Pick a folder as a project")}>
            <FolderPlus {...ic} size={14} className="sicon" /><span className="small">{L("打开一个文件夹作为项目…", "Open a folder as a project…")}</span>
          </div>
        )}
        <div className="section"><span>{L("对话", "Chats")}</span>
          <IconBtn icon={SquarePen} size={13} title={L("新建对话（不属于项目）", "New chat (no project)")} onClick={() => newChat()} /></div>
        {loose.map((s) => <Row key={s.id} s={s} indent={false} />)}
        {hits && !visible.length && <div className="muted pad small">{L("没有匹配的对话", "No matching chats")}</div>}
      </div>
      <Footer openSettings={openSettings} />
      {settingsTab && <Settings tab={settingsTab} onTab={(t) => openSettings(t)} onClose={() => useApp.setState({ settingsTab: undefined })} />}
    </aside>
  );
}
