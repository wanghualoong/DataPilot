"""Generate a multi-table sample dataset (orders/customers/products) incl. GBK CSV and multi-sheet Excel with a title row."""
import os, random, sys
import pandas as pd
out = sys.argv[1]
os.makedirs(out, exist_ok=True)
random.seed(7)
regions = ["华东区", "华北区", "华南区", "西南区", "东北区"]
cust = pd.DataFrame({"customer_id": range(1, 501), "客户名称": [f"客户{i}" for i in range(1, 501)],
                     "大区": [random.choice(regions) for _ in range(500)],
                     "等级": [random.choice(["A", "B", "C"]) for _ in range(500)]})
prod = pd.DataFrame({"product_id": [f"P{i:03d}" for i in range(1, 41)],
                     "品类": [random.choice(["家电", "数码", "服饰", "食品"]) for _ in range(40)],
                     "单价": [round(random.uniform(10, 2000), 2) for _ in range(40)]})
n = 20000
orders = pd.DataFrame({"order_id": range(100001, 100001 + n),
                       "customer_id": [random.randint(1, 500) for _ in range(n)],
                       "product_id": [f"P{random.randint(1, 40):03d}" for _ in range(n)],
                       "数量": [random.randint(1, 5) for _ in range(n)],
                       "下单日期": pd.to_datetime("2025-01-01") + pd.to_timedelta([random.randint(0, 600) for _ in range(n)], unit="D"),
                       "状态": [random.choice(["paid", "paid", "paid", "refund"]) for _ in range(n)]})
orders.to_csv(os.path.join(out, "orders.csv"), index=False, encoding="gb18030")
with pd.ExcelWriter(os.path.join(out, "master.xlsx")) as w:
    # title row above header in the customers sheet
    pd.DataFrame([["客户主数据（2025）"] + [None] * 3]).to_excel(w, sheet_name="customers", header=False, index=False)
    cust.to_excel(w, sheet_name="customers", startrow=2, index=False)
    prod.to_excel(w, sheet_name="products", index=False)
print("ok", out)
