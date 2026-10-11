import { afterEach, describe, expect, it, vi } from "vitest";

import { api } from "../../src/lib/api.js";
import type { ReportFormat, ReportView } from "../../src/lib/types.js";

afterEach(() => vi.unstubAllGlobals());

describe("report IPC transport", () => {
  it.each<[ReportView, ReportFormat]>([
    ["ranking", "html"],
    ["ranking", "pdf"],
    ["tier", "html"],
    ["tier", "pdf"],
  ])("passes raw bytes with the %s view and %s format metadata", async (view, format) => {
    const bytes = new Uint8Array(new ArrayBuffer(16), 3, 5);
    bytes.set([37, 80, 68, 70, 45]);
    const transport = vi.fn().mockResolvedValue(true);
    vi.stubGlobal("window", { __TAURI_INTERNALS__: { invoke: transport } });

    expect(await api.saveReport(format, bytes, view)).toBe(true);

    expect(transport).toHaveBeenCalledOnce();
    const [command, payload, options] = transport.mock.calls[0]!;
    expect(command).toBe("save_report");
    expect(payload).toBe(bytes);
    expect(options).toEqual({
      headers: { "x-pairrank-report-format": format, "x-pairrank-report-view": view },
    });
  });
});
