"""Generate CHANGELOG.md from Resources/about/changelog.json (the app's 更新日志). usage: python3 -I scripts/gen_changelog.py"""
import json, os
root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
data = json.load(open(os.path.join(root, "Resources/about/changelog.json"), encoding="utf-8"))
out = ["# 更新日志 / Changelog", "", "由 `Resources/about/changelog.json` 生成（`python3 -I scripts/gen_changelog.py`），App 内「设置 → 关于 → 更新日志」显示同一内容。", ""]
for r in data["releases"]:
    out += [f"## {r['version']} · {r['date']}", "", r["summary_zh"], "", r["summary_en"], ""]
    for key, title in (("added", "新功能 / Added"), ("fixed", "问题修复 / Fixed"), ("improved", "改进 / Improved")):
        items = r.get(key) or []
        if items:
            out += [f"### {title}", ""] + [f"- {i['zh']} / {i['en']}" for i in items] + [""]
text = "\n".join(out)
dst = os.path.join(root, "CHANGELOG.md")
tmp = dst + ".tmp"
with open(tmp, "w", encoding="utf-8") as f:
    f.write(text)
os.replace(tmp, dst)
print("wrote", dst)
