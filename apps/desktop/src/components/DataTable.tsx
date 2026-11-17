import { useMemo, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

const fmt = (v: unknown) => (v === null || v === undefined ? "" : typeof v === "number" ? (Number.isInteger(v) ? v.toLocaleString() : v.toLocaleString(undefined, { maximumFractionDigits: 4 })) : typeof v === "object" ? JSON.stringify(v) : String(v));

/** Virtualized result grid (fine-grained data is shown as is; only the visible rows are rendered). */
export function DataTable({ columns, rows, max = 2000 }: { columns: string[]; rows: unknown[][]; max?: number }) {
  const shown = useMemo(() => rows.slice(0, max), [rows, max]);
  const ref = useRef<HTMLDivElement>(null);
  const v = useVirtualizer({ count: shown.length, getScrollElement: () => ref.current, estimateSize: () => 26, overscan: 12 });
  const grid = { gridTemplateColumns: `repeat(${columns.length}, minmax(90px, 1fr))` };
  return (
    <div className="dt">
      <div className="dtrow dthead" style={grid}>{columns.map((c) => <div key={c} title={c}>{c}</div>)}</div>
      <div className="dtbody" ref={ref} style={{ maxHeight: Math.min(shown.length * 26, 320) }}>
        <div style={{ height: v.getTotalSize(), position: "relative" }}>
          {v.getVirtualItems().map((r) => (
            <div key={r.key} className="dtrow" style={{ ...grid, position: "absolute", top: 0, left: 0, right: 0, transform: `translateY(${r.start}px)` }}>
              {(shown[r.index] ?? []).map((c, i) => <div key={i} className={typeof c === "number" ? "num" : ""}>{fmt(c)}</div>)}
            </div>
          ))}
        </div>
      </div>
      {rows.length > shown.length && <div className="muted pad">… {rows.length.toLocaleString()} rows</div>}
    </div>
  );
}
