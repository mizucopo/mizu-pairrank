import { describe, expect, it, vi } from "vitest";

import { AppController } from "../../src/lib/controller.js";
import type {
  AppApi,
  ImageCandidate,
  ListState,
  ListSummary,
  PairProposal,
} from "../../src/lib/types.js";

function list(id = 1): ListState {
  return {
    id,
    name: `リスト${id}`,
    revision: 4,
    items: [1, 2].map((offset) => ({
      id: id * 10 + offset,
      listId: id,
      name: `項目${offset}`,
      image: null,
      rating: { mu: 25, sigma: 25 / 3 },
      comparisonCount: 0,
      tagIds: [],
    })),
    tags: [],
    comparisonCount: 0,
    convergence: {
      converged: false,
      maxSigma: 25 / 3,
      maxRankSpan: 0,
      observedAnswers: 0,
      requiredAnswers: 20,
    },
  };
}

function summary(value: ListState): ListSummary {
  return {
    id: value.id,
    name: value.name,
    itemCount: value.items.length,
    comparisonCount: value.comparisonCount,
    converged: value.convergence.converged,
  };
}

function proposal(value: ListState): PairProposal {
  const [b, a] = value.items;
  if (!a || !b) throw new Error("The fixture needs two items");
  return { listId: value.id, revision: value.revision, a, b };
}

function backend(first = list(), second = list(2)) {
  const settings = {
    braveConfigured: false,
    ollamaConfigured: false,
    defaultProvider: "brave" as const,
  };
  return {
    listSummaries: vi
      .fn<AppApi["listSummaries"]>()
      .mockResolvedValue([summary(first), summary(second)]),
    getList: vi
      .fn<AppApi["getList"]>()
      .mockImplementation(async (id) => (id === first.id ? first : second)),
    createList: vi.fn<AppApi["createList"]>().mockResolvedValue(first),
    duplicateList: vi.fn<AppApi["duplicateList"]>().mockResolvedValue(first),
    exportList: vi.fn<AppApi["exportList"]>().mockResolvedValue(true),
    importList: vi.fn<AppApi["importList"]>().mockResolvedValue(first),
    renameList: vi.fn<AppApi["renameList"]>().mockResolvedValue(first),
    deleteList: vi.fn<AppApi["deleteList"]>().mockResolvedValue(undefined),
    addItems: vi.fn<AppApi["addItems"]>().mockResolvedValue(first),
    renameItem: vi.fn<AppApi["renameItem"]>().mockResolvedValue(first),
    deleteItem: vi.fn<AppApi["deleteItem"]>().mockResolvedValue(first),
    createTag: vi.fn<AppApi["createTag"]>().mockResolvedValue(first),
    renameTag: vi.fn<AppApi["renameTag"]>().mockResolvedValue(first),
    deleteTag: vi.fn<AppApi["deleteTag"]>().mockResolvedValue(first),
    setItemTag: vi.fn<AppApi["setItemTag"]>().mockResolvedValue(first),
    resumeList: vi.fn<AppApi["resumeList"]>().mockResolvedValue(first),
    resetComparisons: vi.fn<AppApi["resetComparisons"]>().mockResolvedValue(first),
    nextPair: vi.fn<AppApi["nextPair"]>().mockResolvedValue(proposal(first)),
    answer: vi.fn<AppApi["answer"]>().mockResolvedValue(first),
    searchSettings: vi.fn<AppApi["searchSettings"]>().mockResolvedValue(settings),
    setApiKey: vi.fn<AppApi["setApiKey"]>().mockResolvedValue(undefined),
    searchImages: vi.fn<AppApi["searchImages"]>().mockResolvedValue([]),
    autoRegisterImage: vi.fn<AppApi["autoRegisterImage"]>().mockResolvedValue("skipped"),
    setLocalImage: vi.fn<AppApi["setLocalImage"]>().mockResolvedValue(first),
    setRemoteImage: vi.fn<AppApi["setRemoteImage"]>().mockResolvedValue(first),
    removeImage: vi.fn<AppApi["removeImage"]>().mockResolvedValue(first),
  } satisfies AppApi;
}

async function comparison() {
  const initial = list();
  const api = backend(initial);
  const controller = new AppController(api, vi.fn());
  await controller.initialize();
  await controller.startComparison();
  const saved: ListState = {
    ...initial,
    revision: initial.revision + 1,
    comparisonCount: 1,
    items: initial.items.map((item, index) => ({
      ...item,
      rating: { mu: index ? 27 : 23, sigma: 6.5 },
      comparisonCount: 1,
    })),
    convergence: { ...initial.convergence, observedAnswers: 1, maxSigma: 6.5 },
  };
  return { initial, saved, api, controller };
}

async function skippingComparison(itemCount = 3) {
  const fixture = list();
  const initial: ListState = {
    ...fixture,
    items: Array.from({ length: itemCount }, (_, index) => ({
      ...fixture.items[0]!,
      id: 11 + index,
      name: `項目${index + 1}`,
    })),
  };
  let current = initial;
  const api = backend(initial);
  api.resumeList.mockImplementation(async () => current);
  api.getList.mockImplementation(async () => current);
  api.nextPair.mockImplementation(async (_id, excluded = []) => {
    for (const a of current.items) {
      for (const b of current.items) {
        if (a.id >= b.id || excluded.some(([x, y]) => x === a.id && y === b.id)) continue;
        return { listId: current.id, revision: current.revision, a: b, b: a };
      }
    }
    return null;
  });
  api.answer.mockImplementation(async () => {
    current = {
      ...current,
      revision: current.revision + 1,
      comparisonCount: current.comparisonCount + 1,
    };
    return current;
  });
  const controller = new AppController(api, vi.fn());
  await controller.initialize();
  await controller.startComparison();
  return { initial, api, controller };
}

describe("skipping comparisons", () => {
  it("excludes the displayed pair without changing ratings, counts, history or revision", async () => {
    const { controller, api, initial } = await skippingComparison();
    const before = structuredClone(controller.state.active);
    const skipped = controller.state.pair;
    await controller.skipComparison();
    expect(api.nextPair).toHaveBeenLastCalledWith(initial.id, [[11, 12]], initial.revision);
    expect(controller.state.pair).not.toEqual(skipped);
    expect(controller.state.active).toEqual(before);
    expect(api.answer).not.toHaveBeenCalled();
    expect(api.resumeList).toHaveBeenCalledTimes(1);
    expect(controller.state.comparisonPaused).toBe(false);
  });

  it.each([3, 5])(
    "returns a skipped pair after %i items minus one saved answers",
    async (count) => {
      const { controller, api } = await skippingComparison(count);
      const skipped = controller.state.pair;
      await controller.skipComparison();
      for (let answer = 1; answer < count - 1; answer += 1) {
        await controller.answer("equal");
        expect(api.nextPair.mock.lastCall?.[1]).toEqual([[11, 12]]);
        expect(controller.state.pair?.a.id).not.toBe(skipped?.a.id);
      }
      await controller.answer("equal");
      expect(api.nextPair.mock.lastCall?.[1]).toEqual([]);
      expect(controller.state.pair?.a.id).toBe(skipped?.a.id);
      expect(controller.state.pair?.b.id).toBe(skipped?.b.id);
      expect(api.answer).toHaveBeenCalledTimes(count - 1);
    },
  );

  it.each([2, 3])(
    "pauses after all pairs are skipped with %i items and explicitly resumes",
    async (count) => {
      const { controller, api, initial } = await skippingComparison(count);
      const pairs = (count * (count - 1)) / 2;
      for (let index = 0; index < pairs; index += 1) await controller.skipComparison();
      expect(controller.state.pair).toBeNull();
      expect(controller.state.view).toBe("compare");
      expect(controller.state.comparisonPaused).toBe(true);
      expect(controller.state.active).toEqual(initial);
      expect(controller.state.active?.convergence.converged).toBe(false);
      expect(api.nextPair).toHaveBeenCalledTimes(pairs + 1);
      await controller.skipComparison();
      await controller.answer("equal");
      expect(api.nextPair).toHaveBeenCalledTimes(pairs + 1);
      expect(api.answer).not.toHaveBeenCalled();
      // Merely reopening the Compare tab must not lose the cooldown.
      await controller.navigate("ranking");
      await controller.navigate("compare");
      expect(controller.state.comparisonPaused).toBe(true);
      await controller.resumeSkippedComparisons();
      expect(controller.state.pair).not.toBeNull();
      expect(controller.state.comparisonPaused).toBe(false);
      expect(api.nextPair).toHaveBeenLastCalledWith(initial.id, [], initial.revision);
      expect(controller.state.active).toEqual(initial);
      expect(controller.state.error).toBe("");
    },
  );

  it("keeps the proposal and cooldown unchanged on selection failure so skip can be retried", async () => {
    const { controller, api } = await skippingComparison();
    const pair = controller.state.pair;
    api.nextPair.mockRejectedValueOnce(new Error("選択に失敗"));
    await controller.skipComparison();
    expect(controller.state.pair).toEqual(pair);
    expect(controller.state.error).toBe("選択に失敗");
    await controller.skipComparison();
    expect(api.nextPair.mock.lastCall?.[1]).toEqual([[11, 12]]);
    expect(controller.state.error).toBe("");
  });

  it("does not advance a cooldown when an answer fails", async () => {
    const { controller, api } = await skippingComparison();
    await controller.skipComparison();
    api.answer.mockRejectedValueOnce("保存に失敗");
    await controller.answer("a_weak");
    await controller.answer("a_weak");
    expect(api.nextPair.mock.lastCall?.[1]).toEqual([[11, 12]]);
    await controller.answer("a_weak");
    expect(api.nextPair.mock.lastCall?.[1]).toEqual([]);
  });

  it("rejects a stale skip proposal without treating it as exhaustion", async () => {
    const { controller, api } = await skippingComparison();
    api.nextPair.mockRejectedValueOnce(
      "リストが更新されています。最新の比較を読み直してください。",
    );
    await controller.skipComparison();
    expect(controller.state.pair).toBeNull();
    expect(controller.state.comparisonPaused).toBe(false);
    expect(controller.state.error).toContain("もう一度比較を開始");
    await controller.startComparison();
    expect(api.nextPair.mock.lastCall?.[1]).toEqual([]);
    expect(controller.state.pair).not.toBeNull();
  });

  it("gates duplicate skips, answers and modal operations while selecting a pair", async () => {
    const { controller, api } = await skippingComparison();
    const next = controller.state.pair!;
    let finish!: () => void;
    api.nextPair.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = () => resolve(next);
        }),
    );
    const pending = controller.skipComparison();
    await controller.skipComparison();
    await controller.answer("equal");
    await controller.resumeSkippedComparisons();
    expect(api.nextPair).toHaveBeenCalledTimes(2);
    expect(api.answer).not.toHaveBeenCalled();
    finish();
    await pending;
    await controller.openModal({ kind: "rename-list" });
    await controller.skipComparison();
    expect(api.nextPair).toHaveBeenCalledTimes(2);
  });

  it("clears held pairs on comparison reset and list switch", async () => {
    const { controller, api, initial } = await skippingComparison();
    await controller.skipComparison();
    await controller.openModal({ kind: "reset-comparisons" });
    await controller.resetComparisons();
    await controller.startComparison();
    expect(api.nextPair).toHaveBeenLastCalledWith(initial.id, [], initial.revision);
    await controller.skipComparison();
    api.getList.mockResolvedValueOnce(list(2));
    await controller.selectList(2);
    await controller.selectList(initial.id);
    await controller.startComparison();
    expect(api.nextPair).toHaveBeenLastCalledWith(initial.id, [], initial.revision);
  });
});

async function listSelection(context: "startup" | "after deletion") {
  const api = backend();
  const controller = new AppController(api, vi.fn());
  if (context === "after deletion") {
    await controller.initialize();
    controller.state.drafts.items = "deleted list draft";
    await controller.openModal({ kind: "delete-list" });
  }
  const load = () => (context === "startup" ? controller.initialize() : controller.confirmDelete());
  return { api, controller, load };
}

