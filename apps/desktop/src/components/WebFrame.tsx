import { useEffect, useRef } from "react";
import { L } from "../i18n";
import { useApp } from "../store";
import { onTheme, resolvedTheme } from "../theme";

/** Reuses Resources/web (served same-origin under /web/) in an iframe. The page talks back through host.js →
 *  window.parent.postMessage({__dp: frameId, msg}); we call its globals (DP / DPER) directly — same origin. */
let seq = 0;
const lang = () => (L("zh", "en") === "zh" ? "zh" : "en");
const theme = resolvedTheme;

export function WebFrame({ page = "index.html", render, onMessage, height = 360 }:
  { page?: string; render: (w: any) => void; onMessage?: (m: any, w: any) => void; height?: number | string }) {
  const ref = useRef<HTMLIFrameElement>(null);
  const id = useRef(`f${++seq}`);
  const ready = useRef(false);
  const renderRef = useRef(render); renderRef.current = render;
  const msgRef = useRef(onMessage); msgRef.current = onMessage;
  useEffect(() => {
    const on = (e: MessageEvent) => {
      const w = ref.current?.contentWindow;
      if (!w || e.source !== w || e.data?.__dp !== id.current) return;
      const m = e.data.msg ?? {};
      if (m.type === "ready") {
        ready.current = true;
        try { (w as any).DP?.setPalette?.(useApp.getState().config?.settings?.dashboardPalette ?? "default"); (w as any).DP?.setLang?.(lang()); (w as any).DP?.setTheme?.(theme()); (w as any).DPER?.setLang?.(lang()); (w as any).DPER?.setTheme?.(theme()); } catch { /* page without these */ }
        renderRef.current(w);
        return;
      }
      msgRef.current?.(m, w);
    };
    window.addEventListener("message", on);
    const off = onTheme((t) => { const w: any = ref.current?.contentWindow; if (ready.current && w) { w.DP?.setTheme?.(t); w.DPER?.setTheme?.(t); } });
    return () => { window.removeEventListener("message", on); off(); };
  }, []);
  const palette = useApp((s) => JSON.stringify(s.config?.settings?.dashboardPalette ?? "default"));
  useEffect(() => { const w: any = ref.current?.contentWindow; if (ready.current && w?.DP?.setPalette) w.DP.setPalette(JSON.parse(palette)); }, [palette]);
  // payload changed after load → re-render in place
  useEffect(() => { const w = ref.current?.contentWindow; if (ready.current && w) render(w); }, [render]);
  return <iframe ref={ref} className="webframe" src={`/web/${page}#${id.current}`} style={{ height }} title="chart" />;
}
