import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, Download, LoaderCircle, RotateCcw, RotateCw, ScanText, TriangleAlert, X } from "lucide-react";
import { reveal } from "../native";
import { L } from "../i18n";
import { useApp } from "../store";
import { core } from "../transport";
import { Markdown } from "./Markdown";
import { IconBtn, ic } from "./ui";

const base = (p: string) => p.split(/[\\/]/).pop() ?? p;

/** Image data URL turned by `deg` (multiple of 90) via a canvas, so the rotated page keeps a normal layout. */
export function rotateDataUrl(src: string, deg: number): Promise<string> {
  const d = ((deg % 360) + 360) % 360;
  if (!d) return Promise.resolve(src);
  return new Promise((ok, fail) => {
    const img = new Image();
    img.onload = () => {
      const side = d === 90 || d === 270;
      const c = document.createElement("canvas");
      c.width = side ? img.height : img.width;
      c.height = side ? img.width : img.height;
      const g = c.getContext("2d");
      if (!g) return ok(src);
      g.translate(c.width / 2, c.height / 2);
      g.rotate((d * Math.PI) / 180);
      g.drawImage(img, -img.width / 2, -img.height / 2);
      ok(c.toDataURL("image/png"));
    };
    img.onerror = () => fail(new Error("image"));
    img.src = src;
  });
}

/** Original page next to the recognized text (scanned PDFs / images): `docparse/page`. */
export function PageCompare() {
  const view = useApp((s) => s.pageView);
  const selected = useApp((s) => s.selected);
  const [page, setPage] = useState(1);
  const [r, setR] = useState<any>();
  const [err, setErr] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState(false);
  // rotation per page (degrees), for pages scanned sideways / upside down; kept while the view is open
  const [rot, setRot] = useState<Record<number, number>>({});
  const [shown, setShown] = useState<string>();
  useEffect(() => { if (view) setPage(view.page ?? 1); }, [view?.path, view?.page]);
  useEffect(() => { setRot({}); }, [view?.path]);
  const deg = rot[page] ?? 0;
  useEffect(() => {
    if (!r?.image) { setShown(undefined); return; }
    let live = true;
    rotateDataUrl(r.image, deg).then((u) => { if (live) setShown(u); }).catch(() => { if (live) setShown(r.image); });
    return () => { live = false; };
  }, [r?.image, deg]);
  const turn = (by: number) => setRot((m) => ({ ...m, [page]: ((((m[page] ?? 0) + by) % 360) + 360) % 360 }));
  useEffect(() => {
    if (!view || !selected) return;
    let live = true;
    setBusy(true); setErr(undefined);
    core.request("docparse/page", { session: selected, path: view.path, page })
      .then((x: any) => { if (live) setR(x); })
      .catch((e: any) => { if (live) setErr(e?.message ?? String(e)); })
      .finally(() => { if (live) setBusy(false); });
    return () => { live = false; };
  }, [view?.path, page, selected]);
  if (!view) return null;
  const close = () => useApp.setState({ pageView: undefined });
  const download = async () => {
    setSaving(true);
    try {
      const x: any = await core.request("docparse/export", { session: selected, path: view.path, format: "docx" });
      useApp.getState().note(L("识别结果已导出：", "OCR result exported: ") + x.path);
      reveal(x.path);
    } catch (e) { useApp.getState().fail(e); }
    setSaving(false);
  };
  const total: number = r?.total ?? 1;
  const scanned: number[] = r?.pages ?? [];
  const review: string[] = r?.review ?? [];
  // the merged text ends with the review list; it is shown separately (highlighted), so cut it from the body
  const body = String(r?.text ?? "").split("\n\n> ⚠ 需核对：")[0];
  return (
    <div className="modal" onClick={close}>
      <div className="modalbox pagecmp" onClick={(e) => e.stopPropagation()}>
        <div className="row">
          <ScanText {...ic} size={16} className="muted" />
          <h3 className="grow ell" title={view.path}>{L("原页对照：", "Original vs. recognized: ") + base(view.path)}</h3>
          {total > 1 && (
            <span className="row pager">
              <button className="pgbtn" title={L("上一页", "Previous page")} disabled={page <= 1 || busy} onClick={() => setPage(page - 1)}><ChevronLeft size={18} />{L("上一页", "Prev")}</button>
              <select value={page} disabled={busy} onChange={(e) => setPage(Number(e.target.value))}>
                {Array.from({ length: total }, (_, i) => i + 1).map((n) => (
                  <option key={n} value={n}>{L(`第 ${n} 页`, `Page ${n}`) + (scanned.includes(n) ? L("（扫描）", " (scan)") : "")}</option>
                ))}
              </select>
              <span className="muted">/ {total}</span>
              <button className="pgbtn" title={L("下一页", "Next page")} disabled={page >= total || busy} onClick={() => setPage(page + 1)}>{L("下一页", "Next")}<ChevronRight size={18} /></button>
            </span>
          )}
          <button className="pgbtn primary" disabled={saving || !scanned.length} title={L("把全部页面的识别结果（含需核对项）导出为 Word", "Export the OCR text of all pages (with review items) as Word")} onClick={download}>
            {saving ? <LoaderCircle size={16} className="spin" /> : <Download size={16} />}{L("下载识别结果（docx）", "Download OCR (docx)")}
          </button>
          <IconBtn icon={X} title={L("关闭", "Close")} onClick={close} />
        </div>
        {r?.info && <div className="muted small">{r.info}</div>}
        {err && <div className="err small">⚠️ {err}</div>}
        <div className="pcbody">
          <div className="pcimg">
            {r?.image && (
              <div className="pcrot row">
                <button className="pgbtn" title={L("向左旋转 90°", "Rotate left 90°")} onClick={() => turn(-90)}><RotateCcw size={16} />{L("左转", "Left")}</button>
                <button className="pgbtn" title={L("向右旋转 90°", "Rotate right 90°")} onClick={() => turn(90)}><RotateCw size={16} />{L("右转", "Right")}</button>
                {deg !== 0 && <><span className="muted small">{deg}°</span><button className="pgbtn" onClick={() => setRot((m) => ({ ...m, [page]: 0 }))}>{L("还原", "Reset")}</button></>}
              </div>
            )}
            {busy && !r ? <div className="muted small pad"><LoaderCircle {...ic} size={14} className="spin" /> {L("正在加载（首次需要识别，可能要几十秒）…", "Loading (first time runs recognition, may take a while)…")}</div>
              : shown ? <img src={shown} alt={`page ${page}`} /> : null}
          </div>
          <div className="pctext">
            {busy && <div className="muted small"><LoaderCircle {...ic} size={13} className="spin" /> {L("加载中…", "Loading…")}</div>}
            {review.length > 0 && (
              <div className="pcreview">
                <div className="row small"><TriangleAlert {...ic} size={14} className="warn" /><b>{L(`需核对 ${review.length} 处（请对照左侧原页）`, `${review.length} item(s) to check against the page`)}</b></div>
                <ul>{review.map((x, i) => <li key={i}>{x}</li>)}</ul>
              </div>
            )}
            {r && !r.scanned && !busy && <div className="muted small">{L("这一页有文字层，不需要识别（读取时直接使用原文字）。", "This page has a text layer; no recognition needed.")}</div>}
            {r?.scanned && <Markdown text={body || L("（本页没有识别出文字）", "(no text recognized)")} />}
          </div>
        </div>
        <div className="muted xs">{L("右侧是文档识别（OCR）结果；印章、手写和被印章遮挡的文字可能不准，金额、日期、编号请以原件为准。", "Right: OCR result. Seals, handwriting and text under seals may be wrong; check amounts, dates and numbers against the original.")}</div>
      </div>
    </div>
  );
}