describe("comparison reset", () => {
  it("requires confirmation and preserves the current comparison on cancellation", async () => {
    const { controller, api } = await comparison();
    const before = controller.state.active;
    const pair = controller.state.pair;
    await controller.resetComparisons();
    expect(api.resetComparisons).not.toHaveBeenCalled();
    await controller.openModal({ kind: "reset-comparisons" });
    controller.closeModal();
    expect(api.resetComparisons).not.toHaveBeenCalled();
    expect(controller.state.active).toBe(before);
    expect(controller.state.pair).toBe(pair);
    expect(controller.state.view).toBe("compare");
  });

  it("accepts the reset state, clears the old pair and summary, and compares again", async () => {
    const { controller, api, saved, initial } = await comparison();
    const tag = { id: 1, listId: initial.id, name: "お気に入り" };
    const reset = { ...initial, revision: saved.revision + 1, tags: [tag] };
    controller.state.active = { ...saved, tags: [tag] };
    controller.selectTag(tag.id);
    api.resetComparisons.mockResolvedValue(reset);
    // A reset response is sufficient even if a subsequent sidebar read would fail.
    api.listSummaries.mockRejectedValueOnce("一覧を取得できません。");
    await controller.openModal({ kind: "reset-comparisons" });
    await controller.resetComparisons();
    expect(api.resetComparisons).toHaveBeenCalledExactlyOnceWith(initial.id);
    expect(controller.state.active).toEqual(reset);
    expect(controller.state.pair).toBeNull();
    expect(controller.state.modal).toBeNull();
    expect(controller.state.view).toBe("ranking");
    expect(controller.state.selectedTagId).toBe(tag.id);
    expect(controller.state.lists.find((entry) => entry.id === initial.id)).toEqual(summary(reset));
    expect(controller.state.notice).toContain("比較をリセットしました");
    expect(controller.state.error).toBe("");
    api.listSummaries.mockReset().mockResolvedValue([summary(reset)]);
    api.resumeList.mockResolvedValue(reset);
    api.nextPair.mockResolvedValue(proposal(reset));
    await controller.startComparison();
    expect(controller.state.pair?.revision).toBe(reset.revision);
    expect(controller.state.view).toBe("compare");
  });

  it("retains the warning and saved state after an error so the user can retry", async () => {
    const { controller, api } = await comparison();
    const before = controller.state.active;
    const pair = controller.state.pair;
    api.resetComparisons.mockRejectedValueOnce("保存に失敗しました。");
    await controller.openModal({ kind: "reset-comparisons" });
    await controller.resetComparisons();
    expect(controller.state.error).toBe("保存に失敗しました。");
    expect(controller.state.modal?.kind).toBe("reset-comparisons");
    expect(controller.state.active).toBe(before);
    expect(controller.state.pair).toBe(pair);
    expect(controller.state.notice).toBe("");
    await controller.resetComparisons();
    expect(api.resetComparisons).toHaveBeenCalledTimes(2);
    expect(controller.state.modal).toBeNull();
  });

  it("blocks repeat writes, dismissal and navigation while a reset is pending", async () => {
    const { controller, api } = await comparison();
    let finish: ((value: ListState) => void) | undefined;
    api.resetComparisons.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    await controller.openModal({ kind: "reset-comparisons" });
    const pending = controller.resetComparisons();
    await controller.resetComparisons();
    controller.closeModal();
    await controller.navigate("items");
    expect(controller.state.modal?.kind).toBe("reset-comparisons");
    expect(controller.state.view).toBe("compare");
    expect(api.resetComparisons).toHaveBeenCalledTimes(1);
    if (!finish || !controller.state.active) throw new Error("Missing pending reset");
    finish(controller.state.active);
    await pending;
    expect(controller.state.busy).toBe(false);
  });

  it("recovers when another instance deleted the selected list", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    api.resetComparisons.mockRejectedValueOnce("リストが見つかりません。");
    api.listSummaries.mockResolvedValue([summary(list(2))]);
    await controller.openModal({ kind: "reset-comparisons" });
    await controller.resetComparisons();
    expect(controller.state.active?.id).toBe(2);
    expect(controller.state.modal).toBeNull();
    expect(controller.state.error).toBe("リストが見つかりません。");
  });
});

describe("portable list transfer", () => {
  it("requires an export modal and defaults images off whenever it opens", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.exportList();
    expect(api.exportList).not.toHaveBeenCalled();

    await controller.openModal({ kind: "export-list" });
    expect(controller.state.exportIncludeImages).toBe(false);
    controller.setExportIncludeImages(true);
    controller.closeModal();
    await controller.openModal({ kind: "export-list" });
    expect(controller.state.exportIncludeImages).toBe(false);
    await controller.exportList();
    expect(api.exportList).toHaveBeenCalledExactlyOnceWith(1, false);
    expect(controller.state.modal).toBeNull();
    expect(controller.state.notice).toBe("リストをエクスポートしました。");
  });

  it("retains explicit image selection after native save cancellation or an export error", async () => {
    const initial = list();
    const api = backend(initial);
    api.exportList.mockResolvedValueOnce(false).mockRejectedValueOnce("保存に失敗しました。");
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.openModal({ kind: "export-list" });
    controller.setExportIncludeImages(true);

    await controller.exportList();
    expect(controller.state.modal?.kind).toBe("export-list");
    expect(controller.state.notice).toBe("");
    expect(controller.state.error).toBe("");
    await controller.exportList();
    expect(controller.state.error).toBe("保存に失敗しました。");
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.exportIncludeImages).toBe(true);
    await controller.exportList();
    expect(api.exportList.mock.calls).toEqual([
      [1, true],
      [1, true],
      [1, true],
    ]);
    expect(controller.state.error).toBe("");
    expect(controller.state.modal).toBeNull();
  });

  it.each(["export", "import"] as const)(
    "gates %s writes and dismissal while a native operation is pending",
    async (operation) => {
      const api = backend();
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      let finish: (() => void) | undefined;
      if (operation === "export")
        api.exportList.mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finish = () => resolve(false);
            }),
        );
      else
        api.importList.mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finish = () => resolve(null);
            }),
        );
      await controller.openModal({ kind: operation === "export" ? "export-list" : "import-list" });
      const run = () =>
        operation === "export" ? controller.exportList() : controller.importList();
      const pending = run();
      expect(controller.state.busy).toBe(true);
      controller.closeModal();
      controller.setExportIncludeImages(true);
      await controller.navigate("ranking");
      await controller.openModal({ kind: "create-list" });
      await run();
      expect(controller.state.modal?.kind).toBe(`${operation}-list`);
      expect(controller.state.exportIncludeImages).toBe(false);
      expect(controller.state.view).toBe("items");
      expect(operation === "export" ? api.exportList : api.importList).toHaveBeenCalledTimes(1);
      if (!finish) throw new Error("Native operation did not start");
      finish();
      await pending;
      expect(controller.state.busy).toBe(false);
    },
  );

  it("leaves comparison, draft, and active list unchanged when import is cancelled or invalid", async () => {
    const initial = list();
    const api = backend(initial);
    api.importList
      .mockResolvedValueOnce(null)
      .mockRejectedValueOnce(new Error("対応していないファイルです。"));
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.startComparison();
    const pair = controller.state.pair;
    controller.state.drafts.items = "追加途中";
    await controller.openModal({ kind: "import-list" });
    await controller.importList();
    expect(controller.state.error).toBe("");
    expect(controller.state.notice).toBe("");
    await controller.importList();
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.pair).toEqual(pair);
    expect(controller.state.drafts.items).toBe("追加途中");
    expect(controller.state.view).toBe("compare");
    expect(controller.state.modal?.kind).toBe("import-list");
    expect(controller.state.error).toBe("対応していないファイルです。");
  });

  it("opens an imported list with tags and images and clears state from the old list", async () => {
    const initial = { ...list(), tags: [{ id: 1, listId: 1, name: "以前" }] };
    const imported: ListState = {
      ...list(3),
      tags: [{ id: 2, listId: 3, name: "輸入" }],
      items: list(3).items.map((item) => ({
        ...item,
        tagIds: [2],
        image: { path: "imported.png", sourceUrl: null },
      })),
    };
    const api = backend(initial);
    api.importList.mockResolvedValueOnce(imported);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.selectTag(1);
    controller.state.drafts.items = "古い項目の下書き";
    controller.state.drafts.query = "古い検索";
    await controller.startComparison();
    await controller.openModal({ kind: "import-list" });
    api.listSummaries.mockResolvedValueOnce([summary(initial), summary(imported)]);
    await controller.importList();
    expect(api.importList).toHaveBeenCalledExactlyOnceWith();
    expect(controller.state.active).toEqual(imported);
    expect(controller.state.lists).toEqual([summary(initial), summary(imported)]);
    expect(controller.state.modal).toBeNull();
    expect(controller.state.pair).toBeNull();
    expect(controller.state.selectedTagId).toBeNull();
    expect(controller.state.view).toBe("items");
    expect(controller.state.drafts.items).toBe("");
    expect(controller.state.drafts.query).toBe("");
    expect(controller.state.notice).toBe("新しいリストとしてインポートしました。");
    expect(controller.state.error).toBe("");
  });

  it("keeps a committed imported list usable if refreshing the sidebar fails", async () => {
    const initial = list();
    const imported = list(3);
    const api = backend(initial);
    api.importList.mockResolvedValueOnce(imported);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.openModal({ kind: "import-list" });
    api.listSummaries.mockRejectedValueOnce(new Error("一覧を取得できません。"));
    await controller.importList();
    expect(controller.state.active).toEqual(imported);
    expect(controller.state.lists).toContainEqual(summary(initial));
    expect(controller.state.lists).toContainEqual(summary(imported));
    expect(controller.state.modal).toBeNull();
    expect(controller.state.notice).toBe("新しいリストとしてインポートしました。");
    expect(controller.state.error).toBe(
      "インポート後のリスト一覧を更新できませんでした: 一覧を取得できません。",
    );
    await controller.importList();
    expect(api.importList).toHaveBeenCalledTimes(1);
  });

  it("imports into an empty app without creating or modifying another list", async () => {
    const imported = list(3);
    const api = backend();
    api.listSummaries.mockResolvedValueOnce([]);
    api.importList.mockResolvedValueOnce(imported);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.openModal({ kind: "import-list" });
    api.listSummaries.mockResolvedValueOnce([summary(imported)]);
    await controller.importList();
    expect(controller.state.active).toEqual(imported);
    expect(api.createList).not.toHaveBeenCalled();
    expect(api.deleteList).not.toHaveBeenCalled();
  });
});

describe("tag management", () => {
  it("accepts 30 Unicode characters after trimming and rejects a longer new tag", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.openModal({ kind: "tags" });
    const validName = `${"あ".repeat(29)}😀`;
    controller.state.tagDraft = ` ${validName} `;
    await controller.saveTag();
    expect(api.createTag).toHaveBeenCalledExactlyOnceWith(1, validName, undefined);

    const tooLong = `${"あ".repeat(30)}😀`;
    controller.state.tagDraft = tooLong;
    await controller.saveTag();
    expect(api.createTag).toHaveBeenCalledTimes(1);
    expect(controller.state.error).toBe("タグ名は30文字以内にしてください。");
    expect(controller.state.tagDraft).toBe(tooLong);
  });

  it("rejects a longer tag rename without calling the backend", async () => {
    const initial = { ...list(), tags: [{ id: 3, listId: 1, name: "以前" }] };
    const api = backend(initial);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.openModal({ kind: "tags" });
    controller.editTag(3);
    const tooLong = `${"い".repeat(30)}😀`;
    controller.state.tagDraft = tooLong;

    await controller.saveTag();

    expect(api.renameTag).not.toHaveBeenCalled();
    expect(controller.state.error).toBe("タグ名は30文字以内にしてください。");
    expect(controller.state.tagDraft).toBe(tooLong);
  });

  it("creates a tag for the open item and persists multi-tag assignment", async () => {
    const initial = list();
    const api = backend(initial);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.openModal({ kind: "tags", itemId: 11 });
    controller.state.tagDraft = " 好き ";
    const created: ListState = {
      ...initial,
      tags: [
        { id: 1, listId: 1, name: "好き" },
        { id: 2, listId: 1, name: "果物" },
      ],
      items: initial.items.map((item) => (item.id === 11 ? { ...item, tagIds: [1] } : item)),
    };
    api.createTag.mockResolvedValueOnce(created);
    await controller.saveTag();
    expect(api.createTag).toHaveBeenCalledExactlyOnceWith(1, "好き", 11);
    expect(controller.state.modal).toEqual({ kind: "tags", itemId: 11 });
    const assigned: ListState = {
      ...created,
      items: created.items.map((item) => (item.id === 11 ? { ...item, tagIds: [1, 2] } : item)),
    };
    api.setItemTag.mockResolvedValueOnce(assigned);
    await controller.toggleItemTag(2, true);
    expect(api.setItemTag).toHaveBeenCalledExactlyOnceWith(1, 11, 2, true);
    expect(controller.state.active).toEqual(assigned);
  });

  it.each([undefined, 11])(
    "reloads tags after another instance creates the same name (itemId=%s)",
    async (itemId) => {
      const initial = list();
      const refreshed = {
        ...initial,
        tags: [{ id: 3, listId: 1, name: "好き" }],
      };
      const api = backend(initial);
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      const modal =
        itemId === undefined ? ({ kind: "tags" } as const) : ({ kind: "tags", itemId } as const);
      await controller.openModal(modal);
      controller.state.tagDraft = "好き";
      api.createTag.mockRejectedValueOnce(new Error("同じ名前のタグが既にあります。"));
      api.getList.mockResolvedValueOnce(refreshed);

      await controller.saveTag();

      expect(api.getList).toHaveBeenCalledTimes(2);
      expect(controller.state.active).toEqual(refreshed);
      expect(controller.state.modal).toEqual(modal);
      expect(controller.state.tagDraft).toBe("好き");
      expect(controller.state.error).toBe("同じ名前のタグが既にあります。");
    },
  );

  it("reloads a concurrently deleted tag and clears the editor after a rename error", async () => {
    const initial = {
      ...list(),
      tags: [{ id: 3, listId: 1, name: "以前" }],
    };
    const refreshed = { ...initial, tags: [] };
    const api = backend(initial);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.selectTag(3);
    await controller.openModal({ kind: "tags" });
    controller.editTag(3);
    controller.state.tagDraft = "新しい";
    api.renameTag.mockRejectedValueOnce(new Error("タグが見つかりません。"));
    api.getList.mockResolvedValueOnce(refreshed);

    await controller.saveTag();

    expect(api.getList).toHaveBeenCalledTimes(2);
    expect(controller.state.active).toEqual(refreshed);
    expect(controller.state.selectedTagId).toBeNull();
    expect(controller.state.tagEditingId).toBeNull();
    expect(controller.state.tagDraft).toBe("");
    expect(controller.state.error).toBe("タグが見つかりません。");
  });

  it("reloads a concurrently deleted tag after deletion or assignment fails", async () => {
    for (const action of ["delete", "assignment"] as const) {
      const initial = {
        ...list(),
        tags: [{ id: 3, listId: 1, name: "以前" }],
      };
      const refreshed = { ...initial, tags: [] };
      const api = backend(initial);
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      await controller.openModal({ kind: "tags", itemId: 11 });
      api.getList.mockResolvedValueOnce(refreshed);
      if (action === "delete") {
        controller.deleteTagPrompt(3);
        api.deleteTag.mockRejectedValueOnce(new Error("タグが見つかりません。"));
        await controller.confirmTagDelete();
      } else {
        api.setItemTag.mockRejectedValueOnce(new Error("タグが見つかりません。"));
        await controller.toggleItemTag(3, true);
      }
      expect(controller.state.active).toEqual(refreshed);
      expect(controller.state.tagDeletingId).toBeNull();
      expect(controller.state.error).toBe("タグが見つかりません。");
    }
  });

  it("closes an item tag editor when a successful tag change returns without that item", async () => {
    const initial = {
      ...list(),
      tags: [{ id: 3, listId: 1, name: "以前" }],
    };
    const api = backend(initial);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.openModal({ kind: "tags", itemId: 11 });
    controller.editTag(3);
    controller.state.tagDraft = "新しい";
    api.renameTag.mockResolvedValueOnce({
      ...initial,
      tags: [{ ...initial.tags[0]!, name: "新しい" }],
      items: initial.items.filter((item) => item.id !== 11),
    });

    await controller.saveTag();

    expect(controller.state.modal).toBeNull();
    expect(controller.state.tagEditingId).toBeNull();
    expect(controller.state.error).toBe("");
  });

  it("renames and deletes tags while resetting a deleted ranking filter", async () => {
    const initial: ListState = {
      ...list(),
      tags: [{ id: 3, listId: 1, name: "以前" }],
      items: list().items.map((item) => ({ ...item, tagIds: [3] })),
    };
    const api = backend(initial);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.selectTag(3);
    await controller.openModal({ kind: "tags" });
    controller.editTag(3);
    controller.state.tagDraft = "新しい";
    const renamed = { ...initial, tags: [{ ...initial.tags[0]!, name: "新しい" }] };
    api.renameTag.mockResolvedValueOnce(renamed);
    await controller.saveTag();
    expect(api.renameTag).toHaveBeenCalledExactlyOnceWith(1, 3, "新しい");
    expect(controller.state.selectedTagId).toBe(3);
    controller.deleteTagPrompt(3);
    const deleted = {
      ...renamed,
      tags: [],
      items: renamed.items.map((item) => ({ ...item, tagIds: [] })),
    };
    api.deleteTag.mockResolvedValueOnce(deleted);
    await controller.confirmTagDelete();
    expect(api.deleteTag).toHaveBeenCalledExactlyOnceWith(1, 3);
    expect(controller.state.selectedTagId).toBeNull();
  });

  it("resets the selected tag when switching lists", async () => {
    const first = { ...list(), tags: [{ id: 3, listId: 1, name: "好き" }] };
    const second = { ...list(2), tags: [{ id: 4, listId: 2, name: "好き" }] };
    const api = backend(first, second);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.selectTag(3);
    await controller.selectList(2);
    expect(controller.state.selectedTagId).toBeNull();
    controller.selectTag(3);
    expect(controller.state.selectedTagId).toBeNull();
  });

  it("clears the selected tag when deleting the current list and loading another", async () => {
    const first = { ...list(), tags: [{ id: 3, listId: 1, name: "好き" }] };
    const second = { ...list(2), tags: [{ id: 3, listId: 2, name: "別のタグ" }] };
    const api = backend(first, second);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.selectTag(3);
    await controller.openModal({ kind: "delete-list" });
    api.listSummaries.mockResolvedValueOnce([summary(second)]);
    await controller.confirmDelete();
    expect(controller.state.active?.id).toBe(2);
    expect(controller.state.selectedTagId).toBeNull();
  });
});

