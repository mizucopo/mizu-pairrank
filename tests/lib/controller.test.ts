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
    setApiKey: vi.fn<AppApi["setApiKey"]>().mockResolvedValue(undefined),
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
      break;
    case "deleteList":
      await controller.openModal({ kind: "delete-list" });
      break;
    case "renameItem":
      await controller.openModal({ kind: "rename-item", itemId: 11 });
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

describe("credential mutation acknowledgments", () => {
  it.each([
    ["brave", false, true, false, "brave"],
    ["brave", true, false, false, "brave"],
    ["ollama", false, false, true, "ollama"],
    ["ollama", true, false, false, "brave"],
  ] as const)(
    "keeps successful %s key changes (remove=%s) when the initial settings read and refresh both fail",
    async (provider, remove, braveConfigured, ollamaConfigured, defaultProvider) => {
      const api = backend();
      api.searchSettings.mockRejectedValue(new Error("keyring read failed"));
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
      expect(controller.state.provider).toBe(defaultProvider);
      expect(controller.state.drafts[field]).toBe("");
      expect(controller.state.notice).toBe(
        remove ? "APIキーを削除しました。" : "APIキーを保存しました。",
      );
      expect(controller.state.error).toContain("設定状態を再取得できませんでした");
      expect(controller.state.busy).toBe(false);
    },
  );

  it.each([
    ["brave", false, "brave"],
    ["brave", true, "ollama"],
    ["ollama", false, "ollama"],
    ["ollama", true, "brave"],
  ] as const)(
    "keeps successful %s key changes (remove=%s) when refreshing settings fails",
    async (provider, remove, defaultProvider) => {
      const api = backend();
      api.searchSettings.mockResolvedValue({
        braveConfigured: remove,
        ollamaConfigured: remove,
        defaultProvider: "brave",
      });
      const controller = new AppController(api, vi.fn());
      await controller.navigate("settings");
      const field = provider === "brave" ? "braveKey" : "ollamaKey";
      controller.state.drafts[field] = "saved-key";
      api.searchSettings.mockRejectedValueOnce(new Error("keyring read failed"));
      await controller.saveKey(provider, remove);
      expect(api.setApiKey).toHaveBeenCalledExactlyOnceWith(provider, remove ? "" : "saved-key");
      expect(controller.state.drafts[field]).toBe("");
      expect(controller.state.notice).toBe(
        remove ? "APIキーを削除しました。" : "APIキーを保存しました。",
      );
      expect(controller.state.settings).toEqual({
        braveConfigured: provider === "brave" ? !remove : remove,
        ollamaConfigured: provider === "ollama" ? !remove : remove,
        defaultProvider,
      });
      expect(controller.state.provider).toBe(defaultProvider);
      expect(controller.state.error).toContain("設定状態を再取得できませんでした");
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
    "keeps the draft and settings when %s key mutation fails (remove=%s, settings loaded=%s)",
    async (provider, remove, loaded) => {
      const api = backend();
      const settings = {
        braveConfigured: true,
        ollamaConfigured: true,
        defaultProvider: "brave" as const,
      };
      if (loaded) api.searchSettings.mockResolvedValueOnce(settings);
      else api.searchSettings.mockRejectedValueOnce(new Error("keyring read failed"));
      const controller = new AppController(api, vi.fn());
      await controller.navigate("settings");
      const field = provider === "brave" ? "braveKey" : "ollamaKey";
      controller.state.drafts[field] = "retry-key";
      api.setApiKey.mockRejectedValueOnce(new Error("keyring write failed"));
      await controller.saveKey(provider, remove);
      expect(controller.state.drafts[field]).toBe("retry-key");
      expect(controller.state.settings).toEqual(loaded ? settings : null);
      expect(controller.state.notice).toBe("");
      expect(controller.state.error).toBe("keyring write failed");
      expect(api.searchSettings).toHaveBeenCalledOnce();
    },
  );

  it("refreshes both providers after acknowledging a successful key mutation", async () => {
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
    expect(controller.state.settings).toEqual(current);
    expect(controller.state.provider).toBe("brave");
    expect(controller.state.drafts.ollamaKey).toBe("");
    expect(controller.state.notice).toBe("APIキーを保存しました。");
    expect(controller.state.error).toBe("");
  });
});

describe("comparison state and persistence boundaries", () => {
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
    ["success", "search"],
    ["failure", "search"],
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
      const oldOpen = controller.openModal({ kind: "image", itemId: 11 });
      controller.closeModal();
      const currentSettings = {
        braveConfigured: false,
        ollamaConfigured: true,
        defaultProvider: "ollama" as const,
      };
      api.searchSettings.mockResolvedValue(currentSettings);
      await controller.openModal({ kind: "image", itemId: 12 });
      let finishCurrent: (() => void) | undefined;
      let pending: Promise<void>;
      if (operation === "search") {
        api.searchImages.mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finishCurrent = () => resolve([]);
            }),
        );
        pending = controller.searchImages();
      } else {
        api.setLocalImage.mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finishCurrent = () => resolve(initial);
            }),
        );
        pending = controller.changeImage("local");
      }
      if (!finishOld || !finishCurrent) throw new Error("Both operations must have started");
      finishOld();
      await oldOpen;
      expect(controller.state.modal).toEqual({ kind: "image", itemId: 12 });
      expect(controller.state.settings).toEqual(currentSettings);
      expect(controller.state.provider).toBe("ollama");
      expect(controller.state.busy).toBe(true);
      expect(controller.state.error).toBe("");
      if (operation === "write") {
        controller.closeModal();
        expect(controller.state.modal).toEqual({ kind: "image", itemId: 12 });
      }
      finishCurrent();
      await pending;
      expect(controller.state.busy).toBe(false);
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
