import { useState } from "react";
import { LoaderCircle } from "lucide-react";
import { L } from "../../i18n";
import { core } from "../../transport";
import { ic } from "../ui";
import { sid } from "./common";

/** Short, actionable text for kernel failures (instead of a raw traceback) — port of the Swift KernelErrorText. */
export function friendlyKernelError(raw: string): string {
  if (raw.includes("Kernel busy")) return L("本对话的上一步（如导入大文件）还在执行，这一步排队超时，已放弃；没有重启内核。完成后重试即可。",
    "An earlier step of this chat (e.g. a large import) is still running; this one timed out in the queue and was dropped. Nothing was restarted — retry when it finishes.");
  if (raw.includes("timed out") && raw.includes("was cancelled")) return L("这一步超时，已单独取消；内核仍在运行，其他对话不受影响。", "This step timed out and was cancelled on its own; the kernel keeps running and other chats are unaffected.");
  if (raw.includes("timed out") && raw.includes("could not be cancelled")) return L("这一步超时且无法中断（卡在底层计算），已重启内核；表和已保存的 DataFrame 会保留。", "This step timed out and could not be interrupted, so the kernel was restarted; tables and saved DataFrames are kept.");
  if (raw.includes("Conflicting lock")) return L("数据库正被另一个内核进程占用（通常是上次重启没清理干净）。点「重启内核」即可恢复。", "The database is locked by another kernel process (left over from a restart). Click “Restart kernel”.");
  if (isKernelProblem(raw)) return L("数据内核没有在运行或刚被重启。点「重启内核」后重试。", "The data kernel is not running or was just restarted. Click “Restart kernel” and retry.");
  return raw.slice(0, 400);
}

export const isKernelProblem = (raw: string) =>
  ["Conflicting lock", "Kernel not running", "kernel exited", "kernel restarted"].some((k) => raw.includes(k));

/** Error line + "Restart kernel" (only for kernel problems) + "Copy details". */
export function KernelError({ raw, reload, className = "" }: { raw: string; reload?: () => void; className?: string }) {
  const [busy, setBusy] = useState(false);
  const restart = async () => {
    setBusy(true);
    try { await core.request("kernel/restart", { session: sid() }); } catch { /* reload shows the new state */ }
    setBusy(false); reload?.();
  };
  return (
    <div className={"kerr small " + className}>
      <div className="err">{friendlyKernelError(raw)}</div>
      <div className="row">
        {isKernelProblem(raw) && <button disabled={busy} onClick={restart}>{L("重启内核", "Restart kernel")}</button>}
        {busy && <LoaderCircle {...ic} size={13} className="spin muted" />}
        <button className="link" onClick={() => navigator.clipboard?.writeText(raw)}>{L("复制详情", "Copy details")}</button>
      </div>
    </div>
  );
}