const mutationMethods = [
  "renameList",
  "deleteList",
  "addItems",
  "renameItem",
  "deleteItem",
  "setLocalImage",
  "setRemoteImage",
  "removeImage",
] as const;
type MutationMethod = (typeof mutationMethods)[number];

async function mutation(method: MutationMethod) {
  const initial = list();
  const api = backend(initial);
  const controller = new AppController(api, vi.fn());
  await controller.initialize();
  const candidate: ImageCandidate = {
    id: "https://example.org/image.png",
    title: "Image",
    previewUrl: "data:image/png;base64,AA==",
    sourceUrl: "https://example.org/",
  };
  controller.state.drafts.items = "bulk draft";
  switch (method) {
    case "renameList":
      await controller.openModal({ kind: "rename-list" });
      controller.state.drafts.name = "new list name";
      break;
    case "deleteList":
      await controller.openModal({ kind: "delete-list" });
      break;
    case "renameItem":
      await controller.openModal({ kind: "rename-item", itemId: 11 });
      controller.state.drafts.name = "new item name";
      break;
    case "deleteItem":
      await controller.openModal({ kind: "delete-item", itemId: 11 });
      break;
    case "addItems":
      break;
    default:
      await controller.openModal({ kind: "image", itemId: 11 });
      controller.state.candidates = [candidate];
      controller.state.searched = true;
  }
  const run = () => {
    switch (method) {
      case "renameList":
      case "renameItem":
        return controller.saveName();
      case "deleteList":
      case "deleteItem":
        return controller.confirmDelete();
      case "addItems":
        return controller.addItems();
      case "setLocalImage":
        return controller.changeImage("local");
      case "setRemoteImage":
        return controller.changeImage(candidate);
      case "removeImage":
        return controller.changeImage("none");
    }
  };
  return { initial, api, controller, run, candidate };
}

describe("mutations after concurrent deletion", () => {
  it.each([
    ["renameList", "remaining"],
    ["renameList", "empty"],
    ["setRemoteImage", "remaining"],
    ["setRemoteImage", "empty"],
  ] as const)(
    "reconciles a list deleted after successful %s to the %s state",
    async (method, destination) => {
      const { api, controller, run } = await mutation(method);
      api.listSummaries.mockResolvedValue(destination === "remaining" ? [summary(list(2))] : []);
      await run();
      expect(controller.state.active).toEqual(destination === "remaining" ? list(2) : null);
      expect(controller.state.lists.map((entry) => entry.id)).toEqual(
        destination === "remaining" ? [2] : [],
      );
      expect(controller.state.pair).toBeNull();
      expect(controller.state.modal).toBeNull();
      expect(controller.state.candidates).toEqual([]);
      expect(controller.state.drafts.items).toBe("");
      expect(controller.state.error).toBe("リストが見つかりません。");
      expect(controller.state.busy).toBe(false);
    },
  );

  it("keeps a confirmed deleted list cleared if recovery fails after a successful mutation", async () => {
    const { api, controller, run } = await mutation("setRemoteImage");
    api.listSummaries
      .mockResolvedValueOnce([summary(list(2))])
      .mockRejectedValueOnce("一覧を読み込めません。");
    await run();
    expect(controller.state.active).toBeNull();
    expect(controller.state.pair).toBeNull();
    expect(controller.state.modal).toBeNull();
    expect(controller.state.candidates).toEqual([]);
    expect(controller.state.drafts.items).toBe("");
    expect(controller.state.lists).toEqual([summary(list(2))]);
    expect(controller.state.error).toBe("一覧を読み込めません。");
    await run();
    expect(api.setRemoteImage).toHaveBeenCalledOnce();
  });

  it.each(mutationMethods)("reconciles a missing list during %s", async (method) => {
    const { api, controller, run } = await mutation(method);
    api[method].mockRejectedValueOnce("リストが見つかりません。");
    api.listSummaries.mockResolvedValue([summary(list(2))]);
    await run();
    expect(controller.state.active?.id).toBe(2);
    expect(controller.state.lists.map((entry) => entry.id)).toEqual([2]);
    expect(controller.state.modal).toBeNull();
    expect(controller.state.drafts.items).toBe("");
    expect(controller.state.error).toBe("リストが見つかりません。");
    expect(controller.state.busy).toBe(false);
  });

  it.each(["renameItem", "deleteItem", "setLocalImage", "setRemoteImage", "removeImage"] as const)(
    "removes a stale item and reloads its still-valid list after %s",
    async (method) => {
      const { api, controller, initial, run } = await mutation(method);
      const current = { ...initial, revision: 5, items: initial.items.slice(1) };
      api[method].mockRejectedValueOnce("項目が見つかりません。");
      api.getList.mockResolvedValue(current);
      api.listSummaries.mockResolvedValue([summary(current)]);
      await run();
      expect(controller.state.active).toEqual(current);
      expect(controller.state.modal).toBeNull();
      expect(controller.state.pair).toBeNull();
      expect(controller.state.candidates).toEqual([]);
      expect(controller.state.searched).toBe(false);
      expect(controller.state.drafts.items).toBe("bulk draft");
      expect(controller.state.error).toBe("項目が見つかりません。");
      await run();
      expect(api[method]).toHaveBeenCalledOnce();
    },
  );

  it("keeps a deleted item removed if reloading the list also fails", async () => {
    const { api, controller, initial, run } = await mutation("setRemoteImage");
    initial.convergence.converged = true;
    api.setRemoteImage.mockRejectedValueOnce("項目が見つかりません。");
    api.getList.mockRejectedValueOnce("データベースを読み込めません。");
    await run();
    expect(controller.state.active?.items.map((item) => item.id)).toEqual([12]);
    expect(controller.state.active?.convergence.converged).toBe(false);
    expect(controller.state.modal).toBeNull();
    expect(controller.state.candidates).toEqual([]);
    expect(controller.state.error).toBe("データベースを読み込めません。");
    await run();
    expect(api.setRemoteImage).toHaveBeenCalledOnce();
  });

  it("recovers if the list disappears while reloading a deleted item's list", async () => {
    const { api, controller, run } = await mutation("setRemoteImage");
    api.setRemoteImage.mockRejectedValueOnce("項目が見つかりません。");
    api.getList.mockRejectedValueOnce("リストが見つかりません。");
    api.listSummaries.mockResolvedValue([summary(list(2))]);
    await run();
    expect(controller.state.active?.id).toBe(2);
    expect(controller.state.modal).toBeNull();
    expect(controller.state.drafts.items).toBe("");
  });

  it.each(mutationMethods)("retains state for a transient %s failure", async (method) => {
    const { api, controller, initial, run } = await mutation(method);
    const modal = controller.state.modal;
    const candidates = controller.state.candidates;
    api[method].mockRejectedValueOnce("一時的に保存できません。");
    await run();
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.modal).toEqual(modal);
    expect(controller.state.candidates).toEqual(candidates);
    expect(controller.state.drafts.items).toBe("bulk draft");
    expect(controller.state.error).toBe("一時的に保存できません。");
  });
});

