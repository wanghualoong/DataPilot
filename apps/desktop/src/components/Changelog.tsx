// 更新日志 / Changelog: Settings → About, and shown once after an upgrade (Resources/about/changelog.json via app/about).
import { useState } from "react";
import { ChevronDown, ChevronRight, Sparkles, Wrench, Gauge, X } from "lucide-react";
import { L } from "../i18n";
import { IconBtn } from "./ui";

type Item = { zh: string; en: string };
export type Release = { version: string; date: string; summary_zh: string; summary_en: string; added?: Item[]; fixed?: Item[]; improved?: Item[] };

const GROUPS: [keyof Release, () => string, typeof Sparkles][] = [
  ["added", () => L("新功能", "New"), Sparkles],
  ["fixed", () => L("问题修复", "Fixed"), Wrench],
  ["improved", () => L("改进", "Improved"), Gauge],
];

function ReleaseBlock({ r, open: initial, current }: { r: Release; open: boolean; current: boolean }) {
  const [open, setOpen] = useState(initial);
  return (
    <div className="clrel">
      <div className="clhead" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <b>{r.version}</b><span className="muted small">{r.date}</span>
        {current && <span className="chip">{L("当前版本", "current")}</span>}
      </div>
      <div className="clsum">{L(r.summary_zh, r.summary_en)}</div>
      {open && GROUPS.map(([k, title, Icon]) => {
        const items = (r[k] as Item[] | undefined) ?? [];
        return items.length ? (
          <div key={k} className="clgroup">
            <div className="clgtitle"><Icon size={13} /> {title()}</div>
            <ul>{items.map((it, i) => <li key={i}>{L(it.zh, it.en)}</li>)}</ul>
          </div>
        ) : null;
      })}
    </div>
  );
}

/** All releases, newest first; the current one (or the first) expanded. */
export function ChangelogList({ releases, current }: { releases: Release[]; current?: string }) {
  if (!releases?.length) return <div className="muted small">{L("暂无更新日志", "No changelog")}</div>;
  return <div className="changelog">{releases.map((r, i) => <ReleaseBlock key={r.version} r={r} current={r.version === current} open={current ? r.version === current : i === 0} />)}</div>;
}

export function ChangelogModal({ releases, current, onClose, upgraded }: { releases: Release[]; current?: string; onClose: () => void; upgraded?: boolean }) {
  return (
    <div className="modal" onClick={onClose}>
      <div className="modalbox changelogbox" onClick={(e) => e.stopPropagation()}>
        <div className="row"><h3 className="grow">{upgraded ? L(`DataPilot 已更新到 ${current}`, `DataPilot updated to ${current}`) : L("更新日志", "Changelog")}</h3>
          <IconBtn icon={X} title={L("关闭", "Close")} onClick={onClose} /></div>
        <ChangelogList releases={releases} current={current} />
        {upgraded && <div className="muted small">{L("以后可在 设置 → 关于 → 更新日志 中再次查看。", "You can find this again under Settings → About → Changelog.")}</div>}
        <div className="row"><span className="grow" /><button className="primary" onClick={onClose}>{L("知道了", "Got it")}</button></div>
      </div>
    </div>
  );
}
