import { useEffect, useState } from "react";
import { TriangleAlert, X } from "lucide-react";
import { useApp } from "./store";
import { Sidebar } from "./components/Sidebar";
import { Chat } from "./components/Chat";
import { Welcome } from "./components/Welcome";
import { Onboarding, QuickLogin } from "./components/Onboarding";
import { Inspector } from "./components/Inspector";
import { Resizer, useStored } from "./components/ui";
import { L } from "./i18n";
import { core } from "./transport";
import { ChangelogModal, type Release } from "./components/Changelog";
import { PageCompare } from "./components/PageCompare";
import { TrustFolder } from "./components/TrustFolder";

/** Something of DataPilot was stored here before this launch (read once, before any component writes): an upgrade, not a fresh install. */
const EXISTING_USER = typeof localStorage !== "undefined" && Object.keys(localStorage).some((k) => k.startsWith("dp.") && k !== "dp.seenVersion");

export function App() {
  const { init, ready, selected, error, notice, sidebarHidden, inspector, onboarding, quickLogin } = useApp();
  const [sideW, setSideW] = useStored("dp.sidebarWidth", 280);
  const [inspW, setInspW] = useStored("dp.inspectorWidth", 440);
  useEffect(() => { init(); }, [init]);
  // after an upgrade (version differs from the last one seen on this machine): show that version's changelog once
  const [whatsNew, setWhatsNew] = useState<{ version: string; releases: Release[] }>();
  useEffect(() => {
    if (!ready) return;
    core.request("app/about").then((a: any) => {
      const seen = localStorage.getItem("dp.seenVersion");
      if (seen === a.version) return;
      localStorage.setItem("dp.seenVersion", a.version);
      // a fresh install (nothing stored on this machine yet) has nothing to compare with: only upgrades show it
      if (!seen && !EXISTING_USER) return;
      if ((a.changelog ?? []).some((r: Release) => r.version === a.version)) setWhatsNew({ version: a.version, releases: a.changelog });
    }).catch(() => {});
  }, [ready]);
  // global shortcuts (same as the SwiftUI menu commands)
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (!mod) return;
      const st = useApp.getState();
      if (e.key === "n" && !e.shiftKey) { e.preventDefault(); st.newChat(); }
      else if (e.key === "." ) { e.preventDefault(); st.cancel(); }
      else if (e.key === ",") { e.preventDefault(); st.openSettings("providers"); }
      else if (e.key.toLowerCase() === "k" && e.shiftKey) { e.preventDefault(); st.compactNow(); }
      else if (e.altKey && (e.key.toLowerCase() === "i" || e.code === "KeyI")) { e.preventDefault(); st.openInspector(st.inspector ? undefined : "data"); }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, []);
  const showInspector = !!inspector && !!selected;
  // The chat column keeps ≥ 400px (SwiftUI: chat minWidth / inspector max = detail − 420): side panels give way first.
  const [winW, setWinW] = useState(window.innerWidth);
  useEffect(() => { const r = () => setWinW(window.innerWidth); window.addEventListener("resize", r); return () => window.removeEventListener("resize", r); }, []);
  const CHAT_MIN = 400;
  const insp = showInspector ? Math.max(300, Math.min(inspW, winW - (sidebarHidden ? 0 : 220) - CHAT_MIN)) : 0;
  const side = sidebarHidden ? 0 : Math.max(200, Math.min(sideW, winW - insp - CHAT_MIN));
  return (
    <div className="app">
      {!sidebarHidden && <>
        <Sidebar width={side} />
        <Resizer value={side} onChange={setSideW} dir={1} min={220} max={() => Math.min(480, window.innerWidth - insp - CHAT_MIN)} />
      </>}
      <main className="main">
        {error && <div className="banner error" onClick={() => useApp.setState({ error: undefined })}><TriangleAlert size={14} /> <span className="grow">{error}</span><X size={13} /></div>}
        {notice && <div className="banner" onClick={() => useApp.setState({ notice: undefined })}><span className="grow">{notice}</span><X size={13} /></div>}
        {!ready ? <div className="empty">{L("正在连接核心…", "Connecting to the core…")}</div> : selected ? <Chat id={selected} /> : <Welcome />}
      </main>
      {showInspector && <>
        <Resizer value={insp} onChange={setInspW} dir={-1} min={300} max={() => Math.min(900, window.innerWidth - side - CHAT_MIN)} />
        <Inspector width={insp} />
      </>}
      {onboarding && <Onboarding />}
      {quickLogin && <QuickLogin />}
      <PageCompare />
      <TrustFolder />
      {whatsNew && !onboarding && <ChangelogModal upgraded releases={whatsNew.releases} current={whatsNew.version} onClose={() => setWhatsNew(undefined)} />}
    </div>
  );
}
