import { IconBtn } from "../ui";
import { BadgeCheck, FileText, Pencil, Table2, Trash2 } from "lucide-react";
import { ask } from "../../native";
import { useEffect, useState } from "react";
import { L } from "../../i18n";
import { useApp } from "../../store";
import { core } from "../../transport";
import { openPath } from "../../native";
import { Markdown } from "../Markdown";
import { sid, useLoad } from "./common";

type Topic = { file: string; name: string; description: string; type: string; body: string; modified?: number; usage?: { recalled?: number; cited?: number; count?: number } | null };

const typeLabel = (t: string) => ({ project: L("口径/规则", "project"), feedback: L("纠正", "feedback"), reference: L("资料入口", "reference"), user: L("用户", "user") } as Record<string, string>)[t] ?? t;
/** Status / origin written by the extractor (**状态：** confirmed|pending, **来源：** …). */
export function memStatus(body: string): { status?: "confirmed" | "pending"; origin?: string } {
  const s = /\*\*状态：\*\*\s*(confirmed|pending)/.exec(body ?? "")?.[1] as any;
  const o = /\*\*来源：\*\*\s*([^\n]+)/.exec(body ?? "")?.[1]?.trim();
  return { status: s, origin: o };
}
/** The user confirms an inferred memory in the panel: pending → confirmed, origin noted. */
export function confirmBody(body: string): string {
  let b = body.replace(/\*\*状态：\*\*\s*pending/, "**状态：** confirmed");
  const o = "用户在记忆面板确认（" + new Date().toISOString().slice(0, 10) + "）";
  b = /\*\*来源：\*\*[^\n]*/.test(b) ? b.replace(/\*\*来源：\*\*[^\n]*/, "**来源：** " + o) : b + "\n\n**来源：** " + o;
  return b;
}
const day = (secs?: number) => (secs ? new Date(secs * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" }) : "");
const ago = (secs: number) => {
  const d = (secs * 1000 - Date.now()) / 1000, rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  const [n, u]: [number, Intl.RelativeTimeFormatUnit] = Math.abs(d) < 3600 ? [d / 60, "minute"] : Math.abs(d) < 86400 ? [d / 3600, "hour"] : [d / 86400, "day"];
  return rtf.format(Math.round(n), u);
};

function TopicForm({ t, onSave, onClose }: { t: Topic; onSave: (t: Topic) => void; onClose: () => void }) {
  const [v, setV] = useState(t);
  return (
    <div className="modal" onClick={onClose}>
      <div className="modalbox" onClick={(e) => e.stopPropagation()}>
        <h3>{t.file || L("新建记忆", "New memory")}</h3>
        <label className="fld"><span>{L("标题", "Name")}</span><input value={v.name} onChange={(e) => setV({ ...v, name: e.target.value })} /></label>
        <label className="fld"><span>{L("一句话描述（用于召回）", "One-line description (used for recall)")}</span><input value={v.description} onChange={(e) => setV({ ...v, description: e.target.value })} /></label>
        <label className="fld"><span>{L("类型", "Type")}</span>
          <select value={v.type} onChange={(e) => setV({ ...v, type: e.target.value })}>{["project", "feedback", "reference", "user"].map((x) => <option key={x} value={x}>{typeLabel(x)} {x}</option>)}</select>
        </label>
        <div className="muted small">{L("正文：规则 + **依据：** + **状态：** confirmed / pending + **How to apply:** 适用范围", "Body: rule + **依据：** (evidence) + **状态：** confirmed|pending + **How to apply:**")}</div>
        <textarea rows={8} value={v.body} onChange={(e) => setV({ ...v, body: e.target.value })} />
        <div className="row end"><button onClick={onClose}>{L("取消", "Cancel")}</button><button className="primary" disabled={!v.name.trim() || !v.body.trim()} onClick={() => onSave(v)}>{L("保存", "Save")}</button></div>
      </div>
    </div>
  );
}

function QSQLForm({ q, onSave, onClose }: { q: any; onSave: (question: string, sql: string) => void; onClose: () => void }) {
  const [question, setQ] = useState<string>(q.question ?? "");
  const [sql, setSQL] = useState<string>(q.sql ?? "");
  return (
    <div className="modal" onClick={onClose}>
      <div className="modalbox" onClick={(e) => e.stopPropagation()}>
        <h3>{L("编辑 Q-SQL 示例", "Edit Q-SQL example")}</h3>
        <label className="fld"><span>{L("问题", "Question")}</span><input value={question} onChange={(e) => setQ(e.target.value)} /></label>
        <label className="fld"><span>SQL</span><textarea className="mono" rows={8} value={sql} onChange={(e) => setSQL(e.target.value)} /></label>
        <div className="row end"><button onClick={onClose}>{L("取消", "Cancel")}</button><button className="primary" disabled={!question.trim()} onClick={() => onSave(question.trim(), sql.trim())}>{L("保存", "Save")}</button></div>
      </div>
    </div>
  );
}

/** User-level DATAPILOT.md: standing instructions written by the user, loaded into every chat (all projects). */
function Instructions() {
  const fail = useApp((s) => s.fail);
  const note = useApp((s) => s.note);
  const [text, setText] = useState<string>();
  const [saved, setSaved] = useState("");
  const [open, setOpen] = useState(false);
  useEffect(() => {
    core.request("memory/instructions", { action: "get" }).then((r: any) => { setText(r?.text ?? ""); setSaved(r?.text ?? ""); }).catch(() => setText(""));
  }, []);
  if (text === undefined) return null;
  const save = async () => {
    try { await core.request("memory/instructions", { action: "set", text }); setSaved(text); note(L("已保存「关于我」指令，新的对话轮次生效", "Saved; applies from the next turn")); } catch (e) { fail(e); }
  };
  return (
    <div className="pad instr">
      <div className="row small click" onClick={() => setOpen(!open)}>
        <b className="grow">{L("关于我 · 长期指令（DATAPILOT.md）", "About me · standing instructions (DATAPILOT.md)")}</b>
        <span className="muted xs">{saved.trim() ? L(`${saved.trim().length} 字`, `${saved.trim().length} chars`) : L("未填写", "empty")}</span>
      </div>
      {open && <>
        <div className="muted xs">{L("你自己写的长期要求，每次对话、所有项目都会加载（例如：金额用万元、结论先行、报告用表格）。与下面自动记住的条目不同，这里只由你修改。",
          "Your own standing instructions, loaded into every chat in every project (e.g. amounts in 10k CNY, conclusion first). Only you edit this; the entries below are learned automatically.")}</div>
        <textarea rows={6} value={text} placeholder={L("- 金额默认用万元，保留两位小数\n- 先给结论，再给方法\n- 小结果用表格展示", "- Amounts in 10k, two decimals\n- Conclusion first\n- Use tables for small results")} onChange={(e) => setText(e.target.value)} />
        <div className="row end"><button className="primary" disabled={text === saved} onClick={save}>{L("保存", "Save")}</button></div>
      </>}
    </div>
  );
}

function Topics({ scope, r, reload }: { scope: "user" | "project"; r: any; reload: () => void }) {
  const fail = useApp((s) => s.fail);
  const [edit, setEdit] = useState<Topic>();
  const [open, setOpen] = useState<string>();
  const save = async (t: Topic) => {
    setEdit(undefined);
    await core.request("memory/topics", { scope, session: sid(), action: "write", name: t.name, description: t.description, type: t.type, body: t.body, file: t.file || undefined }).catch(fail);
    reload();
  };
  const topics: Topic[] = r?.topics ?? [];
  return (
    <>
      {scope === "user" && <Instructions />}
      <div className="row pad small">
        <span className="muted grow">{scope === "user" ? L("跨项目的个人偏好，保存在本机；MEMORY.md 是索引", "Personal preferences for all projects; MEMORY.md is the index") : L("随项目文件夹保存；MEMORY.md 是索引", "Stored with the project; MEMORY.md is the index")}</span>
        <button onClick={() => setEdit({ file: "", name: "", description: "", type: scope === "user" ? "user" : "project", body: "" })}>{L("新建", "New")}</button>
        {r?.dir && <button disabled={!topics.length} onClick={() => openPath(r.dir + "/MEMORY.md")}>{L("打开 MEMORY.md", "Open MEMORY.md")}</button>}
      </div>
      {!topics.length && <div className="muted small pad">{scope === "project"
        ? L("说“记住：本项目 GMV 只算已支付订单”，或纠正口径时，会写到这里。", "Say “remember: GMV only counts paid orders” or correct a definition.")
        : L("还没有记忆。对话中告诉 Agent「记住…」，或点新建。", "No memories yet. Tell the agent “remember …” or click New.")}</div>}
      {topics.map((t) => (
        <div key={t.file} className="itopic">
          <div className="irow click" onClick={() => setOpen(open === t.file ? undefined : t.file)}>
            <span className="grow vcol">
              <b>{t.name}</b>
              {t.description && <span className="muted small ell2">{t.description}</span>}
              <span className="muted xs row wrap">
                <span className="ibadge">{typeLabel(t.type)}</span>
                {memStatus(t.body).status === "pending" && <span className="ibadge warn" title={memStatus(t.body).origin ?? ""}>{L("待确认", "unconfirmed")}</span>}
                {memStatus(t.body).status === "confirmed" && <span className="ibadge ok" title={memStatus(t.body).origin ?? ""}>{L("已确认", "confirmed")}</span>}
                {day(t.modified)}
                {t.usage && <>{t.usage.cited ? <span>{L("引用 ", "cited ") + ago(t.usage.cited)}</span> : t.usage.recalled ? <span>{L("召回 ", "recalled ") + ago(t.usage.recalled)}</span> : null}<span>×{t.usage.count ?? 0}</span></>}
                <span>{t.file}</span>
              </span>
            </span>
            <IconBtn icon={FileText} size={13} title={L("在编辑器中打开", "Open in editor")} onClick={() => r?.dir && openPath(`${r.dir}/${t.file}`)} />
            {memStatus(t.body).status === "pending" && (
              <IconBtn icon={BadgeCheck} size={13} title={L("确认这条记忆是对的（之后按已确认处理）", "Confirm this memory")}
                onClick={() => save({ ...t, body: confirmBody(t.body) })} />
            )}
            <IconBtn icon={Pencil} size={13} title={L("编辑", "Edit")} onClick={() => setEdit(t)} />
            <IconBtn icon={Trash2} size={13} className="danger" title={L("删除", "Delete")} onClick={async () => {
              if (!(await ask(scope === "project" ? L(`删除项目记忆「${t.name}」？`, `Delete project memory “${t.name}”?`) : L(`删除记忆「${t.name}」？`, `Delete “${t.name}”?`)))) return;
              await core.request("memory/topics", { scope, session: sid(), action: "delete", file: t.file }).catch(fail); reload();
            }} />
          </div>
          {open === t.file && <div className="ipreview"><Markdown text={t.body} /></div>}
        </div>
      ))}
      {edit && <TopicForm t={edit} onSave={save} onClose={() => setEdit(undefined)} />}
    </>
  );
}

function QSQL({ rows, datasets, reload }: { rows: any[]; datasets: any[]; reload: () => void }) {
  const fail = useApp((s) => s.fail);
  const [edit, setEdit] = useState<any>();
  const update = async (q: any, question: string, sql: string, verified: boolean) => {
    await core.request("memory/qsql", { action: "update", id: q.id, question, sql, verified }).catch(fail); reload();
  };
  const dsName = (fp: string) => datasets.find((d) => d.fp === fp)?.name || (fp ?? "").slice(0, 8);
  return (
    <>
      <div className="muted small pad">{L(`已验证的问题 → SQL 示例（${rows.length}），类似问题会参考`, `Verified question → SQL examples (${rows.length})`)}</div>
      {!rows.length && <div className="muted small pad">{L("SQL 结果正确时点工具卡片上的 👍，问题和 SQL 会保存在这里，以后同类问题会参考。", "Click 👍 on a correct SQL result to save it here.")}</div>}
      {rows.map((q) => (
        <div key={q.id} className="irow top">
          <span className="grow vcol">
            <span className="row">{q.verified && <BadgeCheck size={13} className="ok" aria-label={L("已验证（👍）", "Verified")} />}<span className="ell2">{q.question}</span></span>
            <span className="mono muted ell3">{q.sql}</span>
            <span className="muted xs">{dsName(q.dataset_fp)} · {day(q.last_used)}</span>
          </span>
          <IconBtn icon={BadgeCheck} size={13} on={q.verified} title={q.verified ? L("取消验证标记", "Unverify") : L("标为已验证", "Mark verified")} onClick={() => update(q, q.question, q.sql, !q.verified)} />
          <IconBtn icon={Pencil} size={13} title={L("编辑", "Edit")} onClick={() => setEdit(q)} />
          <IconBtn icon={Trash2} size={13} className="danger" title={L("删除", "Delete")} onClick={async () => {
            if (!(await ask(L("删除这个 Q-SQL 示例？", "Delete this Q-SQL example?")))) return;
            await core.request("memory/qsql", { action: "delete", id: q.id }).catch(fail); reload();
          }} />
        </div>
      ))}
      {edit && <QSQLForm q={edit} onClose={() => setEdit(undefined)} onSave={(question, sql) => { const q = edit; setEdit(undefined); update(q, question, sql, q.verified); }} />}
    </>
  );
}

function Datasets({ rows, reload }: { rows: any[]; reload: () => void }) {
  const fail = useApp((s) => s.fail);
  return (
    <>
      <div className="muted small pad">{L("见过的数据集（按表结构指纹识别，同结构的新文件会沿用口径）", "Datasets seen (by schema fingerprint)")}</div>
      {!rows.length && <div className="muted small pad">{L("还没有识别过数据集", "No datasets yet")}</div>}
      {rows.map((d: any) => {
        const name = d.name || d.fp.slice(0, 10);
        return (
          <div key={d.fp} className="irow top">
            <span className="grow vcol">
              <span className="row"><Table2 size={13} className="muted" /><span className="grow ell">{name}</span><span className="muted xs">{day(d.last_seen)}</span></span>
              <span className="muted xs">{L(`${d.columns} 列 · 出现在 ${d.sessions} 个会话 · ${d.qsql ?? 0} 条口径规则`, `${d.columns} cols · ${d.sessions} sessions · ${d.qsql ?? 0} rules`)}</span>
              <span className="mono muted xs">{L("指纹 ", "fingerprint ") + d.fp}</span>
            </span>
            <IconBtn icon={Pencil} size={13} title={L("重命名", "Rename")} onClick={async () => { const n = window.prompt(L("数据集名称", "Dataset name"), d.name ?? ""); if (n?.trim()) { await core.request("memory/datasets", { action: "rename", fp: d.fp, name: n.trim() }).catch(fail); reload(); } }} />
            <IconBtn icon={Trash2} size={13} className="danger" title={L("忘记这个数据集", "Forget dataset")} onClick={async () => {
              if (!(await ask(L(`忘记数据集「${name}」？它的 ${d.qsql ?? 0} 条口径规则和 Q-SQL 示例也会删除（DuckDB 中的表不受影响）。`, `Forget “${name}”? Its ${d.qsql ?? 0} rules and Q-SQL examples are deleted too (DuckDB tables are kept).`)))) return;
              await core.request("memory/datasets", { action: "delete", fp: d.fp }).catch(fail); reload();
            }} />
          </div>
        );
      })}
    </>
  );
}

export function MemoryPanel() {
  const inProject = useApp((s) => !!s.sessions.find((x) => x.id === s.selected)?.project_id);
  const [sec, setSec] = useState<string>(inProject ? "project" : "user");
  const scope = sec === "project" && inProject ? "project" : "user";
  const [topics, , reloadTopics] = useLoad(async () => core.request("memory/topics", { scope, session: sid() }), [scope]);
  const [ds, , reloadDs] = useLoad(async () => ((await core.request("memory/datasets", {})).datasets ?? []) as any[], [sec]);
  const [qs, , reloadQs] = useLoad(async () => ((await core.request("memory/qsql", {})).qsql ?? []) as any[], [sec]);
  const tabs: [string, string][] = [...(inProject ? [["project", L("项目记忆", "Project")] as [string, string]] : []), ["user", L("用户记忆", "User")], ["datasets", L("数据集", "Datasets")], ["qsql", "Q-SQL"]];
  const stat = (label: string, n: number, Icon: typeof FileText) => <div className="mstat"><span className="row"><Icon size={13} />{n}</span><span className="muted xs">{label}</span></div>;
  return (
    <div className="ipanel">
      <div className="mstats pad">
        {stat(L("主题记忆", "Topics"), topics?.topics?.length ?? 0, FileText)}
        {stat(L("数据集", "Datasets"), ds?.length ?? 0, Table2)}
        {stat(L("Q-SQL 示例", "Q-SQL"), qs?.length ?? 0, BadgeCheck)}
      </div>
      <div className="seg pad">{tabs.map(([k, t]) => <button key={k} className={sec === k ? "on" : ""} onClick={() => setSec(k)}>{t}</button>)}</div>
      {sec === "qsql" ? <QSQL rows={qs ?? []} datasets={ds ?? []} reload={() => { reloadQs(); reloadDs(); }} />
        : sec === "datasets" ? <Datasets rows={ds ?? []} reload={() => { reloadDs(); reloadQs(); }} />
        : <Topics key={scope} scope={scope} r={topics} reload={reloadTopics} />}
    </div>
  );
}
