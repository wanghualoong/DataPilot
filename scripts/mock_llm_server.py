"""Mock OpenAI-compatible (New API style) server for end-to-end tests.

Scripted behaviour per request (based on conversation state):
 - if no tool result yet: stream a tool call (data_import), arguments split across chunks, with reasoning_content
 - after data_import result: call data_schema + sql_query in parallel (two tool calls, index 0/1)
 - after sql result: stream final answer text
 - /v1/models, /v1/embeddings (hash vectors), /v1/rerank (char overlap)
Non-stream requests (titles, memory extraction, notes) get simple JSON content.
"""
import hashlib, json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = []

def chunk(delta, finish=None):
    return "data: " + json.dumps({"id": "c1", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}, ensure_ascii=False) + "\n\n"

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass

    def _json(self, obj, code=200):
        b = json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(code); self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)

    def do_GET(self):
        if self.path.endswith("/models"):
            return self._json({"data": [{"id": m} for m in ["qwen3-32b", "deepseek-chat", "qwen3-embedding-0.6b", "qwen3-reranker-0.6b"]]})
        self._json({"error": "nf"}, 404)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        LOG.append(body)
        if self.path.endswith("/embeddings"):
            inp = body["input"] if isinstance(body["input"], list) else [body["input"]]
            data = []
            for i, t in enumerate(inp):
                v = [0.0] * 64
                for k in range(len(t) - 1):
                    v[int(hashlib.md5(t[k:k+2].encode()).hexdigest(), 16) % 64] += 1
                data.append({"index": i, "embedding": v})
            return self._json({"data": data})
        if self.path.endswith("/rerank"):
            q = set(body["query"])
            res = sorted(({"index": i, "relevance_score": len(q & set(d)) / (len(q) or 1)} for i, d in enumerate(body["documents"])), key=lambda r: -r["relevance_score"])
            return self._json({"results": res[: body.get("top_n") or len(res)]})
        msgs = body.get("messages", [])
        if not body.get("stream"):
            sys_txt = msgs[0]["content"] if msgs else ""
            if "记忆抽取器" in sys_txt:
                content = json.dumps({"memories": [{"scope": "dataset", "kind": "rule", "content": "计算销售额时剔除状态为 refund 的订单", "source": "correction"}]}, ensure_ascii=False)
            elif "标题" in sys_txt:
                content = "大区销售额分析"
            elif "会话笔记" in sys_txt:
                content = "# 会话标题 / Title\n_一句话描述本会话在分析什么_\n大区销售额分析\n\n# 口径与定义 / Definitions\n_指标公式_\n- 剔除 refund"
            else:
                content = "<analysis>草稿</analysis><summary>用户在分析大区销售额，已导入 orders。</summary>"
            return self._json({"choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 100, "completion_tokens": 20}})

        tool_msgs = [m for m in msgs if m.get("role") == "tool"]
        names = [m.get("content", "")[:40] for m in tool_msgs]
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.end_headers()
        out = [": keepalive\n\n", chunk({"role": "assistant", "reasoning_content": "先"}), chunk({"reasoning_content": "思考一下。"})]
        n_tools = len(tool_msgs)
        if n_tools == 0:
            args = json.dumps({"path": "uploads/orders.csv"})
            out += [chunk({"tool_calls": [{"index": 0, "id": "call_imp", "type": "function", "function": {"name": "data_import", "arguments": ""}}]}),
                    chunk({"tool_calls": [{"index": 0, "function": {"arguments": args[:10]}}]}),
                    chunk({"tool_calls": [{"index": 0, "function": {"arguments": args[10:]}}]}),
                    chunk({}, "tool_calls")]
        elif n_tools == 1:
            sql = 'SELECT c."大区", sum(o."数量"*p."单价") AS gmv FROM orders o JOIN master_customers c USING(customer_id) JOIN master_products p USING(product_id) WHERE o."状态"=\'paid\' GROUP BY 1 ORDER BY 2 DESC'
            out += [chunk({"tool_calls": [{"index": 0, "id": "call_imp2", "type": "function", "function": {"name": "data_import", "arguments": json.dumps({"path": "uploads/master.xlsx"})}}]}), chunk({}, "tool_calls")]
            self.sql = sql
        elif n_tools == 2:
            sql = 'SELECT c."大区", sum(o."数量"*p."单价") AS gmv FROM orders o JOIN master_customers c USING(customer_id) JOIN master_products p USING(product_id) WHERE o."状态"=\'paid\' GROUP BY 1 ORDER BY 2 DESC'
            out += [chunk({"tool_calls": [{"index": 0, "id": "call_s", "type": "function", "function": {"name": "data_schema", "arguments": "{}"}},
                                          {"index": 1, "id": "call_q", "type": "function", "function": {"name": "sql_query", "arguments": json.dumps({"sql": sql, "question": "各大区销售额"}, ensure_ascii=False)}}]}),
                    chunk({}, "stop")]  # some gateways send stop with tool calls
        elif n_tools == 4:
            spec = {"title": "销售看板", "datasets": {"d1": {"sql": 'SELECT c."大区" AS 大区, p."品类" AS 品类, sum(o."数量"*p."单价") AS 销售额, count(*) AS 订单数 FROM orders o JOIN master_customers c USING(customer_id) JOIN master_products p USING(product_id) GROUP BY ALL'}},
                    "filters": [{"id": "f1", "field": "大区", "type": "multiselect", "dataset": "d1"}],
                    "widgets": [{"id": "w1", "type": "kpi", "title": "总销售额", "dataset": "d1", "measure": {"field": "销售额", "agg": "sum"}},
                                {"id": "w2", "type": "bar", "title": "销售额", "dataset": "d1", "x": "大区", "y": [{"field": "销售额", "agg": "sum"}], "dimensionOptions": ["大区", "品类"], "measureOptions": ["销售额", "订单数"]},
                                {"id": "w3", "type": "radar", "title": "品类对比", "dataset": "d1", "x": "品类", "y": [{"field": "销售额", "agg": "sum"}, {"field": "订单数", "agg": "sum"}]}]}
            out += [chunk({"tool_calls": [{"index": 0, "id": "call_db", "type": "function", "function": {"name": "dashboard_create", "arguments": json.dumps({"spec": spec}, ensure_ascii=False)}}]}), chunk({}, "tool_calls")]
        else:
            for piece in ["华东区", "销售额最高。", "\n\n| 大区 | 销售额 |\n|---|---|\n"]:
                out.append(chunk({"content": piece}))
            out.append(chunk({}, "stop"))
        out.append("data: " + json.dumps({"choices": [], "usage": {"prompt_tokens": 1200 + 300 * n_tools, "completion_tokens": 50}}) + "\n\n")
        out.append("data: [DONE]\n\n")
        for o in out:
            self.wfile.write(o.encode()); self.wfile.flush(); time.sleep(0.01)

if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18080
    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
