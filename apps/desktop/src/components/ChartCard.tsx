import { useCallback } from "react";
import { WebFrame } from "./WebFrame";
import { saveText } from "../actions";

/** Inline chart (display kind "chart"): Resources/web renderer in an iframe; CSV export goes through the core. */
export function ChartCard({ display }: { display: any }) {
  const render = useCallback((w: any) => w.DP?.renderChart(display), [display]);
  const onMessage = useCallback((m: any) => {
    if (m.type === "exportCSV") saveText(m.name || "chart", "csv", m.csv || "", true);
  }, []);
  return <WebFrame render={render} onMessage={onMessage} height={360} />;
}