describe("explicit credential access", () => {
  it("does not read credentials when entering settings or opening an image dialog", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.navigate("settings");
    await controller.openModal({ kind: "image", itemId: 11 });
    expect(api.searchSettings).not.toHaveBeenCalled();
    expect(controller.state.settings).toBeNull();
    expect(api.searchImages).not.toHaveBeenCalled();
  });

  it.each([
    "sidebar",
    "create-list",
    "rename-list",
    "rename-item",
    "delete-list",
    "delete-item",
    "duplicate-list",
    "import-list",
    "export-list",
    "add-items",
    "local-image",
    "remove-image",
    "create-tag",
    "comparison",
    "answer",
  ] as const)("does not read credentials during the ordinary %s operation", async (operation) => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    expect(api.searchSettings).not.toHaveBeenCalled();
    switch (operation) {
      case "sidebar":
        await controller.selectList(2);
        expect(api.getList).toHaveBeenLastCalledWith(2);
        break;
      case "create-list":
      case "rename-list":
      case "rename-item":
        await controller.openModal(
          operation === "rename-item" ? { kind: operation, itemId: 11 } : { kind: operation },
        );
        controller.state.drafts.name = "新しい名前";
        await controller.saveName();
        if (operation === "create-list") expect(api.createList).toHaveBeenCalledOnce();
        else if (operation === "rename-list") expect(api.renameList).toHaveBeenCalledOnce();
        else expect(api.renameItem).toHaveBeenCalledOnce();
        break;
      case "delete-list":
      case "delete-item":
        await controller.openModal(
          operation === "delete-item" ? { kind: operation, itemId: 11 } : { kind: operation },
        );
        await controller.confirmDelete();
        expect(
          operation === "delete-list" ? api.deleteList : api.deleteItem,
        ).toHaveBeenCalledOnce();
        break;
      case "duplicate-list":
        await controller.duplicateList();
        expect(api.duplicateList).toHaveBeenCalledOnce();
        break;
      case "import-list":
      case "export-list":
        await controller.openModal({ kind: operation });
        if (operation === "import-list") await controller.importList();
        else {
          controller.setExportIncludeImages(true);
          await controller.exportList();
        }
        expect(
          operation === "import-list" ? api.importList : api.exportList,
        ).toHaveBeenCalledOnce();
        break;
      case "add-items":
        controller.state.drafts.items = "新しい項目";
        await controller.addItems();
        expect(api.addItems).toHaveBeenCalledExactlyOnceWith(1, ["新しい項目"]);
        break;
      case "local-image":
      case "remove-image":
        await controller.openModal({ kind: "image", itemId: 11 });
        await controller.changeImage(operation === "local-image" ? "local" : "none");
        expect(
          operation === "local-image" ? api.setLocalImage : api.removeImage,
        ).toHaveBeenCalledOnce();
        break;
      case "create-tag":
        await controller.openModal({ kind: "tags" });
        controller.state.tagDraft = "分類";
        await controller.saveTag();
        expect(api.createTag).toHaveBeenCalledExactlyOnceWith(1, "分類", undefined);
        break;
      case "comparison":
      case "answer":
        await controller.startComparison();
        expect(api.nextPair).toHaveBeenCalledOnce();
        if (operation === "answer") {
          await controller.answer("equal");
          expect(api.answer).toHaveBeenCalledOnce();
        }
        break;
    }
    expect(api.searchSettings).not.toHaveBeenCalled();
    expect(api.searchImages).not.toHaveBeenCalled();
    expect(api.autoRegisterImage).not.toHaveBeenCalled();
    expect(controller.state.settings).toBeNull();
    expect(controller.state.checkedProviders).toEqual([]);
  });

  it("requires the explicit status check to be in a credential-related screen", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.checkSearchSettings();
    await controller.navigate("ranking");
    await controller.checkSearchSettings();
    await controller.navigate("settings");
    await controller.openModal({ kind: "create-list" });
    await controller.checkSearchSettings();
    expect(api.searchSettings).not.toHaveBeenCalled();
    controller.closeModal();
    await controller.checkSearchSettings();
    expect(api.searchSettings).toHaveBeenCalledOnce();
    expect(controller.state.checkedProviders).toEqual(["brave", "ollama"]);
  });

  it("keeps unknown, unset and denied status distinct and retries only on an explicit check", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.navigate("settings");
    expect(controller.state.settings).toBeNull();
    expect(controller.state.checkedProviders).toEqual([]);
    await controller.checkSearchSettings();
    expect(controller.state.settings?.braveConfigured).toBe(false);
    expect(controller.state.settings?.errors?.brave).toBeUndefined();
    expect(controller.state.checkedProviders).toEqual(["brave", "ollama"]);
    api.searchSettings.mockRejectedValueOnce(new Error("OSでアクセスが拒否されました。"));
    await controller.checkSearchSettings();
    expect(controller.state.settings?.errors).toEqual({
      brave: "OSでアクセスが拒否されました。",
      ollama: "OSでアクセスが拒否されました。",
    });
    expect(controller.availableBulkProviders()).toEqual([]);
    await controller.navigate("items");
    await controller.startComparison();
    await controller.answer("equal");
    expect(api.answer).toHaveBeenCalledOnce();
    expect(api.searchSettings).toHaveBeenCalledTimes(2);
    await controller.navigate("settings");
    expect(api.searchSettings).toHaveBeenCalledTimes(2);
    api.searchSettings.mockResolvedValueOnce({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    await controller.checkSearchSettings();
    expect(api.searchSettings).toHaveBeenCalledTimes(3);
    expect(controller.state.settings?.errors?.brave).toBeUndefined();
    expect(controller.availableBulkProviders()).toEqual(["brave"]);
  });

  it("allows local image selection and its cancellation after a denied credential check", async () => {
    const initial = list();
    const api = backend(initial);
    api.searchSettings.mockRejectedValueOnce(new Error("アクセスをキャンセルしました。"));
    api.setLocalImage.mockResolvedValueOnce(null);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.openModal({ kind: "image", itemId: 11 });
    await controller.checkSearchSettings();
    expect(controller.state.settings?.errors?.brave).toBe("アクセスをキャンセルしました。");
    await controller.changeImage("local");
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.modal).toEqual({ kind: "image", itemId: 11 });
    expect(controller.state.busy).toBe(false);
    await controller.changeImage("local");
    expect(api.setLocalImage).toHaveBeenCalledTimes(2);
    expect(controller.state.modal).toBeNull();
    expect(api.searchSettings).toHaveBeenCalledOnce();
    expect(api.searchImages).not.toHaveBeenCalled();
  });

  it("opens bulk registration without reading keys and waits for an explicit successful check", async () => {
    const api = backend();
    api.searchSettings.mockResolvedValueOnce({
      braveConfigured: false,
      ollamaConfigured: true,
      defaultProvider: "ollama",
    });
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.openBulkImages();
    expect(controller.state.modal).toEqual({ kind: "bulk-image" });
    expect(controller.state.bulkProvider).toBeNull();
    expect(api.searchSettings).not.toHaveBeenCalled();
    await controller.runBulkImages();
    expect(api.autoRegisterImage).not.toHaveBeenCalled();
    await controller.checkSearchSettings();
    expect(controller.state.bulkProvider).toBe("ollama");
    await controller.runBulkImages();
    expect(api.autoRegisterImage.mock.calls).toEqual([
      [1, 11, "ollama"],
      [1, 12, "ollama"],
    ]);
    expect(api.searchSettings).toHaveBeenCalledOnce();
    expect(controller.state.bulkRun?.done).toBe(2);
  });

  it("does not offer bulk registration when all items already have images", async () => {
    const initial = list();
    initial.items = initial.items.map((item) => ({
      ...item,
      image: { path: "fixture.png", sourceUrl: null },
    }));
    const api = backend(initial);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.openBulkImages();
    expect(controller.state.modal).toBeNull();
    expect(api.searchSettings).not.toHaveBeenCalled();
  });

  it.each(["unknown", "unset", "error"] as const)(
    "does not invoke image search when the selected provider is %s",
    async (status) => {
      const api = backend();
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      await controller.openModal({ kind: "image", itemId: 11 });
      if (status !== "unknown") {
        api.searchSettings.mockResolvedValueOnce({
          braveConfigured: status === "error",
          ollamaConfigured: false,
          defaultProvider: "brave",
          ...(status === "error" ? { errors: { brave: "アクセスが拒否されました。" } } : {}),
        });
        await controller.checkSearchSettings();
      }
      await controller.searchImages();
      expect(api.searchImages).not.toHaveBeenCalled();
      expect(api.searchSettings).toHaveBeenCalledTimes(status === "unknown" ? 0 : 1);
      expect(controller.state.busy).toBe(false);
    },
  );
});

describe("image search validation", () => {
  it("rejects whitespace-only queries without a request and permits correction", async () => {
    const api = backend();
    api.searchSettings.mockResolvedValue({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.openModal({ kind: "image", itemId: 11 });
    await controller.checkSearchSettings();
    controller.state.drafts.query = " \t　 ";
    await controller.searchImages();
    expect(controller.state.error).toBe("検索語は1〜400文字、50語以内で入力してください。");
    expect(controller.state.modal).toEqual({ kind: "image", itemId: 11 });
    expect(controller.state.drafts.query).toBe(" \t　 ");
    expect(controller.state.busy).toBe(false);
    expect(api.searchImages).not.toHaveBeenCalled();
    controller.state.drafts.query = "  修正した検索語  ";
    await controller.searchImages();
    expect(api.searchImages).toHaveBeenCalledExactlyOnceWith("brave", "修正した検索語");
    expect(controller.state.error).toBe("");
    expect(controller.state.searched).toBe(true);
    expect(controller.state.modal).toEqual({ kind: "image", itemId: 11 });
  });
});

describe("settings navigation", () => {
  it.each(["success", "failure"] as const)(
    "requires an explicit retry after cancelling list creation and ignores the abandoned %s",
    async (outcome) => {
      const api = backend();
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      controller.state.drafts.items = "unsaved item";
      controller.state.drafts.braveKey = "unsaved key";
      let finishOld: () => void = () => {};
      api.searchSettings.mockImplementationOnce(
        () =>
          new Promise((resolve, reject) => {
            finishOld = () => {
              if (outcome === "success") {
                resolve({
                  braveConfigured: true,
                  ollamaConfigured: false,
                  defaultProvider: "brave",
                });
              } else reject(new Error("abandoned settings read failed"));
            };
          }),
      );
      await controller.navigate("settings");
      const abandoned = controller.checkSearchSettings();
      await controller.openModal({ kind: "create-list" });
      const settings = {
        braveConfigured: false,
        ollamaConfigured: true,
        defaultProvider: "ollama" as const,
      };
      let finishCurrent: () => void = () => {};
      api.searchSettings.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishCurrent = () => resolve(settings);
          }),
      );
      controller.closeModal();
      expect(api.searchSettings).toHaveBeenCalledOnce();
      const retry = controller.checkSearchSettings();
      expect(controller.state.modal).toBeNull();
      expect(controller.state.view).toBe("settings");
      finishOld();
      await abandoned;
      await vi.waitFor(() => expect(api.searchSettings).toHaveBeenCalledTimes(2));
      expect(controller.state.busy).toBe(true);
      expect(controller.state.readPending).toBe("settings");
      expect(controller.state.settings).toBeNull();
      expect(controller.state.error).toBe("");
      finishCurrent();
      await retry;
      expect(controller.state.settings).toEqual(settings);
      expect(controller.state.provider).toBe("ollama");
      expect(controller.state.drafts.items).toBe("unsaved item");
      expect(controller.state.drafts.braveKey).toBe("unsaved key");
    },
  );

  it.each(["success", "failure"] as const)(
    "retries a failed settings read after cancelling list creation (retry %s)",
    async (outcome) => {
      const api = backend();
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      api.searchSettings.mockRejectedValueOnce(new Error("initial settings read failed"));
      await controller.navigate("settings");
      await controller.checkSearchSettings();
      expect(controller.state.settings?.errors).toEqual({
        brave: "initial settings read failed",
        ollama: "initial settings read failed",
      });
      await controller.openModal({ kind: "create-list" });
      if (outcome === "failure") {
        api.searchSettings.mockRejectedValueOnce(new Error("retry settings read failed"));
      }
      controller.closeModal();
      expect(api.searchSettings).toHaveBeenCalledOnce();
      await controller.checkSearchSettings();
      expect(api.searchSettings).toHaveBeenCalledTimes(2);
      expect(controller.state.modal).toBeNull();
      expect(controller.state.view).toBe("settings");
      expect(controller.state.error).toBe(
        outcome === "failure" ? "retry settings read failed" : "",
      );
      expect(controller.state.settings?.errors?.brave).toBe(
        outcome === "failure" ? "retry settings read failed" : undefined,
      );
      await controller.selectList(1);
      expect(controller.state.view).toBe("items");
      expect(controller.state.busy).toBe(false);
    },
  );

  it.each([
    ["success", "screen"],
    ["failure", "screen"],
    ["success", "write"],
    ["failure", "write"],
  ] as const)(
    "ignores an explicit post-write confirmation %s after leaving settings for another %s",
    async (outcome, destination) => {
      const api = backend();
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      await controller.navigate("settings");
      controller.state.drafts.braveKey = "saved-key";
      let finishOld: () => void = () => {};
      api.searchSettings.mockImplementationOnce(
        () =>
          new Promise((resolve, reject) => {
            finishOld = () => {
              if (outcome === "success") {
                resolve({
                  braveConfigured: false,
                  ollamaConfigured: false,
                  defaultProvider: "brave",
                });
              } else reject(new Error("old post-write refresh failed"));
            };
          }),
      );
      await controller.saveKey("brave");
      expect(api.searchSettings).not.toHaveBeenCalled();
      const checking = controller.checkSearchSettings();
      expect(controller.state.settings?.braveConfigured).toBe(true);
      await controller.navigate(destination === "screen" ? "ranking" : "items");
      expect(controller.state.view).toBe(destination === "screen" ? "ranking" : "items");
      expect(controller.state.busy).toBe(false);
      let finishNew: () => void = () => {};
      let current: Promise<void> | undefined;
      if (destination === "write") {
        controller.state.drafts.items = "new item";
        api.addItems.mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finishNew = () => resolve(list());
            }),
        );
        current = controller.addItems();
      }
      finishOld();
      await checking;
      expect(controller.state.busy).toBe(destination === "write");
      expect(controller.state.settings?.braveConfigured).toBe(true);
      expect(controller.state.error).toBe("");
      if (current) {
        expect(controller.state.drafts.items).toBe("new item");
        finishNew();
        await current;
        expect(controller.state.busy).toBe(false);
        expect(controller.state.drafts.items).toBe("");
      }
    },
  );

  it("allows leaving settings while credentials are still being read", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    let finish: () => void = () => {};
    api.searchSettings.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = () =>
            resolve({ braveConfigured: false, ollamaConfigured: true, defaultProvider: "ollama" });
        }),
    );
    await controller.navigate("settings");
    const reading = controller.checkSearchSettings();
    expect(controller.state.view).toBe("settings");
    await controller.navigate("ranking");
    expect(controller.state.view).toBe("ranking");
    expect(controller.state.busy).toBe(false);
    finish();
    await reading;
    expect(controller.state.view).toBe("ranking");
    expect(controller.state.settings).toBeNull();
    expect(controller.state.provider).toBe("brave");
  });

  it.each([
    ["success", "read"],
    ["failure", "read"],
    ["success", "write"],
    ["failure", "write"],
  ] as const)(
    "ignores an abandoned settings %s while a newer %s remains pending",
    async (outcome, operation) => {
      const api = backend();
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      let finishOld: () => void = () => {};
      api.searchSettings.mockImplementationOnce(
        () =>
          new Promise((resolve, reject) => {
            finishOld = () => {
              if (outcome === "success") {
                resolve({
                  braveConfigured: true,
                  ollamaConfigured: false,
                  defaultProvider: "brave",
                });
              } else reject(new Error("old credential read failed"));
            };
          }),
      );
      await controller.navigate("settings");
      const abandoned = controller.checkSearchSettings();
      let finishNew: () => void = () => {};
      const pending = new Promise<void>((resolve) => {
        finishNew = resolve;
      });
      const currentSettings = {
        braveConfigured: false,
        ollamaConfigured: true,
        defaultProvider: "ollama" as const,
      };
      let current: Promise<void>;
      if (operation === "read") {
        await controller.navigate("items");
        api.searchSettings.mockImplementationOnce(async () => {
          await pending;
          return currentSettings;
        });
        await controller.navigate("settings");
        current = controller.checkSearchSettings();
      } else {
        await controller.openModal({ kind: "create-list" });
        controller.state.drafts.name = "new list";
        api.createList.mockImplementationOnce(async () => {
          await pending;
          return list(3);
        });
        api.listSummaries.mockResolvedValue([summary(list(3))]);
        current = controller.saveName();
      }
      finishOld();
      await abandoned;
      expect(controller.state.busy).toBe(true);
      expect(controller.state.settings).toBeNull();
      expect(controller.state.provider).toBe("brave");
      expect(controller.state.error).toBe("");
      expect(controller.state.notice).toBe("");
      if (operation === "write") {
        await controller.navigate("ranking");
        controller.closeModal();
        expect(controller.state.view).toBe("settings");
        expect(controller.state.modal).toEqual({ kind: "create-list" });
      }
      finishNew();
      await current;
      expect(controller.state.busy).toBe(false);
      expect(controller.state.error).toBe("");
      if (operation === "read") {
        expect(controller.state.settings).toEqual(currentSettings);
        expect(controller.state.provider).toBe("ollama");
      } else {
        expect(controller.state.active?.id).toBe(3);
        expect(controller.state.modal).toBeNull();
      }
    },
  );

  it("waits for a cancelled native settings worker before an explicit retry through its single slot", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.navigate("settings");
    let occupied = false;
    let finish: () => void = () => {};
    api.searchSettings.mockImplementation(() => {
      if (occupied)
        return Promise.reject("検索設定の読み込みが実行中です。少し待ってから再試行してください。");
      occupied = true;
      return new Promise((resolve) => {
        finish = () => {
          occupied = false;
          resolve({ braveConfigured: true, ollamaConfigured: false, defaultProvider: "brave" });
        };
      });
    });
    const reading = controller.checkSearchSettings();
    await controller.checkSearchSettings();
    await controller.checkSearchSettings();
    expect(api.searchSettings).toHaveBeenCalledOnce();
    await controller.openModal({ kind: "create-list" });
    controller.closeModal();
    expect(controller.state.readPending).toBeNull();
    expect(api.searchSettings).toHaveBeenCalledOnce();
    const retry = controller.checkSearchSettings();
    expect(controller.state.readPending).toBe("settings");
    expect(api.searchSettings).toHaveBeenCalledOnce();
    finish();
    await reading;
    await vi.waitFor(() => expect(api.searchSettings).toHaveBeenCalledTimes(2));
    expect(controller.state.settings).toBeNull();
    expect(controller.state.busy).toBe(true);
    expect(controller.state.error).toBe("");
    finish();
    await retry;
    expect(controller.state.settings?.braveConfigured).toBe(true);
    expect(controller.state.checkedProviders).toEqual(["brave", "ollama"]);
    expect(controller.state.error).toBe("");
    expect(api.searchSettings).toHaveBeenCalledTimes(2);
  });

  it("discards a queued settings retry after starting a list write without releasing its busy state", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    let finishRead: () => void = () => {};
    api.searchSettings.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishRead = () =>
            resolve({
              braveConfigured: true,
              ollamaConfigured: false,
              defaultProvider: "brave",
            });
        }),
    );
    await controller.navigate("settings");
    const reading = controller.checkSearchSettings();
    await controller.openModal({ kind: "create-list" });
    controller.closeModal();
    const queued = controller.checkSearchSettings();
    await controller.openModal({ kind: "create-list" });
    controller.state.drafts.name = "new list";
    let finishWrite: () => void = () => {};
    api.createList.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishWrite = () => resolve(list());
        }),
    );
    const writing = controller.saveName();
    finishRead();
    await reading;
    await queued;
    expect(api.searchSettings).toHaveBeenCalledOnce();
    expect(controller.state.settings).toBeNull();
    expect(controller.state.busy).toBe(true);
    expect(controller.state.modal).toEqual({ kind: "create-list" });
    expect(controller.state.error).toBe("");
    finishWrite();
    await writing;
    expect(controller.state.busy).toBe(false);
    expect(controller.state.modal).toBeNull();
  });
});

