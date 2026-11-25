# 07 · BI 看板规格（DashboardSpec v1）

## 设计原则
- 模型只写**语义**（数据集 SQL + 列映射 + 聚合），不写 ECharts option；数据由程序填充，渲染器（`Resources/web/dashboard.js`）确定性地编译为 ECharts。借鉴 SQLBot `{type, axis}`、WrenAI“无数据 Vega-Lite”、GPT-Vis。
- 数据集 = 一条只读 SQL，应按所有可选维度预聚合（≤10 万行）；前端对它做筛选与二次聚合，交互即时。
- 创建/更新时校验：类型、聚合函数、数据集 SQL 可执行、**所有字段必须存在于数据集列中**，错误回传模型自纠。
- 配色遵循已验证的分类色板（8 色固定顺序，超过 8 个系列折叠为“其他”）、单轴、浅/深主题分别取色。

## 结构
```json
{
  "version": 1,
  "id": "db_xxxx",                       // 自动生成
  "title": "销售看板",
  "datasets": { "d1": { "sql": "SELECT 大区, 品类, date_trunc('month', 下单日期) AS 月份, sum(金额) AS 销售额 FROM ... GROUP BY ALL" } },
  "filters": [
    { "id": "f1", "field": "大区", "type": "multiselect", "dataset": "d1", "label": "大区" },
    { "id": "f2", "field": "月份", "type": "daterange",  "dataset": "d1" }
  ],
  "widgets": [
    { "id": "w1", "type": "kpi", "title": "总销售额", "dataset": "d1", "measure": {"field": "销售额", "agg": "sum"}, "format": "¥,.0f" },
    { "id": "w2", "type": "bar", "title": "销售额", "dataset": "d1", "x": "大区", "y": [{"field": "销售额", "agg": "sum"}],
      "series": null, "dimensionOptions": ["大区", "品类"], "measureOptions": ["销售额", "订单数"],
      "typeOptions": ["bar", "pie", "line"], "crossFilter": true, "options": {"limit": 20} },
    { "id": "w5", "type": "text", "title": "洞察", "content": "**华东区**贡献 25%" }
  ],
  "layout": [ { "i": "w1", "x": 0, "y": 0, "w": 3, "h": 2 } ]   // 12 列栅格，行高 64px；可省略自动排版
}
```

| 字段 | 说明 |
|---|---|
| `filters[].type` | `multiselect` / `select` / `daterange` / `range` |
| `widgets[].type` | `kpi bar hbar stacked_bar line area pie donut radar scatter heatmap funnel treemap table text` |
| `y[].agg` / `measure.agg` | `sum avg count count_distinct max min none` |
| `series` | 拆分系列的列（bar/line/area），热力图中作为 y 轴 |
| `dimensionOptions` / `measureOptions` / `typeOptions` | 卡片头部的维度、指标、图表类型切换器 |
| `crossFilter` | 点击图元联动其它组件（默认 true） |
| `format` | `¥,.0f`、`0.0%`、`,.2f`；缺省时中文自动用 万/亿 |

## 图表编译规则（摘要）
- bar/line：非时间类按值降序，时间/数字类按自然序；最多 30 个类别（line 400）；单系列 ≤8 类目时直接标注数值；有 `series` 时取前 7 个 + “其他”。
- pie/donut/funnel：最多 8 片（第 8 片为“其他”）。
- radar：≥3 个指标时“指标为轴、每个 x 一条多边形（≤6）”；否则“x 为轴、每个指标一条”（多指标时各自归一化到 100）。
- scatter：y[0]→x 轴、y[1]→y 轴，`series` 最多 3 组（全对比较色板只保证前 3 色）。
- heatmap：x × series，单色顺序色阶。
- kpi：筛选后值；有筛选时副标题显示占总体比例。

