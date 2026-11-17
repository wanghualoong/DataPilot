import { core } from "../../transport";
import { useApp } from "../../store";

export const rid = () => Math.random().toString(36).slice(2, 10);

/** Same merge as the core's config/set: objects recursively, `null` deletes, anything else replaces. */
export function mergePatch(dst: any, p: any): any {
  if (!p || typeof p !== "object" || Array.isArray(p) || !dst || typeof dst !== "object" || Array.isArray(dst)) return p;
  const out = { ...dst };
  for (const [k, v] of Object.entries(p)) {
    if (v === null) delete out[k];
    else if (v && typeof v === "object" && !Array.isArray(v) && out[k] && typeof out[k] === "object" && !Array.isArray(out[k])) out[k] = mergePatch(out[k], v);
    else out[k] = v;
  }
  return out;
}

/** Merge-patch config.json through the core (unknown keys kept; `null` deletes). Applied to the store copy first so
 *  controls update at once; on failure the store is reloaded from the file. */
export async function patch(p: object) {
  const st = useApp.getState();
  if (st.config) useApp.setState({ config: mergePatch(st.config, p) });
  try {
    await core.request("config/set", { patch: p });
    await st.loadConfig();
    return true;
  } catch (e) { st.fail(e); await st.loadConfig().catch(() => {}); return false; }
}

export { modelKind } from "../../models";
