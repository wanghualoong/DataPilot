"""Scripted OpenAI-compatible model for golden-sample replay tests (tests/golden/cases/*.json).

Usage: python3 -I scripts/golden_llm_server.py <port> <case.json>
The n-th streamed request (n = number of assistant messages already in the conversation) gets case["model"][n]:
  {"tool_calls": [{"name": "...", "arguments": {...}}, ...]}  or  {"content": "..."}
Non-stream requests (title, notes, memory extraction, recall) get a neutral answer.
Language-neutral on purpose: the Swift core today and a Rust core later replay the same cases against it.
"""
import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CASE = {}


def chunk(delta, finish=None):
    return "data: " + json.dumps({"id": "g", "object": "chat.completion.chunk",
                                  "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}, ensure_ascii=False) + "\n\n"


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _json(self, obj):
        b = json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)

    def do_GET(self):
        self._json({"data": [{"id": "golden-model"}]})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        msgs = body.get("messages", [])
        if not body.get("stream"):
            sys_txt = msgs[0]["content"] if msgs else ""
            content = ('{"memories":[]}' if "记忆抽取器" in sys_txt else '{"files":[]}' if "项目记忆清单" in sys_txt
                       else "黄金样本" if "标题" in sys_txt else "<summary>golden</summary>")
            return self._json({"choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
                               "usage": {"prompt_tokens": 10, "completion_tokens": 5}})
        n = sum(1 for m in msgs if m.get("role") == "assistant")
        # sub-agent conversations (own context, shared prompt Resources/prompts/subagent.md) follow "subagent_model"
        first = msgs[0].get("content") if msgs and msgs[0].get("role") == "system" else ""
        sub = isinstance(first, str) and first.startswith("你是 DataPilot 的子 Agent")
        script = CASE.get("subagent_model" if sub else "model", [])
        step = script[n] if n < len(script) else {"content": "(end of script)"}
        out = [chunk({"role": "assistant"})]
        if step.get("tool_calls"):
            for i, c in enumerate(step["tool_calls"]):
                args = json.dumps(c.get("arguments", {}), ensure_ascii=False)
                out.append(chunk({"tool_calls": [{"index": i, "id": f"call_{n}_{i}", "type": "function",
                                                  "function": {"name": c["name"], "arguments": args}}]}))
            out.append(chunk({}, "tool_calls"))
        else:
            out += [chunk({"content": step.get("content", "")}), chunk({}, "stop")]
        out.append("data: " + json.dumps({"choices": [], "usage": {"prompt_tokens": step.get("prompt_tokens", 1000 + 100 * n), "completion_tokens": 20}}) + "\n\n")
        out.append("data: [DONE]\n\n")
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.end_headers()
        for o in out:
            self.wfile.write(o.encode()); self.wfile.flush(); time.sleep(0.005)


if __name__ == "__main__":
    with open(sys.argv[2], encoding="utf-8") as f:
        CASE = json.load(f)
    ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
