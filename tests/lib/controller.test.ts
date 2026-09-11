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
    })),
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
    renameList: vi.fn<AppApi["renameList"]>().mockResolvedValue(first),
    deleteList: vi.fn<AppApi["deleteList"]>().mockResolvedValue(undefined),
    addItems: vi.fn<AppApi["addItems"]>().mockResolvedValue(first),
    renameItem: vi.fn<AppApi["renameItem"]>().mockResolvedValue(first),
    deleteItem: vi.fn<AppApi["deleteItem"]>().mockResolvedValue(first),
    resumeList: vi.fn<AppApi["resumeList"]>().mockResolvedValue(first),
    nextPair: vi.fn<AppApi["nextPair"]>().mockResolvedValue(proposal(first)),
    answer: vi.fn<AppApi["answer"]>().mockResolvedValue(first),
    searchSettings: vi.fn<AppApi["searchSettings"]>().mockResolvedValue(settings),
    setApiKey: vi.fn<AppApi["setApiKey"]>().mockResolvedValue(settings),
    searchImages: vi.fn<AppApi["searchImages"]>().mockResolvedValue([]),
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

describe("comparison state and persistence boundaries", () => {
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

  it("retains the current proposal and ratings after backend save failure so retry is possible", async () => {
    const { controller, api, initial, saved } = await comparison();
    api.answer.mockRejectedValueOnce(new Error("ディスクへの保存に失敗しました"));
    await controller.answer("equal");
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.pair).toEqual(proposal(initial));
    expect(api.nextPair).toHaveBeenCalledTimes(1);
    expect(controller.state.error).toContain("保存に失敗");
    api.answer.mockResolvedValue(saved);
    await controller.answer("equal");
    expect(api.answer).toHaveBeenCalledTimes(2);
    expect(controller.state.active).toEqual(saved);
    expect(controller.state.error).toBe("");
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
      expect(controller.state.pair).toBeNull();
      expect(controller.state.error).toBe("更新失敗");
      await controller.answer("a_weak");
      expect(api.answer).toHaveBeenCalledTimes(1);
      api.nextPair.mockResolvedValue(proposal(saved));
      await controller.startComparison();
      expect(controller.state.pair?.revision).toBe(saved.revision);
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
    api.nextPair.mockResolvedValue(proposal(resumed));
    await controller.startComparison();
    expect(api.resumeList).toHaveBeenCalledExactlyOnceWith(settled.id);
    expect(controller.state.active).toEqual(resumed);
    expect(controller.state.pair?.revision).toBe(resumed.revision);
    expect(controller.state.view).toBe("compare");
  });

  it("does not discard the current list if selecting a different list fails", async () => {
    const { controller, api, initial } = await comparison();
    api.getList.mockRejectedValueOnce(new Error("読み込み失敗"));
    await controller.selectList(2);
    expect(controller.state.active).toEqual(initial);
    expect(controller.state.pair).toEqual(proposal(initial));
    expect(controller.state.error).toBe("読み込み失敗");
  });

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
