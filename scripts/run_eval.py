"""Text-to-SQL execution-accuracy eval against a real OpenAI-compatible endpoint (New API / vLLM).

Uses the same sandboxed kernel (M-Schema + relationships) and the same rules as the app, with an
error self-correction loop (≤3 retries). Compares result sets (order-insensitive unless ORDER BY + LIMIT).
usage: build/vendor/python/bin/python3 -I scripts/run_eval.py --base-url http://host:3000/v1 --model qwen3-32b [--key sk-..]
"""
import argparse, json, os, re, subprocess, sys, tempfile, urllib.request
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, os.path.join(ROOT, "tests/python"))
from test_kernel import Kernel, PY  # noqa: E402

RULES = """你是 DuckDB SQL 专家。根据 schema 写一条只读 SQL 回答问题。规则：标识符逐字匹配 schema；中文列名用双引号；
用 JOIN 时按外键关系连接；只输出一个 ```sql 代码块，不要解释。"""

def chat(base, key, model, messages):
    req = urllib.request.Request(base.rstrip("/") + "/chat/completions", method="POST",
        data=json.dumps({"model": model, "messages": messages, "temperature": 0, "max_tokens": 800,
                         "chat_template_kwargs": {"enable_thinking": False}}).encode(),
        headers={"Content-Type": "application/json", **({"Authorization": f"Bearer {key}"} if key else {})})
    with urllib.request.urlopen(req, timeout=180) as r:
        return json.load(r)["choices"][0]["message"]["content"] or ""

def extract(text):
    text = re.sub(r"<think>.*?</think>", "", text, flags=re.S)
    m = re.search(r"```(?:sql)?\s*(.*?)```", text, re.S)
    return (m.group(1) if m else text).strip().rstrip(";")

def norm(rows, ordered):
    def cell(v):
        return round(v, 4) if isinstance(v, float) else v
    out = [tuple(cell(c) for c in r) for r in rows]
    return out if ordered else sorted(out, key=lambda r: json.dumps(r, default=str))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", required=True); ap.add_argument("--model", required=True); ap.add_argument("--key", default=os.environ.get("DP_API_KEY", ""))
    a = ap.parse_args()
    ws = os.path.realpath(tempfile.mkdtemp(prefix="dpeval")); os.makedirs(f"{ws}/uploads")
    subprocess.check_call([PY, "-I", f"{ROOT}/tests/python/make_fixtures.py", f"{ws}/uploads"], stdout=subprocess.DEVNULL)
    k = Kernel(ws)
    k.call("import", path="uploads/orders.csv"); k.call("import", path="uploads/master.xlsx")
    schema = k.call("schema")["result"]["m_schema"]
    cases = json.load(open(f"{ROOT}/tests/eval/text2sql_eval.json"))
    ok = 0
    for i, c in enumerate(cases, 1):
        gold = k.call("query_data", sql=c["sql"], cap=10000)["result"]["rows"]
        msgs = [{"role": "system", "content": RULES + "\n\n" + schema}, {"role": "user", "content": c["q"]}]
        sql, res, tries = "", None, 0
        for tries in range(1, 4):
            sql = extract(chat(a.base_url, a.key, a.model, msgs))
            r = k.call("query_data", sql=sql, cap=10000)
            if r["ok"]:
                res = r["result"]["rows"]; break
            msgs += [{"role": "assistant", "content": f"```sql\n{sql}\n```"}, {"role": "user", "content": f"执行报错：{r['error'][:500]}\n请修正。"}]
        ordered = "order by" in c["sql"].lower() and "limit" in c["sql"].lower()
        hit = res is not None and norm(res, ordered) == norm(gold, ordered)
        ok += hit
        print(f"{'✅' if hit else '❌'} [{i:02d}] tries={tries} {c['q']}" + ("" if hit else f"\n     SQL: {sql}"))
    k.close()
    print(f"\nExecution accuracy: {ok}/{len(cases)} = {ok/len(cases):.0%}  (model={a.model})")

if __name__ == "__main__":
    main()
