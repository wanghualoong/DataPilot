// node tests/js/test_expr.js — safe expression engine (no eval)
const E = require('../../Resources/web/expr.js');
let fails = 0;
const ok = (c, m) => { console.log((c ? 'PASS ' : 'FAIL ') + m); if (!c) fails++; };
const row = { 销售额: 12000, 成本: 7000, 客户: 'A', 下单日期: '2025-03-15', q: null };
ok(E.evalRow('销售额 - 成本', row) === 5000, 'subtract');
ok(E.evalRow('1 + 2 * 3', {}) === 7, 'precedence');
ok(E.evalRow('(1 + 2) * 3', {}) === 9, 'parens');
ok(E.evalRow("if(销售额 > 10000, '大客户', '普通')", row) === '大客户', 'if');
ok(E.evalRow("case when 销售额 > 20000 then 'A' when 销售额 > 10000 then 'B' else 'C' end", row) === 'B', 'case');
ok(E.evalRow('销售额 / 0', row) === null, 'div by zero -> null');
ok(E.evalRow('q + 1', row) === null, 'null propagates');
ok(E.evalRow('coalesce(q, 5)', row) === 5, 'coalesce');
ok(E.evalRow('round(成本 / 销售额, 3)', row) === 0.583, 'round');
ok(E.evalRow('year(下单日期)', row) === 2025 && E.evalRow('quarter(下单日期)', row) === 1, 'date parts');
ok(E.evalRow("客户 || '-' || 'x'", row) === 'A-x', 'concat');
ok(E.evalRow('销售额 > 1000 and not (成本 > 9000)', row) === true, 'logic');
ok(E.evalRow('"销售额" * 增长率', row, { 增长率: 0.1 }) === 1200, 'params + quoted id');
const rows = [{ g: 10, s: 100 }, { g: 30, s: 100 }];
ok(E.evalAgg('sum(g) / sum(s)', rows) === 0.2, 'ratio measure after aggregation');
ok(E.evalAgg('count(*)', rows) === 2 && E.evalAgg('avg(g)', rows) === 20, 'count/avg');
let threw = false; try { E.evalRow("constructor.constructor('return 1')()", {}); } catch (e) { threw = true; }
ok(threw, 'no code execution');
threw = false; try { E.compile('1 +'); } catch (e) { threw = true; } ok(threw, 'syntax error reported');
const probs = E.validate({ calcFields: [{ name: '毛利', expr: '销售额 - 成本' }, { name: 'bad', expr: '不存在 + 1', dataset: 'd1' }],
  measures: [{ id: '毛利率', expr: 'sum(毛利)/sum(销售额)' }, { id: 'm2', expr: '毛利 * 2' }] },
  { d1: { columns: ['销售额', '成本'], rows: [[100, 60]] } });
ok(probs.length === 2 && probs.some((p) => p.includes('不存在')) && probs.some((p) => p.includes('m2')), 'validate: ' + JSON.stringify(probs));
const src = require('fs').readFileSync(__dirname + '/../../Resources/web/expr.js', 'utf8');
ok(!/\beval\s*\(|new Function/.test(src), 'source has no eval/Function');
console.log(`\n${fails} failures`); process.exit(fails ? 1 : 0);
