import { describe, expect, it } from "vitest";
import { pinCharts } from "./pin";

describe("pinCharts", () => {
  it("pins all charts in order and returns the dashboard id", async () => {
    const seen: number[] = [];
    const r = await pinCharts([1, 2, 3], async (c) => { seen.push(c); return "dash"; });
    expect(seen).toEqual([1, 2, 3]);
    expect(r).toEqual({ id: "dash", added: 3, failed: 0 });
  });
  it("skips a chart that fails and keeps pinning the rest", async () => {
    const seen: number[] = [];
    const r = await pinCharts([1, 2, 3], async (c) => { seen.push(c); if (c === 2) throw new Error("no sql"); return "dash"; });
    expect(seen).toEqual([1, 2, 3]);
    expect(r).toEqual({ id: "dash", added: 2, failed: 1 });
  });
  it("reports no id when every chart fails", async () => {
    const r = await pinCharts([1], async () => { throw new Error("x"); });
    expect(r).toEqual({ id: "", added: 0, failed: 1 });
  });
});
