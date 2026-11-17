// External links (system browser in Tauri) and assistant-text cleanup shared by chat cards.
import { inTauri } from "../native";

export async function openExternal(url: string) {
  if (inTauri) { const { openUrl } = await import("@tauri-apps/plugin-opener"); await openUrl(url); }
  else window.open(url, "_blank", "noopener,noreferrer");
}

/** Remove `[mem:file.md]` memory citations (same regex as Swift ProjectMemory.stripCitations). */
export const stripCitations = (t: string) => (t.includes("[mem:") ? t.replace(/\s?\[mem:[^\]\s]+\]/g, "") : t);
