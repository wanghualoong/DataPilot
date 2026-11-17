"""Eval fixtures: the standard sample (orders / master.xlsx) plus two extra tables for relation & target tasks.
usage: python3 -I tests/eval/make_eval_fixtures.py <dir>"""
import os, random, subprocess, sys
import pandas as pd
out = sys.argv[1]
here = os.path.dirname(os.path.abspath(__file__))
subprocess.check_call([sys.executable, "-I", os.path.join(here, "..", "python", "make_fixtures.py"), out], stdout=subprocess.DEVNULL)
random.seed(11)
# CRM export: customer key written as "C-0012" (orders use 12) → needs a key transform to join
pd.DataFrame({"客户编码": [f"C-{i:04d}" for i in range(1, 501)],
              "行业": [random.choice(["制造", "零售", "金融", "教育"]) for _ in range(500)]}).to_csv(os.path.join(out, "crm_contacts.csv"), index=False)
# yearly targets per region
pd.DataFrame({"大区": ["华东区", "华北区", "华南区", "西南区", "东北区"],
              "2025年目标": [1_500_000, 1_400_000, 1_300_000, 1_200_000, 1_100_000]}).to_csv(os.path.join(out, "targets_2025.csv"), index=False)
# --- trap fixtures (A1) ---
# business definitions live only in a document
open(os.path.join(out, "口径说明.md"), "w", encoding="utf-8").write(
    "# 销售口径说明（财务部，2026 版）\n\n"
    "- **GMV**：已支付订单的 数量 × 单价 之和，**不含 C 级客户**（C 级是内部测试账号）。\n"
    "- **活跃客户**：统计期内已支付订单 **不少于 20 笔** 的客户（按客户计数，所有等级都算）。\n"
    "- 除 GMV 外，其他金额指标（如已支付销售额、目标完成率）都包含全部客户。\n"
    "- 报表金额单位默认为元；目标表另有说明的以表头为准。\n")
# 2026 targets in 万元 (unit trap)
pd.DataFrame({"大区": ["华东区", "华北区", "华南区", "西南区", "东北区"],
              "目标(万元)": [600, 580, 560, 540, 520]}).to_csv(os.path.join(out, "targets_2026.csv"), index=False)
# refund registrations: order ids written as "SO100005", ~10% registered twice
o = pd.read_csv(os.path.join(out, "orders.csv"), encoding="gb18030")
picked = o.sample(900, random_state=3)["order_id"].tolist()
rows = [{"退货单号": f"R{i:05d}", "订单号": f"SO{oid}", "登记日期": "2026-09-01"} for i, oid in enumerate(picked)]
rows += [dict(r, 退货单号=r["退货单号"] + "-2") for r in rows[:90]]
pd.DataFrame(rows).to_csv(os.path.join(out, "returns_log.csv"), index=False)
print("ok", out)
