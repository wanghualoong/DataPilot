/* DataPilot dashboard renderer: semantic DashboardSpec + datasets -> ECharts.
 * Entry points (called from Swift via evaluateJavaScript, or inline in exported HTML):
 *   DP.renderDashboard(spec, data, opts)   data = {datasetId: {columns, rows}}
 *   DP.renderChart(chartPayload, opts)     chartPayload = {spec:{type,title,x,y,series,options}, data:{columns,rows}}
 *   DP.setTheme('light'|'dark')
 * Spec extensions (safe expressions via DPExpr, expr.js): calcFields (row level, before filters), measures
 * (aggregate after grouping, e.g. sum(毛利)/sum(销售额)), params (sliders / selects referenced in expressions).
 * Events to host: DPHost.post({type, ...}) (host.js — WKWebView / WebView2 / Tauri)
 */
(function () {
  'use strict';
  const PAL = {
    light: ['#2a78d6', '#eb6834', '#1baf7a', '#eda100', '#e87ba4', '#008300', '#6250d6', '#e34948'],
    dark: ['#3987e5', '#d95926', '#199e70', '#c98500', '#d55181', '#008300', '#9085e9', '#e66767']
  };
  const SEQ = ['#cde2fb', '#9ec5f4', '#6da7ec', '#3987e5', '#256abf', '#184f95', '#0d366b'];
  /** Selectable categorical palettes (app setting `dashboardPalette`, or `spec.palette` per dashboard; a hex array = custom). */
  const PALETTES = {
    default: PAL,
    okabe: { light: ['#0072b2', '#e69f00', '#009e73', '#cc79a7', '#56b4e9', '#d55e00', '#f0e442', '#999999'],
             dark: ['#3a9ad9', '#e69f00', '#14b088', '#d98cb8', '#6cc4f0', '#e2702a', '#e6dc5a', '#a8a8a8'] },
    tableau: { light: ['#4e79a7', '#f28e2b', '#e15759', '#76b7b2', '#59a14f', '#edc948', '#b07aa1', '#ff9da7', '#9c755f', '#bab0ac'],
               dark: ['#6b97c6', '#f5a04b', '#ea7375', '#8fcbc6', '#74b96a', '#efd36a', '#c495b6', '#ffb3bb', '#b58f78', '#c9c0bb'] },
    office: { light: ['#4472c4', '#ed7d31', '#a5a5a5', '#ffc000', '#5b9bd5', '#70ad47', '#264478', '#9e480e'],
              dark: ['#6a92d8', '#f0954f', '#b8b8b8', '#ffcd33', '#7cb3e0', '#8cc266', '#5878b8', '#c96a2e'] },
    calm: { light: ['#3b6e8f', '#c8875e', '#6a9a7b', '#a26f9b', '#d1b05c', '#5d8fb8', '#b5605f', '#7d7d7d'],
            dark: ['#5f93b5', '#d9a07c', '#86b596', '#bd8ab6', '#dcc07a', '#7eaad0', '#cc7c7b', '#9a9a9a'] },
    echarts: both(['#5470c6', '#91cc75', '#fac858', '#ee6666', '#73c0de', '#3ba272', '#fc8452', '#9a60b4', '#ea7ccc']),
    material: both(['#1e88e5', '#e53935', '#43a047', '#fb8c00', '#8e24aa', '#00acc1', '#6d4c41', '#3949ab', '#c0ca33']),
    vivid: both(['#0057e7', '#d62d20', '#008744', '#ffa700', '#8e44ad', '#00a8c6', '#e8467c', '#5c6b00']),
    pastel: both(['#7fb3d5', '#f5b7b1', '#a9dfbf', '#f9e79f', '#d2b4de', '#a3e4d7', '#f8c471', '#aeb6bf']),
    morandi: both(['#7a8b99', '#b49b85', '#91a18a', '#c4a4a4', '#8e9bb5', '#c2b28f', '#a497ad', '#8a9a92']),
    warm: both(['#c0392b', '#e67e22', '#f1c40f', '#a0522d', '#d35486', '#e59866', '#7b241c', '#b7950b']),
    cool: both(['#1f5f99', '#17a2b8', '#2a9d8f', '#5e60ce', '#48bfe3', '#264653', '#7fb069', '#6c8ebf']),
    chinese: both(['#c3272b', '#1661ab', '#789262', '#e9a118', '#574266', '#9d2933', '#5e7987', '#b36d61']),
    mono: both(['#1f6fd1', '#3d4a5c', '#6b7889', '#98a3b1', '#c2c9d2', '#e08a2c'])
  };
  /** Light-only palette → dark variant: dark colours are lifted toward white so they stay visible on a dark surface. */
  function both(light) {
    const lift = (c) => {
      const v = [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16));
      const lum = (0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]) / 255;
      const t = lum < 0.45 ? 0.28 : lum < 0.6 ? 0.12 : 0;
      return '#' + v.map((x) => Math.round(x + (255 - x) * t).toString(16).padStart(2, '0')).join('');
    };
    return { light, dark: light.map(lift) };
  }
  const HEX = /^#[0-9a-f]{6}$/i;
  function paletteOf(v) {
    if (Array.isArray(v)) { const c = v.filter((x) => typeof x === 'string' && HEX.test(x.trim())).map((x) => x.trim()); return c.length >= 2 ? both(c) : null; }
    return typeof v === 'string' && PALETTES[v] ? PALETTES[v] : null;
  }
  function curPal(theme) { const p = paletteOf(state.spec && state.spec.palette) || paletteOf(state.palette) || PAL; return p[theme] || p.light; }
  /** Sequential ramp (heatmap / funnel): light → dark of the palette's first colour (one hue, never a rainbow). */
  function curSeq() {
    const c = curPal('light')[0];
    if (c === PAL.light[0]) return SEQ;
    const rgb = [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16));
    const mix = (t) => '#' + rgb.map((v) => Math.round(t < 0 ? v + (255 - v) * -t : v * (1 - t)).toString(16).padStart(2, '0')).join('');
    return [-0.8, -0.6, -0.35, 0, 0.2, 0.4, 0.6].map(mix);
  }
  const CHART_TYPES = ['bar', 'hbar', 'stacked_bar', 'line', 'area', 'pie', 'donut', 'radar', 'scatter', 'heatmap', 'funnel', 'treemap', 'table', 'kpi'];
  const TYPE_LABEL = { zh: { bar: '柱状图', hbar: '条形图', stacked_bar: '堆叠柱状', line: '折线图', area: '面积图', pie: '饼图', donut: '环形图', radar: '雷达图', scatter: '散点图', heatmap: '热力图', funnel: '漏斗图', treemap: '矩形树图', table: '表格', kpi: '指标卡' },
    en: { bar: 'Bar', hbar: 'Horizontal bar', stacked_bar: 'Stacked bar', line: 'Line', area: 'Area', pie: 'Pie', donut: 'Donut', radar: 'Radar', scatter: 'Scatter', heatmap: 'Heatmap', funnel: 'Funnel', treemap: 'Treemap', table: 'Table', kpi: 'KPI' } };
  const T = { zh: { all: '全部', from: '从', to: '至', reset: '重置筛选', exportHtml: '导出 HTML', exportPng: '导出 PNG', noData: '无数据', other: '其他', capped: '数据已截断', dim: '维度', measure: '指标', type: '图表', table: '查看数据', analysis: '分析：参考线 / 细分 / 计算 / 汇总', refLine: '参考线', breakdown: '细分', calc: '计算', agg: '汇总', none: '无', noBreak: '不细分', raw: '原值', avg: '平均值', median: '中位数', minmax: '最大 / 最小', max: '最大值', min: '最小值', target: '目标', yoy: '增长率（较上期）', cumsum: '累计', share: '占比', ma3: '3 期移动平均', sum: '求和', count: '计数', aggAvg: '平均', csv: '导出当前数据 CSV', edit: '编辑卡片', series: '颜色分组', sort: '排序', sortAuto: '自动', valueDesc: '按数值降序', valueAsc: '按数值升序', dimAsc: '按维度升序', dimDesc: '按维度降序', limit: '最多显示', legend: '图例', label: '数据标签', grid: '网格线', symbol: '折线点', apply: '应用', cancel: '取消' },
    en: { all: 'All', from: 'From', to: 'To', reset: 'Reset filters', exportHtml: 'Export HTML', exportPng: 'Export PNG', noData: 'No data', other: 'Other', capped: 'Data capped', dim: 'Dimension', measure: 'Measure', type: 'Chart', table: 'View data', analysis: 'Analysis: reference line / breakdown / calc / aggregation', refLine: 'Reference', breakdown: 'Breakdown', calc: 'Calc', agg: 'Aggregate', none: 'None', noBreak: 'No breakdown', raw: 'Raw', avg: 'Average', median: 'Median', minmax: 'Max / Min', max: 'Max', min: 'Min', target: 'Target', yoy: 'Growth vs previous', cumsum: 'Cumulative', share: 'Share', ma3: '3-period moving avg', sum: 'Sum', count: 'Count', aggAvg: 'Average', csv: 'Export current data (CSV)', edit: 'Edit card', series: 'Color by', sort: 'Sort', sortAuto: 'Auto', valueDesc: 'Value ↓', valueAsc: 'Value ↑', dimAsc: 'Dimension ↑', dimDesc: 'Dimension ↓', limit: 'Show at most', legend: 'Legend', label: 'Labels', grid: 'Grid lines', symbol: 'Line points', apply: 'Apply', cancel: 'Cancel' } };

  const state = { theme: 'light', lang: 'zh', palette: 'default', spec: null, data: null, filters: {}, cross: {}, widgetState: {}, charts: {}, chatMode: false, params: {} };
  const host = (m) => (typeof DPHost !== 'undefined' ? DPHost.post(m) : false);
  const t = (k) => (T[state.lang] || T.zh)[k] || k;
  const cssv = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
  /** Font scale from the view width (inspector can be 360–900 px wide, exported HTML much wider). */
  let FS = 1;
  function computeScale() {
    const w = document.documentElement.clientWidth || 600;
    FS = Math.max(0.85, Math.min(1.3, w / (state.chatMode ? 640 : 720)));
    document.documentElement.style.setProperty('--fs', String(FS));
    return FS;
  }
  const fs = (n) => Math.round(n * FS * 10) / 10;
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

  // ---------- derived values & reference lines ----------
  const CALCS = ['yoy', 'cumsum', 'share', 'ma3'];
  function applyCalc(d, c) {
    if (c === 'cumsum') { let s = 0; return d.map((v) => (v == null ? null : (s += v))); }
    if (c === 'share') { const tot = d.reduce((a, v) => a + (isNum(v) ? v : 0), 0); return d.map((v) => (isNum(v) && tot ? v / tot : null)); }
    if (c === 'yoy' || c === 'pop') return d.map((v, i) => (i === 0 || !isNum(v) || !isNum(d[i - 1]) || d[i - 1] === 0 ? null : (v - d[i - 1]) / Math.abs(d[i - 1])));
    if (c === 'ma3') return d.map((v, i) => { const win = d.slice(Math.max(0, i - 2), i + 1).filter(isNum); return i < 2 || win.length < 3 ? null : win.reduce((a, b) => a + b, 0) / 3; });
    return d;
  }
  function refValue(vals, r) {
    const v = vals.filter(isNum); if (!v.length && r.type !== 'value') return null;
    switch (r.type) {
      case 'avg': return v.reduce((a, b) => a + b, 0) / v.length;
      case 'median': { const s = v.slice().sort((a, b) => a - b), m = s.length >> 1; return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2; }
      case 'max': return Math.max(...v);
      case 'min': return Math.min(...v);
      case 'value': case 'target': return isNum(+r.value) ? +r.value : null;
      default: return null;
    }
  }
  /** normalise options.refLines: 'avg' | ['avg'] | [{type, value, label}] ; 'minmax' expands */
  function refList(o) {
    let r = o && o.refLines; if (!r) return [];
    if (!Array.isArray(r)) r = [r];
    const out = [];
    r.forEach((x) => { const e = typeof x === 'string' ? { type: x } : (x || {}); if (e.type === 'minmax') { out.push({ type: 'max' }, { type: 'min' }); } else if (e.type) out.push(e); });
    return out;
  }

  // ---------- data helpers ----------
  function table(ds) {
    if (!ds) return { columns: [], rows: [] };
    if (!ds._objs) {
      const cols = ds.columns || [];
      ds._objs = (ds.rows || []).map((r) => { const o = {}; cols.forEach((c, i) => { o[c] = r[i]; }); return o; });
    }
    return { columns: ds.columns || [], rows: ds._objs, capped: ds.capped };
  }
  /** Row-level calculated fields (spec.calcFields) are materialised on each dataset's row objects; re-run on param change. */
  function prepareData() {
    const spec = state.spec || {};
    const cfs = spec.calcFields || [];
    for (const [id, ds] of Object.entries(state.data || {})) {
      if (!ds) continue;
      const tb = table(ds);
      if (!ds._baseCols) ds._baseCols = (ds.columns || []).slice();
      const mine = cfs.filter((c) => c && c.name && c.expr && (!c.dataset || c.dataset === id));
      if (!mine.length || !window.DPExpr) continue;
      const cols = ds._baseCols.slice();
      for (const cf of mine) {
        let c; try { c = DPExpr.compile(cf.expr); } catch (e) { continue; }
        if (!cf.dataset && !c.fields.every((f) => cols.includes(f) || f in state.params)) continue;
        for (const r of tb.rows) { try { r[cf.name] = DPExpr.evaluate(c.ast, { row: r, params: state.params }); } catch (e) { r[cf.name] = null; } }
        if (!cols.includes(cf.name)) cols.push(cf.name);
      }
      ds.columns = cols;
    }
  }
  /** spec.measures entry for a field name (aggregate expression evaluated per group). */
  function measureDef(field) { const ms = (state.spec && state.spec.measures) || []; return ms.find((m) => m && m.id === field && m.expr); }
  const pctFmt = (f) => (f === 'pct' || f === 'percent' ? '0.0%' : f);
  const isNum = (v) => typeof v === 'number' && isFinite(v);
  function isDateLike(v) { return typeof v === 'string' && /^\d{4}-\d{2}(-\d{2})?([T ][\d:.]+)?/.test(v); }
  function fmtDim(v) {
    if (v == null) return '∅';
    if (isDateLike(v)) { const s = String(v); return s.length >= 10 && s.slice(8, 10) === '01' && /T00:00:00/.test(s) ? s.slice(0, 7) : s.slice(0, 10).replace(/-01$/, s.length > 10 ? '-01' : ''); }
    return String(v);
  }
  function aggregate(vals, agg) {
    const nums = vals.filter(isNum);
    switch (agg || 'sum') {
      case 'count': return vals.length;
      case 'count_distinct': return new Set(vals.map(String)).size;
      case 'avg': return nums.length ? nums.reduce((a, b) => a + b, 0) / nums.length : null;
      case 'max': return nums.length ? Math.max(...nums) : null;
      case 'min': return nums.length ? Math.min(...nums) : null;
      case 'none': return nums.length ? nums[0] : (vals[0] ?? null);
      default: return nums.reduce((a, b) => a + b, 0);
    }
  }
  function groupBy(rows, keys, measures) {
    const m = new Map();
    const defs = measures.map((ms) => measureDef(ms.field));
    const needRows = defs.some(Boolean);
    for (const r of rows) {
      const k = keys.map((c) => fmtDim(r[c])).join('\u0001');
      let g = m.get(k);
      if (!g) { g = { key: keys.map((c) => fmtDim(r[c])), vals: measures.map(() => []), rows: [] }; m.set(k, g); }
      if (needRows) g.rows.push(r);
      measures.forEach((ms, i) => { if (!defs[i]) g.vals[i].push(ms.field === '*' ? 1 : r[ms.field]); });
    }
    return [...m.values()].map((g) => ({ key: g.key, values: g.vals.map((v, i) => defs[i] ? aggMeasure(defs[i], g.rows) : aggregate(v, measures[i].agg)) }));
  }
  function aggMeasure(def, rows) { try { const v = DPExpr.evalAgg(def.expr, rows, state.params); return typeof v === 'number' && isFinite(v) ? v : null; } catch (e) { return null; } }
  function sortKeys(keys) {
    const allDate = keys.every((k) => /^\d{4}(-\d{2}){0,2}$/.test(k));
    const allNum = keys.every((k) => k !== '' && !isNaN(+k));
    if (allDate) return keys.slice().sort();
    if (allNum) return keys.slice().sort((a, b) => +a - +b);
    return null;
  }

  // ---------- number formatting ----------
  function fmtNum(v, f) {
    if (v == null || !isFinite(v)) return '—';
    f = f || '';
    const pct = f.includes('%');
    let x = pct ? v * 100 : v;
    const prefix = (f.match(/^[^0-9,.#%]*/) || [''])[0];
    const dec = (f.match(/\.(\d)/) || [])[1];
    let s;
    if (dec != null) s = x.toLocaleString(undefined, { minimumFractionDigits: +dec, maximumFractionDigits: +dec });
    else if (!f && Math.abs(x) >= 1e8 && state.lang === 'zh') s = (x / 1e8).toFixed(2) + '亿';
    else if (!f && Math.abs(x) >= 1e4 && state.lang === 'zh') s = (x / 1e4).toFixed(Math.abs(x) >= 1e6 ? 0 : 1) + '万';
    else if (!f && Math.abs(x) >= 1e6) s = (x / 1e6).toFixed(1) + 'M';
    else s = x.toLocaleString(undefined, { maximumFractionDigits: Math.abs(x) < 10 ? 2 : 0 });
    return prefix + s + (pct ? '%' : '');
  }

  // ---------- filters ----------
  function applyFilters(rows, datasetId, excludeWidget) {
    const spec = state.spec;
    const fs = (spec && spec.filters) || [];
    let out = rows;
    for (const f of fs) {
      if (f.dataset && f.dataset !== datasetId) continue;
      const v = state.filters[f.id];
      if (v == null || (Array.isArray(v) && !v.length)) continue;
      if (f.type === 'daterange' || f.type === 'range') {
        const [a, b] = v;
        out = out.filter((r) => { const x = r[f.field]; if (x == null) return false; const s = f.type === 'range' ? +x : String(x).slice(0, 10); return (a == null || a === '' || s >= (f.type === 'range' ? +a : a)) && (b == null || b === '' || s <= (f.type === 'range' ? +b : b)); });
      } else {
        const set = new Set((Array.isArray(v) ? v : [v]).map(String));
        out = out.filter((r) => set.has(fmtDim(r[f.field])));
      }
    }
    for (const [wid, c] of Object.entries(state.cross)) {
      if (wid === excludeWidget || c.dataset !== datasetId) continue;
      out = out.filter((r) => fmtDim(r[c.field]) === c.value);
    }
    return out;
  }

  function renderFilters(root) {
    const spec = state.spec; const fs = spec.filters || [];
    const bar = document.createElement('div'); bar.className = 'dp-filters';
    for (const p of spec.params || []) {
      if (!p || !p.id) continue;
      const box = document.createElement('div'); box.className = 'dp-filter dp-param';
      const lbl = document.createElement('label'); lbl.textContent = p.label || p.id; box.appendChild(lbl);
      const apply = (v) => { state.params[p.id] = v; prepareData(); refresh(); };
      if (Array.isArray(p.options) && p.options.length) {
        const sel = document.createElement('select');
        sel.innerHTML = p.options.map((o) => `<option>${esc(o)}</option>`).join('');
        sel.value = String(state.params[p.id]); sel.onchange = () => { const n = +sel.value; apply(sel.value !== '' && isFinite(n) ? n : sel.value); };
        box.appendChild(sel);
      } else {
        const inp = document.createElement('input'); inp.type = 'range';
        const v0 = +state.params[p.id] || 0;
        inp.min = p.min != null ? p.min : Math.min(0, v0); inp.max = p.max != null ? p.max : (v0 ? Math.abs(v0) * 3 : 1);
        inp.step = p.step != null ? p.step : ((+inp.max - +inp.min) / 100 || 0.01); inp.value = v0;
        const out = document.createElement('span'); out.className = 'dp-param-v'; out.textContent = fmtNum(v0, p.format);
        inp.oninput = () => { out.textContent = fmtNum(+inp.value, p.format); };
        inp.onchange = () => apply(+inp.value);
        box.appendChild(inp); box.appendChild(out);
      }
      bar.appendChild(box);
    }
    for (const f of fs) {
      const ds = table(state.data[f.dataset || Object.keys(state.data)[0]]);
      const box = document.createElement('div'); box.className = 'dp-filter';
      box.innerHTML = `<label>${esc(f.label || f.field)}</label>`;
      if (f.type === 'daterange' || f.type === 'range') {
        const vals = ds.rows.map((r) => r[f.field]).filter((x) => x != null);
        const isRange = f.type === 'range';
        const sorted = isRange ? vals.map(Number).sort((a, b) => a - b) : vals.map((x) => String(x).slice(0, 10)).sort();
        const mk = (i, ph) => { const inp = document.createElement('input'); inp.type = isRange ? 'number' : 'date'; inp.placeholder = ph; inp.min = sorted[0]; inp.max = sorted[sorted.length - 1]; inp.value = (state.filters[f.id] || [])[i] || (i === 0 ? inp.min : inp.max) || '';
          inp.onchange = () => { const cur = state.filters[f.id] || [null, null]; cur[i] = (inp.value && inp.value !== (i === 0 ? inp.min : inp.max)) ? inp.value : null; state.filters[f.id] = cur; refresh(); }; return inp; };
        box.appendChild(mk(0, t('from'))); box.appendChild(document.createTextNode('–')); box.appendChild(mk(1, t('to')));
      } else {
        const opts = [...new Set(ds.rows.map((r) => fmtDim(r[f.field])))];
        const so = sortKeys(opts) || opts.sort((a, b) => a.localeCompare(b, 'zh'));
        if (f.type === 'select') {
          const sel = document.createElement('select');
          sel.innerHTML = `<option value="">${t('all')}</option>` + so.map((o) => `<option>${esc(o)}</option>`).join('');
          sel.value = state.filters[f.id] || ''; sel.onchange = () => { state.filters[f.id] = sel.value || null; refresh(); };
          box.appendChild(sel);
        } else {
          const ms = document.createElement('div'); ms.className = 'dp-ms';
          const btn = document.createElement('span'); btn.className = 'dp-ms-btn';
          const pop = document.createElement('div'); pop.className = 'dp-ms-pop';
          const upd = () => { const v = state.filters[f.id] || []; btn.textContent = v.length ? (v.length <= 2 ? v.join('、') : `${v.length} ✓`) : t('all') + ' ▾'; };
          so.slice(0, 500).forEach((o) => {
            const it = document.createElement('div'); const cb = document.createElement('input'); cb.type = 'checkbox';
            cb.checked = (state.filters[f.id] || []).includes(o);
            it.appendChild(cb); it.appendChild(document.createTextNode(o));
            it.onclick = (e) => { if (e.target !== cb) cb.checked = !cb.checked; const v = new Set(state.filters[f.id] || []); cb.checked ? v.add(o) : v.delete(o); state.filters[f.id] = [...v]; upd(); refresh(); };
            pop.appendChild(it);
          });
          btn.onclick = (e) => { e.stopPropagation(); document.querySelectorAll('.dp-ms.open').forEach((x) => x !== ms && x.classList.remove('open')); ms.classList.toggle('open'); };
          ms.appendChild(btn); ms.appendChild(pop); upd(); box.appendChild(ms);
        }
      }
      bar.appendChild(box);
    }
    const chips = document.createElement('div'); chips.className = 'dp-chips'; chips.id = 'dp-cross-chips'; bar.appendChild(chips);
    if (fs.length) { const r = document.createElement('button'); r.className = 'dp-btn'; r.textContent = t('reset'); r.onclick = () => { state.filters = {}; state.cross = {}; renderDashboard(state.spec, state.data, { keepState: false }); }; bar.appendChild(r); }
    root.appendChild(bar);
  }
  function renderCrossChips() {
    const el = document.getElementById('dp-cross-chips'); if (!el) return;
    el.innerHTML = '';
    for (const [wid, c] of Object.entries(state.cross)) {
      const chip = document.createElement('span'); chip.className = 'dp-chip'; chip.textContent = `${c.field} = ${c.value}`;
      chip.onclick = () => { delete state.cross[wid]; refresh(); }; el.appendChild(chip);
    }
  }
  document.addEventListener('click', () => document.querySelectorAll('.dp-ms.open').forEach((x) => x.classList.remove('open')));

  // ---------- ECharts option builders ----------
  function base(theme) {
    const ink = cssv('--ink'), ink2 = cssv('--ink2'), muted = cssv('--muted'), grid = cssv('--grid'), axis = cssv('--axis'), surf = cssv('--surface');
    return {
      color: curPal(theme), backgroundColor: 'transparent', textStyle: { fontFamily: 'system-ui,-apple-system,"PingFang SC",sans-serif', color: ink2 },
      animation: !window.DP_NOANIM, animationDuration: 300,
      tooltip: { backgroundColor: surf, borderColor: cssv('--border') || grid, textStyle: { color: ink, fontSize: fs(12) }, extraCssText: 'box-shadow:0 4px 16px rgba(0,0,0,.12);border-radius:8px' },
      legend: { type: 'scroll', top: 0, textStyle: { color: ink2, fontSize: fs(12) }, icon: 'roundRect', itemWidth: 10, itemHeight: 10 },
      grid: { left: 8, right: 16, top: 30, bottom: 8, containLabel: true },
      _axis: { axisLine: { lineStyle: { color: axis } }, axisTick: { show: false }, axisLabel: { color: muted, fontSize: fs(11) }, splitLine: { lineStyle: { color: grid } } }
    };
  }
  function axisCat(b, data, rotate) { return Object.assign({ type: 'category', data, axisLabel: { color: cssv('--muted'), fontSize: fs(11), rotate: rotate ? 30 : 0, hideOverlap: true, formatter: (v) => { const t = fmtDim(v); return t.length > 14 ? t.slice(0, 13) + '…' : t; } } }, { axisLine: b._axis.axisLine, axisTick: b._axis.axisTick }); }
  function axisVal(b, fmt) { return { type: 'value', axisLine: { show: false }, axisTick: { show: false }, axisLabel: { color: cssv('--muted'), fontSize: fs(11), formatter: (v) => fmtNum(v, fmt) }, splitLine: b._axis.splitLine }; }

  /** Build an ECharts option for a widget-like definition against filtered rows. */
  function buildOption(w, rows) {
    computeScale();
    const theme = state.theme, b = base(theme);
    const type = w.type;
    const ys = (w.y || []).map((y) => (typeof y === 'string' ? { field: y, agg: w.agg || 'sum' } : y));
    let fmt = w.format || (w.options && w.options.format);
    if (!fmt && ys[0]) { const md = measureDef(ys[0].field); if (md && md.format) fmt = pctFmt(md.format); }
    const x = w.x, series = w.series;
    const opt = { color: b.color, backgroundColor: b.backgroundColor, textStyle: b.textStyle, animation: !window.DP_NOANIM, animationDuration: 300, tooltip: b.tooltip };
    if (!rows.length) return null;

    if (['pie', 'donut', 'funnel', 'treemap'].includes(type)) {
      const m = ys[0] || { field: '*', agg: 'count' };
      let g = groupBy(rows, [x], [m]).filter((d) => d.values[0] != null).sort((a, c) => c.values[0] - a.values[0]);
      const maxSlices = type === 'treemap' ? 60 : 8;
      if (g.length > maxSlices) { const rest = g.slice(maxSlices - 1); g = g.slice(0, maxSlices - 1); g.push({ key: [t('other')], values: [rest.reduce((a, d) => a + (d.values[0] || 0), 0)] }); }
      const data = g.map((d) => ({ name: d.key[0], value: d.values[0] }));
      opt.tooltip = Object.assign({}, b.tooltip, { trigger: 'item', valueFormatter: (v) => fmtNum(v, fmt) });
      if (type === 'treemap') {
        opt.series = [{ type: 'treemap', data, roam: false, nodeClick: false, breadcrumb: { show: false }, label: { color: '#fff', fontSize: fs(12) }, itemStyle: { borderColor: cssv('--surface'), borderWidth: 2, gapWidth: 2 }, levels: [{ color: b.color }] }];
      } else if (type === 'funnel') {
        opt.series = [{ type: 'funnel', data, left: '10%', width: '80%', top: 10, bottom: 10, gap: 2, sort: 'descending', label: { color: cssv('--ink2') }, itemStyle: { borderColor: cssv('--surface'), borderWidth: 2 }, color: curSeq().slice(2).reverse() }];
      } else {
        opt.legend = Object.assign({}, b.legend, { top: undefined, bottom: 0, formatter: (n) => (String(n).length > 18 ? String(n).slice(0, 17) + '…' : n) });
        opt.series = [{ type: 'pie', radius: type === 'donut' ? ['42%', '64%'] : [0, '64%'], center: ['50%', '46%'], data, itemStyle: { borderColor: cssv('--surface'), borderWidth: 2, borderRadius: 4 },
          label: { show: data.length <= 6, color: cssv('--ink2'), formatter: (p) => `${p.name}\n${p.percent.toFixed(1)}%` }, labelLine: { lineStyle: { color: cssv('--axis') } } }];
      }
      return opt;
    }

    if (type === 'radar') {
      const ms = ys.length ? ys : [{ field: '*', agg: 'count' }];
      if (ms.length >= 3) {
        // indicators = measures, one polygon per x value (max 6)
        let g = groupBy(rows, [x], ms).slice(0, 6);
        const maxes = ms.map((_, i) => Math.max(...g.map((d) => d.values[i] || 0)) * 1.1 || 1);
        opt.radar = { indicator: ms.map((m, i) => ({ name: m.label || m.field, max: maxes[i] })), axisName: { color: cssv('--ink2') }, splitLine: { lineStyle: { color: cssv('--grid') } }, splitArea: { show: false }, axisLine: { lineStyle: { color: cssv('--grid') } } };
        opt.series = [{ type: 'radar', data: g.map((d) => ({ name: d.key[0], value: d.values })), symbolSize: 6, lineStyle: { width: 2 }, areaStyle: { opacity: 0.08 } }];
      } else {
        // indicators = x categories (max 12), one polygon per measure (normalised per measure)
        const g = groupBy(rows, [x], ms).slice(0, 12);
        opt.radar = { indicator: g.map((d) => ({ name: d.key[0] })), axisName: { color: cssv('--ink2') }, splitLine: { lineStyle: { color: cssv('--grid') } }, splitArea: { show: false }, axisLine: { lineStyle: { color: cssv('--grid') } } };
        opt.series = [{ type: 'radar', symbolSize: 6, lineStyle: { width: 2 }, areaStyle: { opacity: 0.08 },
          data: ms.map((m, i) => { const vals = g.map((d) => d.values[i] || 0); const mx = Math.max(...vals) || 1; return { name: m.label || m.field, value: ms.length > 1 ? vals.map((v) => +(v / mx * 100).toFixed(1)) : vals, raw: vals }; }) }];
        if (ms.length > 1) opt.radar.indicator.forEach((i) => { i.max = 100; });
      }
      opt.legend = Object.assign({}, b.legend);
      opt.tooltip = Object.assign({}, b.tooltip, { trigger: 'item' });
      return opt;
    }

    if (type === 'scatter') {
      const [mx, my] = [ys[0], ys[1] || ys[0]];
      const pts = rows.filter((r) => isNum(r[mx.field]) && isNum(r[my.field])).slice(0, 5000);
      const groups = series ? [...new Set(pts.map((r) => fmtDim(r[series])))].slice(0, 3) : [null];
      opt.grid = b.grid; opt.legend = groups[0] != null ? b.legend : undefined;
      opt.xAxis = Object.assign(axisVal(b), { name: mx.label || mx.field, nameLocation: 'middle', nameGap: 24, nameTextStyle: { color: cssv('--muted') } });
      opt.yAxis = Object.assign(axisVal(b), { name: my.label || my.field, nameTextStyle: { color: cssv('--muted') } });
      opt.tooltip = Object.assign({}, b.tooltip, { trigger: 'item', formatter: (p) => `${esc(p.data[2] || '')}<br>${esc(mx.field)}: ${fmtNum(p.data[0])}<br>${esc(my.field)}: ${fmtNum(p.data[1])}` });
      opt.series = groups.map((gname) => ({ type: 'scatter', name: gname || my.field, symbolSize: 8, itemStyle: { borderColor: cssv('--surface'), borderWidth: 1, opacity: 0.85 },
        data: pts.filter((r) => gname == null || fmtDim(r[series]) === gname).map((r) => [r[mx.field], r[my.field], x ? fmtDim(r[x]) : ''] ) }));
      return opt;
    }

    if (type === 'heatmap') {
      const m = ys[0] || { field: '*', agg: 'count' };
      const g = groupBy(rows, [x, series], [m]);
      const xs = sortKeys([...new Set(g.map((d) => d.key[0]))]) || [...new Set(g.map((d) => d.key[0]))];
      const ss = sortKeys([...new Set(g.map((d) => d.key[1]))]) || [...new Set(g.map((d) => d.key[1]))];
      const vals = g.map((d) => d.values[0]).filter(isNum);
      opt.grid = Object.assign({}, b.grid, { bottom: 44 });
      opt.xAxis = axisCat(b, xs, xs.length > 8); opt.yAxis = axisCat(b, ss);
      opt.visualMap = { min: Math.min(...vals), max: Math.max(...vals), calculable: false, orient: 'horizontal', left: 'center', bottom: 2, itemHeight: 140, itemWidth: 10, text: [fmtNum(Math.max(...vals), fmt), fmtNum(Math.min(...vals), fmt)], showLabel: false, inRange: { color: curSeq() }, textStyle: { color: cssv('--muted') }, formatter: (v) => fmtNum(v, fmt) };
      opt.tooltip = Object.assign({}, b.tooltip, { trigger: 'item', formatter: (p) => `${esc(xs[p.data[0]])} · ${esc(ss[p.data[1]])}<br><b>${fmtNum(p.data[2], fmt)}</b>` });
      opt.series = [{ type: 'heatmap', data: g.map((d) => [xs.indexOf(d.key[0]), ss.indexOf(d.key[1]), d.values[0]]), itemStyle: { borderColor: cssv('--surface'), borderWidth: 2, borderRadius: 2 } }];
      return opt;
    }

    // bar / hbar / stacked_bar / line / area
    const horizontal = type === 'hbar';
    const isLine = type === 'line' || type === 'area';
    const stacked = type === 'stacked_bar' || (w.options && w.options.stack);
    let cats, seriesList;
    if (series) {
      const m = ys[0] || { field: '*', agg: 'count' };
      const g = groupBy(rows, [x, series], [m]);
      cats = [...new Set(g.map((d) => d.key[0]))];
      // series: top 7 by total, rest -> Other (never more than 8 hues)
      const totals = new Map();
      g.forEach((d) => totals.set(d.key[1], (totals.get(d.key[1]) || 0) + (d.values[0] || 0)));
      let sNames = [...totals.entries()].sort((a, c) => c[1] - a[1]).map((e) => e[0]);
      const keep = new Set(sNames.slice(0, sNames.length > 8 ? 7 : 8));
      const lookup = new Map();
      g.forEach((d) => { const s = keep.has(d.key[1]) ? d.key[1] : t('other'); const k = d.key[0] + '\u0001' + s; lookup.set(k, (lookup.get(k) || 0) + (d.values[0] || 0)); });
      if (sNames.length > 8) sNames = [...sNames.slice(0, 7), t('other')];
      const sorted = sortKeys(cats); cats = sorted || cats;
      seriesList = sNames.map((s) => ({ name: s, data: cats.map((c) => lookup.has(c + '\u0001' + s) ? lookup.get(c + '\u0001' + s) : (isLine ? null : 0)) }));
    } else {
      const ms = ys.length ? ys : [{ field: '*', agg: 'count' }];
      let g = groupBy(rows, [x], ms);
      const sorted = sortKeys(g.map((d) => d.key[0]));
      if (sorted) g.sort((a, c) => sorted.indexOf(a.key[0]) - sorted.indexOf(c.key[0]));
      else if (!isLine) g.sort((a, c) => (c.values[0] || 0) - (a.values[0] || 0));
      sortGroups(g, (w.options || {}).sort);
      const maxCats = (w.options && w.options.limit) || (isLine ? 400 : 30);
      if (g.length > maxCats) g = g.slice(0, maxCats);
      cats = g.map((d) => d.key[0]);
      const lineSet = new Set((w.options && w.options.lineMeasures) || []);
      seriesList = ms.map((m, i) => ({ name: m.label || m.field, data: g.map((d) => d.values[i]), kind: m.chart || (lineSet.has(m.field) ? 'line' : undefined) }));
    }
    const wo = w.options || {};
    if (wo.calc && CALCS.concat(['pop']).includes(wo.calc)) {
      seriesList.forEach((s) => { s.data = applyCalc(s.data, wo.calc); });
      if (wo.calc === 'yoy' || wo.calc === 'pop' || wo.calc === 'share') fmt = '0.0%';
    }
    const refs = refList(wo);
    if (horizontal) { cats = cats.slice().reverse(); seriesList.forEach((s) => s.data.reverse()); }
    opt.grid = b.grid;
    opt.legend = seriesList.length > 1 ? b.legend : undefined;
    if (seriesList.length <= 1) opt.grid = Object.assign({}, b.grid, { top: 12 });
    const catAxis = axisCat(b, cats, !horizontal && cats.length > 8 && cats.some((c) => String(c).length > 4));
    if (isLine) catAxis.boundaryGap = false;
    opt.xAxis = horizontal ? axisVal(b, fmt) : catAxis;
    opt.yAxis = horizontal ? catAxis : axisVal(b, fmt);
    opt.tooltip = Object.assign({}, b.tooltip, { trigger: 'axis', axisPointer: { type: isLine ? 'line' : 'shadow', lineStyle: { color: cssv('--axis') }, shadowStyle: { color: cssv('--hover') } }, valueFormatter: (v) => fmtNum(v, fmt) });
    const many = cats.length > 40;
    opt.series = seriesList.map((s, i) => (s.kind ? s.kind === 'line' : isLine) ? {
      type: 'line', name: s.name, data: s.data, smooth: !!(w.options && w.options.smooth), showSymbol: !many && cats.length <= 24, symbolSize: 8, lineStyle: { width: 2 },
      areaStyle: type === 'area' ? { opacity: seriesList.length > 1 ? 0.12 : 0.18 } : undefined, stack: stacked ? 'total' : undefined, connectNulls: false,
      emphasis: { focus: 'series' }, itemStyle: { borderColor: cssv('--surface'), borderWidth: 2 }
    } : {
      type: 'bar', name: s.name, data: s.data, stack: stacked ? 'total' : undefined, barMaxWidth: 36, barGap: '10%',
      itemStyle: { borderRadius: stacked ? (i === seriesList.length - 1 ? (horizontal ? [0, 4, 4, 0] : [4, 4, 0, 0]) : 0) : (horizontal ? [0, 4, 4, 0] : [4, 4, 0, 0]), borderColor: cssv('--surface'), borderWidth: stacked ? 1 : 0 },
      label: { show: !series && seriesList.length === 1 && cats.length <= 8, position: horizontal ? 'right' : 'top', color: cssv('--ink2'), fontSize: fs(11), formatter: (p) => fmtNum(p.value, fmt) },
      emphasis: { focus: 'series' }
    });
    if (refs.length) {
      const targets = opt.series.length <= 3 ? opt.series : opt.series.slice(0, 1);
      targets.forEach((ser, si) => {
        const data = [];
        refs.forEach((r) => {
          const v = refValue(seriesList[opt.series.indexOf(ser)].data, r); if (v == null) return;
          const name = r.label || t(r.type === 'value' ? 'target' : r.type);
          const lbl = (targets.length > 1 ? ser.name + ' · ' : '') + name + ' ' + fmtNum(v, fmt);
          data.push(Object.assign(horizontal ? { xAxis: v } : { yAxis: v }, { name: lbl, label: { formatter: lbl } }));
        });
        if (!data.length) return;
        ser.markLine = { silent: true, symbol: 'none', animation: false, data,
          lineStyle: { type: 'dashed', width: 1.5, color: targets.length > 1 ? undefined : cssv('--ink2'), opacity: 0.85 },
          label: { position: horizontal ? 'end' : 'insideEndTop', color: cssv('--ink2'), fontSize: fs(11), backgroundColor: cssv('--surface'), padding: [1, 4] } };
      });
    }
    if (cats.length > 60 && !horizontal) opt.dataZoom = [{ type: 'inside' }];
    return opt;
  }

  // ---------- widget rendering ----------
  /** Shrink a KPI number until it fits its card (CSS cqw does most of it; this is the exact fit). */
  function fitKPI(root) {
    (root || document).querySelectorAll('.dp-kpi-v').forEach((el) => {
      el.style.fontSize = '';
      let px = parseFloat(getComputedStyle(el).fontSize) || 28;
      for (let i = 0; i < 30 && el.scrollWidth > el.clientWidth + 1 && px > 10; i++) { px -= 1; el.style.fontSize = px + 'px'; }
      el.title = el.textContent;
    });
  }
  function kpiHTML(w, rows, allRows) {
    const m = w.measure || (w.y && (typeof w.y[0] === 'string' ? { field: w.y[0], agg: 'sum' } : w.y[0])) || { field: '*', agg: 'count' };
    const md = measureDef(m.field);
    const agg = (rs) => (md ? aggMeasure(md, rs) : aggregate(rs.map((r) => (m.field === '*' ? 1 : r[m.field])), m.agg));
    const fmtK = w.format || (md && pctFmt(md.format));
    w = Object.assign({}, w, { format: fmtK });
    const v = agg(rows);
    let sub = '';
    if (allRows && rows.length !== allRows.length && !md) {
      const tot = agg(allRows);
      if (tot && m.agg !== 'avg') sub = `${(v / tot * 100).toFixed(1)}% / ${fmtNum(tot, w.format)}`;
    }
    if (w.subtitle) sub = esc(w.subtitle) + (sub ? ' · ' + sub : '');
    return `<div class="dp-kpi"><div class="dp-kpi-v">${fmtNum(v, w.format)}</div><div class="dp-kpi-s">${sub}</div></div>`;
  }
  function tableHTML(w, rows, columns) {
    const cols = (w.columns && w.columns.length ? w.columns : columns).slice(0, 30);
    const ws = state.widgetState[w.id] || {};
    let data = rows.slice();
    if (ws.sort) { const { c, dir } = ws.sort; data.sort((a, b) => { const x = a[c], y = b[c]; return (x > y ? 1 : x < y ? -1 : 0) * dir; }); }
    data = data.slice(0, 500);
    const head = cols.map((c) => `<th data-col="${esc(c)}">${esc(c)}${ws.sort && ws.sort.c === c ? (ws.sort.dir > 0 ? ' ↑' : ' ↓') : ''}</th>`).join('');
    const body = data.map((r) => '<tr>' + cols.map((c) => { const v = r[c]; return isNum(v) ? `<td class="num">${fmtNum(v, w.format)}</td>` : `<td>${esc(fmtDim(v))}</td>`; }).join('') + '</tr>').join('');
    return `<div class="dp-table-wrap"><table class="dp-table"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>${rows.length > 500 ? `<div class="dp-note">${rows.length} rows (first 500 shown)</div>` : ''}</div>`;
  }
  function mdLite(s) {
    return esc(s).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>').replace(/^### (.*)$/gm, '<b>$1</b>').replace(/^## (.*)$/gm, '<b>$1</b>').replace(/^[-*] (.*)$/gm, '• $1').replace(/\n/g, '<br>');
  }

  function effectiveWidget(w) {
    const ws = state.widgetState[w.id] || {};
    const e = Object.assign({}, w);
    if (ws.type) e.type = ws.type;
    if (ws.dim) e.x = ws.dim;
    if (ws.measure) { const agg = (w.y && w.y[0] && w.y[0].agg) || (w.measure && w.measure.agg) || 'sum'; e.y = [{ field: ws.measure, agg }]; if (e.measure) e.measure = { field: ws.measure, agg }; }
    e.options = Object.assign({}, w.options || {});
    if (ws.ref !== undefined) e.options.refLines = ws.ref === 'none' ? [] : [ws.ref];
    if (ws.calc !== undefined) { if (ws.calc === 'raw') delete e.options.calc; else e.options.calc = ws.calc; }
    if (ws.breakdown !== undefined) e.series = ws.breakdown || undefined;
    if (ws.agg) {
      e.y = (e.y || []).map((y) => Object.assign({}, typeof y === 'string' ? { field: y } : y, { agg: ws.agg }));
      if (e.measure) e.measure = Object.assign({}, e.measure, { agg: ws.agg });
    }
    return e;
  }
  const ANALYSIS_TYPES = ['bar', 'hbar', 'stacked_bar', 'line', 'area'];
  /** Second header row: reference line / breakdown / calc / aggregation (toggled by the ƒx button). */
  function analysisBar(w, ws, rerender) {
    const bar = document.createElement('div'); bar.className = 'dp-card-tools';
    const sel = (label, opts, cur, onch) => {
      const l = document.createElement('label'); l.textContent = label + ' ';
      const s = document.createElement('select'); s.title = label;
      s.innerHTML = opts.map(([v, txt]) => `<option value="${esc(v)}">${esc(txt)}</option>`).join('');
      s.value = cur; s.onchange = () => { onch(s.value); host({ type: 'widgetChange', widget: w.id, change: label, value: s.value }); rerender(); };
      l.appendChild(s); bar.appendChild(l);
    };
    const o = w.options || {};
    const curRef = ws.ref !== undefined ? ws.ref : (refList(o).length ? (refList(o).length === 2 && refList(o)[0].type === 'max' ? 'minmax' : refList(o)[0].type) : 'none');
    sel(t('refLine'), [['none', t('none')], ['avg', t('avg')], ['median', t('median')], ['minmax', t('minmax')]], ['none', 'avg', 'median', 'minmax'].includes(curRef) ? curRef : 'none', (v) => { ws.ref = v; });
    const bds = w.breakdownOptions || [];
    if (bds.length) sel(t('breakdown'), [['', t('noBreak')]].concat(bds.map((f) => [f, f])), ws.breakdown !== undefined ? ws.breakdown : (w.series || ''), (v) => { ws.breakdown = v; });
    sel(t('calc'), [['raw', t('raw')], ['yoy', t('yoy')], ['cumsum', t('cumsum')], ['share', t('share')], ['ma3', t('ma3')]], ws.calc || o.calc || 'raw', (v) => { ws.calc = v; });
    const y0 = (w.y && w.y[0]) || w.measure;
    const curAgg = ws.agg || (y0 && typeof y0 === 'object' && y0.agg) || w.agg || 'sum';
    sel(t('agg'), [['sum', t('sum')], ['avg', t('aggAvg')], ['count', t('count')], ['max', t('max')], ['min', t('min')]], curAgg, (v) => { ws.agg = v; });
    return bar;
  }

  function renderWidget(card, w) {
    const e = effectiveWidget(w);
    const body = card.querySelector('.dp-card-b');
    if (state.charts[w.id]) { state.charts[w.id].dispose(); delete state.charts[w.id]; }
    if (e.type === 'text') { body.innerHTML = `<div class="dp-text">${mdLite(e.content || '')}</div>`; return; }
    const ds = table(state.data[e.dataset]);
    const rows = applyFilters(ds.rows, e.dataset, w.id);
    if (e.type === 'kpi') { body.innerHTML = kpiHTML(e, rows, ds.rows); fitKPI(body); return; }
    if (e.type === 'table') {
      body.innerHTML = tableHTML(e, rows, ds.columns);
      body.querySelectorAll('th').forEach((th) => th.onclick = () => { const c = th.dataset.col; const ws = state.widgetState[w.id] = state.widgetState[w.id] || {}; ws.sort = ws.sort && ws.sort.c === c ? { c, dir: -ws.sort.dir } : { c, dir: -1 }; renderWidget(card, w); });
      return;
    }
    const opt = applyDisplay(buildOption(e, rows), (e.options || {}).display);
    if (!opt) { body.innerHTML = `<div class="dp-empty">${t('noData')}</div>`; return; }
    body.innerHTML = '<div class="dp-chart"></div>';
    const chart = echarts.init(body.firstChild, null, { renderer: 'canvas' });
    chart.setOption(opt);
    state.charts[w.id] = chart;
    if (w.crossFilter !== false && e.x && !['scatter', 'heatmap'].includes(e.type)) {
      chart.on('click', (p) => {
        const value = p.name; if (value == null || value === t('other')) return;
        const cur = state.cross[w.id];
        if (cur && cur.value === value) delete state.cross[w.id]; else state.cross[w.id] = { field: e.x, value, dataset: e.dataset };
        host({ type: 'crossFilter', widget: w.id, field: e.x, value });
        refresh(w.id);
      });
    }
  }

  function cardHeader(w) {
    const ws = state.widgetState[w.id] = state.widgetState[w.id] || {};
    const h = document.createElement('div'); h.className = 'dp-card-h';
    h.innerHTML = `<div class="dp-card-t" title="${esc(w.title || '')}">${esc(w.title || '')}</div>`;
    const mkSel = (opts, cur, label, onch, labeler) => {
      const s = document.createElement('select'); s.title = label;
      s.innerHTML = opts.map((o) => `<option value="${esc(o)}">${esc(labeler ? labeler(o) : o)}</option>`).join('');
      s.value = cur; s.onchange = () => onch(s.value); h.appendChild(s);
    };
    const card = () => h.parentElement;
    if (w.dimensionOptions && w.dimensionOptions.length > 1) mkSel(w.dimensionOptions, ws.dim || w.x, t('dim'), (v) => { ws.dim = v; delete state.cross[w.id]; host({ type: 'widgetChange', widget: w.id, change: 'dimension', value: v }); renderWidget(card(), w); });
    if (w.measureOptions && w.measureOptions.length > 1) mkSel(w.measureOptions, ws.measure || (w.y && w.y[0] && (w.y[0].field || w.y[0])) || (w.measure && w.measure.field), t('measure'), (v) => { ws.measure = v; host({ type: 'widgetChange', widget: w.id, change: 'measure', value: v }); renderWidget(card(), w); });
    if (!['text', 'kpi'].includes(w.type)) {
      let types = w.typeOptions && w.typeOptions.length ? w.typeOptions.slice() : CHART_TYPES.filter((x) => x !== 'kpi');
      if (!types.includes(w.type)) types.unshift(w.type);
      mkSel(types, ws.type || w.type, t('type'), (v) => { ws.type = v; host({ type: 'widgetChange', widget: w.id, change: 'chartType', from: w.type, value: v }); renderWidget(card(), w); }, (o) => (TYPE_LABEL[state.lang] || TYPE_LABEL.zh)[o] || o);
    }
    if (ANALYSIS_TYPES.includes(w.type) && !state.chatMode || (state.chatMode && ANALYSIS_TYPES.includes(w.type))) {
      const fx = document.createElement('button'); fx.className = 'dp-icon dp-fx'; fx.title = t('analysis'); fx.textContent = 'ƒx';
      const hasAnalysis = () => { const o = w.options || {}; return refList(o).length || o.calc || ws.ref || ws.calc || ws.agg || ws.breakdown; };
      if (hasAnalysis()) fx.classList.add('on');
      fx.onclick = () => {
        ws.showTools = !ws.showTools;
        const c = card(); const old = c.querySelector('.dp-card-tools'); if (old) old.remove();
        if (ws.showTools) h.after(analysisBar(w, ws, () => { renderWidget(c, w); fx.classList.toggle('on', !!hasAnalysis()); }));
      };
      h.appendChild(fx);
      if (ws.showTools) setTimeout(() => { const c = card(); if (c && !c.querySelector('.dp-card-tools')) h.after(analysisBar(w, ws, () => renderWidget(c, w))); }, 0);
    }
    if (w.type !== 'text' && w.dataset && !state.chatMode) {
      const ed = document.createElement('button'); ed.className = 'dp-icon'; ed.title = t('edit'); ed.textContent = '✎';
      ed.onclick = () => {
        const c = card(); const old = c.querySelector('.dp-edit'); if (old) { old.remove(); return; }
        h.after(editPanel(w, (patch) => {
          const cur = c.querySelector('.dp-edit'); if (cur) cur.remove();
          if (!patch) return;
          applyWidgetPatch(w, patch);
          delete ws.dim; delete ws.measure; delete ws.agg; delete ws.breakdown; delete state.cross[w.id];
          host({ type: 'widgetEdit', widget: w.id, patch });
          renderWidget(c, w);
        }));
      };
      h.appendChild(ed);
    }
    if (w.type !== 'text') {
      const dl = document.createElement('button'); dl.className = 'dp-icon'; dl.title = t('csv'); dl.textContent = '⤓';
      dl.onclick = () => exportWidgetCSV(w);
      h.appendChild(dl);
    }
    if (!['text', 'kpi', 'table'].includes(w.type)) {
      const b = document.createElement('button'); b.className = 'dp-icon'; b.title = t('table'); b.textContent = '▦';
      b.onclick = () => { ws.type = (ws.type || w.type) === 'table' ? w.type : 'table'; renderWidget(card(), w); h.querySelectorAll('select').forEach((s) => { if (s.title === t('type')) s.value = ws.type; }); };
      h.appendChild(b);
    }
    return h;
  }

  /** Explicit card sort (ChartSchema orderByType / orderByRule in Chat2DB v5.3.8, reduced to four choices). */
  function sortGroups(g, mode) {
    const num = (v) => (v == null ? -Infinity : v);
    const cmp = (a, b) => String(a).localeCompare(String(b), 'zh', { numeric: true });
    if (mode === 'value_desc') g.sort((a, c) => num(c.values[0]) - num(a.values[0]));
    else if (mode === 'value_asc') g.sort((a, c) => num(a.values[0]) - num(c.values[0]));
    else if (mode === 'dim_asc') g.sort((a, c) => cmp(a.key[0], c.key[0]));
    else if (mode === 'dim_desc') g.sort((a, c) => cmp(c.key[0], a.key[0]));
    return g;
  }

  /** Spec versions: 1 = original; 2 = y entries are {field, agg} objects and display switches live in options.display.
   *  Older specs are upgraded on load (shared by the app, exported HTML and a future Tauri shell). */
  const SPEC_VERSION = 2;
  function migrateSpec(spec) {
    if (!spec || typeof spec !== 'object') return spec;
    const v = spec.version || 1;
    if (v < 2) {
      for (const w of spec.widgets || []) {
        if (Array.isArray(w.y)) w.y = w.y.map((y) => (typeof y === 'string' ? { field: y, agg: w.agg || 'sum' } : y));
        const o = w.options || (w.options = {});
        const d = o.display || {};
        for (const k of ['legend', 'label']) if (typeof o[k] === 'boolean' && d[k] === undefined) { d[k] = o[k]; delete o[k]; }
        if (Object.keys(d).length) o.display = d;
      }
    }
    spec.version = Math.max(v, SPEC_VERSION);
    return spec;
  }

  /** Card edit panel (fields after Chat2DB ChartSchema: dimension / measure / color group / sort / display switches).
   *  The change is applied at once and sent to the host as {type:'widgetEdit', widget, patch} to be saved into the spec. */
  function editPanel(w, done) {
    const ds = table(state.data[w.dataset]);
    const cols = ds.columns || [];
    const measures = ((state.spec && state.spec.measures) || []).filter((m) => m && m.id).map((m) => m.id);
    const p = document.createElement('div'); p.className = 'dp-card-tools dp-edit';
    const o = w.options || {}, d = o.display || {};
    const y0 = (w.y && w.y[0]) ? (typeof w.y[0] === 'string' ? { field: w.y[0], agg: w.agg || 'sum' } : w.y[0]) : (w.measure || { field: '*', agg: 'count' });
    const field = (label, el) => { const l = document.createElement('label'); l.textContent = label + ' '; l.appendChild(el); p.appendChild(l); return el; };
    const select = (opts, cur) => { const s = document.createElement('select'); s.innerHTML = opts.map(([v, txt]) => `<option value="${esc(v)}">${esc(txt)}</option>`).join(''); s.value = cur; return s; };
    const isKpi = w.type === 'kpi';
    const xs = isKpi ? null : field(t('dim'), select(cols.map((c) => [c, c]), w.x || cols[0]));
    const ms = field(t('measure'), select([['*', t('count')]].concat(measures.map((m) => [m, 'ƒ ' + m]), cols.map((c) => [c, c])), y0.field));
    const ag = field(t('agg'), select([['sum', t('sum')], ['avg', t('aggAvg')], ['count', t('count')], ['max', t('max')], ['min', t('min')]], y0.agg || 'sum'));
    const ss = isKpi ? null : field(t('series'), select([['', t('noBreak')]].concat(cols.map((c) => [c, c])), w.series || ''));
    const so = isKpi ? null : field(t('sort'), select([['', t('sortAuto')], ['value_desc', t('valueDesc')], ['value_asc', t('valueAsc')], ['dim_asc', t('dimAsc')], ['dim_desc', t('dimDesc')]], o.sort || ''));
    let lim = null;
    if (!isKpi) { lim = document.createElement('input'); lim.type = 'number'; lim.min = 1; lim.max = 400; lim.style.width = '64px'; lim.value = o.limit || ''; field(t('limit'), lim); }
    const checks = {};
    if (!isKpi && w.type !== 'table') {
      [['legend', t('legend')], ['label', t('label')], ['splitLine', t('grid')], ['symbol', t('symbol')]].forEach(([k, txt]) => {
        const c = document.createElement('input'); c.type = 'checkbox'; c.indeterminate = d[k] === undefined; c.checked = d[k] === true;
        c.onchange = () => { c.indeterminate = false; }; checks[k] = c; field(txt, c);
      });
    }
    const ok = document.createElement('button'); ok.className = 'dp-btn'; ok.textContent = t('apply');
    const no = document.createElement('button'); no.className = 'dp-btn'; no.textContent = t('cancel');
    p.appendChild(ok); p.appendChild(no);
    no.onclick = () => done(null);
    ok.onclick = () => {
      const m = ms.value === '*' ? { field: '*', agg: 'count' } : (measures.includes(ms.value) ? { field: ms.value } : { field: ms.value, agg: ag.value });
      const patch = {};
      if (isKpi) patch.measure = m; else { patch.x = xs.value; patch.y = [m]; patch.series = ss.value || null; }
      const opts = Object.assign({}, o);
      if (so) { if (so.value) opts.sort = so.value; else delete opts.sort; }
      if (lim) { if (+lim.value > 0) opts.limit = +lim.value; else delete opts.limit; }
      const disp = Object.assign({}, d);
      Object.entries(checks).forEach(([k, c]) => { if (c.indeterminate) delete disp[k]; else disp[k] = c.checked; });
      if (Object.keys(disp).length) opts.display = disp; else delete opts.display;
      patch.options = opts;
      done(patch);
    };
    return p;
  }
  /** Merge an edit patch into a widget (null clears a key). */
  function applyWidgetPatch(w, patch) {
    for (const [k, v] of Object.entries(patch || {})) { if (v === null) delete w[k]; else w[k] = v; }
    return w;
  }

  /* Display switches (legend / labels / grid lines / axes / point markers).
   * Adapted from Chat2DB Community v5.3.8 `blocks/BI/Chart/chartOptionUtils/handleOption.ts`
   * (LicenseRef-Chat2DB, see Resources/about/CHAT2DB_LICENSE): same switches, rewritten without lodash and
   * as explicit on/off overrides (unset = keep DataPilot's defaults). */
  function applyDisplay(opt, d) {
    if (!opt || !d) return opt;
    const axes = (a) => (Array.isArray(a) ? a : a ? [a] : []);
    const set = (o, k, v) => { o[k] = Object.assign({}, o[k] || {}, v); };
    if (d.legend === true && !opt.legend) opt.legend = { type: 'scroll', top: 0, left: 'center', textStyle: { color: cssv('--ink2') } };
    if (d.legend === false) opt.legend = undefined;
    (opt.series || []).forEach((s) => {
      if (d.label === true) set(s, 'label', { show: true });
      if (d.label === false) set(s, 'label', { show: false });
      if (d.symbol === false && s.type !== 'scatter') { s.showSymbol = false; s.symbolSize = 0; }
      if (d.symbol === true && s.type === 'line') s.showSymbol = true;
    });
    for (const a of axes(opt.xAxis).concat(axes(opt.yAxis))) {
      if (d.splitLine === false) set(a, 'splitLine', { show: false });
      if (d.splitLine === true) set(a, 'splitLine', { show: true, lineStyle: { color: cssv('--grid') } });
      if (d.axisLine === false) { set(a, 'axisLine', { show: false }); set(a, 'axisTick', { show: false }); set(a, 'axisLabel', { show: false }); }
    }
    return opt;
  }

  /** The data behind a card as shown now (filters, cross-filter, card switches applied): grouped rows for charts. */
  function widgetTable(w) {
    const e = effectiveWidget(w);
    const ds = table(state.data[e.dataset]);
    const rows = applyFilters(ds.rows, e.dataset, null);
    if (e.type === 'table') { const cols = (e.columns && e.columns.length ? e.columns : ds.columns); return { columns: cols, rows: rows.map((r) => cols.map((c) => r[c])) }; }
    const ys = e.type === 'kpi' ? [e.measure || (e.y && e.y[0]) || { field: '*', agg: 'count' }] : (e.y || []).map((y) => (typeof y === 'string' ? { field: y, agg: e.agg || 'sum' } : y));
    const ms = ys.length ? ys : [{ field: '*', agg: 'count' }];
    const keys = e.type === 'kpi' ? [] : [e.x, e.series].filter(Boolean);
    const g = groupBy(rows, keys, ms);
    const label = (m) => m.label || (m.field === '*' ? t('count') : m.field) + (measureDef(m.field) || !m.agg || m.agg === 'sum' ? '' : ` (${m.agg})`);
    return { columns: keys.concat(ms.map(label)), rows: g.map((d) => d.key.concat(d.values)) };
  }
  function toCSV(tb) {
    const cell = (v) => { if (v == null) return ''; const s = String(v); return /[",\n\r]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s; };
    return [tb.columns.map(cell).join(',')].concat(tb.rows.map((r) => r.map(cell).join(','))).join('\r\n');
  }
  function exportWidgetCSV(w) {
    const csv = toCSV(widgetTable(w));
    const name = (w.title || w.id || 'data').replace(/[\\/:*?"<>|]/g, '_');
    const inApp = typeof DPHost !== 'undefined' && DPHost.available();
    if (inApp) { host({ type: 'exportCSV', name, csv }); return; }
    const a = document.createElement('a');
    a.href = URL.createObjectURL(new Blob(['\ufeff' + csv], { type: 'text/csv;charset=utf-8' }));
    a.download = name + '.csv'; document.body.appendChild(a); a.click(); setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 0);
  }

  function refresh(skipWidget) {
    const spec = state.spec; if (!spec) return;
    renderCrossChips();
    for (const w of spec.widgets || []) {
      if (w.id === skipWidget && state.charts[w.id]) { /* keep clicked chart, just highlight */ continue; }
      const card = document.querySelector(`[data-wid="${CSS.escape(w.id)}"]`);
      if (card) renderWidget(card, w);
    }
    host({ type: 'filtersChanged', filters: state.filters, cross: state.cross });
  }

  function renderDashboard(spec, data, opts) {
    opts = opts || {};
    state.spec = migrateSpec(spec); state.data = data || {};
    if (!opts.keepState) { state.filters = {}; state.cross = {}; state.widgetState = {}; state.params = {}; }
    for (const p of spec.params || []) if (p && p.id && !(p.id in state.params)) state.params[p.id] = p.default ?? p.value ?? (Array.isArray(p.options) ? p.options[0] : 0);
    prepareData();
    state.chatMode = false; document.body.classList.remove('chat-mode');
    Object.values(state.charts).forEach((c) => c.dispose()); state.charts = {};
    const root = document.getElementById('root'); root.innerHTML = '';
    const head = document.createElement('div'); head.className = 'dp-header';
    head.innerHTML = `<div class="dp-title">${esc(spec.title || 'Dashboard')}</div>`;
    if (!opts.standalone) {
      const act = document.createElement('div'); act.className = 'dp-actions';
      const b1 = document.createElement('button'); b1.textContent = t('exportHtml'); b1.onclick = () => host({ type: 'export', format: 'html', state: snapshotState() });
      const b2 = document.createElement('button'); b2.textContent = t('exportPng'); b2.onclick = () => host({ type: 'export', format: 'png' });
      act.appendChild(b1); act.appendChild(b2); head.appendChild(act);
    }
    root.appendChild(head);
    renderFilters(root);
    const capped = Object.entries(state.data).filter(([, d]) => d && d.capped).map(([k]) => k);
    if (capped.length) { const n = document.createElement('div'); n.className = 'dp-note'; n.style.padding = '0 16px'; n.textContent = `${t('capped')}: ${capped.join(', ')}`; root.appendChild(n); }
    const grid = document.createElement('div'); grid.className = 'dp-grid'; root.appendChild(grid);
    const layout = {}; (spec.layout || []).forEach((l) => { layout[l.i] = l; });
    for (const w of spec.widgets || []) {
      const l = layout[w.id] || { w: w.type === 'kpi' ? 3 : 6, h: w.type === 'kpi' ? 2 : 5 };
      const card = document.createElement('div'); card.className = 'dp-card'; card.dataset.wid = w.id;
      card.style.gridColumn = l.x != null ? `${l.x + 1} / span ${l.w}` : `span ${l.w}`;
      card.style.gridRow = l.y != null ? `${l.y + 1} / span ${l.h}` : `span ${l.h}`;
      card.appendChild(cardHeader(w));
      const body = document.createElement('div'); body.className = 'dp-card-b'; card.appendChild(body);
      grid.appendChild(card);
    }
    refresh();
    host({ type: 'rendered', widgets: (spec.widgets || []).length });
  }

  function renderChart(payload, opts) {
    opts = opts || {};
    state.chatMode = true; document.body.classList.add('chat-mode');
    const s = payload.spec || {};
    const w = { id: 'chat', type: s.type || 'bar', title: s.title, x: s.x, y: s.y, series: s.series, options: s.options || {}, format: (s.options || {}).format, dataset: 'd', typeOptions: opts.typeOptions };
    renderDashboard({ title: '', widgets: [w], layout: [{ i: 'chat', w: 12, h: 5 }] }, { d: payload.data }, opts);
    document.body.classList.add('chat-mode');
    const hdr = document.querySelector('.dp-header'); if (hdr) hdr.remove();
    const f = document.querySelector('.dp-filters'); if (f && !f.children.length) f.remove();
  }

  function snapshotState() { return { filters: state.filters, cross: state.cross, widgetState: state.widgetState }; }
  function setTheme(th) {
    state.theme = th === 'dark' ? 'dark' : 'light';
    document.documentElement.dataset.theme = state.theme;
    if (state.spec) { if (state.chatMode) refresh(); else renderDashboard(state.spec, state.data, { keepState: true }); }
  }
  function setPalette(p) {
    state.palette = p || 'default';
    if (state.spec) { if (state.chatMode) refresh(); else renderDashboard(state.spec, state.data, { keepState: true }); }
  }
  function setLang(l) { state.lang = l && l.startsWith('en') ? 'en' : 'zh'; }
  /** Whole dashboard → PNG data URL (Promise): title, active filters and every card at its on-screen position.
   *  Charts are copied from ECharts at 2×; KPI / table / text cards are redrawn as text. */
  async function exportPNG() {
    const root = document.getElementById('root');
    if (!root) return null;
    const R = 2, rr = root.getBoundingClientRect();
    const W = Math.ceil(root.scrollWidth), H = Math.ceil(root.scrollHeight);
    const cv = document.createElement('canvas'); cv.width = W * R; cv.height = H * R;
    const g = cv.getContext('2d'); g.scale(R, R);
    const font = 'system-ui,-apple-system,"PingFang SC","Microsoft YaHei",sans-serif';
    const pos = (el) => { const b = el.getBoundingClientRect(); return { x: b.left - rr.left + root.scrollLeft, y: b.top - rr.top + root.scrollTop, w: b.width, h: b.height }; };
    const rrect = (x, y, w, h, r) => { g.beginPath(); g.moveTo(x + r, y); g.arcTo(x + w, y, x + w, y + h, r); g.arcTo(x + w, y + h, x, y + h, r); g.arcTo(x, y + h, x, y, r); g.arcTo(x, y, x + w, y, r); g.closePath(); };
    const text = (el, color, weight, maxW) => {
      if (!el || !el.textContent.trim()) return;
      const p = pos(el), cs = getComputedStyle(el);
      g.fillStyle = color || cs.color; g.font = `${weight || cs.fontWeight} ${cs.fontSize} ${font}`; g.textBaseline = 'middle';
      let t = el.textContent.trim(); const mw = maxW || p.w + 2;
      while (t.length > 1 && g.measureText(t).width > mw) t = t.slice(0, -2) + '…';
      g.fillText(t, p.x, p.y + p.h / 2);
    };
    const img = (url) => new Promise((ok) => { const i = new Image(); i.onload = () => ok(i); i.onerror = () => ok(null); i.src = url; });
    g.fillStyle = cssv('--page') || '#fff'; g.fillRect(0, 0, W, H);
    text(root.querySelector('.dp-title'), cssv('--ink'), 600);
    // active filters / cross-filter chips as one line of text
    const bar = root.querySelector('.dp-filters');
    if (bar) {
      const parts = [];
      bar.querySelectorAll('.dp-filter').forEach((f) => {
        const lab = (f.querySelector('label,span') || {}).textContent || '';
        const sel = f.querySelector('select'), inp = f.querySelector('input'), ms = f.querySelector('.dp-ms-btn');
        const v = sel ? (sel.options[sel.selectedIndex] || {}).text : ms ? ms.textContent : inp ? inp.value : '';
        if (v) parts.push(`${lab.trim()} ${v}`.trim());
      });
      bar.querySelectorAll('.dp-chip').forEach((c) => parts.push(c.textContent));
      if (parts.length) { const first = bar.querySelector('.dp-filter,.dp-chip'); const p = pos(first || bar); g.fillStyle = cssv('--ink2'); g.font = `12px ${font}`; g.textBaseline = 'middle'; g.fillText(parts.join('   ·   '), p.x, p.y + p.h / 2); }
    }
    const surface = cssv('--surface'), border = cssv('--axis') || '#ccc';
    for (const card of root.querySelectorAll('.dp-card')) {
      const p = pos(card);
      rrect(p.x + 0.5, p.y + 0.5, p.w - 1, p.h - 1, 10); g.fillStyle = surface; g.fill(); g.strokeStyle = border; g.lineWidth = 1; g.globalAlpha = 0.5; g.stroke(); g.globalAlpha = 1;
      const tt = card.querySelector('.dp-card-t'); if (tt) { const tp = pos(tt); text(tt, cssv('--ink'), 600, p.x + p.w - tp.x - 12); }
      const body = card.querySelector('.dp-card-b'); if (!body) continue;
      const b = pos(body);
      g.save(); g.beginPath(); g.rect(b.x, b.y, b.w, b.h); g.clip();
      const chart = state.charts[card.dataset.wid];
      if (chart) {
        const el = chart.getDom(), cp = pos(el);
        const im = await img(chart.getDataURL({ pixelRatio: R, backgroundColor: surface }));
        if (im) g.drawImage(im, cp.x, cp.y, cp.w, cp.h);
      } else if (body.querySelector('.dp-kpi')) {
        text(body.querySelector('.dp-kpi-v'), cssv('--ink'), 600);
        text(body.querySelector('.dp-kpi-s'), cssv('--ink2'));
      } else if (body.querySelector('table')) {
        const rows = body.querySelectorAll('tr');
        for (const tr of rows) {
          const rp = pos(tr); if (rp.y > b.y + b.h) break;
          for (const td of tr.children) text(td, td.tagName === 'TH' ? cssv('--ink2') : cssv('--ink'), td.tagName === 'TH' ? 600 : 400);
          g.fillStyle = cssv('--grid'); g.fillRect(rp.x, rp.y + rp.h - 0.5, rp.w, 0.5);
        }
      } else {
        // text / note cards: draw every text run where the browser laid it out (wrapped lines handled per character)
        const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
        const rng = document.createRange(); let budget = 4000;
        for (let n = walker.nextNode(); n && budget > 0; n = walker.nextNode()) {
          const str = n.textContent; if (!str.trim()) continue;
          const cs = getComputedStyle(n.parentElement);
          g.fillStyle = cs.color; g.font = `${cs.fontWeight} ${cs.fontSize} ${font}`; g.textBaseline = 'middle';
          rng.selectNodeContents(n);
          const rects = rng.getClientRects();
          if (rects.length === 1) { const r = rects[0]; g.fillText(str.trim() === str ? str : str.replace(/^\s+/, ''), r.left - rr.left + root.scrollLeft, r.top - rr.top + root.scrollTop + r.height / 2); budget -= str.length; continue; }
          let line = '', lx = 0, ly = null, lh = 0;
          const flush = () => { if (line.trim()) g.fillText(line, lx, ly + lh / 2); line = ''; };
          for (let i = 0; i < str.length && budget > 0; i++, budget--) {
            rng.setStart(n, i); rng.setEnd(n, i + 1);
            const r = rng.getBoundingClientRect(); if (!r.width && str[i] !== ' ') continue;
            const y = r.top - rr.top + root.scrollTop;
            if (ly === null || Math.abs(y - ly) > r.height / 2) { flush(); lx = r.left - rr.left + root.scrollLeft; ly = y; lh = r.height; }
            line += str[i];
          }
          flush();
        }
      }
      g.restore();
    }
    return cv.toDataURL('image/png');
  }
  let resizeTimer = null, lastFS = FS;
  function onResize() {
    Object.values(state.charts).forEach((c) => c.resize());
    fitKPI();
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      const before = lastFS; lastFS = computeScale();
      if (Math.abs(lastFS - before) > 0.04 && state.spec) refresh();   // re-apply scaled fonts
    }, 180);
  }
  window.addEventListener('resize', onResize);
  if (window.ResizeObserver) new ResizeObserver(onResize).observe(document.documentElement);

  window.DP = { renderDashboard, renderChart, setTheme, setPalette, PALETTES, setLang, exportPNG, snapshotState, _buildOption: buildOption, _effectiveWidget: effectiveWidget, _applyCalc: applyCalc, _refValue: refValue, _groupBy: groupBy, _applyFilters: applyFilters, _state: state, _widgetTable: widgetTable, _toCSV: toCSV, _applyDisplay: applyDisplay, _prepareData: prepareData, _migrateSpec: migrateSpec, _sortGroups: sortGroups, _applyWidgetPatch: applyWidgetPatch };
  host({ type: 'ready' });
})();
