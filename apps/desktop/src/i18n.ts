// Bilingual strings, same convention as the SwiftUI app: L(zh, en).
// Settings → General → 界面语言 stores "system" | "zh-Hans" | "en" (applied on reload, like the SwiftUI app's restart).
export const UI_LANG_KEY = "dp.uiLanguage";
const pref = (() => { try { return (typeof localStorage !== "undefined" && typeof localStorage.getItem === "function" && localStorage.getItem(UI_LANG_KEY)) || "system"; } catch { return "system"; } })();
const zh = pref === "system" ? (typeof navigator !== "undefined" ? navigator.language : "zh").toLowerCase().startsWith("zh") : pref === "zh-Hans";
export const uiLanguage = () => pref;
export const L = (z: string, e: string) => (zh ? z : e);
