import { ChevronDown, ChevronRight, Ellipsis } from "lucide-react";
import { MenuItem, Popover } from "../ui";
import { L } from "../../i18n";
import { useEffect, useState } from "react";
import { core } from "../../transport";
import { useApp } from "../../store";

export const sid = () => useApp.getState().selected!;
export const kcall = async (op: string, params: object = {}, timeout = 120) =>
  (await core.request("kernel/call", { session: sid(), op, params, timeout })).result;

/** Load data when the session, the data version (turn finished / files imported) or `deps` change. */
export function useLoad<T>(f: () => Promise<T>, deps: unknown[] = []): [T | undefined, string | undefined, () => void, boolean] {
  const session = useApp((s) => s.selected);
  const version = useApp((s) => s.dataVersion);
  const opened = useApp((s) => (s.selected ? !!s.open[s.selected] : false));
  const [v, setV] = useState<T>();
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!opened) return;
    let live = true;
    setBusy(true);
    f().then((r) => { if (live) { setV(r); setErr(undefined); } })
      .catch((e) => { if (live) setErr(e instanceof Error ? e.message : String(e)); })
      .finally(() => { if (live) setBusy(false); });
    return () => { live = false; };
  }, [session, version, n, opened, ...deps]);
  return [v, err, () => setN((x) => x + 1), busy];
}

export function Section({ title, right, children, open: o = true }: { title: string; right?: React.ReactNode; children: React.ReactNode; open?: boolean }) {
  const [open, setOpen] = useState(o);
  return (
    <div className="isec">
      <div className="ihead" onClick={() => setOpen(!open)}>
        <span>{open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}{title}</span>
        <span className="row" onClick={(e) => e.stopPropagation()}>{right}</span>
      </div>
      {open && <div className="ibody">{children}</div>}
    </div>
  );
}

/** "⋯" menu (popover). `label` is kept for call-site compatibility; the button shows an ellipsis icon. */
export function Menu({ label, items }: { label?: string; items: [string, () => void][] }) {
  void label;
  return (
    <Popover align="right" caret={false} className="more" title={L("更多", "More")} label={<Ellipsis size={15} />}>
      {items.map(([t, f]) => <MenuItem key={t} onClick={f}>{t}</MenuItem>)}
    </Popover>
  );
}