describe("unchanged names", () => {
  it.each([
    ["rename-list", false],
    ["rename-list", true],
    ["rename-item", false],
    ["rename-item", true],
  ] as const)(
    "closes %s without a mutation when its trimmed name is unchanged (padding=%s)",
    async (kind, padding) => {
      const { api, controller, initial } = await comparison();
      const pair = controller.state.pair;
      const summaries = controller.state.lists;
      const summaryReads = api.listSummaries.mock.calls.length;
      controller.state.drafts.items = "unsaved items";
      await controller.openModal(kind === "rename-list" ? { kind } : { kind, itemId: 11 });
      if (padding) controller.state.drafts.name = `　 ${controller.state.drafts.name} \t`;
      await controller.saveName();
      expect(api.renameList).not.toHaveBeenCalled();
      expect(api.renameItem).not.toHaveBeenCalled();
      expect(api.listSummaries).toHaveBeenCalledTimes(summaryReads);
      expect(controller.state.modal).toBeNull();
      expect(controller.state.active).toEqual(initial);
      expect(controller.state.pair).toEqual(pair);
      expect(controller.state.view).toBe("compare");
      expect(controller.state.lists).toEqual(summaries);
      expect(controller.state.drafts.items).toBe("unsaved items");
      expect(controller.state.busy).toBe(false);
      expect(controller.state.error).toBe("");
    },
  );
});

describe("credential mutation acknowledgments", () => {
  it.each(["brave", "ollama"] as const)(
    "rejects a whitespace-only %s key without changing credentials and permits correction",
    async (provider) => {
      const api = backend();
      const controller = new AppController(api, vi.fn());
      await controller.navigate("settings");
      const field = provider === "brave" ? "braveKey" : "ollamaKey";
      controller.state.drafts[field] = " \t　 ";
      await controller.saveKey(provider);
      expect(controller.state.error).toBe("APIキーの形式が正しくありません。");
      expect(controller.state.drafts[field]).toBe(" \t　 ");
      expect(controller.state.busy).toBe(false);
      expect(api.setApiKey).not.toHaveBeenCalled();
      expect(api.searchSettings).not.toHaveBeenCalled();
      controller.state.drafts[field] = " saved-key ";
      await controller.saveKey(provider);
      expect(api.setApiKey).toHaveBeenCalledExactlyOnceWith(provider, "saved-key");
      expect(controller.state.error).toBe("");
      expect(controller.state.drafts[field]).toBe("");
      expect(controller.state.checkedProviders).toEqual([provider]);
      expect(api.searchSettings).not.toHaveBeenCalled();
    },
  );

  it.each([false, true])(
    "clears the acknowledged provider error without another credential read (remove=%s)",
    async (remove) => {
      const api = backend();
      api.searchSettings.mockResolvedValueOnce({
        braveConfigured: false,
        ollamaConfigured: true,
        defaultProvider: "ollama",
        errors: { brave: "old read error" },
      });
      const controller = new AppController(api, vi.fn());
      await controller.navigate("settings");
      await controller.checkSearchSettings();
      controller.state.drafts.braveKey = "saved-key";
      api.searchSettings.mockRejectedValueOnce(new Error("unexpected refresh"));
      await controller.saveKey("brave", remove);
      expect(controller.state.settings?.errors?.brave).toBeUndefined();
      expect(controller.state.settings?.braveConfigured).toBe(!remove);
      expect(controller.state.settings?.ollamaConfigured).toBe(true);
      expect(controller.state.checkedProviders).toEqual(
        expect.arrayContaining(["brave", "ollama"]),
      );
      expect(controller.state.error).toBe("");
      expect(api.searchSettings).toHaveBeenCalledOnce();
    },
  );

  it.each(["brave", "ollama"] as const)(
    "preserves an acknowledged %s key across a later explicit partial read failure",
    async (provider) => {
      const api = backend();
      api.searchSettings.mockResolvedValue({
        braveConfigured: false,
        ollamaConfigured: false,
        defaultProvider: "brave",
        errors: { [provider]: "keyring read failed" },
      });
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      await controller.navigate("settings");
      await controller.checkSearchSettings();
      const configured = provider === "brave" ? "braveConfigured" : "ollamaConfigured";
      expect(controller.state.settings?.[configured]).toBe(false);
      expect(controller.state.checkedProviders).not.toContain(provider);
      controller.state.drafts[provider === "brave" ? "braveKey" : "ollamaKey"] = "saved-key";
      await controller.saveKey(provider);
      expect(controller.state.settings?.[configured]).toBe(true);
      expect(controller.state.provider).toBe(provider);
      expect(controller.state.settings?.errors?.[provider]).toBeUndefined();
      expect(controller.state.notice).toBe("APIキーを保存しました。");
      expect(api.searchSettings).toHaveBeenCalledOnce();
      await controller.openModal({ kind: "image", itemId: 11 });
      expect(api.searchSettings).toHaveBeenCalledOnce();
      await controller.checkSearchSettings();
      expect(controller.state.settings?.[configured]).toBe(true);
      expect(controller.state.provider).toBe(provider);
      expect(controller.state.settings?.errors?.[provider]).toBe("keyring read failed");
      await controller.searchImages();
      expect(api.searchImages).not.toHaveBeenCalled();
    },
  );

  it.each([
    ["brave", false, true, false, "brave"],
    ["brave", true, false, false, "brave"],
    ["ollama", false, false, true, "ollama"],
    ["ollama", true, false, false, "brave"],
  ] as const)(
    "acknowledges successful %s key changes without reading the unchecked peer (remove=%s)",
    async (provider, remove, braveConfigured, ollamaConfigured, defaultProvider) => {
      const api = backend();
      api.searchSettings.mockRejectedValue(new Error("must not read credentials"));
      const controller = new AppController(api, vi.fn());
      await controller.navigate("settings");
      expect(controller.state.settings).toBeNull();
      const field = provider === "brave" ? "braveKey" : "ollamaKey";
      controller.state.drafts[field] = "saved-key";
      await controller.saveKey(provider, remove);
      expect(api.setApiKey).toHaveBeenCalledExactlyOnceWith(provider, remove ? "" : "saved-key");
      expect(controller.state.settings).toEqual({
        braveConfigured,
        ollamaConfigured,
        defaultProvider,
      });
      expect(controller.state.checkedProviders).toEqual([provider]);
      expect(controller.state.provider).toBe(defaultProvider);
      expect(controller.state.drafts[field]).toBe("");
      expect(controller.state.notice).toBe(
        remove ? "APIキーを削除しました。" : "APIキーを保存しました。",
      );
      expect(controller.state.error).toBe("");
      expect(controller.state.busy).toBe(false);
      expect(api.searchSettings).not.toHaveBeenCalled();
    },
  );

  it.each([
    ["brave", false, "brave"],
    ["brave", true, "ollama"],
    ["ollama", false, "ollama"],
    ["ollama", true, "brave"],
  ] as const)(
    "keeps successful %s key changes after an explicit confirmation fails (remove=%s)",
    async (provider, remove, defaultProvider) => {
      const api = backend();
      api.searchSettings.mockResolvedValue({
        braveConfigured: remove,
        ollamaConfigured: remove,
        defaultProvider: "brave",
      });
      const controller = new AppController(api, vi.fn());
      await controller.navigate("settings");
      await controller.checkSearchSettings();
      const field = provider === "brave" ? "braveKey" : "ollamaKey";
      controller.state.drafts[field] = "saved-key";
      api.searchSettings.mockRejectedValueOnce(new Error("keyring read failed"));
      await controller.saveKey(provider, remove);
      expect(api.searchSettings).toHaveBeenCalledOnce();
      expect(controller.state.error).toBe("");
      expect(controller.state.notice).toBe(
        remove ? "APIキーを削除しました。" : "APIキーを保存しました。",
      );
      await controller.checkSearchSettings();
      expect(api.setApiKey).toHaveBeenCalledExactlyOnceWith(provider, remove ? "" : "saved-key");
      expect(controller.state.drafts[field]).toBe("");
      expect(controller.state.settings).toMatchObject({
        braveConfigured: provider === "brave" ? !remove : remove,
        ollamaConfigured: provider === "ollama" ? !remove : remove,
        defaultProvider,
        errors: { brave: "keyring read failed", ollama: "keyring read failed" },
      });
      expect(controller.state.provider).toBe(defaultProvider);
      expect(controller.state.error).toContain("keyring read failed");
      expect(controller.state.busy).toBe(false);
    },
  );

  it.each([
    ["brave", false, true],
    ["brave", true, true],
    ["ollama", false, true],
    ["ollama", true, true],
    ["brave", false, false],
    ["brave", true, false],
    ["ollama", false, false],
    ["ollama", true, false],
  ] as const)(
    "keeps the draft and settings when %s key mutation fails (remove=%s, settings checked=%s)",
    async (provider, remove, checked) => {
      const api = backend();
      const settings = {
        braveConfigured: true,
        ollamaConfigured: true,
        defaultProvider: "brave" as const,
      };
      api.searchSettings.mockResolvedValueOnce(settings);
      const controller = new AppController(api, vi.fn());
      await controller.navigate("settings");
      if (checked) await controller.checkSearchSettings();
      const before = controller.state.settings;
      const known = [...controller.state.checkedProviders];
      const field = provider === "brave" ? "braveKey" : "ollamaKey";
      controller.state.drafts[field] = "retry-key";
      api.setApiKey.mockRejectedValueOnce(new Error("keyring write failed"));
      await controller.saveKey(provider, remove);
      expect(controller.state.drafts[field]).toBe("retry-key");
      expect(controller.state.settings).toEqual(before);
      expect(controller.state.checkedProviders).toEqual(known);
      expect(controller.state.notice).toBe("");
      expect(controller.state.error).toBe("keyring write failed");
      expect(api.searchSettings).toHaveBeenCalledTimes(checked ? 1 : 0);
      await controller.saveKey(provider, remove);
      expect(api.setApiKey).toHaveBeenCalledTimes(2);
      expect(controller.state.error).toBe("");
      expect(controller.state.drafts[field]).toBe("");
      expect(api.searchSettings).toHaveBeenCalledTimes(checked ? 1 : 0);
    },
  );

  it("checks both providers only when explicitly requested after saving a key", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.navigate("settings");
    controller.state.drafts.ollamaKey = "saved-key";
    const current = {
      braveConfigured: true,
      ollamaConfigured: true,
      defaultProvider: "brave" as const,
    };
    api.searchSettings.mockResolvedValueOnce(current);
    await controller.saveKey("ollama");
    expect(controller.state.checkedProviders).toEqual(["ollama"]);
    expect(controller.state.provider).toBe("ollama");
    expect(api.searchSettings).not.toHaveBeenCalled();
    await controller.checkSearchSettings();
    expect(controller.state.settings).toEqual(current);
    expect(controller.state.checkedProviders).toEqual(expect.arrayContaining(["brave", "ollama"]));
    expect(controller.state.provider).toBe("brave");
    expect(controller.state.drafts.ollamaKey).toBe("");
    expect(controller.state.error).toBe("");
    expect(api.searchSettings).toHaveBeenCalledOnce();
  });
});

