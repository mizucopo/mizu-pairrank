// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";

import { mountApp } from "../src/main.js";
import type { AppController } from "../src/lib/controller.js";
import type {
  AppApi,
  ImageCandidate,
  ListState,
  PairProposal,
  Preference,
} from "../src/lib/types.js";

function setup() {
  const first = {
    id: 10,
    listId: 1,
    name: "りんご",
    image: null,
    rating: { mu: 25, sigma: 8 },
    comparisonCount: 0,
  };
  const second = { ...first, id: 11, name: "みかん" };
  const state: ListState = {
    id: 1,
    name: "好きな果物",
    revision: 5,
    items: [first, second],
    comparisonCount: 0,
    convergence: {
      converged: false,
      maxSigma: 8,
      maxRankSpan: 0,
      observedAnswers: 0,
      requiredAnswers: 20,
    },
  };
  // The selected presentation deliberately reverses registration order.
  const pair: PairProposal = { listId: 1, revision: 5, a: second, b: first };
  const settings = {
    braveConfigured: false,
    ollamaConfigured: false,
    defaultProvider: "brave" as const,
  };
  const api = {
    listSummaries: vi
      .fn<AppApi["listSummaries"]>()
      .mockResolvedValue([
        { id: 1, name: state.name, itemCount: 2, comparisonCount: 0, converged: false },
      ]),
    getList: vi.fn<AppApi["getList"]>().mockResolvedValue(state),
    createList: vi.fn<AppApi["createList"]>().mockResolvedValue(state),
    renameList: vi.fn<AppApi["renameList"]>().mockResolvedValue(state),
    deleteList: vi.fn<AppApi["deleteList"]>().mockResolvedValue(undefined),
    addItems: vi.fn<AppApi["addItems"]>().mockResolvedValue(state),
    renameItem: vi.fn<AppApi["renameItem"]>().mockResolvedValue(state),
    deleteItem: vi.fn<AppApi["deleteItem"]>().mockResolvedValue(state),
    resumeList: vi.fn<AppApi["resumeList"]>().mockResolvedValue(state),
    nextPair: vi.fn<AppApi["nextPair"]>().mockResolvedValue(pair),
    answer: vi
      .fn<AppApi["answer"]>()
      .mockResolvedValue({ ...state, revision: 6, comparisonCount: 1 }),
    searchSettings: vi.fn<AppApi["searchSettings"]>().mockResolvedValue(settings),
    setApiKey: vi.fn<AppApi["setApiKey"]>().mockResolvedValue(settings),
    searchImages: vi.fn<AppApi["searchImages"]>().mockResolvedValue([]),
    setLocalImage: vi.fn<AppApi["setLocalImage"]>().mockResolvedValue(state),
    setRemoteImage: vi.fn<AppApi["setRemoteImage"]>().mockResolvedValue(state),
    removeImage: vi.fn<AppApi["removeImage"]>().mockResolvedValue(state),
  } satisfies AppApi;
  const root = document.createElement("div");
  document.body.append(root);
  const controller = mountApp(root, api, (path) => `asset://${path}`);
  return { root, controller, api, state, pair };
}

function button(root: HTMLElement, selector: string): HTMLButtonElement {
  const element = root.querySelector(selector);
  if (!(element instanceof HTMLButtonElement)) throw new Error(`Missing button: ${selector}`);
  return element;
}

async function settle(controller: AppController): Promise<void> {
  await vi.waitFor(() => expect(controller.state.busy).toBe(false));
}

async function click(
  root: HTMLElement,
  controller: AppController,
  selector: string,
): Promise<void> {
  button(root, selector).click();
  await settle(controller);
}

afterEach(() => {
  document.body.replaceChildren();
});

