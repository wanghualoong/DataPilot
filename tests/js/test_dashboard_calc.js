// node tests/js/test_dashboard_calc.js — calcFields / measures / CSV in dashboard.js (DOM stubbed)
const fs = require('fs'), vm = require('vm');
const el = () => ({ style: { setProperty() {} }, classList: { add() {}, remove() {}, toggle() {} }, appendChild() {}, querySelector() { return null; }, querySelectorAll() { return []; }, dataset: {}, innerHTML: '' });
const ctx = { console, Math, JSON, Object, Array, Set, Map, String, Number, isFinite, parseFloat, setTimeout, clearTimeout,
  document: { documentElement: Object.assign(el(), { clientWidth: 800, dataset: {} }), addEventListener() {}, createElement: el, getElementById: () => null, querySelector: () => null, querySelectorAll: () => [], body: el() },
  getComputedStyle: () => ({ getPropertyValue: () => '#000' }), addEventListener() {} };
ctx.window = ctx; ctx.globalThis = ctx;
vm.createContext(ctx);
vm.runInContext(fs.readFileSync(__dirname + '/../../Resources/web/expr.js', 'utf8'), ctx);
vm.runInContext(fs.readFileSync(__dirname + '/../../Resources/web/dashboard.js', 'utf8'), ctx);
const DP = ctx.DP; let fails = 0;
const ok = (c, m) => { console.log((c ? 'PASS ' : 'FAIL ') + m); if (!c) fails++; };
DP._state.spec = { calcFields: [{ name: '毛利', expr: '销售额 - 成本' }], measures: [{ id: '毛利率', expr: 'sum(毛利)/sum(销售额)', format: 'pct' }],
  widgets: [{ id: 'w', type: 'bar', dataset: 'd', x: '区', y: [{ field: '毛利率' }, { field: '毛利', agg: 'sum' }] }] };
DP._state.data = { d: { columns: ['区', '销售额', '成本'], rows: [['A', 100, 60], ['A', 300, 120], ['B', 200, 190]] } };
DP._state.params = {};
DP._prepareData();
ok(DP._state.data.d.columns.includes('毛利'), 'calc field column added');
const g = DP._groupBy(DP._state.data.d._objs, ['区'], [{ field: '毛利率' }, { field: '毛利', agg: 'sum' }]);
const a = g.find((x) => x.key[0] === 'A');
ok(Math.abs(a.values[0] - 0.55) < 1e-9 && a.values[1] === 220, 'ratio computed after grouping (A: 220/400=0.55, not avg of ratios): ' + JSON.stringify(a.values));
const tb = DP._widgetTable(DP._state.spec.widgets[0]);
ok(tb.rows.length === 2 && tb.columns[0] === '区', 'widget table: ' + JSON.stringify(tb));
ok(DP._toCSV({ columns: ['a', 'b'], rows: [['x,y', 'q"'], [null, 1]] }) === 'a,b\r\n"x,y","q"""\r\n,1', 'csv quoting');
const opt = DP._applyDisplay({ series: [{ type: 'bar' }], xAxis: {}, yAxis: {} }, { label: true, splitLine: false, legend: true });
ok(opt.series[0].label.show === true && opt.yAxis.splitLine.show === false && !!opt.legend, 'display switches');
// spec migration v1 → v2, explicit sort, widget patch
const sp = DP._migrateSpec({ widgets: [{ id: 'a', type: 'bar', y: ['金额'], agg: 'avg', options: { legend: false } }] });
ok(sp.version === 2 && sp.widgets[0].y[0].field === '金额' && sp.widgets[0].y[0].agg === 'avg' && sp.widgets[0].options.display.legend === false && !('legend' in sp.widgets[0].options), 'spec v1 → v2 migration');
ok(DP._migrateSpec({ version: 2, widgets: [] }).version === 2, 'v2 spec unchanged');
const gg = [{ key: ['b'], values: [1] }, { key: ['a'], values: [3] }, { key: ['c'], values: [2] }];
ok(DP._sortGroups(gg.slice(), 'value_asc').map((d) => d.key[0]).join('') === 'bca', 'sort by value asc');
ok(DP._sortGroups(gg.slice(), 'dim_asc').map((d) => d.key[0]).join('') === 'abc', 'sort by dimension');
const wv = DP._applyWidgetPatch({ id: 'w', x: '区', series: '品类' }, { x: '月份', series: null, options: { sort: 'value_desc' } });
ok(wv.x === '月份' && !('series' in wv) && wv.options.sort === 'value_desc', 'widget patch (null clears)');
console.log(`\n${fails} failures`); process.exit(fails ? 1 : 0);
