import { describe, expect, it } from "vitest";
import { confirmBody, memStatus } from "./components/inspector/MemoryPanel";

describe("memory status", () => {
  it("reads status / origin and confirms", () => {
    const b = "金额用万元\n\n**依据：** 助手说的\n\n**来源：** 助手推断（待用户确认）\n\n**状态：** pending";
    expect(memStatus(b)).toEqual({ status: "pending", origin: "助手推断（待用户确认）" });
    const c = confirmBody(b);
    expect(memStatus(c).status).toBe("confirmed");
    expect(memStatus(c).origin).toMatch(/^用户在记忆面板确认/);
    expect(memStatus("x").status).toBeUndefined();
    expect(confirmBody("y\n\n**状态：** pending")).toMatch(/\*\*来源：\*\* 用户在记忆面板确认/);
  });
});