## 分析能力（widget.options，聊天图表与看板通用）
| 字段 | 取值 | 作用 |
|---|---|---|
| `refLines` | `"avg"` / `"median"` / `"max"` / `"min"` / `"minmax"` / `{type:"value", value:100, label:"目标"}`，可为数组 | 画水平（hbar 为垂直）虚线参考线，**按筛选后的数据实时计算**；≤3 个系列时每个系列一条，否则只画第一个 |
| `calc` | `"yoy"`（较上期增长率）/ `"cumsum"` / `"share"` / `"ma3"` | 派生指标，在聚合之后计算；增长率、占比自动用百分比格式 |
| `lineMeasures` 或 y 项的 `chart:"line"` | 指标名数组 | 柱 + 线组合图（单一 Y 轴；单位不同的指标请拆成两张图，不用双 Y 轴） |
| `breakdownOptions`（widget 字段） | 字段名数组 | 卡片上可选的细分维度（等同 `series`） |

卡片标题栏的 **ƒx** 按钮展开分析栏：参考线（无 / 平均 / 中位数 / 最大最小）、细分、计算（原值 / 增长率 / 累计 / 占比 / 3 期移动平均）、汇总（求和 / 平均 / 计数 / 最大 / 最小）。有分析设置时 ƒx 高亮。`DashboardValidator.checkAnalysis` 校验取值。
例：工会人数按年度 + 年平均线 → `{"type":"bar","x":"年度","y":[{"field":"人数","agg":"sum"}],"options":{"refLines":["avg"]}}`。测试：`liveDashboardAnalysis`（DP_LIVE_CHROME=1）。

## 字号随宽度缩放
渲染器按视图宽度计算缩放系数 `FS = clamp(宽度 / 720, 0.85, 1.3)`（聊天内联图以 640 为基准），ECharts 的所有 `fontSize` 和 CSS 字号（`calc(… * var(--fs))`）都乘以它。拖动窗口或检查器分隔条时先 `resize()` 图表，停止拖动约 180 ms 后若系数变化超过 0.04 再重建选项，避免拖动过程中反复重绘。指标卡数字用容器查询单位（`cqw`）按卡片宽度缩小，并由 `fitKPI()` 逐像素收缩到不溢出（悬停显示完整值）。

检查器宽度上限 = 详情区宽度 − 420pt（聊天列至少保留约 420pt），拉宽检查器只会挤压聊天列，不会挤压左侧侧栏。

## 交互
筛选器一行置顶；多选下拉；日期区间默认显示数据范围；“重置筛选”；交叉筛选标签（点 × 移除）；表格列头点击排序；每卡片 ▦ 切换数据表视图。交互事件回传宿主：`widgetChange`（用于学习图表/维度偏好）、`filtersChanged`、`crossFilter`、`export`。

## 导出
- **HTML**：内联 ECharts + 渲染器 + 规格 + 数据，离线可交互（`WebAssets.standaloneHTML`）。
- **规格 JSON**：可重新导入。
- **PNG**：整个看板（标题、当前筛选、所有卡片按屏幕位置，2 倍清晰度）；明细表只含卡片内可见的行。
- 看板规格存于 `workspace/dashboards/<id>.json`，打开时实时重查数据，数据更新后看板自动反映。

## 增量修改
`dashboard_update {id, patch:{widgets:[{id:"w2", type:"bar"}]}, remove:["w5"]}`：按 id 合并组件，未给出 layout 时自动重排。


## 加载性能（2026-11）
以 Olist 看板为例（18 个组件、9 个数据集，最大的数据集 13k 行 ≈ 0.9 MB）实测，原来打开一次约 1.5–2 s，主要耗时在：
- 数据集**逐个串行**查询（约 1 s）；
- Swift 用 `JSONDecoder` 解析内核返回的大结果（每 MB 约 0.25 s）；
- 刷新时整个 WebView 被转圈替换，重新加载 ECharts 并重绘所有卡片；
- SwiftUI 每次重绘都把整份数据重新编码成脚本字符串再比较。

ECharts 渲染本身只占约 0.15 s。