/** Tool-card line for scanned pages: which pages were recognized, how many items need checking, button to compare. */
export function OcrInfo({ o }: { o: any }) {
  const reviewPages = Object.entries(o.review ?? {}).filter(([, v]) => Array.isArray(v) && (v as any[]).length);
  const n = reviewPages.reduce((a, [, v]) => a + (v as any[]).length, 0);
  const first = reviewPages.length ? Number(reviewPages[0][0]) : (o.pages ?? [])[0];
  const open = () => useApp.setState({ pageView: { path: o.path, page: first || 1 } });
  if (o.unavailable) return <div className="ocrinfo warnbox small"><TriangleAlert {...ic} size={13} className="warn" /><span className="grow">{o.unavailable}</span></div>;
  return (
    <div className="ocrinfo small">
      <ScanText {...ic} size={13} className="muted" />
      <span className="grow">
        {L(`扫描件第 ${rangeStr(o.pages ?? [])} 页已识别（${o.primary_tier}${o.check_tier ? ` + ${o.check_tier} 核对` : ""}）`,
          `Scanned page(s) ${rangeStr(o.pages ?? [])} recognized (${o.primary_tier}${o.check_tier ? ` + ${o.check_tier} check` : ""})`)}
        {n > 0 && <span className="warn">{L(` · ${n} 处需核对`, ` · ${n} to check`)}</span>}
        {o.check_error && <span className="warn">{L(" · 核对档失败", " · check tier failed")}</span>}
      </span>
      <button className="small" onClick={open}>{L("原页对照", "Compare with page")}</button>
    </div>
  );
}

export function rangeStr(nums: number[]): string {
  const n = [...new Set(nums)].sort((a, b) => a - b);
  const out: string[] = [];
  for (let i = 0; i < n.length; ) {
    let j = i;
    while (j + 1 < n.length && n[j + 1] === n[j] + 1) j++;
    out.push(i === j ? String(n[i]) : `${n[i]}-${n[j]}`);
    i = j + 1;
  }
  return out.join(",");
}
