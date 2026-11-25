// Settings → General → 看板配色: default colour scheme for dashboards and chat charts (a dashboard can override it).
import { imeEnter } from "../../ime";
import { useEffect, useState } from "react";
import { Plus, X } from "lucide-react";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { MAX_CUSTOM, PALETTE_NAMES, Swatch, parseColors, swatches } from "../../palette";
import { Status } from "../ui";
import { patch } from "./common";

/** Bars in the draft colours so the user sees them as series, not just swatches. */
function Preview({ colors }: { colors: string[] }) {
  const h = [62, 88, 45, 74, 56, 92, 38, 67, 80, 50, 70, 42];
  return (
    <div className="palpreview" aria-label={L("预览", "Preview")}>
      {colors.map((c, i) => <i key={i} style={{ background: c, height: `${h[i % h.length]}%` }} title={c} />)}
    </div>
  );
}

function CustomEditor({ cur, active }: { cur: unknown; active: boolean }) {
  const saved = Array.isArray(cur) ? (cur as string[]) : null;
  const [colors, setColors] = useState<string[]>(() => saved ?? swatches(cur));
  const [text, setText] = useState("");
  const [msg, setMsg] = useState<string>();
  useEffect(() => { if (saved) setColors(saved); }, [JSON.stringify(saved)]);
  const dirty = JSON.stringify(colors) !== JSON.stringify(saved);
  const set = (i: number, c: string) => { setColors(colors.map((x, j) => (j === i ? c : x))); setMsg(undefined); };
  const apply = async () => {
    if (colors.length < 2) return setMsg("❌ " + L("至少需要 2 个颜色", "At least 2 colours"));
    if (await patch({ settings: { dashboardPalette: colors } })) setMsg("✅ " + L("已应用自定义配色", "Custom colours applied"));
  };
  return (
    <div className={"palcustom" + (active ? " on" : "")}>
      <div className="palchips">
        {colors.map((c, i) => (
          <span key={i} className="palchip">
            <input type="color" value={c} title={`${i + 1}. ${c}`} onChange={(e) => set(i, e.target.value)} />
            {colors.length > 2 && <button className="x" title={L("删除这个颜色", "Remove colour")} onClick={() => setColors(colors.filter((_, j) => j !== i))}><X size={10} /></button>}
          </span>
        ))}
        {colors.length < MAX_CUSTOM && (
          <button className="iconbtn" title={L("添加颜色", "Add colour")}
            onClick={() => setColors([...colors, PALETTE_NAMES[1][2][colors.length % 6]])}><Plus size={14} /></button>
        )}
      </div>
      <Preview colors={colors} />
      <div className="row">
        <select value="" onChange={(e) => { const p = PALETTE_NAMES.find((x) => x[0] === e.target.value); if (p) { setColors([...p[2]]); setMsg(undefined); } }}>
          <option value="">{L("从模板开始…", "Start from a template…")}</option>
          {PALETTE_NAMES.map(([k, label]) => <option key={k} value={k}>{label}</option>)}
        </select>
        <input className="grow" placeholder={L("或粘贴颜色：#1f77b4, #ff7f0e …", "Or paste: #1f77b4, #ff7f0e …")} value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !imeEnter(e)) { const c = parseColors(text); if (c) { setColors(c.slice(0, MAX_CUSTOM)); setText(""); setMsg(undefined); } else setMsg("❌ " + L("请输入至少 2 个 #RRGGBB 颜色", "Enter at least 2 #RRGGBB colours")); } }} />
        <button className="primary" disabled={!dirty && active} onClick={apply}>{active && !dirty ? L("已应用", "Applied") : L("应用自定义", "Use custom")}</button>
      </div>
      <Status msg={msg} />
      <div className="muted small">{L(`点色块用取色器修改；最多 ${MAX_CUSTOM} 个颜色，按顺序分配给系列。粘贴后按回车填入。深色模式下会自动提亮偏暗的颜色。`,
        `Click a swatch to pick a colour; up to ${MAX_CUSTOM}, assigned to series in order. Press Enter after pasting. Dark mode lifts dark colours automatically.`)}</div>
    </div>
  );
}

export function PaletteSection() {
  const cur = useApp((s) => s.config?.settings?.dashboardPalette) ?? "default";
  const custom = Array.isArray(cur);
  const [showCustom, setShowCustom] = useState(custom);
  return (
    <section>
      <h4>{L("看板配色", "Dashboard colours")}</h4>
      <div className="palettes">
        {PALETTE_NAMES.map(([k, label, c]) => (
          <label key={k} className={"palopt" + (!custom && cur === k ? " on" : "")}>
            <input type="radio" name="dp-palette" checked={!custom && cur === k} onChange={() => patch({ settings: { dashboardPalette: k } })} />
            <span className="grow">{label}</span><Swatch colors={c} />
          </label>
        ))}
        <label className={"palopt" + (custom ? " on" : "")}>
          <input type="radio" name="dp-palette" checked={custom} onChange={() => setShowCustom(true)} />
          <span className="grow">{L("自定义…", "Custom…")}</span>{custom && <Swatch colors={swatches(cur)} />}
        </label>
      </div>
      {(showCustom || custom) && <CustomEditor cur={cur} active={custom} />}
      <div className="muted small">{L("用于聊天里的图表和所有看板；颜色按系列顺序固定分配。单个看板可在看板右上角单独换配色。热力图 / 漏斗图使用第一种颜色的深浅渐变。",
        "Used by chat charts and dashboards; colours are assigned to series in a fixed order. A dashboard can override it from its toolbar. Heatmaps / funnels use shades of the first colour.")}</div>
    </section>
  );
}
