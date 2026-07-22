import { describe, expect, it } from "vitest";

import { createGreetPayload } from "../../src/lib/greeting.js";

describe("createGreetPayload", () => {
  it("trims a provided name", () => {
    expect(createGreetPayload("  Mizu  ")).toEqual({ name: "Mizu" });
  });

  it("uses the default Tauri name when the input is blank", () => {
    expect(createGreetPayload("   ")).toEqual({ name: "Tauri" });
  });
});