describe("comparison state and persistence boundaries", () => {
  it.each(["create-list", "rename-list", "rename-item"] as const)(
    "rejects whitespace-only names in %s and preserves the draft for correction",
    async (kind) => {
      const api = backend();
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      const modal = kind === "rename-item" ? { kind, itemId: 11 } : { kind };
      await controller.openModal(modal);
      controller.state.drafts.name = " \t　 ";
      await controller.saveName();
      expect(controller.state.error).toBe("名前を入力してください。");
      expect(controller.state.modal).toEqual(modal);
      expect(controller.state.drafts.name).toBe(" \t　 ");
      expect(controller.state.busy).toBe(false);
      expect(api.createList).not.toHaveBeenCalled();
      expect(api.renameList).not.toHaveBeenCalled();
      expect(api.renameItem).not.toHaveBeenCalled();
      controller.state.drafts.name = " 修正した名前 ";
      await controller.saveName();
      expect(controller.state.error).toBe("");
      expect(controller.state.modal).toBeNull();
    },
  );

  it("updates the selected sidebar entry from the current list snapshot", async () => {
    const api = backend();
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    const latest = {
      ...list(2),
      name: "別のウィンドウで変更したリスト",
      items: list(2).items.slice(0, 1),
      comparisonCount: 12,
      convergence: { ...list(2).convergence, converged: true },
    };
    api.getList.mockResolvedValue(latest);
    await controller.selectList(2);
    expect(controller.state.active).toEqual(latest);
    expect(controller.state.lists).toEqual([
      summary(list()),
      {
        id: 2,
        name: "別のウィンドウで変更したリスト",
        itemCount: 1,
        comparisonCount: 12,
        converged: true,
      },
    ]);
    expect(controller.state.view).toBe("ranking");
    await controller.selectList(2);
    expect(controller.state.lists).toHaveLength(2);
  });

  it("does not read bulk credentials when a sidebar selection enters items from ranking", async () => {
    const settled = {
      ...list(),
      convergence: { ...list().convergence, converged: true },
    };
    const api = backend(settled, list(2));
    api.searchSettings.mockResolvedValue({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    expect(controller.state.view).toBe("ranking");

    await controller.selectList(2);
    expect(controller.state.view).toBe("items");
    expect(controller.availableBulkProviders()).toEqual([]);
    expect(api.searchSettings).not.toHaveBeenCalled();
  });

  it("does not read bulk credentials when renaming a converged list enters items", async () => {
    const settled = {
      ...list(),
      convergence: { ...list().convergence, converged: true },
    };
    const api = backend(settled);
    api.searchSettings.mockResolvedValue({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    api.renameList.mockResolvedValue({ ...settled, name: "改名後" });
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    expect(controller.state.view).toBe("ranking");

    await controller.openModal({ kind: "rename-list" });
    controller.state.drafts.name = "改名後";
    await controller.saveName();
    expect(controller.state.view).toBe("items");
    expect(controller.availableBulkProviders()).toEqual([]);
    expect(api.searchSettings).not.toHaveBeenCalled();
  });

  it("does not read bulk credentials when deleting a converged list enters items", async () => {
    const settled = {
      ...list(),
      convergence: { ...list().convergence, converged: true },
    };
    const remaining = list(2);
    const api = backend(settled, remaining);
    api.listSummaries.mockResolvedValueOnce([summary(settled), summary(remaining)]);
    api.listSummaries.mockResolvedValue([summary(remaining)]);
    api.searchSettings.mockResolvedValue({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    expect(controller.state.view).toBe("ranking");

    await controller.openModal({ kind: "delete-list" });
    await controller.confirmDelete();
    expect(controller.state.active).toEqual(remaining);
    expect(controller.state.view).toBe("items");
    expect(controller.availableBulkProviders()).toEqual([]);
    expect(api.searchSettings).not.toHaveBeenCalled();
  });

  it("does not read bulk credentials when comparison returns to items", async () => {
    const settled = {
      ...list(),
      convergence: { ...list().convergence, converged: true },
    };
    const api = backend(settled);
    api.searchSettings.mockResolvedValue({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    expect(controller.state.view).toBe("ranking");

    api.resumeList.mockResolvedValue({ ...settled, items: settled.items.slice(0, 1) });
    api.nextPair.mockResolvedValue(null);
    await controller.startComparison();
    expect(controller.state.view).toBe("items");
    expect(controller.availableBulkProviders()).toEqual([]);
    expect(api.searchSettings).not.toHaveBeenCalled();
  });

  it("does not read bulk credentials when a deleted ranking list is replaced", async () => {
    const settled = {
      ...list(),
      convergence: { ...list().convergence, converged: true },
    };
    const remaining = list(2);
    const api = backend(settled, remaining);
    api.listSummaries.mockResolvedValueOnce([summary(settled), summary(remaining)]);
    api.listSummaries.mockResolvedValue([summary(remaining)]);
    api.searchSettings.mockResolvedValue({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    expect(controller.state.view).toBe("ranking");

    api.resumeList.mockRejectedValueOnce("リストが見つかりません。");
    await controller.startComparison();
    expect(controller.state.active).toEqual(remaining);
    expect(controller.state.view).toBe("items");
    expect(controller.availableBulkProviders()).toEqual([]);
    expect(api.searchSettings).not.toHaveBeenCalled();
  });

  it.each(["restart", "settled answer"] as const)(
    "recovers when a successful %s is followed by a sidebar refresh confirming list deletion",
    async (operation) => {
      const { controller, api, saved } = await comparison();
      const remaining = list(2);
      api.listSummaries.mockResolvedValue([summary(remaining)]);
      if (operation === "restart") await controller.startComparison();
      else {
        api.answer.mockResolvedValueOnce({
          ...saved,
          convergence: { ...saved.convergence, converged: true },
        });
        await controller.answer("equal");
      }
      expect(controller.state.active).toEqual(remaining);
      expect(controller.state.pair).toBeNull();
      expect(controller.state.view).toBe("items");
      expect(controller.state.notice).toBe("");
      expect(controller.state.error).toBe("リストが見つかりません。");
      expect(controller.state.busy).toBe(false);
    },
  );

  it.each(["remaining", "empty"] as const)(
    "recovers a deleted active sidebar selection to the %s state",
    async (destination) => {
      const { controller, api } = await comparison();
      controller.state.drafts.items = "deleted draft";
      const remaining = list(2);
      api.getList.mockRejectedValueOnce("リストが見つかりません。");
      api.listSummaries.mockResolvedValue(destination === "remaining" ? [summary(remaining)] : []);
      await controller.selectList(1);
      expect(controller.state.active).toEqual(destination === "remaining" ? remaining : null);
      expect(controller.state.lists).toEqual(
        destination === "remaining" ? [summary(remaining)] : [],
      );
      expect(controller.state.pair).toBeNull();
      expect(controller.state.drafts.items).toBe("");
      expect(controller.state.view).toBe("items");
      expect(controller.state.error).toBe("リストが見つかりません。");
      expect(controller.state.busy).toBe(false);
    },
  );

  it.each([false, true])(
    "preserves the current list when another sidebar entry is missing (refresh failure=%s)",
    async (refreshFailure) => {
      const { controller, api, initial } = await comparison();
      const pair = controller.state.pair;
      controller.state.drafts.items = "current draft";
      api.getList.mockRejectedValueOnce(new Error("リストが見つかりません。"));
      if (refreshFailure) api.listSummaries.mockRejectedValueOnce("一覧を読み込めません。");
      else api.listSummaries.mockResolvedValue([summary(initial)]);
      await controller.selectList(2);
      expect(controller.state.active).toEqual(initial);
      expect(controller.state.lists).toEqual([summary(initial)]);
      expect(controller.state.pair).toEqual(pair);
      expect(controller.state.drafts.items).toBe("current draft");
      expect(controller.state.view).toBe("compare");
      expect(controller.state.error).toBe(
        refreshFailure ? "一覧を読み込めません。" : "リストが見つかりません。",
      );
    },
  );

  it("clears a deleted active sidebar selection even when refreshing fails", async () => {
    const { controller, api } = await comparison();
    controller.state.drafts.items = "deleted draft";
    api.getList.mockRejectedValueOnce("リストが見つかりません。");
    api.listSummaries.mockRejectedValueOnce("一覧を読み込めません。");
    await controller.selectList(1);
    expect(controller.state.active).toBeNull();
    expect(controller.state.pair).toBeNull();
    expect(controller.state.lists.map((entry) => entry.id)).toEqual([2]);
    expect(controller.state.drafts.items).toBe("");
    expect(controller.state.error).toBe("一覧を読み込めません。");
  });

  it("reconciles both the requested and active sidebar entries if both were deleted", async () => {
    const { controller, api } = await comparison();
    const remaining = list(3);
    controller.state.drafts.items = "deleted draft";
    api.getList.mockRejectedValueOnce("リストが見つかりません。").mockResolvedValueOnce(remaining);
    api.listSummaries.mockResolvedValue([summary(remaining)]);
    await controller.selectList(2);
    expect(controller.state.active).toEqual(remaining);
    expect(controller.state.pair).toBeNull();
    expect(controller.state.lists).toEqual([summary(remaining)]);
    expect(controller.state.drafts.items).toBe("");
    expect(controller.state.view).toBe("items");
  });

  it("preserves sidebar state for a transient selection failure", async () => {
    const { controller, api, initial } = await comparison();
    const pair = controller.state.pair;
    const summaries = controller.state.lists;
    const summaryReads = api.listSummaries.mock.calls.length;
    controller.state.drafts.items = "current draft";
    api.getList.mockRejectedValueOnce("データベースを読み込めません。");
    await controller.selectList(2);
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.pair).toEqual(pair);
    expect(controller.state.lists).toEqual(summaries);
    expect(controller.state.drafts.items).toBe("current draft");
    expect(api.listSummaries).toHaveBeenCalledTimes(summaryReads);
    expect(controller.state.error).toBe("データベースを読み込めません。");
  });

  it("opens a remaining list when the initial list is deleted before loading", async () => {
    const first = list();
    const second = list(2);
    second.convergence.converged = true;
    const api = backend(first, second);
    api.listSummaries
      .mockResolvedValueOnce([summary(first), summary(second)])
      .mockResolvedValueOnce([summary(second)]);
    api.getList.mockRejectedValueOnce("リストが見つかりません。");
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    expect(controller.state.fatal).toBe(false);
    expect(controller.state.initialized).toBe(true);
    expect(controller.state.active).toEqual(second);
    expect(controller.state.lists).toEqual([summary(second)]);
    expect(controller.state.view).toBe("ranking");
    expect(controller.state.error).toBe("");
  });

  describe.each(["startup", "after deletion"] as const)("list selection %s", (context) => {
    it("refreshes the first available list summary from the loaded snapshot", async () => {
      const { api, controller, load } = await listSelection(context);
      const stale = list(2);
      const current = {
        ...stale,
        name: "別のウィンドウで更新",
        items: stale.items.slice(0, 1),
        comparisonCount: 15,
        convergence: { ...stale.convergence, converged: true },
      };
      api.listSummaries.mockResolvedValue([summary(stale), summary(list(3))]);
      api.getList.mockResolvedValueOnce(current);
      if (context === "startup") controller.state.drafts.items = "startup draft";
      await load();
      expect(controller.state.active).toEqual(current);
      expect(controller.state.lists).toEqual([
        { id: 2, name: "別のウィンドウで更新", itemCount: 1, comparisonCount: 15, converged: true },
        summary(list(3)),
      ]);
      expect(controller.state.view).toBe(context === "startup" ? "ranking" : "items");
      expect(controller.state.drafts.items).toBe(context === "startup" ? "startup draft" : "");
      expect(controller.state.pair).toBeNull();
    });

    it("opens an empty interactive state when no lists remain", async () => {
      const { api, controller, load } = await listSelection(context);
      api.listSummaries.mockResolvedValueOnce([summary(list(2))]).mockResolvedValueOnce([]);
      api.getList.mockRejectedValueOnce("リストが見つかりません。");
      await load();
      expect(controller.state.fatal).toBe(false);
      expect(controller.state.initialized).toBe(true);
      expect(controller.state.active).toBeNull();
      expect(controller.state.lists).toEqual([]);
      expect(controller.state.error).toBe("");
      expect(controller.state.drafts.items).toBe("");
      await controller.openModal({ kind: "create-list" });
      expect(controller.state.modal).toEqual({ kind: "create-list" });
    });

    it("bounds repeated deletions and permits selecting a fresh list", async () => {
      const { api, controller, load } = await listSelection(context);
      const remaining = list(5);
      api.listSummaries
        .mockResolvedValueOnce([summary(list(2))])
        .mockResolvedValueOnce([summary(list(3))])
        .mockResolvedValueOnce([summary(list(4))])
        .mockResolvedValueOnce([summary(remaining)]);
      api.getList
        .mockRejectedValueOnce("リストが見つかりません。")
        .mockRejectedValueOnce("リストが見つかりません。")
        .mockRejectedValueOnce("リストが見つかりません。")
        .mockResolvedValueOnce(remaining);
      await load();
      expect(controller.state.fatal).toBe(false);
      expect(controller.state.initialized).toBe(true);
      expect(controller.state.busy).toBe(false);
      expect(controller.state.active).toBeNull();
      expect(controller.state.lists).toEqual([summary(remaining)]);
      expect(controller.state.notice).toContain("リストを選択");
      expect(controller.state.drafts.items).toBe("");
      await controller.selectList(remaining.id);
      expect(controller.state.active).toEqual(remaining);
    });

    it.each(["initial summaries", "lookup", "refreshed summaries"] as const)(
      "reports database errors in %s with the appropriate fatal state",
      async (failure) => {
        const { api, controller, load } = await listSelection(context);
        const error = "データベースを読み込めません。";
        if (failure === "initial summaries") api.listSummaries.mockRejectedValueOnce(error);
        else if (failure === "lookup") api.getList.mockRejectedValueOnce(error);
        else {
          api.getList.mockRejectedValueOnce("リストが見つかりません。");
          api.listSummaries.mockResolvedValueOnce([summary(list(2))]).mockRejectedValueOnce(error);
        }
        await load();
        expect(controller.state.fatal).toBe(context === "startup");
        expect(controller.state.initialized).toBe(context === "after deletion");
        expect(controller.state.error).toBe(error);
        expect(controller.state.active).toBeNull();
        expect(controller.state.drafts.items).toBe("");
      },
    );
  });

  it("trims bulk input, retains duplicate names, and submits only to the selected list", async () => {
    const first = list();
    const second = list(2);
    const api = backend(first, second);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.state.drafts.items = "first list draft";
    await controller.selectList(second.id);
    expect(controller.state.drafts.items).toBe("");
    controller.state.drafts.items = "  A \r\n\n B \nA\n  ";
    api.addItems.mockResolvedValue({ ...second, revision: second.revision + 1 });
    await controller.addItems();
    expect(api.addItems).toHaveBeenCalledExactlyOnceWith(second.id, ["A", "B", "A"]);
    expect(controller.state.active?.id).toBe(second.id);
    expect(controller.state.drafts.items).toBe("");
    expect(controller.state.notice).toContain("3件");
    await controller.selectList(first.id);
    expect(controller.state.active).toEqual(first);
    controller.state.drafts.items = " \n\t";
    await controller.addItems();
    expect(controller.state.error).toBe("項目名を入力してください。");
    expect(controller.state.drafts.items).toBe(" \n\t");
    expect(api.addItems).toHaveBeenCalledTimes(1);
  });

  it("keeps entered names available when adding items cannot be saved", async () => {
    const { controller, api, initial } = await comparison();
    controller.state.drafts.items = "new one\nnew two";
    api.addItems.mockRejectedValue(new Error("保存できません"));
    await controller.addItems();
    expect(controller.state.drafts.items).toBe("new one\nnew two");
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.error).toBe("保存できません");
  });

  it.each(["remaining list", "new list"] as const)(
    "does not transfer item drafts from a deleted list to the %s",
    async (destination) => {
      const first = list();
      const second = list(2);
      const api = backend(first, second);
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      controller.state.drafts.items = "only for the deleted list";
      await controller.openModal({ kind: "delete-list" });
      api.listSummaries.mockResolvedValue(
        destination === "remaining list" ? [summary(second)] : [],
      );
      await controller.confirmDelete();
      if (destination === "new list") {
        api.createList.mockResolvedValue(second);
        api.listSummaries.mockResolvedValue([summary(second)]);
        await controller.openModal({ kind: "create-list" });
        controller.state.drafts.name = second.name;
        await controller.saveName();
      }
      expect(controller.state.active?.id).toBe(second.id);
      expect(controller.state.drafts.items).toBe("");
      await controller.addItems();
      expect(api.addItems).not.toHaveBeenCalled();
    },
  );

  it("removes a committed list deletion locally even if refreshing summaries fails", async () => {
    const first = list();
    const second = list(2);
    const api = backend(first, second);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.state.drafts.items = "deleted list draft";
    await controller.openModal({ kind: "delete-list" });
    api.listSummaries.mockRejectedValueOnce(new Error("一覧の更新に失敗しました"));
    await controller.confirmDelete();
    expect(controller.state.lists).toEqual([summary(second)]);
    expect(controller.state.active).toBeNull();
    expect(controller.state.modal).toBeNull();
    expect(controller.state.drafts.items).toBe("");
    expect(controller.state.error).toBe("一覧の更新に失敗しました");
    await controller.selectList(second.id);
    expect(controller.state.active).toEqual(second);
  });

  it("loads a remaining list if the replacement disappears after deleting the active list", async () => {
    const first = list();
    const replacement = list(2);
    const remaining = list(3);
    remaining.convergence.converged = true;
    const api = backend(first, replacement);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.state.drafts.items = "deleted list draft";
    await controller.navigate("ranking");
    await controller.openModal({ kind: "delete-list" });
    api.listSummaries
      .mockResolvedValueOnce([summary(replacement), summary(remaining)])
      .mockResolvedValueOnce([summary(remaining)]);
    api.getList.mockRejectedValueOnce("リストが見つかりません。").mockResolvedValueOnce(remaining);
    await controller.confirmDelete();
    expect(controller.state.active).toEqual(remaining);
    expect(controller.state.lists).toEqual([summary(remaining)]);
    expect(controller.state.view).toBe("items");
    expect(controller.state.drafts.items).toBe("");
    expect(controller.state.modal).toBeNull();
    expect(controller.state.fatal).toBe(false);
    expect(controller.state.error).toBe("");
  });

  it("retains the active list, summaries, and draft when deletion itself fails", async () => {
    const { controller, api, initial } = await comparison();
    const summaries = controller.state.lists;
    controller.state.drafts.items = "keep this draft";
    await controller.openModal({ kind: "delete-list" });
    api.deleteList.mockRejectedValueOnce(new Error("削除できません"));
    await controller.confirmDelete();
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.lists).toEqual(summaries);
    expect(controller.state.pair).toEqual(proposal(initial));
    expect(controller.state.modal).toEqual({ kind: "delete-list" });
    expect(controller.state.drafts.items).toBe("keep this draft");
    expect(controller.state.error).toBe("削除できません");
  });

  it("does not transfer the previous list's item draft when creating a new list", async () => {
    const first = list();
    const second = list(2);
    const api = backend(first, second);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    controller.state.drafts.items = "only for the previous list";
    await controller.openModal({ kind: "create-list" });
    controller.state.drafts.name = second.name;
    api.createList.mockResolvedValueOnce(second);
    await controller.saveName();
    expect(controller.state.active).toEqual(second);
    expect(controller.state.drafts.items).toBe("");
    await controller.addItems();
    expect(api.addItems).not.toHaveBeenCalled();
  });

  it("retains the previous list and its item draft when creating a new list fails", async () => {
    const { controller, api, initial } = await comparison();
    controller.state.drafts.items = "keep the previous list draft";
    await controller.openModal({ kind: "create-list" });
    controller.state.drafts.name = "new list";
    api.createList.mockRejectedValueOnce(new Error("作成できません"));
    await controller.saveName();
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.drafts.items).toBe("keep the previous list draft");
    expect(controller.state.modal).toEqual({ kind: "create-list" });
    expect(controller.state.error).toBe("作成できません");
  });

  it("replaces an existing sidebar summary after a committed edit despite refresh failure", async () => {
    const first = list();
    const second = list(2);
    const api = backend(first, second);
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    for (const name of ["一度目の変更", "二度目の変更"]) {
      await controller.openModal({ kind: "rename-list" });
      controller.state.drafts.name = name;
      api.renameList.mockResolvedValueOnce({ ...first, name });
      api.listSummaries.mockRejectedValueOnce(new Error("一覧の更新に失敗しました"));
      await controller.saveName();
      expect(controller.state.lists).toEqual([
        { id: 1, name, itemCount: 2, comparisonCount: 0, converged: false },
        summary(second),
      ]);
    }
  });

  it("accepts only one answer while a save is pending", async () => {
    const { controller, api, initial, saved } = await comparison();
    let finish: ((value: ListState) => void) | undefined;
    api.answer.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    api.nextPair.mockResolvedValue(proposal(saved));
    const pending = controller.answer("a_strong");
    await controller.answer("b_strong");
    expect(api.answer).toHaveBeenCalledExactlyOnceWith(proposal(initial), "a_strong");
    expect(controller.state.busy).toBe(true);
    expect(controller.state.active).toEqual(initial);
    if (!finish) throw new Error("The save was not started");
    finish(saved);
    await pending;
    expect(controller.state.busy).toBe(false);
    expect(controller.state.active).toEqual(saved);
    expect(controller.state.pair?.revision).toBe(saved.revision);
  });

  it.each(["string", "Error"] as const)(
    "retains the current proposal and ratings after a transient save failure reported as %s",
    async (representation) => {
      const { controller, api, initial, saved } = await comparison();
      const message = "ディスクへの保存に失敗しました";
      api.answer.mockRejectedValueOnce(representation === "string" ? message : new Error(message));
      await controller.answer("equal");
      expect(controller.state.active).toEqual(initial);
      expect(controller.state.pair).toEqual(proposal(initial));
      expect(api.nextPair).toHaveBeenCalledTimes(1);
      expect(controller.state.error).toBe(message);
      api.answer.mockResolvedValue(saved);
      api.nextPair.mockResolvedValue(proposal(saved));
      await controller.answer("equal");
      expect(api.answer).toHaveBeenCalledTimes(2);
      expect(controller.state.active).toEqual(saved);
      expect(controller.state.error).toBe("");
    },
  );

  it.each(["string", "Error"] as const)(
    "discards a revision-rejected proposal reported as %s and resumes with a fresh proposal",
    async (representation) => {
      const { controller, api, initial, saved } = await comparison();
      const message = "リストが更新されています。最新の比較を読み直してください。";
      api.answer.mockRejectedValueOnce(representation === "string" ? message : new Error(message));
      await controller.answer("equal");
      expect(controller.state.active).toEqual(initial);
      expect(controller.state.pair).toBeNull();
      expect(controller.state.error).toBe(
        "リストが更新されています。もう一度比較を開始してください。",
      );
      expect(controller.state.busy).toBe(false);
      await controller.answer("equal");
      expect(api.answer).toHaveBeenCalledOnce();

      api.resumeList.mockResolvedValue(saved);
      api.nextPair.mockResolvedValue(proposal(saved));
      await controller.startComparison();
      expect(controller.state.pair).toEqual(proposal(saved));
      expect(controller.state.error).toBe("");
      const updated = { ...saved, revision: saved.revision + 1, comparisonCount: 2 };
      api.answer.mockResolvedValue(updated);
      api.nextPair.mockResolvedValue(proposal(updated));
      await controller.answer("equal");
      expect(api.answer).toHaveBeenNthCalledWith(2, proposal(saved), "equal");
      expect(controller.state.active).toEqual(updated);
    },
  );

  it.each([
    ["string", "remaining"],
    ["Error", "remaining"],
    ["string", "empty"],
    ["Error", "empty"],
  ] as const)(
    "removes a deleted comparison list after a %s rejection and opens the %s state",
    async (representation, destination) => {
      const { controller, api } = await comparison();
      const remaining = list(2);
      remaining.convergence.converged = true;
      const message = "リストが見つかりません。";
      controller.state.drafts.items = "deleted list draft";
      api.answer.mockRejectedValueOnce(representation === "string" ? message : new Error(message));
      api.listSummaries.mockResolvedValue(destination === "remaining" ? [summary(remaining)] : []);
      api.getList.mockResolvedValue(remaining);
      await controller.answer("equal");
      expect(controller.state.pair).toBeNull();
      expect(controller.state.active).toEqual(destination === "remaining" ? remaining : null);
      expect(controller.state.lists).toEqual(
        destination === "remaining" ? [summary(remaining)] : [],
      );
      expect(controller.state.drafts.items).toBe("");
      expect(controller.state.view).toBe(destination === "remaining" ? "ranking" : "items");
      expect(controller.state.error).toBe(message);
      expect(controller.state.busy).toBe(false);
      await controller.answer("equal");
      expect(api.answer).toHaveBeenCalledOnce();
    },
  );

  it("keeps a deleted comparison list removed when refreshing the remaining lists fails", async () => {
    const { controller, api } = await comparison();
    controller.state.drafts.items = "deleted list draft";
    api.answer.mockRejectedValueOnce("リストが見つかりません。");
    api.listSummaries.mockRejectedValueOnce("一覧を読み込めません。");
    await controller.answer("equal");
    expect(controller.state.pair).toBeNull();
    expect(controller.state.active).toBeNull();
    expect(controller.state.lists.map((entry) => entry.id)).toEqual([2]);
    expect(controller.state.drafts.items).toBe("");
    expect(controller.state.view).toBe("items");
    expect(controller.state.error).toBe("一覧を読み込めません。");
    await controller.answer("equal");
    expect(api.answer).toHaveBeenCalledOnce();
    await controller.selectList(2);
    expect(controller.state.active?.id).toBe(2);
  });

  it.each(["resume", "pair selection"] as const)(
    "recovers when the current list disappears during %s after a rejected stale answer",
    async (failure) => {
      const { controller, api } = await comparison();
      api.answer.mockRejectedValueOnce(
        "リストが更新されています。最新の比較を読み直してください。",
      );
      await controller.answer("equal");
      const message = "リストが見つかりません。";
      if (failure === "resume") api.resumeList.mockRejectedValueOnce(message);
      else api.nextPair.mockRejectedValueOnce(message);
      api.listSummaries.mockResolvedValue([summary(list(2))]);
      await controller.startComparison();
      expect(controller.state.pair).toBeNull();
      expect(controller.state.active?.id).toBe(2);
      expect(controller.state.lists.map((entry) => entry.id)).toEqual([2]);
      expect(controller.state.view).toBe("items");
      expect(controller.state.error).toBe(message);
      await controller.answer("equal");
      expect(api.answer).toHaveBeenCalledOnce();
    },
  );

  it("recovers if a successfully answered list is deleted before automatic pair selection", async () => {
    const { controller, api, saved } = await comparison();
    api.answer.mockResolvedValueOnce(saved);
    api.nextPair.mockRejectedValueOnce("リストが見つかりません。");
    api.listSummaries.mockResolvedValue([summary(list(2))]);
    await controller.answer("equal");
    expect(controller.state.pair).toBeNull();
    expect(controller.state.active?.id).toBe(2);
    expect(controller.state.lists.map((entry) => entry.id)).toEqual([2]);
    expect(controller.state.error).toBe("リストが見つかりません。");
    await controller.answer("equal");
    expect(api.answer).toHaveBeenCalledOnce();
  });

  it.each(["sidebar", "next pair"] as const)(
    "does not resubmit a committed answer when refreshing the %s fails",
    async (failure) => {
      const { controller, api, saved } = await comparison();
      api.answer.mockResolvedValue(saved);
      if (failure === "sidebar") api.listSummaries.mockRejectedValueOnce(new Error("更新失敗"));
      else api.nextPair.mockRejectedValueOnce(new Error("更新失敗"));
      await controller.answer("a_weak");
      expect(controller.state.active).toEqual(saved);
      if (failure === "sidebar") {
        expect(controller.state.lists[0]).toEqual({
          id: 1,
          name: "リスト1",
          itemCount: 2,
          comparisonCount: 1,
          converged: false,
        });
      }
      expect(controller.state.pair).toBeNull();
      expect(controller.state.error).toBe("更新失敗");
      await controller.answer("a_weak");
      expect(api.answer).toHaveBeenCalledTimes(1);
      api.nextPair.mockResolvedValue(proposal(saved));
      api.resumeList.mockResolvedValue(saved);
      await controller.startComparison();
      expect(controller.state.pair?.revision).toBe(saved.revision);
    },
  );

  it.each(["newer revision", "older revision", "missing pair"] as const)(
    "requires explicit restart when automatic pair selection returns a %s after saving",
    async (interference) => {
      const { controller, api, saved } = await comparison();
      const latest = { ...saved, revision: saved.revision + 2 };
      api.answer.mockResolvedValue(saved);
      let next: PairProposal | null = null;
      if (interference === "newer revision") next = proposal(latest);
      else if (interference === "older revision")
        next = proposal({ ...saved, revision: saved.revision - 1 });
      api.nextPair.mockResolvedValue(next);
      await controller.answer("equal");
      expect(controller.state.active).toEqual(saved);
      expect(controller.state.pair).toBeNull();
      expect(controller.state.error).toBe(
        "リストが更新されています。もう一度比較を開始してください。",
      );
      expect(controller.state.busy).toBe(false);
      await controller.answer("equal");
      expect(api.answer).toHaveBeenCalledOnce();

      api.resumeList.mockResolvedValue(latest);
      api.nextPair.mockResolvedValue(proposal(latest));
      await controller.startComparison();
      expect(controller.state.active).toEqual(latest);
      expect(controller.state.pair).toEqual(proposal(latest));
      expect(controller.state.error).toBe("");
    },
  );

  it("shows the ranking at convergence and uses the reset returned by resume before comparing again", async () => {
    const { controller, api, saved } = await comparison();
    const settled: ListState = {
      ...saved,
      convergence: {
        converged: true,
        maxSigma: 2.4,
        maxRankSpan: 1,
        observedAnswers: 20,
        requiredAnswers: 20,
      },
    };
    api.answer.mockResolvedValue(settled);
    await controller.answer("a_weak");
    expect(controller.state.view).toBe("ranking");
    expect(controller.state.notice).toContain("順位ほぼ確定");
    expect(controller.state.pair).toBeNull();
    expect(api.nextPair).toHaveBeenCalledTimes(1);
    const resumed = {
      ...settled,
      revision: settled.revision + 1,
      convergence: { ...settled.convergence, converged: false, observedAnswers: 0 },
    };
    api.resumeList.mockResolvedValue(resumed);
    api.resumeList.mockClear();
    api.nextPair.mockResolvedValue(proposal(resumed));
    await controller.startComparison();
    expect(api.resumeList).toHaveBeenCalledExactlyOnceWith(settled.id);
    expect(controller.state.active).toEqual(resumed);
    expect(controller.state.pair?.revision).toBe(resumed.revision);
    expect(controller.state.view).toBe("compare");
  });

  it.each([false, true])(
    "starts comparison from current backend state when cached convergence is %s",
    async (cachedConverged) => {
      const cached = list();
      cached.convergence.converged = cachedConverged;
      const current: ListState = {
        ...cached,
        revision: 9,
        comparisonCount: 27,
        convergence: {
          ...cached.convergence,
          converged: false,
          observedAnswers: cachedConverged ? 7 : 0,
        },
      };
      const api = backend(cached);
      api.resumeList.mockResolvedValue(current);
      api.nextPair.mockResolvedValue(proposal(current));
      const controller = new AppController(api, vi.fn());
      await controller.initialize();
      await controller.startComparison();
      expect(controller.state.active).toEqual(current);
      expect(controller.state.pair).toEqual(proposal(current));
      expect(controller.state.view).toBe("compare");
    },
  );

  it("retries comparison start when a proposal belongs to a newer revision", async () => {
    const cached = list();
    const current = { ...cached, revision: 6 };
    const api = backend(cached);
    api.resumeList.mockResolvedValueOnce(cached).mockResolvedValueOnce(current);
    api.nextPair.mockResolvedValue(proposal(current));
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    api.listSummaries.mockClear();
    await controller.startComparison();
    expect(controller.state.active).toEqual(current);
    expect(controller.state.pair).toEqual(proposal(current));
    expect(api.listSummaries).toHaveBeenCalledOnce();
    expect(controller.state.error).toBe("");
  });

  it("starts comparison if another instance added items to a cached empty list", async () => {
    const current = list();
    const cached = { ...current, items: [], revision: 1 };
    const api = backend(current);
    api.getList.mockResolvedValueOnce(cached);
    api.resumeList.mockResolvedValue(current);
    api.nextPair.mockResolvedValue(proposal(current));
    const controller = new AppController(api, vi.fn());
    await controller.initialize();
    await controller.startComparison();
    expect(controller.state.active).toEqual(current);
    expect(controller.state.pair).toEqual(proposal(current));
    expect(controller.state.view).toBe("compare");
  });

  it.each(["newer revision", "missing pair"] as const)(
    "bounds comparison start retries for a %s and leaves no actionable stale pair",
    async (interference) => {
      const { controller, api, initial, saved } = await comparison();
      api.resumeList.mockClear();
      api.nextPair.mockClear();
      api.listSummaries.mockClear();
      api.nextPair.mockResolvedValue(interference === "newer revision" ? proposal(saved) : null);
      await controller.startComparison();
      expect(api.resumeList).toHaveBeenCalledTimes(3);
      expect(api.nextPair).toHaveBeenCalledTimes(3);
      expect(api.listSummaries).not.toHaveBeenCalled();
      expect(controller.state.active).toEqual(initial);
      expect(controller.state.pair).toBeNull();
      expect(controller.state.busy).toBe(false);
      expect(controller.state.error).toBe(
        "リストが更新されています。もう一度比較を開始してください。",
      );
      await controller.answer("equal");
      expect(api.answer).not.toHaveBeenCalled();

      api.resumeList.mockResolvedValue(saved);
      api.nextPair.mockResolvedValue(proposal(saved));
      await controller.startComparison();
      expect(controller.state.active).toEqual(saved);
      expect(controller.state.pair).toEqual(proposal(saved));
      expect(controller.state.error).toBe("");
    },
  );

  it.each(["before resume", "after resume"] as const)(
    "returns to item management when another instance removes an item %s",
    async (timing) => {
      const { controller, api, initial } = await comparison();
      const current = { ...initial, revision: 8, items: initial.items.slice(0, 1) };
      if (timing === "after resume") api.resumeList.mockResolvedValueOnce(initial);
      api.resumeList.mockResolvedValue(current);
      api.nextPair.mockResolvedValue(null);
      await controller.startComparison();
      expect(controller.state.active).toEqual(current);
      expect(controller.state.pair).toBeNull();
      expect(controller.state.view).toBe("items");
      expect(controller.state.error).toBe("");
      expect(controller.state.busy).toBe(false);
    },
  );

  it.each(["resume", "next pair", "sidebar"] as const)(
    "keeps the latest returned list but discards the previous pair when %s fails during start",
    async (failure) => {
      const { controller, api, initial, saved } = await comparison();
      api.resumeList.mockResolvedValue(saved);
      api.nextPair.mockResolvedValue(proposal(saved));
      const error = new Error("比較の準備に失敗しました");
      if (failure === "resume") api.resumeList.mockRejectedValueOnce(error);
      else if (failure === "next pair") api.nextPair.mockRejectedValueOnce(error);
      else api.listSummaries.mockRejectedValueOnce(error);
      await controller.startComparison();
      expect(controller.state.active).toEqual(failure === "resume" ? initial : saved);
      expect(controller.state.pair).toBeNull();
      expect(controller.state.busy).toBe(false);
      expect(controller.state.error).toBe("比較の準備に失敗しました");
      await controller.answer("equal");
      expect(api.answer).not.toHaveBeenCalled();
    },
  );

  it("does not discard the current list if selecting a different list fails", async () => {
    const { controller, api, initial } = await comparison();
    api.getList.mockRejectedValueOnce(new Error("読み込み失敗"));
    await controller.selectList(2);
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.pair).toEqual(proposal(initial));
    expect(controller.state.error).toBe("読み込み失敗");
  });

  it.each([
    ["success", "read"],
    ["failure", "read"],
    ["success", "write"],
    ["failure", "write"],
  ] as const)(
    "ignores a dismissed modal settings %s while a newer %s remains pending",
    async (outcome, operation) => {
      const initial = list();
      const api = backend(initial);
      const controller = new AppController(api, vi.fn());
      let finishOld: (() => void) | undefined;
      api.searchSettings.mockImplementationOnce(
        () =>
          new Promise((resolve, reject) => {
            finishOld = () => {
              if (outcome === "success")
                resolve({
                  braveConfigured: false,
                  ollamaConfigured: false,
                  defaultProvider: "brave",
                });
              else reject(new Error("古い設定の読み込みに失敗しました"));
            };
          }),
      );
      await controller.initialize();
      await controller.openModal({ kind: "image", itemId: 11 });
      const oldOpen = controller.checkSearchSettings();
      controller.closeModal();
      const currentSettings = {
        braveConfigured: false,
        ollamaConfigured: true,
        defaultProvider: "ollama" as const,
      };
      let finishCurrent: (() => void) | undefined;
      let pending: Promise<void>;
      if (operation === "read") {
        api.searchSettings.mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finishCurrent = () => resolve(currentSettings);
            }),
        );
        await controller.openModal({ kind: "image", itemId: 12 });
        pending = controller.checkSearchSettings();
      } else {
        await controller.openModal({ kind: "create-list" });
        controller.state.drafts.name = "new list";
        api.createList.mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finishCurrent = () => resolve(initial);
            }),
        );
        pending = controller.saveName();
      }
      if (!finishOld) throw new Error("The first read must have started");
      finishOld();
      await oldOpen;
      if (operation === "read") {
        await vi.waitFor(() => expect(api.searchSettings).toHaveBeenCalledTimes(2));
        expect(controller.state.modal).toEqual({ kind: "image", itemId: 12 });
      }
      expect(controller.state.settings).toBeNull();
      expect(controller.state.provider).toBe("brave");
      expect(controller.state.busy).toBe(true);
      expect(controller.state.error).toBe("");
      if (operation === "write") {
        controller.closeModal();
        expect(controller.state.modal).toEqual({ kind: "create-list" });
      }
      if (!finishCurrent) throw new Error("The current operation must have started");
      finishCurrent();
      await pending;
      expect(controller.state.busy).toBe(false);
      if (operation === "read") {
        expect(controller.state.settings).toEqual(currentSettings);
        expect(controller.state.provider).toBe("ollama");
      }
    },
  );

  it.each([
    ["result", "before"],
    ["error", "before"],
    ["result", "after"],
    ["error", "after"],
  ] as const)(
    "ignores a dismissed image search's %s arriving %s the reopened modal's search completes",
    async (outcome, order) => {
      const initial = list();
      const api = backend(initial);
      api.searchSettings.mockResolvedValue({
        braveConfigured: true,
        ollamaConfigured: false,
        defaultProvider: "brave",
      });
      const controller = new AppController(api, vi.fn());
      const oldCandidate: ImageCandidate = {
        id: "old",
        title: "Old image",
        previewUrl: "data:image/png;base64,AA==",
        sourceUrl: "https://example.org/old",
      };
      const newCandidate = { ...oldCandidate, id: "new", title: "New image" };
      let finishOld: (() => void) | undefined;
      let finishNew: (() => void) | undefined;
      api.searchImages
        .mockImplementationOnce(
          () =>
            new Promise((resolve, reject) => {
              finishOld = () => {
                if (outcome === "result") resolve([oldCandidate]);
                else reject(new Error("古い検索のエラー"));
              };
            }),
        )
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finishNew = () => resolve([newCandidate]);
            }),
        );
      await controller.initialize();
      await controller.openModal({ kind: "image", itemId: 11 });
      await controller.checkSearchSettings();
      const oldSearch = controller.searchImages();
      controller.closeModal();
      await controller.openModal({ kind: "image", itemId: 12 });
      const newSearch = controller.searchImages();
      if (!finishOld || !finishNew) throw new Error("Both searches must have started");
      if (order === "before") {
        finishOld();
        await oldSearch;
        expect(controller.state.busy).toBe(true);
        expect(controller.state.candidates).toEqual([]);
        expect(controller.state.searched).toBe(false);
      }
      finishNew();
      await newSearch;
      if (order === "after") {
        finishOld();
        await oldSearch;
      }
      expect(controller.state.modal).toEqual({ kind: "image", itemId: 12 });
      expect(controller.state.busy).toBe(false);
      expect(controller.state.searched).toBe(true);
      expect(controller.state.candidates).toEqual([newCandidate]);
      expect(controller.state.error).toBe("");
    },
  );
});
