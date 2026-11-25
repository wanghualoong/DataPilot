import { expect, it } from "vitest";
import { builtins, canonicalCmd, filterCmds, parseSlash } from "./slash";
it("parses and ranks slash commands", () => {
  expect(parseSlash("/export html")).toEqual({ name: "export", args: "html" });
  expect(parseSlash("/btw 哪个大区\n最高")).toEqual({ name: "btw", args: "哪个大区\n最高" });
  expect(parseSlash("hello")).toBeNull();
  expect(filterCmds(builtins, "da")[0].name).toBe("dashboard");
  expect(filterCmds(builtins, "cpt").map((c) => c.name)).toContain("compact");
});
import { mergePatch } from "./components/settings/common";
it("config merge patch matches the core", () => {
  expect(mergePatch({ a: { b: 1, c: 2 }, d: [1] }, { a: { b: null, e: 3 }, d: [2] })).toEqual({ a: { c: 2, e: 3 }, d: [2] });
});

it("accepts aliases for slash commands", () => {
  expect(canonicalCmd("skill_save")).toBe("skill-new");
  expect(canonicalCmd("save_skill")).toBe("skill-new");
  expect(canonicalCmd("skill_new")).toBe("skill-new");
  expect(canonicalCmd("Context")).toBe("context");
  expect(canonicalCmd("my_skill", ["my_skill"])).toBe("my_skill");
  expect(canonicalCmd("nope")).toBe("nope");
  expect(canonicalCmd("数据对比", ["data-compare"])).toBe("data-compare");
  expect(filterCmds([...builtins, { name: "data-compare", desc: "数据对比 / Data Compare" }], "对比").map((c) => c.name)).toContain("data-compare");
});