**改进**
- `DashboardStore.hydrate`：数据集**并行**查询（项目内核支持并发）。结果按「工作区 + SQL + cap」缓存在内存（`HydrateCache`，最多 200 条），以 `data.duckdb` / `.wal` 的修改时间和大小作为数据版本，数据库没变化就直接命中缓存。实测冷启动 1.08 s → 0.54 s，命中缓存约 0.3 ms。
- `JSONValue.parse` 改用 `JSONSerialization`，约快 3 倍。顺带修正了 NSNumber 1/0 被当成 true/false 的桥接问题，测试见 `jsonParseNumbersAndBools`。
- 看板页：切换或重新打开看板时，先立即显示上次的数据（`cachedData`），后台再刷新；刷新期间保留 WebView，只在右上角显示一个小转圈；数据没变化时不重绘。
- `WebChartView` 改为比较 payload 和主题是否变化，不再每次生成脚本字符串来比较。

**大数据集首次打开慢（2026-11 补充）**：Contoso 1m 看板（单个数据集 10 万行 × 16 列，约 18 MB JSON）首次打开约 35 s，100k 看板约 16 s。根因不是缓存，而是 App 读取内核输出时的按行切分（`DataBox.appendTakingLines`）：每收到一块数据（约 16–64 KB）都从缓冲区开头重新查找换行符，18 MB 的单行结果复杂度是 O(n²)。改为只扫描新追加的字节（线性）后：1m 看板 35 s → 2.1 s，100k 看板 16 s → 1.2 s；其余耗时主要是 DuckDB 查询（约 1.1 s）和 JSON 解析（约 1 s）。测试：`pipeLineSplitterIsLinearAndCorrect`（18 MB 单行、CRLF、跨块边界）。

**数据传输去掉重复编解码（2026-11 第二轮）**：原则不变，数据集保持明细粒度，不做粗粒度预聚合。改动如下：
- 内核：`query_data` 加 `to_file: true` 后，结果行只用 pandas `to_json` 序列化一次，写到 `tmp/qd_*.json`，返回里只带列信息和 `rows_file`。原来的流程是先 `json.loads` 再 `json.dumps`，同一份数据要序列化两遍。
- DuckDB 线程数改为 `min(8, CPU 核数)`。1m 数据集实测：4 线程 1.23 s，8 线程 0.58 s。
- Swift：读入临时文件后立即删除，原始字节存进 `DatasetRows`（按引用、LRU 淘汰、上限 768 MB）。数据集元信息里只放 `rows_ref`，`HydrateCache` 缓存的也是这个引用，不在 Swift 里解析 18 MB 的数据。
- 渲染：`DashboardStore.inlineDataJS` 把原始行 JSON 原样拼进 `DP.renderDashboard(...)` 脚本。JS 引擎解析数组字面量很快，18 MB 实测约 0.4 s。导出 HTML 也走这个函数。
- 实测（1m：10 万行 × 16 列；100k：6.8 万行 × 19 列）：
  | 环节 | 1m 改前 | 1m 改后 | 100k 改后 |
  |---|---|---|---|
  | 取数（hydrate） | 2.1 s | 0.93 s | 0.30 s |
  | 生成页面脚本 | 0.45 s（编码） | 0.008 s | 0.007 s |
  | Swift 解析 | 约 1 s | 0 | 0 |
  | ECharts 渲染 | 0.32 s | 0.32 s | 0.19 s |
- 测试：`dashboardRawRowsPipeline` 覆盖 5000 行、含引号的字符串、日期；验证临时文件已删除、拼出的字面量是合法 JSON、第二次调用命中缓存。

## spec 版本与卡片编辑（2026-11-10）
- `version: 2`：`y` 统一为 `{field, agg}`；显示开关放在 `options.display`（legend / label / splitLine / symbol）。旧 spec 加载时由 `dashboard.js` 的 `migrateSpec` 自动升级。
- 卡片 ✎ 编辑面板：维度、度量（含 measures）、汇总方式、颜色分组 `series`、排序 `options.sort`（value_desc / value_asc / dim_asc / dim_desc）、最多显示 `options.limit`、显示开关。修改即时生效，并以 `{type:'widgetEdit', widget, patch}` 通知外壳写回 spec。
