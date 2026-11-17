// Light / dark UI: "system" (default, follows the OS live), "light" or "dark". Stored per machine like the UI language.
export type ThemePref = "system" | "light" | "dark";
export const THEME_KEY = "dp.theme";
const mq = typeof window !== "undefined" ? window.matchMedia?.("(prefers-color-scheme: dark)") : undefined;
const subs = new Set<(t: "light" | "dark") => void>();

export function themePref(): ThemePref {
  try { const v = localStorage.getItem(THEME_KEY); return v === "light" || v === "dark" ? v : "system"; } catch { return "system"; }
}
export const resolvedTheme = (): "light" | "dark" => {
  const p = themePref();
  return p === "system" ? (mq?.matches ? "dark" : "light") : p;
};

function apply() {
  const t = resolvedTheme();
  document.documentElement.dataset.theme = t;
  document.documentElement.style.colorScheme = t;
  // native title bar / scrollbars (Tauri); null = follow the OS
  if ("__TAURI_INTERNALS__" in window) {
    const p = themePref();
    import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().setTheme(p === "system" ? null : p)).catch(() => {});
  }
  subs.forEach((f) => f(t));
}

export function setThemePref(p: ThemePref) {
  try { if (p === "system") localStorage.removeItem(THEME_KEY); else localStorage.setItem(THEME_KEY, p); } catch { /* ignore */ }
  apply();
}
export function onTheme(f: (t: "light" | "dark") => void) { subs.add(f); return () => { subs.delete(f); }; }
export function initTheme() { apply(); mq?.addEventListener?.("change", () => { if (themePref() === "system") apply(); }); }
