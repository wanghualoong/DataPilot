// Small shared UI pieces: popover menus, icon buttons, switches, column resizers.
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { Check, ChevronDown, CircleCheck, CircleX, LoaderCircle, type LucideIcon } from "lucide-react";

export const ic = { size: 15, strokeWidth: 1.8 } as const;

/** Button + floating menu (position: fixed, so it is never clipped by scrolling panels). */
export function Popover({ label, title, className = "", up = false, align = "left", caret = true, open: o, onOpenChange, children }: {
  label: ReactNode; title?: string; className?: string; up?: boolean; align?: "left" | "right"; caret?: boolean;
  open?: boolean; onOpenChange?: (on: boolean) => void; children: ReactNode;
}) {
  const [own, setOwn] = useState(false);
  const open = o ?? own;
  const setOpen = (v: boolean) => { setOwn(v); onOpenChange?.(v); };
  const root = useRef<HTMLDivElement>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const [pos, setPos] = useState<CSSProperties>({});
  const place = () => {
    const r = btn.current?.getBoundingClientRect();
    if (!r) return;
    const s: CSSProperties = {};
    if (up) { s.bottom = window.innerHeight - r.top + 4; s.maxHeight = r.top - 12; }
    else { s.top = r.bottom + 4; s.maxHeight = window.innerHeight - r.bottom - 12; }
    if (align === "right") s.right = Math.max(4, window.innerWidth - r.right); else s.left = Math.max(4, r.left);
    setPos(s);
  };
  useEffect(() => { if (open) place(); }, [open]);
  useEffect(() => {
    if (!open) return;
    const h = (e: MouseEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    const k = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", h); document.addEventListener("keydown", k);
    return () => { document.removeEventListener("mousedown", h); document.removeEventListener("keydown", k); };
  }, [open]);
  return (
    <div className={"pop" + (open ? " open" : "") + " " + className} ref={root} onClick={(e) => e.stopPropagation()}>
      <button ref={btn} className="popbtn" title={title} aria-label={typeof label === "string" ? undefined : title} onClick={() => setOpen(!open)}>
        {label}{caret && <ChevronDown size={11} strokeWidth={2} className="muted" />}
      </button>
      {open && <div className="popmenu" style={pos} onClick={() => setOpen(false)}>{children}</div>}
    </div>
  );
}

export function MenuItem({ icon: Icon, children, onClick, checked, danger, hint, disabled }: {
  icon?: LucideIcon; children: ReactNode; onClick?: () => void; checked?: boolean; danger?: boolean; hint?: string; disabled?: boolean;
}) {
  return (
    <div className={"mi" + (danger ? " danger" : "") + (disabled ? " dis" : "")} onClick={(e) => { if (disabled) { e.stopPropagation(); return; } onClick?.(); }}>
      <span className="miicon">{checked ? <Check {...ic} /> : Icon ? <Icon {...ic} /> : null}</span>
      <span className="mitext"><span>{children}</span>{hint && <span className="mihint">{hint}</span>}</span>
    </div>
  );
}
export const MenuSep = () => <div className="msep" />;
export const MenuHead = ({ children }: { children: ReactNode }) => <div className="mhead">{children}</div>;

export function IconBtn({ icon: Icon, title, onClick, disabled, on, className = "", size, spin }: {
  icon: LucideIcon; title: string; onClick?: () => void; disabled?: boolean; on?: boolean; className?: string; size?: number; spin?: boolean;
}) {
  return (
    <button className={"iconbtn" + (on ? " on" : "") + " " + className} title={title} aria-label={title} disabled={disabled}
      onClick={(e) => { e.stopPropagation(); onClick?.(); }}>
      <Icon {...ic} size={size ?? ic.size} className={spin ? "spin" : undefined} />
    </button>
  );
}

export function Switch({ on, onChange, label, title, disabled }: { on: boolean; onChange: (v: boolean) => void; label: string; title?: string; disabled?: boolean }) {
  return (
    <label className={"switch" + (disabled ? " dis" : "")} title={title}>
      <input type="checkbox" checked={on} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span className="track" /><span className="slabel">{label}</span>
    </label>
  );
}

/** Drag handle between columns. `dir` = +1 when dragging right widens the column on the left, -1 for a column on the right. */
export function Resizer({ value, onChange, dir, min, max }: { value: number; onChange: (v: number) => void; dir: 1 | -1; min: number; max: () => number }) {
  const down = (e: React.MouseEvent) => {
    e.preventDefault();
    const x0 = e.clientX, base = value;
    const move = (ev: MouseEvent) => onChange(Math.round(Math.min(Math.max(min, base + dir * (ev.clientX - x0)), Math.max(min, max()))));
    const up = () => { document.removeEventListener("mousemove", move); document.removeEventListener("mouseup", up); document.body.classList.remove("resizing"); };
    document.addEventListener("mousemove", move); document.addEventListener("mouseup", up);
    document.body.classList.add("resizing");
  };
  return <div className="resizer" onMouseDown={down} />;
}

/** Number kept in localStorage (column widths). */
export function useStored(key: string, def: number): [number, (v: number) => void] {
  const [v, setV] = useState(() => Number(localStorage.getItem(key)) || def);
  return [v, (n: number) => { setV(n); localStorage.setItem(key, String(n)); }];
}

/** Status line: a leading "✅ " / "❌ " / "…" in the message becomes an icon. */
export function Status({ msg, className = "" }: { msg?: string | null; className?: string }) {
  if (!msg) return null;
  let icon: ReactNode = null, text = msg;
  if (msg.startsWith("✅")) { icon = <CircleCheck {...ic} size={13} className="ok" />; text = msg.slice(1).trimStart(); }
  else if (msg.startsWith("❌")) { icon = <CircleX {...ic} size={13} className="err" />; text = msg.slice(1).trimStart(); }
  else if (msg === "…") { icon = <LoaderCircle {...ic} size={13} className="spin muted" />; text = ""; }
  return <div className={"status small " + className}>{icon}<span>{text}</span></div>;
}
