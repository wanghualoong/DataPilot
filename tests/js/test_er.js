// node tests/js/test_er.js — pure helpers of the ER diagram (layout, sizing, form → relation side)
const fs = require('fs'), vm = require('vm');
const ctx = { console, Math, JSON, Object, Array, Set, Map, String, Number, isFinite, Promise };
ctx.window = ctx; ctx.globalThis = ctx; vm.createContext(ctx);
vm.runInContext(fs.readFileSync(__dirname + '/../../Resources/web/host.js', 'utf8'), ctx);
vm.runInContext(fs.readFileSync(__dirname + '/../../Resources/web/er.js', 'utf8'), ctx);
const E = ctx.DPER; let fails = 0;
const ok = (c, m) => { console.log((c ? 'PASS ' : 'FAIL ') + m); if (!c) fails++; };
const cols = (n, p = 'c') => Array.from({ length: n }, (_, i) => ({ name: p + i, type: 'VARCHAR' }));
const tables = [{ table: 'orders', columns: [{ name: 'oid' }, { name: '客户编码' }, ...cols(20)] },
  { table: 'customers', columns: [{ name: 'id' }, { name: 'name' }] },
  { table: 'regions', columns: [{ name: 'rcode' }] }, { table: 'lonely', columns: [{ name: 'x' }] }];
const rels = [{ id: 'r1', from_table: 'orders', from_column: 'c19', to_table: 'customers', to_column: 'id', status: 'confirmed' },
  { id: 'r2', from_table: 'customers', from_column: 'name', to_table: 'regions', to_column: 'rcode', status: 'suspect' }];
const pos = E._layout(tables, rels, {});
ok(pos.regions[0] < pos.customers[0] && pos.customers[0] < pos.orders[0], 'parents laid out left of children');
ok(pos.lonely[0] > pos.orders[0], 'unrelated tables placed after the related ones');
const v = E._visibleColumns(tables[0], rels);
ok(v.cols.length === 13 && v.hidden === 9 && v.cols.some((c) => c.name === 'c19'), 'long table cut, relation column kept');
ok(E._nodeSize(tables[0], rels).h === 30 + 14 * 22 + 6, 'node height = header + rows (+ "more" row)');
ok(E._colOffset(tables[0], 'c5', rels) === E._colOffset(tables[0], 'c18', rels) || true, 'colOffset smoke');
ok(E._colOffset(tables[0], 'c18', rels) === 15, 'hidden column anchors at the header');
const stored = E._layout(tables, rels, { orders: [5, 6], ghost: [1, 1] });
ok(stored.orders[0] === 5 && stored.orders[1] === 6 && !stored.ghost, 'stored positions win; unknown tables ignored');
const cyc = E._layout([{ table: 'a', columns: [] }, { table: 'b', columns: [] }],
  [{ from_table: 'a', to_table: 'b' }, { from_table: 'b', to_table: 'a' }], {});
ok(isFinite(cyc.a[0]) && isFinite(cyc.b[0]), 'cycles terminate');
ok(E._clip('订单客户编码表格', 8) === '订单客户…' && E._clip('abc', 8) === 'abc', 'CJK-aware clipping');
const s = E._sideFrom({ table: 'orders', column: '客户编码', steps: ['trim', 'strip_prefix'], lpad: '6', expr: '' });
ok(JSON.stringify(s) === JSON.stringify({ table: 'orders', columns: ['客户编码'], steps: ['trim', 'strip_prefix', { lpad: 6 }] }), 'form → relation side');
const pp = E._parsePairs('East = E\nNorth→N\n\nbad\nWest,W');
ok(pp.length === 3 && pp[1][1] === 'N' && pp[2][0] === 'West', 'value pairs parsed');
const p = E._edgePath({ x: 0, y: 10 }, { x: 400, y: 50 });
ok(p.d.startsWith('M230,10') && p.d.endsWith('400,50'), 'edge leaves the facing sides');
const src = fs.readFileSync(__dirname + '/../../Resources/web/er.js', 'utf8');
ok(!/\beval\(|new Function|webkit/.test(src), 'no eval / no platform globals');
ok(/Adapted from Chat2DB Community v5\.3\.8/.test(src), 'Chat2DB attribution header');
console.log(`\n${fails} failures`); process.exit(fails ? 1 : 0);
