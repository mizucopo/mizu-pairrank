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

describe("OpenAI IPC transport", () => {
  it("sends model, mode and authentication actions without credential reads", async () => {
    const transport = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("window", { __TAURI_INTERNALS__: { invoke: transport } });
    await api.saveOpenAiSettings("unlisted-model", "chatgpt");
    await api.openAiModels();
    await api.connectChatGpt(false);
    await api.connectChatGpt(true);
    await api.selectChatGptProfile("profile-a");
    await api.disconnectChatGpt();
    await api.openChatGptUsage();
    await api.setApiKey("openai", "test-key");
    await api.searchImages("openai", "りんご");
    await api.autoRegisterImage(1, 10, "openai");
    expect(transport.mock.calls.map(([command, payload]) => [command, payload])).toEqual([
      ["save_openai_settings", { model: "unlisted-model", authMode: "chatgpt" }],
      ["openai_models", {}],
      ["connect_chatgpt", { newAccount: false }],
      ["connect_chatgpt", { newAccount: true }],
      ["select_chatgpt_profile", { profileId: "profile-a" }],
      ["disconnect_chatgpt", {}],
      ["open_chatgpt_usage", {}],
      ["set_api_key", { provider: "openai", key: "test-key" }],
      ["search_images", { provider: "openai", query: "りんご" }],
      ["auto_register_image", { listId: 1, itemId: 10, provider: "openai" }],
    ]);
  });
});
