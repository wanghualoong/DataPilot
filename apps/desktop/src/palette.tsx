// Dashboard / chart colour schemes (rendered by Resources/web/dashboard.js; names must match its PALETTES).
import { L } from "./i18n";

export const PALETTE_NAMES: [string, string, string[]][] = [
  ["default", L("默认", "Default"), ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300"]],
  ["okabe", L("色盲友好（Okabe-Ito）", "Colour-blind safe (Okabe-Ito)"), ["#0072b2", "#e69f00", "#009e73", "#cc79a7", "#56b4e9", "#d55e00"]],
  ["tableau", L("经典（Tableau）", "Classic (Tableau)"), ["#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f", "#edc948"]],
  ["office", L("商务（Office）", "Business (Office)"), ["#4472c4", "#ed7d31", "#a5a5a5", "#ffc000", "#5b9bd5", "#70ad47"]],
  ["calm", L("柔和", "Calm"), ["#3b6e8f", "#c8875e", "#6a9a7b", "#a26f9b", "#d1b05c", "#5d8fb8"]],
  ["echarts", L("ECharts 经典", "ECharts classic"), ["#5470c6", "#91cc75", "#fac858", "#ee6666", "#73c0de", "#3ba272"]],
  ["material", L("Material 鲜明", "Material"), ["#1e88e5", "#e53935", "#43a047", "#fb8c00", "#8e24aa", "#00acc1"]],
  ["vivid", L("高对比（投影 / 打印）", "High contrast (projector / print)"), ["#0057e7", "#d62d20", "#008744", "#ffa700", "#8e44ad", "#00a8c6"]],
  ["pastel", L("马卡龙（浅色）", "Pastel"), ["#7fb3d5", "#f5b7b1", "#a9dfbf", "#f9e79f", "#d2b4de", "#a3e4d7"]],
  ["morandi", L("莫兰迪（低饱和）", "Morandi (muted)"), ["#7a8b99", "#b49b85", "#91a18a", "#c4a4a4", "#8e9bb5", "#c2b28f"]],
  ["warm", L("暖色", "Warm"), ["#c0392b", "#e67e22", "#f1c40f", "#a0522d", "#d35486", "#e59866"]],
  ["cool", L("冷色（蓝绿）", "Cool (blue-green)"), ["#1f5f99", "#17a2b8", "#2a9d8f", "#5e60ce", "#48bfe3", "#264653"]],
  ["chinese", L("中国传统色", "Chinese traditional"), ["#c3272b", "#1661ab", "#789262", "#e9a118", "#574266", "#9d2933"]],
  ["mono", L("灰阶 + 强调色", "Greys + accent"), ["#1f6fd1", "#3d4a5c", "#6b7889", "#98a3b1", "#c2c9d2", "#e08a2c"]],
];

export const MAX_CUSTOM = 12;

/** "#aa0000, #00bb00 …" → valid hex list (≥ 2), else null. */
export function parseColors(t: string): string[] | null {
  const c = t.split(/[\s,，;；]+/).map((x) => x.trim()).filter(Boolean).map((x) => (x.startsWith("#") ? x : "#" + x));
  return c.length >= 2 && c.every((x) => /^#[0-9a-f]{6}$/i.test(x)) ? c : null;
}

export const swatches = (p: unknown): string[] =>
  Array.isArray(p) ? (p as string[]).slice(0, 12) : (PALETTE_NAMES.find((x) => x[0] === p) ?? PALETTE_NAMES[0])[2];

export function Swatch({ colors }: { colors: string[] }) {
  return <span className="swatch">{colors.map((c, i) => <i key={i} style={{ background: c }} />)}</span>;
}
