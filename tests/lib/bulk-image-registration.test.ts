import { afterEach, describe, expect, it, vi } from "vitest";

import {
  runBulkImageRegistration,
  stopBulkImageRegistration,
  type BulkImageRun,
} from "../../src/lib/bulk-image-registration.js";
import { setLanguagePreference } from "../../src/lib/i18n.js";

function setup() {
  const state = { bulkRun: null as BulkImageRun | null, busy: false, error: "old", notice: "old" };
  const registerImage = vi.fn<(itemId: number) => Promise<unknown>>().mockResolvedValue("skipped");
  const refreshList = vi.fn<() => Promise<void>>().mockResolvedValue(undefined);
  const changed = vi.fn();
  return { state, dependencies: { registerImage, refreshList, changed } };
}

afterEach(() => setLanguagePreference("ja"));

describe("bulk image registration", () => {
  it("publishes initial, per-item and final progress without needing an app controller", async () => {
    const { state, dependencies } = setup();
    dependencies.registerImage
      .mockResolvedValueOnce("registered")
      .mockResolvedValueOnce("skipped")
      .mockResolvedValueOnce("unavailable");
    const progress: unknown[] = [];
    dependencies.changed.mockImplementation(() => {
      progress.push({
        busy: state.busy,
        error: state.error,
        notice: state.notice,
        ...state.bulkRun,
      });
    });
    await runBulkImageRegistration(state, [5, 2, 9], dependencies);
    expect(dependencies.registerImage.mock.calls).toEqual([[5], [2], [9]]);
    expect(progress).toHaveLength(5);
    expect(progress[0]).toMatchObject({
      busy: true,
      error: "",
      notice: "",
      total: 3,
      done: 0,
      running: true,
    });
    expect(progress.slice(1, 4)).toMatchObject([
      { done: 1, registered: 1, skipped: 0, running: true },
      { done: 2, registered: 1, skipped: 1, running: true },
      { done: 3, registered: 1, skipped: 2, running: true },
    ]);
    expect(progress[4]).toMatchObject({ busy: false, done: 3, running: false });
    expect(dependencies.refreshList).toHaveBeenCalledOnce();
  });

  it("finishes the active item after stop, then refreshes with partial counts", async () => {
    const { state, dependencies } = setup();
    let finish!: (outcome: string) => void;
    dependencies.registerImage.mockReturnValueOnce(new Promise((resolve) => (finish = resolve)));
    const running = runBulkImageRegistration(state, [3, 4], dependencies);
    stopBulkImageRegistration(state, dependencies.changed);
    expect(state.bulkRun).toMatchObject({ done: 0, stopping: true, running: true });
    expect(dependencies.refreshList).not.toHaveBeenCalled();
    finish("registered");
    await running;
    expect(dependencies.registerImage).toHaveBeenCalledExactlyOnceWith(3);
    expect(state.bulkRun).toMatchObject({ done: 1, registered: 1, stopped: true, running: false });
    expect(dependencies.refreshList).toHaveBeenCalledOnce();
    expect(state.notice).toContain("停止しました。1 / 2 件を処理");
  });

  it.each([undefined, null, "unexpected"])(
    "rejects unknown outcome %s without counting it",
    async (outcome) => {
      const { state, dependencies } = setup();
      dependencies.registerImage.mockResolvedValueOnce(outcome);
      await runBulkImageRegistration(state, [1, 2], dependencies);
      expect(dependencies.registerImage).toHaveBeenCalledOnce();
      expect(state.bulkRun).toMatchObject({ done: 0, registered: 0, skipped: 0, running: false });
      expect(state.error).toBe("画像登録の結果を確認できませんでした。");
      expect(dependencies.refreshList).toHaveBeenCalledOnce();
      expect(state.busy).toBe(false);
    },
  );

  it("retains the stop notice when the active item and refresh both fail", async () => {
    const { state, dependencies } = setup();
    dependencies.registerImage.mockImplementationOnce(async () => {
      stopBulkImageRegistration(state, dependencies.changed);
      throw new Error("registration failed");
    });
    dependencies.refreshList.mockRejectedValueOnce("reload failed");
    await runBulkImageRegistration(state, [1, 2], dependencies);
    expect(state.error).toBe("registration failed リストを再取得できませんでした: reload failed");
    expect(state.notice).toContain("停止しました。0 / 2 件を処理");
    expect(state.bulkRun).toMatchObject({ stopped: true, running: false });
    expect(state.busy).toBe(false);
  });

  it("starts a new run with fresh counters and cleared messages", async () => {
    const { state, dependencies } = setup();
    dependencies.registerImage.mockRejectedValueOnce(new Error("first run failed"));
    await runBulkImageRegistration(state, [1, 2], dependencies);
    const firstRun = state.bulkRun;
    dependencies.registerImage.mockResolvedValueOnce("registered");
    await runBulkImageRegistration(state, [1], dependencies);
    expect(state.bulkRun).not.toBe(firstRun);
    expect(state.bulkRun).toEqual({
      total: 1,
      done: 1,
      registered: 1,
      skipped: 0,
      stopping: false,
      stopped: false,
      running: false,
    });
    expect(state.error).toBe("");
    expect(state.notice).toContain("完了しました。1 / 1 件を処理");
  });

  it("continues to localize errors and completion notices", async () => {
    setLanguagePreference("en");
    const { state, dependencies } = setup();
    dependencies.registerImage.mockResolvedValueOnce("unknown");
    await runBulkImageRegistration(state, [1], dependencies);
    expect(state.error).toBe("Could not confirm the image registration result.");
    expect(state.notice).toBe(
      "Bulk image registration interrupted. Processed 0 / 1 items; registered 0, skipped 0.",
    );
  });
});
