// Native helpers: Tauri plugins when available, browser fallbacks in the dev bridge.
const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function pickFolder(): Promise<string | null> {
  if (isTauri) {
    // Windows: own dialog that lists files too (only folders can be chosen), like the macOS panel
    if (/Windows/i.test(navigator.userAgent)) {
      const { invoke } = await import("@tauri-apps/api/core");
      const { L } = await import("./i18n");
      const r = await invoke<string | null>("pick_folder_native", {
        title: L("选择文件夹", "Choose a folder"), ok: L("选择文件夹", "Select folder"), field: L("文件夹：", "Folder:"), start: null,
      }).then((v) => ({ v }), () => undefined);
      if (r) return r.v ?? null;   // null = cancelled; failure → standard dialog below
    }
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false });
    return typeof picked === "string" ? picked : null;
  }
  return window.prompt("Folder path") || null;
}

export async function pickFiles(): Promise<string[]> {
  if (isTauri) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const data = ["csv", "tsv", "xlsx", "xls", "parquet", "json", "jsonl", "txt"];
    const docs = ["pdf", "docx", "pptx", "md"];
    const images = ["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"];
    // first filter = everything supported (default); the others narrow it down
    const r = await open({ multiple: true, filters: [
      { name: "All supported / 全部支持的文件", extensions: [...data, ...docs, ...images] },
      { name: "Data / 数据", extensions: data },
      { name: "Documents / 文档 (PDF, Word, PPT)", extensions: docs },
      { name: "Images / 图片", extensions: images },
    ] });
    return Array.isArray(r) ? r : typeof r === "string" ? [r] : [];
  }
  const p = window.prompt("File path");
  return p ? [p] : [];
}

export async function reveal(path: string) {
  if (isTauri) {
    const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
    await revealItemInDir(path);
  }
}

/** Save dialog: path, null when cancelled, undefined outside Tauri (caller downloads instead). */
export async function saveDialog(defaultPath: string, ext: string): Promise<string | null | undefined> {
  if (!isTauri) return undefined;
  const { save } = await import("@tauri-apps/plugin-dialog");
  return (await save({ defaultPath, filters: [{ name: ext.toUpperCase(), extensions: [ext] }] })) ?? null;
}

/** Files dropped on the window (Tauri gives real paths). Returns an unsubscribe function. */
export async function onFileDrop(fn: (paths: string[]) => void, hover?: (on: boolean) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((e) => {
    const p: any = e.payload;
    if (p.type === "enter" || p.type === "over") hover?.(true);
    else if (p.type === "leave") hover?.(false);
    else if (p.type === "drop") { hover?.(false); if (p.paths?.length) fn(p.paths); }
  });
}

export async function openPath(path: string) {
  if (isTauri) { const { openPath } = await import("@tauri-apps/plugin-opener"); await openPath(path); }
}

export async function pickFile(defaultPath?: string): Promise<string | null> {
  if (isTauri) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const r = await open({ multiple: false, directory: false, defaultPath });
    return typeof r === "string" ? r : null;
  }
  return window.prompt("File path") || null;
}

/** Yes / no question: native dialog in Tauri (WKWebView ignores window.confirm), browser confirm otherwise. */
export async function ask(message: string): Promise<boolean> {
  if (isTauri) { const { ask } = await import("@tauri-apps/plugin-dialog"); return ask(message, { title: "DataPilot", kind: "warning" }); }
  return window.confirm(message);
}

export const inTauri = isTauri;

/** New API gateway: open its own login page in a separate window (all login methods: password, SSO / OIDC, GitHub…).
 *  Resolves with the dashboard auth (kept in memory only) or rejects with "cancelled" when the window is closed. */
export async function newapiWebLogin(origin: string, fresh = false): Promise<any> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("newapi_login", { origin, fresh });
}
export async function newapiLoginCancel() {
  if (!isTauri) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("newapi_login_cancel").catch(() => {});
}
export async function newapiLoginForce() {
  if (!isTauri) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("newapi_login_force").catch(() => {});
}

/** System notification when DataPilot is in the background (the shell checks focus). Off via Settings → General. */
export async function notifyBackground(title: string, body: string) {
  if (!isTauri || localStorage.getItem("dp.notify") === "off") return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("notify", { title, body: body.slice(0, 180) }).catch(() => {});
}
