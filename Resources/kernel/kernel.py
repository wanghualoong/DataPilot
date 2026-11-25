"""DataPilot persistent data kernel.

JSON-lines protocol over stdin/stdout (one request per line):
  {"id": "...", "op": "exec"|"import"|"profile"|"schema"|"sql"|"relations"|"tables"|"preview"|
                    "checkpoint"|"restore"|"ping"|"query_data"|"shutdown", ...params}
Response: {"id": "...", "ok": true, "result": {...}, "stdout": "...", "ms": 12}
Runs under sandbox-exec; only WORKSPACE is writable.

Concurrency (a project kernel is shared by all chats of the project):
  * the main thread only reads requests; every request runs on its own worker thread with its own DuckDB
    cursor, so one chat's long import / query never blocks another chat's (or the inspector's) queries;
  * ops that mutate a chat's Python namespace (exec/import/checkpoint/vars) are serialized per chat;
  * `exec` is additionally serialized process-wide (matplotlib's pyplot state is global);
  * control ops are answered immediately by the main thread:
      {"op": "cancel", "target": "<id>"} | {"op": "cancel", "ns": "<chat>"}   -> cancel one request / a chat's
      {"op": "running"}                                                      -> what is queued / running
    A cancelled request answers {"ok": false, "cancelled": true, "state": "queued"|"running", ...}.
"""
import collections, ctypes, io, json, os, sys, threading, time, traceback

WORKSPACE = os.environ.get("DP_WORKSPACE") or os.getcwd()
# matplotlib: our rc file (Agg, Chinese fonts) unless the environment points elsewhere
os.environ.setdefault("MATPLOTLIBRC", os.path.join(os.path.dirname(os.path.abspath(__file__)), "matplotlibrc"))
import logging  # noqa: E402
logging.getLogger("matplotlib.font_manager").setLevel(logging.ERROR)
# library deprecation / future notices (pandas' "default engine for reading 'xlsx' will change…" on every read_excel)
# fill tool outputs and distract models; real problems still raise or show as other warnings
import warnings  # noqa: E402
warnings.filterwarnings("ignore", category=DeprecationWarning)
warnings.filterwarnings("ignore", category=FutureWarning)   # "findfont: … weight" noise on stderr
os.chdir(os.environ.get("DP_CWD") or WORKSPACE)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from dataagent import engine  # noqa: E402
from dataagent.util import clip, json_default  # noqa: E402
engine.clip = clip
engine.json_default = json_default

