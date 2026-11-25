"""matplotlib in the sandboxed kernel: Agg backend, Chinese font found, figures saved as artifacts."""
import ast, os, sys, tempfile
sys.path.insert(0, os.path.dirname(__file__))
from test_kernel import Kernel  # noqa: E402


def main():
    ws = os.path.realpath(tempfile.mkdtemp(prefix="dpws"))
    k = Kernel(ws)
    r = k.call("exec", code="""
import os, matplotlib, matplotlib.pyplot as plt
from matplotlib import font_manager as fm
fig, ax = plt.subplots(); ax.bar(['华东', '华北'], [3, -1]); ax.set_title('各大区销售额')
f = fm.findfont(fm.FontProperties(family=matplotlib.rcParams['font.sans-serif']), fallback_to_default=False)
(matplotlib.get_backend(), os.path.basename(f))
""")
    fails = []
    ok = r.get("ok")
    v = ast.literal_eval(r["result"]["value"]["text"]) if ok else None
    imgs = r.get("result", {}).get("images") or []
    for c, m in [(ok, f"exec ok {r.get('error')}"), (v and v[0].lower() == "agg", f"backend {v}"),
                 (v and "dejavu" not in v[1].lower(), f"CJK font {v}"), (imgs and os.path.exists(imgs[0]), f"figure saved {imgs}")]:
        print(("PASS " if c else "FAIL ") + m)
        if not c: fails.append(m)
    k.close()
    print("ALL PASS" if not fails else f"{len(fails)} FAIL")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
