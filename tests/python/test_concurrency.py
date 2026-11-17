"""Concurrency tests for the shared project kernel: per-request threads, per-chat ordering, cancel."""
import json, os, subprocess, sys, tempfile, threading, time
sys.path.insert(0, os.path.dirname(__file__))
from test_kernel import PY, RES  # noqa: E402


class AsyncKernel:
    def __init__(self, ws):
        tmp = os.path.realpath(tempfile.mkdtemp(prefix="dptmp"))
        pol = open(os.path.join(RES, "sandbox/base.sbpl")).read()
        cmd = ["/usr/bin/sandbox-exec", "-p", pol, f"-DPYROOT={os.path.realpath(os.path.dirname(os.path.dirname(PY)))}",
               f"-DRESROOT={os.path.realpath(RES)}", f"-DWORKSPACE={ws}", f"-DTMPDIR={tmp}", "--",
               PY, "-I", "-u", os.path.join(RES, "kernel/kernel.py")]
        env = {"DP_WORKSPACE": ws, "TMPDIR": tmp, "HOME": ws, "PATH": "/usr/bin:/bin", "MPLCONFIGDIR": tmp, "MPLBACKEND": "Agg"}
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
        self.ready = json.loads(self.p.stdout.readline())
        self.n = 0
        self.resp, self.arrival, self.cv = {}, [], threading.Condition()
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self):
        for line in self.p.stdout:
            v = json.loads(line)
            with self.cv:
                self.resp[v.get("id")] = (v, time.time())
                self.arrival.append(v.get("id"))
                self.cv.notify_all()

    def send(self, op, **kw):
        self.n += 1
        rid = str(self.n)
        self.p.stdin.write(json.dumps({"id": rid, "op": op, **kw}, ensure_ascii=False) + "\n"); self.p.stdin.flush()
        return rid

    def wait(self, rid, timeout=30):
        with self.cv:
            ok = self.cv.wait_for(lambda: rid in self.resp, timeout)
            if not ok:
                raise TimeoutError(rid)
            return self.resp[rid]

    def call(self, op, **kw):
        return self.wait(self.send(op, **kw))[0]

    def close(self):
        self.call("shutdown"); self.p.wait(10)


SLOW = "con.execute('select count(*) from range(30000000000)').fetchall()"


def main():
    ws = os.path.realpath(tempfile.mkdtemp(prefix="dpws"))
    k = AsyncKernel(ws)
    fails = []

    def check(cond, msg):
        print(("PASS " if cond else "FAIL ") + msg)
        if not cond:
            fails.append(msg)

    k.call("exec", ns="a", code="con.execute('create table t as select range x from range(100)')")

    # 1. chat A runs a slow query; chat B (and the inspector, no ns) answer immediately
    slow = k.send("exec", ns="a", code=SLOW)
    time.sleep(0.4)
    t0 = time.time(); r = k.call("tables", ns="b"); dt = time.time() - t0
    check(r["ok"] and dt < 1.0, f"B tables while A busy: {dt:.3f}s")
    t0 = time.time(); r = k.call("sql", sql="select count(*) from t"); dt = time.time() - t0
    check(r["ok"] and r["result"]["rows"][0][0] == 100 and dt < 1.0, f"inspector sql while A busy: {dt:.3f}s")

    # 2. running() reports the long request
    run = k.call("running")["result"]["running"]
    check(any(x["id"] == slow and x["state"] == "running" and x["ns"] == "a" for x in run), f"running() lists A: {run}")

    # 3. a second exec of chat A is queued behind the first; cancel it while queued
    queued = k.send("exec", ns="a", code="1+1")
    time.sleep(0.3)
    k.call("cancel", target=queued)
    v, _ = k.wait(queued, 5)
    check(v.get("cancelled") and v.get("state") == "queued", f"cancel queued: {v.get('error')} {v.get('state')}")

    # 4. cancel the running query: only A stops, quickly, and the kernel stays usable
    t0 = time.time(); k.call("cancel", target=slow); v, t1 = k.wait(slow, 10)
    check(v.get("cancelled") and v.get("state") == "running" and t1 - t0 < 3, f"cancel running sql in {t1 - t0:.2f}s: {v.get('error')}")
    r = k.call("exec", ns="a", code="con.execute('select count(*) from t').fetchall()[0][0]")
    check(r["ok"] and "100" in (r["result"]["value"] or {}).get("text", ""), "A usable after cancel")

    # 5. cancel a pure-python loop (async exception)
    loop = k.send("exec", ns="a", code="import time\nwhile True:\n    time.sleep(0.01)")
    time.sleep(0.4)
    t0 = time.time(); k.call("cancel", ns="a"); v, t1 = k.wait(loop, 10)
    check(v.get("cancelled") and t1 - t0 < 3, f"cancel python loop in {t1 - t0:.2f}s")

    # 6. per-chat order: A's cells run in submit order
    ids = [k.send("exec", ns="a", code=f"import time; time.sleep(0.{3 - i}); order = globals().get('order', []) + [{i}]") for i in range(3)]
    for i in ids:
        k.wait(i)
    r = k.call("exec", ns="a", code="order")
    check("[0, 1, 2]" in r["result"]["value"]["text"], f"per-chat order kept: {r['result']['value']['text']}")

    # 7. stdout isolation between concurrent requests
    ia = k.send("exec", ns="a", code="import time\nfor _ in range(20):\n    print('AAA'); time.sleep(0.01)")
    ib = k.send("query_data", ns="b", sql="select 1")
    va, vb = k.wait(ia)[0], k.wait(ib)[0]
    check("AAA" in va["stdout"] and "AAA" not in vb.get("stdout", ""), "stdout isolated per request")

    # 8. two chats' exec never interleave (process-global interpreter state, e.g. plotting libraries)
    code = "import time\n_t0 = time.time(); time.sleep(0.3); (_t0, time.time())"
    ea, eb = k.send("exec", ns="a", code=code), k.send("exec", ns="b", code=code)
    va, vb = k.wait(ea)[0], k.wait(eb)[0]
    sa, sb = eval(va["result"]["value"]["text"]), eval(vb["result"]["value"]["text"])
    overlap = min(sa[1], sb[1]) - max(sa[0], sb[0])
    check(va["ok"] and vb["ok"] and overlap <= 0.01, f"exec serialized across chats (overlap {overlap:.3f}s)")

    # 9. namespaces stay separate under concurrency
    k.call("exec", ns="b", code="who = 'b'"); k.call("exec", ns="a", code="who = 'a'")
    check("'b'" in k.call("exec", ns="b", code="who")["result"]["value"]["text"], "namespaces isolated")

    # 10. concurrent imports from two chats (temp table name per cursor)
    fx = os.path.join(ws, "uploads"); os.makedirs(fx, exist_ok=True)
    for n in ("x", "y"):
        with open(os.path.join(fx, f"{n}.csv"), "w") as f:
            f.write("id,v\n" + "\n".join(f"{i},{i*2}" for i in range(20000)))
    ix, iy = k.send("import", ns="a", path="uploads/x.csv"), k.send("import", ns="b", path="uploads/y.csv")
    vx, vy = k.wait(ix)[0], k.wait(iy)[0]
    check(vx["ok"] and vy["ok"], f"parallel imports: {vx.get('error')} {vy.get('error')}")
    names = {t["table"] for t in k.call("tables")["result"]["tables"]}
    check({"x", "y"} <= names, f"both imported tables visible: {sorted(names)}")

    k.close()
    print(f"\n{len(fails)} failures")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