describe("desktop app interaction", () => {
  const preferences: [string, Preference][] = [
    ["Aが大好き", "a_strong"],
    ["Aが好き", "a_weak"],
    ["同じ", "equal"],
    ["Bが好き", "b_weak"],
    ["Bが大好き", "b_strong"],
  ];

  it.each(preferences)(
    "saves %s against the actual displayed A and B",
    async (label, preference) => {
      const { root, controller, api, pair } = setup();
      await controller.initialize();
      await click(root, controller, '[data-view="compare"]');
      expect(root.querySelector(".side-a h3")?.textContent).toBe("みかん");
      expect(root.querySelector(".side-b h3")?.textContent).toBe("りんご");
      const answer = Array.from(root.querySelectorAll("button")).find((element) =>
        element.textContent?.startsWith(label),
      );
      if (!answer) throw new Error(`Missing answer: ${label}`);
      answer.click();
      await settle(controller);
      expect(api.answer).toHaveBeenCalledExactlyOnceWith(pair, preference);
    },
  );

  it("does not treat typing in the bulk input or pressing a key with a modal open as an answer", async () => {
    const { root, controller, api } = setup();
    await controller.initialize();
    const input = root.querySelector("textarea");
    if (!(input instanceof HTMLTextAreaElement)) throw new Error("Missing bulk input");
    input.value = " item one \r\n\nitem two";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "1", bubbles: true }));
    const form = root.querySelector('[data-form="add-items"]');
    if (!form) throw new Error("Missing add form");
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle(controller);
    expect(api.addItems).toHaveBeenCalledExactlyOnceWith(1, ["item one", "item two"]);
    await controller.startComparison();
    await controller.openModal({ kind: "rename-list" });
    root.dispatchEvent(new KeyboardEvent("keydown", { key: "5", bubbles: true }));
    expect(api.answer).not.toHaveBeenCalled();
    controller.closeModal();
    root.dispatchEvent(new KeyboardEvent("keydown", { key: "5", bubbles: true }));
    await settle(controller);
    expect(api.answer).toHaveBeenCalledExactlyOnceWith(expect.anything(), "b_strong");
  });

  it("keeps keyboard shortcuts working after navigation and answer buttons are replaced", async () => {
    const { root, controller, api, pair } = setup();
    const pressKeyAtFocus = (key: string) => {
      const focused = document.activeElement;
      if (!focused) throw new Error("No focused element receives keyboard input");
      focused.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
    };
    await controller.initialize();
    const navigation = button(root, '[data-view="compare"]');
    navigation.focus();
    navigation.click();
    await settle(controller);
    expect(navigation.isConnected).toBe(false);
    expect(root.contains(document.activeElement)).toBe(true);
    pressKeyAtFocus("1");
    await settle(controller);
    expect(api.answer).toHaveBeenNthCalledWith(1, pair, "a_strong");

    const answer = button(root, '[data-answer="equal"]');
    answer.focus();
    answer.click();
    await settle(controller);
    expect(answer.isConnected).toBe(false);
    expect(root.contains(document.activeElement)).toBe(true);
    pressKeyAtFocus("2");
    await settle(controller);
    expect(api.answer).toHaveBeenNthCalledWith(2, pair, "equal");
    expect(api.answer).toHaveBeenNthCalledWith(3, pair, "a_weak");

    await controller.openModal({ kind: "rename-list" });
    const input = root.querySelector("#name-input");
    if (!(input instanceof HTMLInputElement)) throw new Error("Missing modal input");
    input.focus();
    pressKeyAtFocus("5");
    button(root, '[data-action="close-modal"]').focus();
    pressKeyAtFocus("5");
    expect(api.answer).toHaveBeenCalledTimes(3);
  });

  it("requires a confirmation before deletion and cancellation preserves the item", async () => {
    const { root, controller, api, state } = setup();
    await controller.initialize();
    await click(root, controller, '[data-action="delete-item"][data-id="10"]');
    expect(root.querySelector("dialog")?.textContent).toContain("りんご");
    expect(api.deleteItem).not.toHaveBeenCalled();
    await click(root, controller, '[data-action="close-modal"]');
    expect(root.querySelector("dialog")).toBeNull();
    expect(controller.state.active).toEqual(state);
    api.deleteItem.mockResolvedValue({
      ...state,
      items: state.items.filter((item) => item.id !== 10),
    });
    await click(root, controller, '[data-action="delete-item"][data-id="10"]');
    await click(root, controller, '[data-action="confirm-delete"]');
    expect(api.deleteItem).toHaveBeenCalledExactlyOnceWith(1, 10);
    expect(root.querySelector("dialog")).toBeNull();
    expect(root.querySelectorAll(".item-row")).toHaveLength(1);
  });

  it("keeps a created list selectable when refreshing the sidebar fails", async () => {
    const { root, controller, api, state } = setup();
    const created: ListState = { ...state, id: 2, name: "新しいリスト", items: [] };
    await controller.initialize();
    await click(root, controller, '[data-action="create-list"]');
    controller.state.drafts.name = created.name;
    api.createList.mockResolvedValueOnce(created);
    api.listSummaries.mockRejectedValueOnce(new Error("一覧の更新に失敗しました"));
    await controller.saveName();
    expect(root.querySelector('[data-action="select-list"][data-id="2"]')?.textContent).toContain(
      "新しいリスト",
    );
    expect(root.querySelector('[role="alert"]')?.textContent).toBe("一覧の更新に失敗しました");
    await click(root, controller, '[data-action="select-list"][data-id="1"]');
    expect(root.querySelector("h1")?.textContent).toBe(state.name);
    api.getList.mockResolvedValueOnce(created);
    await click(root, controller, '[data-action="select-list"][data-id="2"]');
    expect(root.querySelector("h1")?.textContent).toBe(created.name);
    expect(root.querySelectorAll('[data-action="select-list"]')).toHaveLength(2);
  });

  it("allows local images and no image while search API keys are unconfigured", async () => {
    const { root, controller, api } = setup();
    await controller.initialize();
    await click(root, controller, '[data-action="image"][data-id="10"]');
    expect(button(root, '[data-form="search-images"] button[type="submit"]').disabled).toBe(true);
    expect(button(root, '[data-action="local-image"]').disabled).toBe(false);
    await click(root, controller, '[data-action="local-image"]');
    expect(api.setLocalImage).toHaveBeenCalledExactlyOnceWith(1, 10);
    await click(root, controller, '[data-action="image"][data-id="10"]');
    await click(root, controller, '[data-action="no-image"]');
    expect(api.removeImage).toHaveBeenCalledExactlyOnceWith(1, 10);
    expect(api.searchImages).not.toHaveBeenCalled();
    expect(api.setApiKey).not.toHaveBeenCalled();
  });

  it("preserves image choices when the file picker is cancelled and closes after a successful retry", async () => {
    const { root, controller, api, state } = setup();
    const candidate: ImageCandidate = {
      id: "fruit",
      title: "Fruit image",
      previewUrl: "data:image/png;base64,AA==",
      sourceUrl: "https://example.org/fruit",
    };
    api.searchSettings.mockResolvedValue({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    api.searchImages.mockResolvedValueOnce([candidate]);
    await controller.initialize();
    await click(root, controller, '[data-action="image"][data-id="10"]');
    controller.state.drafts.query = "red apple";
    await controller.searchImages();
    api.listSummaries.mockClear();
    api.setLocalImage.mockResolvedValueOnce(null);
    await click(root, controller, '[data-action="local-image"]');
    expect(root.querySelector("dialog")).not.toBeNull();
    const query = root.querySelector("#image-query");
    if (!(query instanceof HTMLInputElement)) throw new Error("Missing image query");
    expect(query.value).toBe("red apple");
    expect(root.querySelectorAll(".image-result")).toHaveLength(1);
    expect(controller.state.candidates).toEqual([candidate]);
    expect(controller.state.searched).toBe(true);
    expect(controller.state.active).toEqual(state);
    expect(root.querySelector('[role="alert"]')).toBeNull();
    expect(api.listSummaries).not.toHaveBeenCalled();

    const saved: ListState = {
      ...state,
      items: state.items.map((item) => ({
        ...item,
        image: { path: "/images/picked.png", sourceUrl: null },
      })),
    };
    api.setLocalImage.mockResolvedValueOnce(saved);
    await click(root, controller, '[data-action="local-image"]');
    expect(root.querySelector("dialog")).toBeNull();
    expect(controller.state.active).toEqual(saved);
    expect(api.listSummaries).toHaveBeenCalledOnce();
  });

  it("clears old candidates when switching provider and preserves the item after search failure", async () => {
    const { root, controller, api, state } = setup();
    api.searchSettings.mockResolvedValue({
      braveConfigured: true,
      ollamaConfigured: true,
      defaultProvider: "brave",
    });
    const candidate: ImageCandidate = {
      id: "candidate-1",
      title: "Fruit image",
      previewUrl: "data:image/png;base64,AA==",
      sourceUrl: "https://example.org/fruit",
    };
    api.searchImages
      .mockResolvedValueOnce([candidate])
      .mockRejectedValueOnce(new Error("検索サービスに接続できません"));
    await controller.initialize();
    await click(root, controller, '[data-action="image"][data-id="10"]');
    const search = () => {
      const form = root.querySelector('[data-form="search-images"]');
      if (!form) throw new Error("Missing image search");
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    };
    search();
    await settle(controller);
    expect(api.searchImages).toHaveBeenNthCalledWith(1, "brave", "りんご");
    expect(root.querySelectorAll(".image-result")).toHaveLength(1);
    const provider = root.querySelector("#search-provider");
    if (!(provider instanceof HTMLSelectElement)) throw new Error("Missing provider");
    provider.value = "ollama";
    provider.dispatchEvent(new Event("change", { bubbles: true }));
    expect(root.querySelectorAll(".image-result")).toHaveLength(0);
    search();
    await settle(controller);
    expect(api.searchImages).toHaveBeenNthCalledWith(2, "ollama", "りんご");
    expect(root.querySelector('[role="alert"]')?.textContent).toContain("接続できません");
    expect(controller.state.active).toEqual(state);
    expect(api.setRemoteImage).not.toHaveBeenCalled();
  });

  it.each(["close button", "Escape"] as const)(
    "dismisses a pending image search with %s and ignores its late failure",
    async (dismissal) => {
      const { root, controller, api, state } = setup();
      api.searchSettings.mockResolvedValue({
        braveConfigured: true,
        ollamaConfigured: false,
        defaultProvider: "brave",
      });
      await controller.initialize();
      await click(root, controller, '[data-action="image"][data-id="10"]');
      let rejectSearch: ((error: Error) => void) | undefined;
      api.searchImages.mockImplementationOnce(
        () =>
          new Promise((_, reject) => {
            rejectSearch = reject;
          }),
      );
      const pending = controller.searchImages();
      expect(controller.state.busy).toBe(true);
      expect(button(root, '[data-action="local-image"]').disabled).toBe(true);
      if (dismissal === "close button") button(root, '[data-action="close-modal"]').click();
      else {
        const dialog = root.querySelector("dialog");
        if (!dialog) throw new Error("Missing image dialog");
        dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
      }
      expect(root.querySelector("dialog")).toBeNull();
      expect(controller.state.busy).toBe(false);
      await controller.navigate("ranking");
      if (!rejectSearch) throw new Error("The search was not started");
      rejectSearch(new Error("古い検索に失敗しました"));
      await pending;
      expect(controller.state.view).toBe("ranking");
      expect(controller.state.active).toEqual(state);
      expect(root.querySelector('[role="alert"]')).toBeNull();
      expect(controller.state.candidates).toEqual([]);
    },
  );

  it.each(["result", "error"] as const)(
    "keeps an image write protected when a dismissed search returns a late %s",
    async (outcome) => {
      const { root, controller, api, state } = setup();
      await controller.initialize();
      await click(root, controller, '[data-action="image"][data-id="10"]');
      let finishSearch: (() => void) | undefined;
      api.searchImages.mockImplementationOnce(
        () =>
          new Promise((resolve, reject) => {
            finishSearch = () => {
              if (outcome === "result") resolve([]);
              else reject(new Error("古い検索に失敗しました"));
            };
          }),
      );
      const search = controller.searchImages();
      button(root, '[data-action="close-modal"]').click();
      await click(root, controller, '[data-action="image"][data-id="10"]');
      let finishWrite: (() => void) | undefined;
      const saved = {
        ...state,
        items: state.items.map((item) => ({
          ...item,
          image: { path: "/images/new.png", sourceUrl: null },
        })),
      };
      api.setLocalImage.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishWrite = () => resolve(saved);
          }),
      );
      const write = controller.changeImage("local");
      if (!finishSearch || !finishWrite) throw new Error("The search and write must have started");
      finishSearch();
      await search;
      expect(controller.state.busy).toBe(true);
      expect(button(root, '[data-action="close-modal"]').disabled).toBe(true);
      button(root, '[data-action="close-modal"]').click();
      const dialog = root.querySelector("dialog");
      if (!dialog) throw new Error("The writing modal was closed");
      dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
      expect(root.querySelector("dialog")).not.toBeNull();
      expect(root.querySelector('[role="alert"]')).toBeNull();
      await controller.changeImage("none");
      expect(api.removeImage).not.toHaveBeenCalled();
      finishWrite();
      await write;
      expect(controller.state.active).toEqual(saved);
      expect(controller.state.busy).toBe(false);
      expect(root.querySelector("dialog")).toBeNull();
    },
  );

  it("shows migration failure without exposing normal actions or resetting data", async () => {
    const { root, controller, api } = setup();
    api.listSummaries.mockRejectedValue(
      new Error("データベースの更新（バージョン2）に失敗しました"),
    );
    await controller.initialize();
    expect(root.textContent).toContain("データを開けませんでした");
    expect(root.querySelector('[role="alert"]')?.textContent).toContain("バージョン2");
    expect(root.querySelector("button")).toBeNull();
    expect(api.getList).not.toHaveBeenCalled();
    expect(api.createList).not.toHaveBeenCalled();
    expect(api.deleteList).not.toHaveBeenCalled();
  });

  it("renders names, input values, and backend error text literally without injecting markup", async () => {
    const { root, controller, api, state } = setup();
    const text = `<img src=x onerror="alert(1)"><script>alert(2)</script>&"'`;
    state.name = text;
    for (const item of state.items) item.name = text;
    api.listSummaries.mockResolvedValue([
      { id: 1, name: text, itemCount: 2, comparisonCount: 0, converged: false },
    ]);
    await controller.initialize();
    expect(root.querySelector("h1")?.textContent).toBe(text);
    expect(root.querySelector(".item-row h3")?.textContent).toBe(text);
    expect(root.querySelector("script, img, [onerror]")).toBeNull();
    await controller.openModal({ kind: "rename-list" });
    const input = root.querySelector("#name-input");
    if (!(input instanceof HTMLInputElement)) throw new Error("Missing name field");
    expect(input.value).toBe(text);
    api.renameList.mockRejectedValue(new Error(text));
    const form = root.querySelector('[data-form="save-name"]');
    if (!form) throw new Error("Missing name form");
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle(controller);
    expect(root.querySelector('[role="alert"]')?.textContent).toBe(text);
    expect(root.querySelector("script, img, [onerror]")).toBeNull();
  });
});
