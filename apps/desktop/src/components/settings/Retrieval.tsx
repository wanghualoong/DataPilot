import { useEffect, useState } from "react";
import { L } from "../../i18n";
import { core } from "../../transport";

/** 索引与检索 (Swift RetrievalSettings): the keyword-only pipeline and the core-level index sizes. */
export function Retrieval() {
  const [n, setN] = useState<[string, number | undefined][]>([["qsql", undefined], ["datasets", undefined]]);
  useEffect(() => {
    Promise.all([
      core.request("memory/qsql", { limit: 100000 }).then((r) => r.count ?? r.qsql?.length ?? 0).catch(() => undefined),
      core.request("memory/datasets", { limit: 100000 }).then((r) => r.datasets?.length ?? 0).catch(() => undefined),
    ]).then(([q, d]) => setN([["qsql", q], ["datasets", d]]));
  }, []);
  return (
    <div className="pane">
      <section>
        <h4>{L("当前检索管线", "Pipeline")}</h4>
        <div>{L("与 Codex / Claude Code 一致：不使用向量模型。记忆（主题文件）、Q-SQL 用关键词全文检索（SQLite FTS5 trigram，中文可用），无命中时退化为中文二元组重叠匹配；文件内容由 Agent 调用 grep / glob 工具按需搜索。",
          "Same as Codex / Claude Code: no vector models. Memories (topic files) and Q-SQL use keyword full-text search (SQLite FTS5 trigram) with a bigram-overlap fallback; file contents are searched on demand with the grep / glob tools.")}</div>
      </section>
      <section>
        <h4>{L("索引", "Indexes")}</h4>
        {n.map(([k, c]) => <div key={k} className="irow"><span className="grow">{k}</span><span className="muted num">{c === undefined ? "…" : L(`条目 ${c}`, `items ${c}`)}</span></div>)}
      </section>
    </div>
  );
}
