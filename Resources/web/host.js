/* DataPilot web ↔ host bridge. One API for every shell, so web assets never touch platform globals:
 *   macOS WKWebView : window.webkit.messageHandlers.dp
 *   Windows WebView2: window.chrome.webview
 *   Tauri 2         : window.__TAURI__.event.emit('dp', …)
 *   iframe (Tauri / browser React host): window.parent.postMessage({__dp: frameId, msg}) — frameId from location.hash
 * Host → web stays plain function calls (DP.renderDashboard(...), DPER.render(...)) via evaluateJavaScript / ExecuteScriptAsync / eval.
 */
(function (g) {
  'use strict';
  function sink() {
    // embedded as an iframe (React host): parent first — WebView2 may expose chrome.webview inside iframes too
    try {
      if (g.parent && g.parent !== g) {
        const frame = decodeURIComponent((g.location.hash || '').slice(1)) || 'frame';
        return (m) => g.parent.postMessage({ __dp: frame, msg: m }, g.location.origin);
      }
    } catch (e) { /* none */ }
    try { if (g.webkit && g.webkit.messageHandlers && g.webkit.messageHandlers.dp) return (m) => g.webkit.messageHandlers.dp.postMessage(m); } catch (e) { /* none */ }
    try { if (g.chrome && g.chrome.webview && g.chrome.webview.postMessage) return (m) => g.chrome.webview.postMessage(m); } catch (e) { /* none */ }
    try { if (g.__TAURI__ && g.__TAURI__.event) return (m) => g.__TAURI__.event.emit('dp', m); } catch (e) { /* none */ }
    return null;
  }
  let seq = 0; const waits = {};
  const post = (m) => { const s = sink(); if (!s) return false; try { s(m); return true; } catch (e) { return false; } };
  g.DPHost = {
    /** True when running inside an app shell (false in exported standalone HTML / tests). */
    available: () => !!sink(),
    /** Send {type, ...} to the host; returns false when there is no host. */
    post,
    /** Ask the host to run a (whitelisted) kernel op: posts {type:'call', id, op, params}; the host answers with
     *  DPHost._resolve(id, result, error). Web components get data without any shell-specific code. */
    call: (op, params) => new Promise((resolve, reject) => {
      const id = 'c' + (++seq);
      waits[id] = { resolve, reject };
      if (!post({ type: 'call', id, op, params: params || {} })) { delete waits[id]; reject(new Error('no host')); }
    }),
    _resolve: (id, result, error) => {
      const w = waits[id]; if (!w) return; delete waits[id];
      if (error) w.reject(new Error(error)); else w.resolve(result);
    }
  };
})(typeof window !== 'undefined' ? window : globalThis);