# protocol streams are UTF-8 whatever the system code page (Chinese Windows: GBK); the core decodes UTF-8
for _s in (sys.stdin, sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8", errors="replace" if _s is not sys.stdout else "strict")
    except Exception:  # noqa: BLE001
        pass
_real_stdout = sys.stdout
_real_stderr = sys.stderr
_out_lock = threading.Lock()


def _finite(o):
    """NaN / ±Infinity → None (recursively): the core reads strict JSON."""
    if isinstance(o, float):
        return None if o != o or o in (float("inf"), float("-inf")) else o
    if isinstance(o, dict):
        return {k: _finite(v) for k, v in o.items()}
    if isinstance(o, (list, tuple)):
        return [_finite(v) for v in o]
    return o


def respond(obj):
    try:
        s = json.dumps(obj, ensure_ascii=False, default=engine.json_default, allow_nan=False)
    except ValueError:
        try:
            s = json.dumps(_finite(obj), ensure_ascii=False, default=engine.json_default, allow_nan=False)
        except Exception as e:  # noqa: BLE001  (always answer, or the caller waits for its timeout)
            s = json.dumps({"id": obj.get("id") if isinstance(obj, dict) else None, "ok": False,
                            "error": f"unserializable result: {e}"}, ensure_ascii=False)
    s += "\n"
    with _out_lock:
        _real_stdout.write(s)
        _real_stdout.flush()


# ---------- per-thread output capture (print() of request A must not land in request B) ----------
_cap = threading.local()


class _ThreadOut(io.TextIOBase):
    """sys.stdout / sys.stderr stand-in: writes go to the current request thread's buffer; threads without one
    (e.g. started by user code) go to the real stderr — never to the protocol stream."""

    @property
    def encoding(self):
        return "utf-8"

    def writable(self):
        return True

    def isatty(self):
        return False

    def write(self, s):
        b = getattr(_cap, "buf", None)
        (b if b is not None else _real_stderr).write(s)
        return len(s)

    def flush(self):
        if getattr(_cap, "buf", None) is None:
            _real_stderr.flush()


sys.stdout = _ThreadOut()
sys.stderr = _ThreadOut()


# ---------- request registry / scheduling ----------
class Cancelled(Exception):
    pass


class Req:
    __slots__ = ("id", "op", "ns", "detail", "state", "t0", "t_run", "tid", "cursor", "cancelled")

    def __init__(self, rid, op, ns, detail):
        self.id, self.op, self.ns, self.detail = rid, op, ns, detail
        self.state, self.t0, self.t_run, self.tid, self.cursor, self.cancelled = "queued", time.time(), None, None, None, False


NS_OPS = {"exec", "import", "checkpoint", "vars"}
_reqs = {}
_reqs_lock = threading.Lock()
_turn = threading.Condition(_reqs_lock)
_ns_queues = collections.defaultdict(collections.deque)   # per-chat FIFO of namespace-mutating request ids
_exec_lock = threading.Lock()


def _wait_turn(r):
    """Block until `r` is the oldest namespace-mutating request of its chat (cells run in submit order)."""
    with _turn:
        while _ns_queues[r.ns][0] != r.id:
            if r.cancelled:
                raise Cancelled("cancelled while queued")
            _turn.wait(0.1)


def _leave_turn(r):
    with _turn:
        q = _ns_queues.get(r.ns)
        if q is not None:
            try:
                q.remove(r.id)
            except ValueError:
                pass
            if not q:
                _ns_queues.pop(r.ns, None)
        _turn.notify_all()


def _acquire(lock, r):
    while not lock.acquire(timeout=0.1):
        if r.cancelled:
            raise Cancelled("cancelled while queued")


def _detail(op, req):
    for k in ("path", "table", "sql", "code"):
        if req.get(k):
            return clip(" ".join(str(req[k]).split()), 120)
    return ""


def _async_raise(tid, exc):
    ctypes.pythonapi.PyThreadState_SetAsyncExc(ctypes.c_ulong(tid), ctypes.py_object(exc) if exc else None)


def cancel(target=None, ns=None):
    """Mark matching requests cancelled. Queued ones bail out of their lock wait; running ones get their DuckDB
    query interrupted and a Cancelled exception raised in their thread (pure-C work stops at the next bytecode)."""
    n = 0
    with _reqs_lock:
        for r in _reqs.values():
            if (target is not None and r.id == target) or (target is None and ns is not None and r.ns == ns):
                r.cancelled = True
                n += 1
                if r.state == "running":
                    if r.cursor is not None:
                        try:
                            r.cursor.interrupt()
                        except Exception:  # noqa: BLE001
                            pass
                    if r.tid is not None:
                        _async_raise(r.tid, Cancelled)
    return n


def running():
    now = time.time()
    with _reqs_lock:
        return [{"id": r.id, "op": r.op, "ns": r.ns, "state": r.state, "detail": r.detail,
                 "secs": round(now - (r.t_run or r.t0), 1), "waited": round((r.t_run or now) - r.t0, 1)}
                for r in _reqs.values()]


def _run(eng, r, req):
    _cap.buf = buf = io.StringIO()
    r.tid = threading.get_ident()
    held, cur, own = [], None, False
    try:
        if r.op in NS_OPS:
            _wait_turn(r)
        if r.op == "exec":
            _acquire(_exec_lock, r); held.append(_exec_lock)
        own = r.op != "exec"
        cur = eng.ns_con(r.ns) if r.op == "exec" else eng.cursor()
        with _reqs_lock:
            if r.cancelled:
                raise Cancelled("cancelled while queued")
            r.state, r.cursor, r.t_run = "running", cur, time.time()
        eng.bind(r.ns, cur)
        eng._tl.progress = (lambda msg, _r=r: setattr(_r, "detail", clip(str(msg), 120)))
        try:
            result = eng.dispatch(r.op, req)
        finally:
            eng.unbind()
        with _reqs_lock:
            r.state = "done"
            _async_raise(r.tid, None)   # drop a cancel that raced with completion
        respond({"id": r.id, "ok": True, "result": result, "stdout": clip(buf.getvalue(), 20000),
                 "ms": int((time.time() - r.t0) * 1000)})
    except BaseException as e:  # noqa: BLE001  (Cancelled arrives asynchronously, possibly as anything DuckDB wraps it in)
        with _reqs_lock:
            r.state = "done"            # no further async raises into this thread
            _async_raise(r.tid, None)
        if r.cancelled:
            respond({"id": r.id, "ok": False, "cancelled": True, "state": "queued" if r.t_run is None else "running",
                     "error": "Cancelled" + ("" if r.t_run is None else f" after {time.time() - r.t_run:.1f}s"),
                     "stdout": clip(buf.getvalue(), 8000), "ms": int((time.time() - r.t0) * 1000)})
        else:
            tb = traceback.format_exc(limit=6)
            respond({"id": r.id, "ok": False, "error": f"{type(e).__name__}: {e}", "traceback": clip(tb, 4000),
                     "stdout": clip(buf.getvalue(), 8000), "ms": int((time.time() - r.t0) * 1000)})
    finally:
        for lk in reversed(held):
            lk.release()
        if r.op in NS_OPS:
            _leave_turn(r)
        if own and cur is not None:
            try:
                cur.close()
            except Exception:  # noqa: BLE001
                pass
        with _reqs_lock:
            _reqs.pop(r.id, None)
        _cap.buf = None


def main():
    eng = engine.Engine(WORKSPACE)
    respond({"id": "_ready", "ok": True, "result": eng.status()})
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as e:  # noqa: BLE001
            respond({"id": None, "ok": False, "error": f"bad json: {e}"})
            continue
        rid, op = req.get("id"), req.get("op")
        if op == "shutdown":
            for i in list(_reqs):
                cancel(target=i)
            for t in [t for t in threading.enumerate() if t.name.startswith("dp-req-")]:
                t.join(timeout=3)
            eng.close()
            respond({"id": rid, "ok": True, "result": {}})
            return
        if op == "cancel":
            ns = req.get("ns")
            respond({"id": rid, "ok": True, "result": {"cancelled": cancel(target=req.get("target"),
                                                                               ns=engine.Engine.ns_key(ns) if ns is not None else None)}})
            continue
        if op == "running":
            respond({"id": rid, "ok": True, "result": {"running": running()}})
            continue
        r = Req(rid, op, engine.Engine.ns_key(req.get("ns")), _detail(op, req))
        with _reqs_lock:
            _reqs[rid] = r
            if op in NS_OPS:
                _ns_queues[r.ns].append(rid)   # enqueue in arrival order, before the worker thread starts
        threading.Thread(target=_run, args=(eng, r, req), name=f"dp-req-{rid}", daemon=True).start()


if __name__ == "__main__":
    main()
