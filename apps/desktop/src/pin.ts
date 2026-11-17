// "一键生成看板": pin every chart in order; a chart that cannot be pinned is skipped (same as Swift pinAllCharts).
export async function pinCharts<T>(charts: T[], pin: (c: T) => Promise<string>): Promise<{ id: string; added: number; failed: number }> {
  let id = "", added = 0, failed = 0;
  for (const c of charts) {
    try { id = await pin(c); added++; } catch { failed++; }
  }
  return { id, added, failed };
}
