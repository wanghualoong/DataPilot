/* DataPilot ER diagram: tables as nodes, relations (relations.json) as edges, plus a "new / edit relation" form.
 * Pure SVG, no dependencies; talks to the host only through DPHost (host.js), so the same file runs in the macOS
 * WKWebView, a Windows WebView2 or a Tauri shell.
 *
 * Adapted from Chat2DB Community v5.3.8 (LicenseRef-Chat2DB), blocks/ERModal/utils:
 *   - node size = header + one row per column (calculateNodeSize; constants scaled down for the inspector),
 *   - stored layout = node positions + user-drawn "virtual" edges (IStoredLayout) → here: data/er_layout.json for
 *     positions, and user relations in relations.json (source "user") for virtual edges,
 *   - layered left→right layout, parent (pk) side first (ELK "layered", direction RIGHT) — re-implemented, no elkjs.
 *
 * Entry points: DPER.setLang(l), DPER.setTheme(t), DPER.load() (fetch via host), DPER.render({tables, relations, positions}).
 * Host ops used: tables, relations, er_layout, relation_test, relation_define. Events: relationsChanged, exportPNG.
 */
(function (g) {
  'use strict';
  const NODE_W = 230, HEADER_H = 30, ROW_H = 22, MAX_ROWS = 14, GAP_X = 120, GAP_Y = 30, PAD = 40;
  const STEPS = ['trim', 'upper', 'lower', 'strip_prefix', 'strip_zeros', 'num', 'digits', 'strip_spaces'];
  const T = {
    zh: { title: '表间关系图', add: '新建关系', layout: '自动布局', infer: '重新推断', png: '导出 PNG', fit: '适应窗口',
      confirmed: '已确认', inferred: '推断', suspect: '疑似', more: '还有 {n} 列', match: '匹配', rows: '行',
      from: '子表（外键侧）', to: '父表（被引用侧）', table: '表', column: '列', steps: '键转换（按顺序）', lpad: '左补零到位数',
      expr: '自定义表达式（用 {col} 表示列）', pairs: '对照值（每行一对：子表值 = 父表值）', note: '备注', test: '测试',
      save: '保存', force: '仍然保存', cancel: '取消', close: '关闭', confirm: '确认', reject: '否决', del: '删除', edit: '编辑',
      testing: '测试中…', saving: '保存中…', unmatched: '未匹配样例', samplesFrom: '子表样例', samplesTo: '父表样例',
      suggestion: '建议转换', apply: '采用', pairsOnSave: '使用对照值时，保存时会先验证匹配率。', checked: '检查',
      card: '基数', transform: '转换', join: 'JOIN 写法', noHost: '需要在 DataPilot 中打开', empty: '至少需要两张表',
      loading: '加载中…', dragHint: '拖动表头移动；从列右侧的圆点拖到另一张表的列可新建关系；点击连线查看详情',
      steps_label: { trim: '去首尾空格', upper: '转大写', lower: '转小写', strip_prefix: '去字母前缀', strip_zeros: '去前导零',
        num: '数值规范化(1.0→1)', digits: '只保留数字', strip_spaces: '去所有空格' } },
    en: { title: 'Relationship diagram', add: 'New relation', layout: 'Auto layout', infer: 'Re-infer', png: 'Export PNG', fit: 'Fit',
      confirmed: 'Confirmed', inferred: 'Inferred', suspect: 'Suspect', more: '{n} more columns', match: 'match', rows: 'rows',
      from: 'Child (foreign key)', to: 'Parent (referenced)', table: 'Table', column: 'Column', steps: 'Key transforms (in order)', lpad: 'Left-pad zeros to',
      expr: 'Custom expression ({col} = column)', pairs: 'Value pairs (one per line: child = parent)', note: 'Note', test: 'Test',
      save: 'Save', force: 'Save anyway', cancel: 'Cancel', close: 'Close', confirm: 'Confirm', reject: 'Reject', del: 'Delete', edit: 'Edit',
      testing: 'Testing…', saving: 'Saving…', unmatched: 'Unmatched samples', samplesFrom: 'Child samples', samplesTo: 'Parent samples',
      suggestion: 'Suggested transform', apply: 'Apply', pairsOnSave: 'With value pairs, the match rate is verified on save.', checked: 'checked',
      card: 'Cardinality', transform: 'Transform', join: 'JOIN', noHost: 'Open inside DataPilot', empty: 'Needs at least two tables',
      loading: 'Loading…', dragHint: 'Drag a header to move; drag from a column dot to another table\'s column to relate them; click an edge for details',
      steps_label: { trim: 'Trim', upper: 'Upper case', lower: 'Lower case', strip_prefix: 'Strip letter prefix', strip_zeros: 'Strip leading zeros',
        num: 'Normalize numbers (1.0→1)', digits: 'Digits only', strip_spaces: 'Remove spaces' } }
  };
  const S = { lang: 'zh', theme: 'light', tables: [], rels: [], pos: {}, view: { x: 0, y: 0, k: 1 }, root: null, svg: null, vp: null, fitted: false };
  const t = (k) => (T[S.lang] || T.zh)[k] || k;
  const NS = 'http://www.w3.org/2000/svg';
  const hasHost = () => typeof DPHost !== 'undefined' && DPHost.available();
  const call = (op, p) => (hasHost() ? DPHost.call(op, p) : Promise.reject(new Error(t('noHost'))));
  const post = (m) => (typeof DPHost !== 'undefined' ? DPHost.post(m) : false);

  // ---------- pure helpers (unit-tested) ----------
  /** Columns used by any relation endpoint of `table`. */
  function relCols(table, rels) {
    const s = new Set();
    rels.forEach((r) => {
      if (r.from_table === table) String(r.from_column || '').split(',').forEach((c) => s.add(c));
      if (r.to_table === table) String(r.to_column || '').split(',').forEach((c) => s.add(c));
    });
    return s;
  }
  /** Visible rows: relation columns always shown; long tables are cut at MAX_ROWS with a "+N" row. */
  function visibleColumns(tb, rels) {
    const cols = tb.columns || [];
    if (cols.length <= MAX_ROWS) return { cols, hidden: 0 };
    const rc = relCols(tb.table, rels);
    const keep = cols.filter((c) => rc.has(c.name));
    for (const c of cols) { if (keep.length >= MAX_ROWS - 1) break; if (!rc.has(c.name)) keep.push(c); }
    const order = new Map(cols.map((c, i) => [c.name, i]));
    keep.sort((a, b) => order.get(a.name) - order.get(b.name));
    return { cols: keep, hidden: cols.length - keep.length };
  }
  function nodeSize(tb, rels) {
    const v = visibleColumns(tb, rels);
    return { w: NODE_W, h: HEADER_H + (v.cols.length + (v.hidden ? 1 : 0)) * ROW_H + 6 };
  }
  /** Layered layout: parents (referenced tables) left, children right; unrelated tables in trailing columns. */
  function layout(tables, rels, stored) {
    const names = tables.map((x) => x.table), byName = new Map(tables.map((x) => [x.table, x]));
    const parents = new Map(names.map((n) => [n, new Set()]));
    const linked = new Set();
    rels.forEach((r) => {
      if (r.from_table === r.to_table || !byName.has(r.from_table) || !byName.has(r.to_table)) return;
      parents.get(r.from_table).add(r.to_table); linked.add(r.from_table); linked.add(r.to_table);
    });
    const layer = new Map(names.map((n) => [n, 0]));
    for (let i = 0; i < names.length; i++) {      // longest path, bounded (cycles stop growing at n)
      let changed = false;
      names.forEach((n) => parents.get(n).forEach((p) => {
        const L = layer.get(p) + 1;
        if (L > layer.get(n) && L < names.length) { layer.set(n, L); changed = true; }
      }));
      if (!changed) break;
    }
    const cols = [];
    names.filter((n) => linked.has(n)).forEach((n) => { const L = layer.get(n); (cols[L] = cols[L] || []).push(n); });
    const pos = {};
    let x = PAD, tallest = 0;
    const index = new Map();
    cols.forEach((col) => {
      if (!col) return;
      // order by the mean row of the parents already placed (fewer crossings), then by name
      const key = (n) => { const ps = [...parents.get(n)].filter((p) => index.has(p)); return ps.length ? ps.reduce((s, p) => s + index.get(p), 0) / ps.length : 1e9; };
      col.sort((a, b) => key(a) - key(b) || a.localeCompare(b));
      let y = PAD;
      col.forEach((n, i) => { pos[n] = [x, y]; index.set(n, i); y += nodeSize(byName.get(n), rels).h + GAP_Y; });
      tallest = Math.max(tallest, y); x += NODE_W + GAP_X;
    });
    const maxH = Math.max(tallest, 700);
    let y = PAD;
    names.filter((n) => !linked.has(n)).sort().forEach((n) => {
      const h = nodeSize(byName.get(n), rels).h;
      if (y > PAD && y + h > maxH) { x += NODE_W + GAP_X / 2; y = PAD; }
      pos[n] = [x, y]; y += h + GAP_Y;
    });
    Object.entries(stored || {}).forEach(([n, p]) => { if (byName.has(n) && Array.isArray(p) && p.length === 2) pos[n] = [+p[0], +p[1]]; });
    return pos;
  }
  /** y offset (from the node top) of a column's row centre; the header when the column is hidden. */
  function colOffset(tb, col, rels) {
    const v = visibleColumns(tb, rels);
    const i = v.cols.findIndex((c) => c.name === col);
    return i < 0 ? HEADER_H / 2 : HEADER_H + i * ROW_H + ROW_H / 2;
  }
  /** Bezier between a child column and a parent column, leaving from the facing sides. */
  function edgePath(a, b) {
    const right = a.x + NODE_W <= b.x ? 1 : (b.x + NODE_W <= a.x ? -1 : 0);
    const x1 = right >= 0 ? a.x + NODE_W : a.x, x2 = right > 0 ? b.x : (right < 0 ? b.x + NODE_W : b.x + NODE_W);
    const dx = right === 0 ? 60 : Math.max(40, Math.abs(x2 - x1) / 2);
    const c1 = x1 + (right >= 0 ? dx : -dx), c2 = x2 + (right > 0 ? -dx : dx);
    return { d: `M${x1},${a.y} C${c1},${a.y} ${c2},${b.y} ${x2},${b.y}`, mx: (x1 + x2) / 2, my: (a.y + b.y) / 2 };
  }
  /** Display width in "half-width" units (CJK = 2) and clipping. */
  function clip(s, units) {
    s = String(s == null ? '' : s); let w = 0;
    for (let i = 0; i < s.length; i++) { w += s.charCodeAt(i) > 0x2e80 ? 2 : 1; if (w > units) return s.slice(0, i) + '…'; }
    return s;
  }
  /** Form state → relation_test / relation_define side. */
  function sideFrom(f) {
    const side = { table: f.table, columns: [f.column] };
    const steps = (f.steps || []).slice();
    if (f.lpad) steps.push({ lpad: +f.lpad });
    if (steps.length) side.steps = steps;
    if (f.expr && f.expr.trim()) side.expr = f.expr.trim();
    return side;
  }
  function parsePairs(text) {
    return String(text || '').split(/\r?\n/).map((l) => l.split(/\s*(?:=|→|->|,|\t)\s*/)).filter((p) => p.length >= 2 && p[0] !== '' && p[1] !== '').map((p) => [p[0].trim(), p[1].trim()]);
  }

  // ---------- rendering ----------
  const cssv = (n, d) => { try { return getComputedStyle(document.documentElement).getPropertyValue(n).trim() || d; } catch (e) { return d; } };
  function colors() {
    const dark = S.theme === 'dark';
    return { surface: cssv('--surface', dark ? '#1a1a19' : '#fcfcfb'), page: cssv('--page', dark ? '#0d0d0d' : '#f9f9f7'),
      ink: cssv('--ink', dark ? '#f0efec' : '#0b0b0b'), ink2: cssv('--ink2', '#52514e'), muted: cssv('--muted', '#898781'),
      grid: cssv('--grid', dark ? '#2c2c2a' : '#e1e0d9'), accent: cssv('--accent', '#2a78d6'), head: dark ? '#252524' : '#efeee9',
      suspect: dark ? '#d95926' : '#eb6834', font: 'system-ui,-apple-system,"PingFang SC","Segoe UI",sans-serif' };
  }
  function el(tag, attrs, parent, text) {
    const e = document.createElementNS(NS, tag);
    Object.entries(attrs || {}).forEach(([k, v]) => { if (v != null) e.setAttribute(k, v); });
    if (text != null) e.textContent = text;
    if (parent) parent.appendChild(e);
    return e;
  }
  function h(tag, cls, parent, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    if (parent) parent.appendChild(e);
    return e;
  }
  const byName = () => new Map(S.tables.map((x) => [x.table, x]));
  function edgeStyle(r, C) {
    if (r.status === 'confirmed') return { stroke: C.accent, dash: null, w: 2 };
    if (r.status === 'suspect') return { stroke: C.suspect, dash: '6 4', w: 1.6 };
    return { stroke: C.muted, dash: '5 5', w: 1.4 };
  }

  function drawEdges() {
    const C = colors(), g0 = S.edgesG, m = byName();
    while (g0.firstChild) g0.removeChild(g0.firstChild);
    S.rels.forEach((r) => {
      const a = m.get(r.from_table), b = m.get(r.to_table);
      if (!a || !b || a === b || !S.pos[a.table] || !S.pos[b.table]) return;
      const fc = String(r.from_column || '').split(',')[0], tc = String(r.to_column || '').split(',')[0];
      const pa = { x: S.pos[a.table][0], y: S.pos[a.table][1] + colOffset(a, fc, S.rels) };
      const pb = { x: S.pos[b.table][0], y: S.pos[b.table][1] + colOffset(b, tc, S.rels) };
      const p = edgePath(pa, pb), st = edgeStyle(r, C);
      const grp = el('g', { 'data-rel': r.id, cursor: 'pointer' }, g0);
      el('title', {}, grp, `${r.from_table}.${r.from_column} → ${r.to_table}.${r.to_column}` + (r.transform ? `\n${t('transform')}: ${r.transform}` : ''));
      el('path', { d: p.d, fill: 'none', stroke: 'transparent', 'stroke-width': 12 }, grp);
      el('path', { d: p.d, fill: 'none', stroke: st.stroke, 'stroke-width': st.w, 'stroke-dasharray': st.dash, 'stroke-linecap': 'round' }, grp);
      el('circle', { cx: p.d.split(' ').slice(-1)[0].split(',')[0], cy: pb.y, r: 3.5, fill: st.stroke }, grp);
      const label = (r.match_rate != null ? Math.round(r.match_rate * 100) + '%' : '') + (r.transform ? ' ƒ' : '') + (r.cardinality ? ' ' + r.cardinality : '');
      if (label.trim()) {
        const tw = label.length * 6.4 + 10;
        el('rect', { x: p.mx - tw / 2, y: p.my - 9, width: tw, height: 18, rx: 9, fill: C.surface, stroke: st.stroke, 'stroke-width': 1 }, grp);
        el('text', { x: p.mx, y: p.my + 4, 'text-anchor': 'middle', 'font-size': 11, fill: C.ink2, 'font-family': C.font }, grp, label.trim());
      }
      grp.addEventListener('click', (ev) => { ev.stopPropagation(); showEdge(r); });
    });
  }

  function drawNode(tb) {
    const C = colors(), [x, y] = S.pos[tb.table], v = visibleColumns(tb, S.rels), sz = nodeSize(tb, S.rels);
    const rc = relCols(tb.table, S.rels);
    const grp = el('g', { 'data-table': tb.table, transform: `translate(${x},${y})` }, S.nodesG);
    el('rect', { width: sz.w, height: sz.h, rx: 7, fill: C.surface, stroke: C.grid, 'stroke-width': 1 }, grp);
    const head = el('path', { d: `M0,7 a7,7 0 0 1 7,-7 h${sz.w - 14} a7,7 0 0 1 7,7 v${HEADER_H - 7} h-${sz.w} z`, fill: C.head, cursor: 'move', 'data-drag': tb.table }, grp);
    el('title', {}, head, tb.table + (tb.rows != null ? ` · ${tb.rows} ${t('rows')}` : ''));
    el('text', { x: 10, y: 20, 'font-size': 13, 'font-weight': 600, fill: C.ink, 'font-family': C.font, 'pointer-events': 'none' }, grp, clip(tb.table, 22));
    if (tb.rows != null) el('text', { x: sz.w - 10, y: 20, 'font-size': 11, fill: C.muted, 'text-anchor': 'end', 'font-family': C.font, 'pointer-events': 'none' }, grp, Number(tb.rows).toLocaleString());
    v.cols.forEach((c, i) => {
      const cy = HEADER_H + i * ROW_H;
      const row = el('g', { 'data-col': c.name, 'data-coltable': tb.table }, grp);
      el('rect', { x: 0, y: cy, width: sz.w, height: ROW_H, fill: 'transparent' }, row);
      const key = rc.has(c.name);
      el('text', { x: 12, y: cy + 15, 'font-size': 12, fill: key ? C.accent : C.ink, 'font-weight': key ? 600 : 400, 'font-family': C.font, 'pointer-events': 'none' }, row, clip(c.name, 20));
      el('text', { x: sz.w - 16, y: cy + 15, 'font-size': 10.5, fill: C.muted, 'text-anchor': 'end', 'font-family': C.font, 'pointer-events': 'none' }, row, clip(String(c.type || '').toLowerCase(), 11));
      el('circle', { cx: sz.w, cy: cy + ROW_H / 2, r: 4.5, fill: C.surface, stroke: key ? C.accent : C.muted, 'stroke-width': 1.2, cursor: 'crosshair', 'data-port': '1', 'data-col': c.name, 'data-coltable': tb.table }, grp);
    });
    if (v.hidden) el('text', { x: 12, y: HEADER_H + v.cols.length * ROW_H + 15, 'font-size': 11, fill: C.muted, 'font-style': 'italic', 'font-family': C.font }, grp, t('more').replace('{n}', v.hidden));
    return grp;
  }

  function applyView() { if (S.vp) S.vp.setAttribute('transform', `translate(${S.view.x},${S.view.y}) scale(${S.view.k})`); }
  function draw() {
    const C = colors();
    while (S.vp.firstChild) S.vp.removeChild(S.vp.firstChild);
    S.svg.style.background = C.page;
    S.edgesG = el('g', {}, S.vp); S.nodesG = el('g', {}, S.vp); S.tempG = el('g', {}, S.vp);
    S.tables.forEach(drawNode);
    drawEdges(); applyView();
  }
  function bbox() {
    const m = byName(); let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    S.tables.forEach((tb) => { const p = S.pos[tb.table]; if (!p) return; const sz = nodeSize(tb, S.rels);
      x0 = Math.min(x0, p[0]); y0 = Math.min(y0, p[1]); x1 = Math.max(x1, p[0] + sz.w); y1 = Math.max(y1, p[1] + sz.h); });
    void m; return isFinite(x0) ? { x0: x0 - 20, y0: y0 - 20, x1: x1 + 20, y1: y1 + 20 } : { x0: 0, y0: 0, x1: 100, y1: 100 };
  }
  function fit() {
    const b = bbox(), r = S.svg.getBoundingClientRect(), W = r.width || 900, H = r.height || 600;
    const k = Math.min(1.2, Math.max(0.2, Math.min(W / (b.x1 - b.x0), H / (b.y1 - b.y0))));
    S.view = { k, x: (W - (b.x1 - b.x0) * k) / 2 - b.x0 * k, y: Math.max(10, (H - (b.y1 - b.y0) * k) / 2) - b.y0 * k };
    applyView();
  }
  const toWorld = (ev) => { const r = S.svg.getBoundingClientRect(); return { x: (ev.clientX - r.left - S.view.x) / S.view.k, y: (ev.clientY - r.top - S.view.y) / S.view.k }; };

  function bindCanvas() {
    let drag = null;
    S.svg.addEventListener('pointerdown', (ev) => {
      const tg = ev.target;
      if (tg.dataset && tg.dataset.port) {
        const from = { table: tg.dataset.coltable, column: tg.dataset.col }, w = toWorld(ev);
        const line = el('line', { x1: w.x, y1: w.y, x2: w.x, y2: w.y, stroke: colors().accent, 'stroke-width': 2, 'stroke-dasharray': '4 3' }, S.tempG);
        drag = { kind: 'link', from, line };
      } else if (tg.dataset && tg.dataset.drag) {
        const n = tg.dataset.drag, w = toWorld(ev);
        drag = { kind: 'node', n, dx: w.x - S.pos[n][0], dy: w.y - S.pos[n][1], g: S.nodesG.querySelector(`[data-table="${CSS.escape(n)}"]`) };
      } else {
        drag = { kind: 'pan', x: ev.clientX - S.view.x, y: ev.clientY - S.view.y };
      }
      S.svg.setPointerCapture(ev.pointerId);
    });
    S.svg.addEventListener('pointermove', (ev) => {
      if (!drag) return;
      if (drag.kind === 'pan') { S.view.x = ev.clientX - drag.x; S.view.y = ev.clientY - drag.y; applyView(); return; }
      const w = toWorld(ev);
      if (drag.kind === 'node') {
        S.pos[drag.n] = [Math.round(w.x - drag.dx), Math.round(w.y - drag.dy)]; drag.moved = true;
        if (drag.g) drag.g.setAttribute('transform', `translate(${S.pos[drag.n][0]},${S.pos[drag.n][1]})`);
        drawEdges();
      } else if (drag.kind === 'link') { drag.line.setAttribute('x2', w.x); drag.line.setAttribute('y2', w.y); }
    });
    S.svg.addEventListener('pointerup', (ev) => {
      const d = drag; drag = null;
      if (!d) return;
      if (d.kind === 'node' && d.moved) savePositions();
      if (d.kind === 'link') {
        d.line.remove();
        const hit = document.elementFromPoint(ev.clientX, ev.clientY);
        const node = hit && hit.closest ? hit.closest('[data-coltable]') : null;
        const tgt = node || hit;
        if (tgt && tgt.dataset && tgt.dataset.coltable && tgt.dataset.coltable !== d.from.table) {
          openForm({ from: { table: d.from.table, column: d.from.column }, to: { table: tgt.dataset.coltable, column: tgt.dataset.col } });
        }
      }
    });
    S.svg.addEventListener('wheel', (ev) => {
      ev.preventDefault();
      const r = S.svg.getBoundingClientRect(), mx = ev.clientX - r.left, my = ev.clientY - r.top;
      const k = Math.min(2.5, Math.max(0.15, S.view.k * Math.exp(-ev.deltaY * 0.0015)));
      S.view.x = mx - (mx - S.view.x) * (k / S.view.k); S.view.y = my - (my - S.view.y) * (k / S.view.k); S.view.k = k; applyView();
    }, { passive: false });
  }

  let saveTimer = null;
  function savePositions() {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => { if (hasHost()) call('er_layout', { positions: S.pos }).catch(() => {}); }, 300);
  }

  // ---------- panel: edge details & relation form ----------
  function panel(title) {
    S.panel.innerHTML = ''; S.panel.classList.add('open');
    const hd = h('div', 'er-panel-head', S.panel);
    h('div', 'er-panel-title', hd, title);
    const x = h('button', 'dp-btn', hd, '×'); x.onclick = closePanel;
    return h('div', 'er-panel-body', S.panel);
  }
  function closePanel() { S.panel.classList.remove('open'); S.panel.innerHTML = ''; }
  function kv(parent, k, v, mono) { if (v == null || v === '') return; const r = h('div', 'er-kv', parent); h('span', 'er-k', r, k); h(mono ? 'code' : 'span', 'er-v', r, String(v)); }
  function statusLabel(s) { return t(s === 'confirmed' ? 'confirmed' : s === 'suspect' ? 'suspect' : 'inferred'); }

  function showEdge(r) {
    const b = panel(`${r.from_table}.${r.from_column} → ${r.to_table}.${r.to_column}`);
    kv(b, '', statusLabel(r.status));
    kv(b, t('match'), r.match_rate != null ? Math.round(r.match_rate * 100) + '%' : null);
    kv(b, t('card'), r.cardinality);
    kv(b, t('transform'), r.transform);
    if (r.unmatched_samples && r.unmatched_samples.length) kv(b, t('unmatched'), r.unmatched_samples.slice(0, 6).join(', '), true);
    kv(b, t('join'), r.join_sql, true);
    kv(b, t('note'), r.note);
    const bar = h('div', 'er-actions', b);
    const act = (label, action, cls) => { const bt = h('button', 'dp-btn' + (cls ? ' ' + cls : ''), bar, label); bt.onclick = () => relAction(r.id, action); };
    if (r.status !== 'confirmed') act(t('confirm'), 'confirm', 'primary');
    act(t('reject'), 'reject');
    if (r.source && r.source !== 'auto') act(t('del'), 'delete', 'danger');
    const ed = h('button', 'dp-btn', bar, t('edit'));
    ed.onclick = () => openForm({
      from: Object.assign({ table: r.from_table, column: String(r.from_column).split(',')[0] }, stepsToForm((r.from || {}))),
      to: Object.assign({ table: r.to_table, column: String(r.to_column).split(',')[0] }, stepsToForm((r.to || {}))), note: r.note });
  }
  function stepsToForm(side) {
    const out = { steps: [], lpad: '', expr: side.expr || '' };
    (side.steps || []).forEach((s) => { if (typeof s === 'string') out.steps.push(s); else if (s && s.lpad) out.lpad = s.lpad; });
    return out;
  }
  function relAction(id, action) {
    call('relation_define', { id, action }).then(reloadRelations).then(closePanel).catch((e) => alert(e.message));
  }

  function openForm(init) {
    init = init || {};
    const b = panel(t('add'));
    const F = { from: Object.assign({ steps: [], lpad: '', expr: '' }, init.from || {}), to: Object.assign({ steps: [], lpad: '', expr: '' }, init.to || {}) };
    if (!F.from.table) F.from.table = (S.tables[0] || {}).table;
    if (!F.to.table) F.to.table = (S.tables[1] || S.tables[0] || {}).table;
    const sideUI = (key, label) => {
      const f = F[key], box = h('fieldset', 'er-side', b);
      h('legend', '', box, label);
      const r1 = h('div', 'er-row', box);
      const ts = h('select', '', r1), cs = h('select', '', r1);
      S.tables.forEach((x) => { const o = h('option', '', ts, x.table); o.value = x.table; if (x.table === f.table) o.selected = true; });
      const fillCols = () => {
        cs.innerHTML = '';
        const tb = S.tables.find((x) => x.table === f.table) || { columns: [] };
        tb.columns.forEach((c) => { const o = h('option', '', cs, c.name); o.value = c.name; });
        if (!tb.columns.some((c) => c.name === f.column)) f.column = (tb.columns[0] || {}).name;
        cs.value = f.column;
      };
      ts.onchange = () => { f.table = ts.value; fillCols(); };
      cs.onchange = () => { f.column = cs.value; };
      fillCols();
      h('div', 'er-sub', box, t('steps'));
      const chips = h('div', 'er-steps', box);
      STEPS.forEach((s) => {
        const lb = h('label', 'er-step', chips), cb = h('input', '', lb); cb.type = 'checkbox'; cb.checked = f.steps.includes(s);
        h('span', '', lb, t('steps_label')[s] || s);
        cb.onchange = () => { f.steps = cb.checked ? f.steps.concat([s]) : f.steps.filter((x) => x !== s); };
        f['_cb_' + s] = cb;
      });
      const r2 = h('div', 'er-row', box);
      h('span', 'er-sub', r2, t('lpad'));
      const lp = h('input', 'er-num', r2); lp.type = 'number'; lp.min = 1; lp.max = 32; lp.value = f.lpad || ''; lp.oninput = () => { f.lpad = lp.value; };
      const ex = h('input', 'er-text', box); ex.placeholder = t('expr'); ex.value = f.expr || ''; ex.oninput = () => { f.expr = ex.value; };
    };
    sideUI('from', t('from')); sideUI('to', t('to'));
    const pairs = h('textarea', 'er-text', b); pairs.rows = 3; pairs.placeholder = t('pairs');
    const note = h('input', 'er-text', b); note.placeholder = t('note'); note.value = init.note || '';
    const out = h('div', 'er-result', b);
    const bar = h('div', 'er-actions', b);
    const bt = h('button', 'dp-btn', bar, t('test')), bs = h('button', 'dp-btn primary', bar, t('save')), bc = h('button', 'dp-btn', bar, t('cancel'));
    bc.onclick = closePanel;
    const sides = () => ({ from: sideFrom(F.from), to: sideFrom(F.to) });
    const showResult = (res, failed) => {
      out.innerHTML = '';
      if (failed) h('div', 'er-warn', out, res.reason || res.error || '');
      if (res.match_rate != null) {
        const pct = Math.round(res.match_rate * 100);
        const big = h('div', 'er-rate ' + (pct >= 90 ? 'good' : pct >= 30 ? 'mid' : 'bad'), out, `${t('match')} ${pct}%`);
        void big;
        kv(out, t('checked'), res.rows_checked != null ? `${res.rows_checked} ${t('rows')}` : null);
        kv(out, t('card'), res.cardinality);
        if (res.unmatched_samples && res.unmatched_samples.length) kv(out, t('unmatched'), res.unmatched_samples.join(', '), true);
        if (res.from_samples) kv(out, t('samplesFrom'), res.from_samples.join(', '), true);
        if (res.to_samples) kv(out, t('samplesTo'), res.to_samples.join(', '), true);
      }
      if (res.suggestion) {
        const sg = h('div', 'er-suggest', out);
        h('span', '', sg, `${t('suggestion')}: ${res.suggestion.label} → ${Math.round(res.suggestion.match_rate * 100)}%`);
        const ap = h('button', 'dp-btn', sg, t('apply'));
        ap.onclick = () => {
          ['from', 'to'].forEach((k) => {
            F[k].steps = (res.suggestion.steps || []).filter((s) => typeof s === 'string');
            STEPS.forEach((s) => { if (F[k]['_cb_' + s]) F[k]['_cb_' + s].checked = F[k].steps.includes(s); });
          });
          bt.onclick();
        };
      }
    };
    bt.onclick = () => {
      if (parsePairs(pairs.value).length) { out.textContent = t('pairsOnSave'); return; }
      out.textContent = t('testing');
      call('relation_test', sides()).then((res) => showResult(res, false)).catch((e) => { out.textContent = e.message; });
    };
    const save = (force) => {
      const p = Object.assign(sides(), { note: note.value, source: 'user' });
      const pp = parsePairs(pairs.value); if (pp.length) p.mapping_pairs = pp;
      if (force) p.force = true;
      out.textContent = t('saving');
      call('relation_define', p).then((res) => {
        if (res.ok) { reloadRelations().then(closePanel); return; }
        showResult(res, true);
        const f2 = h('button', 'dp-btn danger', out, t('force')); f2.onclick = () => save(true);
      }).catch((e) => { out.textContent = e.message; });
    };
    bs.onclick = () => save(false);
  }

  // ---------- data ----------
  function reloadRelations(refresh) {
    return call('relations', refresh ? { refresh: true } : {}).then((r) => {
      S.rels = r.relations || []; draw(); post({ type: 'relationsChanged' });
    });
  }
  function render(data) {
    S.tables = data.tables || []; S.rels = data.relations || [];
    S.pos = layout(S.tables, S.rels, data.positions || {});
    if (!S.root) mount();
    S.empty.style.display = S.tables.length < 2 ? 'block' : 'none';
    draw();
    if (!S.fitted || !data.keepView) { requestAnimationFrame(fit); S.fitted = true; }
  }
  function load() {
    if (!S.root) mount();
    S.empty.textContent = t('loading'); S.empty.style.display = 'block';
    return Promise.all([call('tables'), call('relations'), call('er_layout')]).then(([a, b, c]) => {
      S.empty.textContent = t('empty');
      render({ tables: a.tables || [], relations: b.relations || [], positions: (c && c.positions) || {} });
    }).catch((e) => { S.empty.textContent = e.message; });
  }

  function exportPNG() {
    const b = bbox(), C = colors();
    const clone = S.svg.cloneNode(true);
    const vp = clone.querySelector('g'); vp.setAttribute('transform', `translate(${-b.x0},${-b.y0})`);
    const W = Math.ceil(b.x1 - b.x0), H = Math.ceil(b.y1 - b.y0);
    clone.setAttribute('width', W); clone.setAttribute('height', H); clone.setAttribute('xmlns', NS);
    clone.removeAttribute('style');
    const bg = document.createElementNS(NS, 'rect'); bg.setAttribute('width', W); bg.setAttribute('height', H); bg.setAttribute('fill', C.page);
    clone.insertBefore(bg, clone.firstChild);
    const src = new XMLSerializer().serializeToString(clone);
    const img = new Image();
    img.onload = () => {
      const cv = document.createElement('canvas'), k = 2; cv.width = W * k; cv.height = H * k;
      const cx = cv.getContext('2d'); cx.scale(k, k); cx.drawImage(img, 0, 0);
      const url = cv.toDataURL('image/png');
      if (!post({ type: 'exportPNG', name: 'ER', data: url })) { const a = document.createElement('a'); a.href = url; a.download = 'ER.png'; a.click(); }
    };
    img.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(src);
  }

  function mount() {
    const root = document.getElementById('root') || document.body;
    root.innerHTML = '';
    S.root = h('div', 'er-root', root);
    const bar = h('div', 'er-toolbar', S.root);
    h('div', 'er-title', bar, t('title'));
    const legend = h('div', 'er-legend', bar);
    [['confirmed', 'solid'], ['inferred', 'dash'], ['suspect', 'warn']].forEach(([k, c]) => { const s = h('span', 'er-lg ' + c, legend); s.textContent = t(k); });
    const btn = (label, fn, cls) => { const b = h('button', 'dp-btn' + (cls ? ' ' + cls : ''), bar, label); b.onclick = fn; return b; };
    btn(t('add'), () => openForm({}), 'primary');
    btn(t('layout'), () => { S.pos = layout(S.tables, S.rels, {}); draw(); fit(); if (hasHost()) call('er_layout', { positions: {} }).catch(() => {}); });
    btn(t('infer'), () => reloadRelations(true).catch((e) => alert(e.message)));
    btn(t('fit'), fit);
    btn(t('png'), exportPNG);
    const wrap = h('div', 'er-canvas', S.root);
    S.svg = el('svg', { width: '100%', height: '100%' }, wrap);
    S.vp = el('g', {}, S.svg);
    S.empty = h('div', 'er-empty', wrap, t('empty'));
    h('div', 'er-hint', wrap, t('dragHint'));
    S.panel = h('div', 'er-panel', S.root);
    bindCanvas();
    addEventListener('resize', () => applyView());
  }

  function setLang(l) { S.lang = l && String(l).startsWith('en') ? 'en' : 'zh'; }
  function setTheme(th) { S.theme = th === 'dark' ? 'dark' : 'light'; document.documentElement.dataset.theme = S.theme; if (S.root) draw(); }

  g.DPER = { load, render, setLang, setTheme, exportPNG, _layout: layout, _nodeSize: nodeSize, _visibleColumns: visibleColumns,
    _edgePath: edgePath, _clip: clip, _sideFrom: sideFrom, _parsePairs: parsePairs, _colOffset: colOffset };
  if (typeof document !== 'undefined' && typeof DPHost !== 'undefined') DPHost.post({ type: 'ready' });
})(typeof window !== 'undefined' ? window : globalThis);
if (typeof module !== 'undefined') module.exports = (typeof window !== 'undefined' ? window : globalThis).DPER;
