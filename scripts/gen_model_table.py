"""Build datapilot-core/data/model_table.json: model name → [context window, max output] for the built-in model table.

usage: python3 -I scripts/gen_model_table.py <litellm model_prices_and_context_window.json> [codex models.json]
Sources (data only, refreshed by re-running this script):
  - LiteLLM model_prices_and_context_window.json (MIT, https://github.com/BerriAI/litellm): max_input_tokens / max_output_tokens.
    Per model name the vendor's own API wins (openai, anthropic, gemini, zai, moonshot, deepseek, dashscope…);
    otherwise the median over the providers that list it.
  - Codex models-manager/models.json (Apache-2.0, https://github.com/openai/codex): context_window of OpenAI models
    (what Codex itself uses, e.g. gpt-5.5 → 272k), overriding LiteLLM for those names.
"""
import json, re, statistics, sys

OFFICIAL = ["openai", "anthropic", "gemini", "vertex_ai-language-models", "zai", "moonshot", "deepseek", "dashscope",
            "volcengine", "tencent", "xai", "mistral", "minimax", "qwen_ai_platform"]
VENDOR_DOT = re.compile(r"^(?:[a-z]{2,7}\.)?(?:anthropic|deepseek|openai|meta|qwen|mistral|cohere|amazon|ai21|moonshot|moonshotai|zai|google|minimax|writer)\.")


def norm(key: str) -> str:
    n = key.lower().split("/")[-1]
    n = VENDOR_DOT.sub("", n)
    n = re.sub(r"-v\d+:\d+$", "", n)           # bedrock "…-v1:0"
    n = re.sub(r"@.*$", "", n)                  # vertex "…@20250929" / "@default"
    if n.startswith("databricks-"): n = n[len("databricks-"):]
    return n


def main():
    data = json.load(open(sys.argv[1], encoding="utf-8"))
    by_name = {}
    for k, v in data.items():
        if not isinstance(v, dict) or v.get("mode") not in ("chat", "responses"): continue
        ctx = v.get("max_input_tokens") or (v.get("max_tokens") if isinstance(v.get("max_tokens"), int) and not v.get("max_output_tokens") else None)
        if not isinstance(ctx, int) or ctx < 2048: continue
        if k.startswith("ft:") or ":variant" in k or "abliterated" in k: continue
        out = v.get("max_output_tokens")
        n = norm(k)
        if not n or len(n) > 80: continue
        by_name.setdefault(n, []).append((v.get("litellm_provider", ""), ctx, out if isinstance(out, int) and out > 0 else None))
    table = {}
    for n, rows in by_name.items():
        off = sorted((r for r in rows if r[0] in OFFICIAL), key=lambda r: OFFICIAL.index(r[0]))
        if off:
            ctx, out = off[0][1], off[0][2]
        else:
            ctx = int(statistics.median_low(sorted(r[1] for r in rows)))
            outs = sorted(r[2] for r in rows if r[2])
            out = int(statistics.median_low(outs)) if outs else None
        table[n] = [ctx, out] if out else [ctx]
    if len(sys.argv) > 2:
        cdx = json.load(open(sys.argv[2], encoding="utf-8"))
        for m in (cdx["models"] if isinstance(cdx, dict) else cdx):
            c = m.get("context_window") or m.get("max_context_window")
            if isinstance(c, int) and c >= 2048:
                prev = table.get(m["slug"].lower(), [c])
                table[m["slug"].lower()] = [c] + prev[1:]
    out = {"_source": "LiteLLM model_prices_and_context_window.json (MIT) + Codex models.json (Apache-2.0); regenerate with scripts/gen_model_table.py",
           "models": dict(sorted(table.items()))}
    sys.stdout.write(json.dumps(out, ensure_ascii=False, separators=(",", ":")) + "\n")


main()
