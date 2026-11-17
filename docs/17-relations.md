# 17 · 表间关系

## 存储：`data/relations.json`
```
{id, from:{table, columns[], steps?, expr?}, to:{…}, mapping?, cardinality,
 status: inferred|suspect|confirmed|rejected, match_rate, unmatched_samples[], note, source: auto|user|model, updated}
```
- 键转换 `steps`：trim、upper、lower、strip_zeros、num（1.0→1）、digits、strip_prefix、strip_spaces、`{lpad:N}`、`{replace:[a,b]}`、`{regex_extract}`、`{regex_replace}`、`{substr}`、`{prefix}`。
- `expr`：用 `{col}` 占位的表达式，sqlglot 校验，只允许白名单标量函数。
- `mapping`：对照表（`mapping_pairs` 生成 `map_*` 表），关联时经它中转。
- 被否决的关系永远不再出现；旧 catalog.json 的关系首次加载时迁移。

## 推断（`relations.infer`）
按需计算，按表结构戳缓存；候选是像键的列（名字或高基数），允许 int/str/float 互比；匹配率低时依次探测对称转换；≥95% 记为 inferred，30%–95% 记为 suspect。

## 工具与流程
1. `relation_test`（只读）：匹配率、基数、未匹配样例、两边样例、建议转换、JOIN 片段。
2. 规则不明时 `ask_user` 给出两边样例请用户说明，或请用户给对照值。
3. `relation_define`：写入前再验证（默认 ≥90%，可 `force`）；可 `materialize` 生成规范化键视图。
4. `data_schema` 的关系区给出可直接复制的 `ON` 条件。

## 数据更新后
源文件重新导入后，`op_relation_revalidate` 重测涉及这些表的已确认关系；匹配率低于 90% 且下降超过 5 个百分点时，在下一轮对话开头提示模型先告诉用户。

## 界面
- 数据面板「表间关系」：状态标记、匹配率、转换说明；确认 / 否决 / 删除；「重新推断」。
- 「关系图…」：`Resources/web/er.js`（纯 SVG，经 `host.js` 调白名单内核 op）。节点尺寸、布局存储、手工连线的数据模型改编自 Chat2DB v5.3.8 ERModal；分层布局自写，不用 elkjs。拖动位置存 `data/er_layout.json`；从列圆点拖到另一表的列新建关系；表单可选转换、填表达式或对照值并当场测试；导出 PNG。
