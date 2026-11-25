import { describe, expect, it } from "vitest";
import { imeEnter } from "./ime";

describe("imeEnter", () => {
  it("treats IME commit keys as composition", () => {
    expect(imeEnter({ key: "Enter", keyCode: 229, nativeEvent: { isComposing: false, keyCode: 229 } })).toBe(true);
    expect(imeEnter({ key: "Enter", keyCode: 13, nativeEvent: { isComposing: true } })).toBe(true);
  });
  it("lets a plain Enter through", () => {
    expect(imeEnter({ key: "Enter", keyCode: 13, nativeEvent: { isComposing: false, keyCode: 13 } })).toBe(false);
  });
});
