/* DataPilot safe expression engine (no eval / Function).
 * Used by the dashboard renderer (calculated fields, ratio measures, parameters) and by the app
 * (JavaScriptCore) to validate expressions when a dashboard is created — one implementation for both.
 *
 * Row expressions:      销售额 - 成本 · if(销售额 > 10000, '大客户', '普通') · round(a / b, 2) · year(下单日期)
 *                       case when x > 0 then 'up' when x < 0 then 'down' else 'flat' end
 * Aggregate (measures): sum(毛利) / sum(销售额) · count_distinct(客户) · avg(单价) * 1.1
 * Identifiers: bare names (letters, digits, _, CJK), or quoted "col name" / [col name] / `col name`.
 * Strings: 'text'. null propagates through arithmetic; x / 0 → null.
 */
(function (root) {
  'use strict';
  const AGG = new Set(['sum', 'avg', 'count', 'count_distinct', 'min', 'max', 'median']);
  const KW = new Set(['and', 'or', 'not', 'case', 'when', 'then', 'else', 'end', 'null', 'true', 'false']);

  function tokenize(src) {
    const out = []; let i = 0; const s = String(src);
    const isIdStart = (c) => /[A-Za-z_À-￿]/.test(c);
    const isId = (c) => /[A-Za-z0-9_À-￿]/.test(c);
    while (i < s.length) {
      const c = s[i];
      if (/\s/.test(c)) { i++; continue; }
      if (/[0-9]/.test(c) || (c === '.' && /[0-9]/.test(s[i + 1] || ''))) {
        let j = i; while (j < s.length && /[0-9._eE]/.test(s[j])) { if ((s[j] === 'e' || s[j] === 'E') && /[+-]/.test(s[j + 1] || '')) j++; j++; }
        out.push({ t: 'num', v: parseFloat(s.slice(i, j).replace(/_/g, '')) }); i = j; continue;
      }
      if (c === "'") {
        let j = i + 1, v = '';
        while (j < s.length) { if (s[j] === "'" && s[j + 1] === "'") { v += "'"; j += 2; continue; } if (s[j] === "'") break; v += s[j++]; }
        if (j >= s.length) throw new Error('unterminated string');
        out.push({ t: 'str', v }); i = j + 1; continue;
      }
      if (c === '"' || c === '`' || c === '[') {
        const close = c === '[' ? ']' : c; const j = s.indexOf(close, i + 1);
        if (j < 0) throw new Error('unterminated identifier');
        out.push({ t: 'id', v: s.slice(i + 1, j) }); i = j + 1; continue;
      }
      if (isIdStart(c)) {
        let j = i; while (j < s.length && isId(s[j])) j++;
        const w = s.slice(i, j), lw = w.toLowerCase();
        out.push(KW.has(lw) ? { t: 'kw', v: lw } : { t: 'id', v: w }); i = j; continue;
      }
      const two = s.slice(i, i + 2);
      if (['<=', '>=', '!=', '<>', '==', '||', '&&'].includes(two)) { out.push({ t: 'op', v: two === '&&' ? 'and' : two === '<>' ? '!=' : two === '==' ? '=' : two }); i += 2; continue; }
      if ('+-*/%<>=(),!'.includes(c)) { out.push({ t: 'op', v: c === '!' ? 'not' : c }); i++; continue; }
      throw new Error(`unexpected character '${c}'`);
    }
    return out;
  }

  // Pratt parser → AST
  function parse(src) {
    const tk = tokenize(src); let p = 0;
    const peek = () => tk[p]; const next = () => tk[p++];
    const isOp = (v) => tk[p] && (tk[p].t === 'op' || tk[p].t === 'kw') && tk[p].v === v;
    const expect = (v) => { if (!isOp(v)) throw new Error(`expected '${v}'${tk[p] ? ` near '${tk[p].v}'` : ' at end'}`); p++; };
    const BP = { or: 1, and: 2, '=': 4, '!=': 4, '<': 4, '<=': 4, '>': 4, '>=': 4, '||': 5, '+': 6, '-': 6, '*': 7, '/': 7, '%': 7 };
    function primary() {
      const t = next();
      if (!t) throw new Error('unexpected end of expression');
      if (t.t === 'num') return { k: 'lit', v: t.v };
      if (t.t === 'str') return { k: 'lit', v: t.v };
      if (t.t === 'kw') {
        if (t.v === 'null') return { k: 'lit', v: null };
        if (t.v === 'true' || t.v === 'false') return { k: 'lit', v: t.v === 'true' };
        if (t.v === 'not') return { k: 'un', op: 'not', a: expr(3) };
        if (t.v === 'case') {
          const whens = []; let els = { k: 'lit', v: null };
          while (isOp('when')) { p++; const c = expr(0); expect('then'); whens.push([c, expr(0)]); }
          if (isOp('else')) { p++; els = expr(0); }
          expect('end');
          if (!whens.length) throw new Error('case needs at least one when');
          return { k: 'case', whens, els };
        }
        throw new Error(`unexpected '${t.v}'`);
      }
      if (t.t === 'op' && t.v === '(') { const e = expr(0); expect(')'); return e; }
      if (t.t === 'op' && t.v === '-') return { k: 'un', op: '-', a: expr(8) };
      if (t.t === 'op' && t.v === '+') return expr(8);
      if (t.t === 'op' && t.v === 'not') return { k: 'un', op: 'not', a: expr(3) };
      if (t.t === 'id') {
        if (isOp('(')) {
          p++; const args = [];
          if (!isOp(')')) { do { if (t.v.toLowerCase() === 'count' && isOp('*')) { p++; args.push({ k: 'lit', v: 1 }); } else args.push(expr(0)); } while (isOp(',') && ++p); }
          expect(')');
          return { k: 'call', fn: t.v.toLowerCase(), args };
        }
        return { k: 'id', name: t.v };
      }
      throw new Error(`unexpected '${t.v}'`);
    }
    function expr(minBp) {
      let left = primary();
      for (;;) {
        const t = peek(); if (!t || !(t.t === 'op' || t.t === 'kw')) break;
        const bp = BP[t.v]; if (bp == null || bp <= minBp) break;
        p++; left = { k: 'bin', op: t.v, a: left, b: expr(bp) };
      }
      return left;
    }
    const ast = expr(0);
    if (p < tk.length) throw new Error(`unexpected '${tk[p].v}'`);
    return ast;
  }

  const num = (v) => (v == null || v === '' ? null : typeof v === 'number' ? v : typeof v === 'boolean' ? +v : isFinite(+v) ? +v : null);
  const truthy = (v) => v != null && v !== false && v !== 0 && v !== '';
  function cmp(op, a, b) {
    if (a == null || b == null) return op === '=' ? a == b : op === '!=' ? a != b : null;  // eslint-disable-line eqeqeq
    const na = num(a), nb = num(b);
    const [x, y] = na != null && nb != null && typeof a !== 'string' || (na != null && nb != null && typeof b !== 'string') ? [na, nb] : [String(a), String(b)];
    switch (op) { case '=': return x === y; case '!=': return x !== y; case '<': return x < y; case '<=': return x <= y; case '>': return x > y; default: return x >= y; }
  }
  function dateParts(v) {
    if (v == null) return null;
    const m = /^(\d{4})-(\d{2})(?:-(\d{2}))?/.exec(String(v)); if (!m) return null;
    return { y: +m[1], m: +m[2], d: +(m[3] || 1) };
  }
  const ROW_FN = {
    if: (a) => (truthy(a[0]) ? a[1] : a[2] === undefined ? null : a[2]),
    coalesce: (a) => { for (const v of a) if (v != null && v !== '') return v; return null; },
    ifnull: (a) => (a[0] == null || a[0] === '' ? a[1] : a[0]),
    nullif: (a) => (a[0] == a[1] ? null : a[0]),  // eslint-disable-line eqeqeq
    round: (a) => { const x = num(a[0]); if (x == null) return null; const d = Math.pow(10, num(a[1]) || 0); return Math.round(x * d) / d; },
    floor: (a) => { const x = num(a[0]); return x == null ? null : Math.floor(x); },
    ceil: (a) => { const x = num(a[0]); return x == null ? null : Math.ceil(x); },
    abs: (a) => { const x = num(a[0]); return x == null ? null : Math.abs(x); },
    sqrt: (a) => { const x = num(a[0]); return x == null || x < 0 ? null : Math.sqrt(x); },
    pow: (a) => { const x = num(a[0]), y = num(a[1]); return x == null || y == null ? null : Math.pow(x, y); },
    least: (a) => { const v = a.map(num).filter((x) => x != null); return v.length ? Math.min(...v) : null; },
    greatest: (a) => { const v = a.map(num).filter((x) => x != null); return v.length ? Math.max(...v) : null; },
    concat: (a) => a.map((v) => (v == null ? '' : String(v))).join(''),
    lower: (a) => (a[0] == null ? null : String(a[0]).toLowerCase()),
    upper: (a) => (a[0] == null ? null : String(a[0]).toUpperCase()),
    trim: (a) => (a[0] == null ? null : String(a[0]).trim()),
    length: (a) => (a[0] == null ? null : String(a[0]).length),
    substr: (a) => (a[0] == null ? null : String(a[0]).substr(Math.max(0, (num(a[1]) || 1) - 1), a[2] == null ? undefined : num(a[2]))),
    contains: (a) => (a[0] == null || a[1] == null ? null : String(a[0]).includes(String(a[1]))),
    year: (a) => { const d = dateParts(a[0]); return d ? d.y : null; },
    month: (a) => { const d = dateParts(a[0]); return d ? d.m : null; },
    day: (a) => { const d = dateParts(a[0]); return d ? d.d : null; },
    quarter: (a) => { const d = dateParts(a[0]); return d ? Math.ceil(d.m / 3) : null; },
    yearmonth: (a) => { const d = dateParts(a[0]); return d ? `${d.y}-${String(d.m).padStart(2, '0')}` : null; },
    number: (a) => num(a[0]),
    text: (a) => (a[0] == null ? null : String(a[0])),
  };
  ROW_FN.iif = ROW_FN.if;
  ROW_FN.substring = ROW_FN.substr;

  function aggregate(fn, vals) {
    if (fn === 'count') return vals.filter((v) => v != null).length;
    if (fn === 'count_distinct') return new Set(vals.filter((v) => v != null).map(String)).size;
    const n = vals.map(num).filter((v) => v != null);
    if (!n.length) return fn === 'sum' ? 0 : null;
    switch (fn) {
      case 'sum': return n.reduce((s, v) => s + v, 0);
      case 'avg': return n.reduce((s, v) => s + v, 0) / n.length;
      case 'min': return Math.min(...n);
      case 'max': return Math.max(...n);
      default: { const s = n.slice().sort((x, y) => x - y); const m = s.length >> 1; return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2; }
    }
  }

  /** env: {row?, rows?, params?, lookup?(name)} — `rows` set = aggregate context. */
  function evaluate(n, env) {
    switch (n.k) {
      case 'lit': return n.v;
      case 'id': {
        if (env.params && Object.prototype.hasOwnProperty.call(env.params, n.name)) return env.params[n.name];
        if (env.row) { if (!(n.name in env.row) && env.strict) throw new Error(`unknown field '${n.name}'`); return env.row[n.name] ?? null; }
        throw new Error(`field '${n.name}' must be inside an aggregate such as sum(${n.name})`);
      }
      case 'un': { const v = evaluate(n.a, env); if (n.op === 'not') return v == null ? null : !truthy(v); const x = num(v); return x == null ? null : -x; }
      case 'case': { for (const [c, v] of n.whens) if (truthy(evaluate(c, env))) return evaluate(v, env); return evaluate(n.els, env); }
      case 'bin': {
        if (n.op === 'and') { const a = evaluate(n.a, env); if (a != null && !truthy(a)) return false; const b = evaluate(n.b, env); return a == null || b == null ? (b != null && !truthy(b) ? false : null) : truthy(b); }
        if (n.op === 'or') { const a = evaluate(n.a, env); if (truthy(a)) return true; const b = evaluate(n.b, env); return truthy(b) ? true : (a == null || b == null ? null : false); }
        const a = evaluate(n.a, env), b = evaluate(n.b, env);
        if (n.op === '||') return (a == null ? '' : String(a)) + (b == null ? '' : String(b));
        if (['=', '!=', '<', '<=', '>', '>='].includes(n.op)) return cmp(n.op, a, b);
        if (n.op === '+' && (typeof a === 'string' && num(a) == null || typeof b === 'string' && num(b) == null)) return a == null || b == null ? null : String(a) + String(b);
        const x = num(a), y = num(b); if (x == null || y == null) return null;
        switch (n.op) { case '+': return x + y; case '-': return x - y; case '*': return x * y; case '/': return y === 0 ? null : x / y; default: return y === 0 ? null : x % y; }
      }
      case 'call': {
        if (AGG.has(n.fn) && env.rows) {
          if (n.fn === 'count' && !n.args.length) return env.rows.length;
          const arg = n.args[0]; if (!arg) throw new Error(`${n.fn}() needs a field`);
          return aggregate(n.fn, env.rows.map((r) => evaluate(arg, { row: r, params: env.params, strict: env.strict })));
        }
        if (AGG.has(n.fn) && !env.rows && (n.fn === 'min' || n.fn === 'max') && n.args.length > 1) {
          return ROW_FN[n.fn === 'min' ? 'least' : 'greatest'](n.args.map((a) => evaluate(a, env)));
        }
        if (AGG.has(n.fn)) throw new Error(`aggregate ${n.fn}() is only allowed in measures`);
        const f = ROW_FN[n.fn]; if (!f) throw new Error(`unknown function ${n.fn}()`);
        return f(n.args.map((a) => evaluate(a, env)));
      }
    }
    return null;
  }

  function fields(n, out = new Set()) {
    if (!n) return out;
    if (n.k === 'id') out.add(n.name);
    if (n.a) fields(n.a, out); if (n.b) fields(n.b, out);
    (n.args || []).forEach((a) => fields(a, out));
    (n.whens || []).forEach(([c, v]) => { fields(c, out); fields(v, out); });
    if (n.els) fields(n.els, out);
    return out;
  }
  function hasAgg(n) {
    if (!n) return false;
    if (n.k === 'call' && AGG.has(n.fn) && !((n.fn === 'min' || n.fn === 'max') && n.args.length > 1)) return true;
    return hasAgg(n.a) || hasAgg(n.b) || (n.args || []).some(hasAgg) || (n.whens || []).some(([c, v]) => hasAgg(c) || hasAgg(v)) || hasAgg(n.els);
  }

  const cache = new Map();
  function compile(src) {
    let c = cache.get(src);
    if (!c) { const ast = parse(src); c = { ast, fields: [...fields(ast)], aggregate: hasAgg(ast) }; cache.set(src, c); if (cache.size > 500) cache.delete(cache.keys().next().value); }
    return c;
  }
  const evalRow = (src, row, params) => evaluate(compile(src).ast, { row, params });
  const evalAgg = (src, rows, params) => evaluate(compile(src).ast, { rows, params });

  /** Validate a dashboard's calcFields / measures / params against dataset columns + sample rows.
   * spec: {calcFields?:[{name,expr,dataset?}], measures?:[{id,expr,dataset?}], params?:[{id,default}]}
   * data: {datasetId: {columns:[...], rows:[[...]] (a sample is enough)}}. Returns [] or a list of problems. */
  function validate(spec, data) {
    const probs = [];
    const params = {}; (spec.params || []).forEach((p) => { if (!p.id) probs.push('param without id'); else params[p.id] = p.default ?? p.value ?? 0; });
    const dsIds = Object.keys(data || {});
    const cols = {}; dsIds.forEach((d) => { cols[d] = new Set((data[d] && data[d].columns) || []); });
    const sample = (d) => { const ds = data[d] || {}; const cs = ds.columns || []; return (ds.rows || []).slice(0, 200).map((r) => { const o = {}; cs.forEach((c, i) => { o[c] = Array.isArray(r) ? r[i] : r[c]; }); return o; }); };
    const targets = (x) => (x.dataset ? [x.dataset] : dsIds);
    for (const cf of spec.calcFields || []) {
      if (!cf.name || !cf.expr) { probs.push(`calcField needs name and expr: ${JSON.stringify(cf)}`); continue; }
      let c; try { c = compile(cf.expr); } catch (e) { probs.push(`calcField '${cf.name}': ${e.message}`); continue; }
      if (c.aggregate) { probs.push(`calcField '${cf.name}' uses an aggregate; put it in measures instead`); continue; }
      for (const d of targets(cf)) {
        if (!cols[d]) { probs.push(`calcField '${cf.name}': unknown dataset ${d}`); continue; }
        const miss = c.fields.filter((f) => !cols[d].has(f) && !(f in params));
        if (miss.length) { if (cf.dataset) probs.push(`calcField '${cf.name}': unknown field(s) ${miss.join(', ')} in dataset ${d} (columns: ${[...cols[d]].join(', ')})`); continue; }
        try { for (const r of sample(d)) { r[cf.name] = evaluate(c.ast, { row: r, params }); } } catch (e) { probs.push(`calcField '${cf.name}': ${e.message}`); }
        cols[d].add(cf.name);
      }
      if (!cf.dataset && !dsIds.some((d) => cols[d] && cols[d].has(cf.name))) probs.push(`calcField '${cf.name}': fields ${c.fields.join(', ')} not found in any dataset`);
    }
    for (const m of spec.measures || []) {
      if (!m.id || !m.expr) { probs.push(`measure needs id and expr: ${JSON.stringify(m)}`); continue; }
      let c; try { c = compile(m.expr); } catch (e) { probs.push(`measure '${m.id}': ${e.message}`); continue; }
      if (!c.aggregate) { probs.push(`measure '${m.id}' must aggregate, e.g. sum(x)/sum(y)`); continue; }
      const ok = targets(m).filter((d) => cols[d] && c.fields.every((f) => cols[d].has(f) || f in params));
      if (!ok.length) { probs.push(`measure '${m.id}': field(s) ${c.fields.join(', ')} not all present in ${m.dataset || 'any dataset'}`); continue; }
      try { evaluate(c.ast, { rows: sample(ok[0]), params }); } catch (e) { probs.push(`measure '${m.id}': ${e.message}`); }
    }
    return probs;
  }

  const api = { tokenize, parse, compile, evalRow, evalAgg, evaluate, aggregate, validate, AGG: [...AGG] };
  root.DPExpr = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(typeof window !== 'undefined' ? window : globalThis);
