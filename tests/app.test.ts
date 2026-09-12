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

function setup(
  assetUrl: (path: string, protocol?: string) => string = (path) => `asset://${path}`,
) {
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
  const nextPair = vi.fn<AppApi["nextPair"]>().mockResolvedValue(pair);
  let comparisonCount = state.comparisonCount;
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
    nextPair,
    answer: vi.fn<AppApi["answer"]>().mockImplementation(async () => {
      comparisonCount += 1;
      const revision = state.revision + comparisonCount;
      nextPair.mockResolvedValue({ ...pair, revision });
      return { ...state, revision, comparisonCount };
    }),
    searchSettings: vi.fn<AppApi["searchSettings"]>().mockResolvedValue(settings),
    setApiKey: vi.fn<AppApi["setApiKey"]>().mockResolvedValue(undefined),
    searchImages: vi.fn<AppApi["searchImages"]>().mockResolvedValue([]),
    setLocalImage: vi.fn<AppApi["setLocalImage"]>().mockResolvedValue(state),
    setRemoteImage: vi.fn<AppApi["setRemoteImage"]>().mockResolvedValue(state),
    removeImage: vi.fn<AppApi["removeImage"]>().mockResolvedValue(state),
  } satisfies AppApi;
  const root = document.createElement("div");
  document.body.append(root);
  const controller = mountApp(root, api, assetUrl);
  return { root, controller, api, state, pair };
}

function button(root: HTMLElement, selector: string): HTMLButtonElement {
  const element = root.querySelector(selector);
  if (!(element instanceof HTMLButtonElement)) throw new Error(`Missing button: ${selector}`);
  return element;
}

function withLocalImage(state: ListState): ListState {
  return {
    ...state,
    items: state.items.map((item, index) =>
      index === 0 ? { ...item, image: { path: "/images/picked.png", sourceUrl: null } } : item,
    ),
  };
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
  it("loads managed image references through their protocol and preserves legacy absolute images", async () => {
    const { root, controller, api, state } = setup(
      (path, protocol) => `${protocol}://localhost/${encodeURIComponent(path)}`,
    );
    const references = ["c21901c2-7226-43e9-8548-c17f286c17e2.png", "/legacy/images/old.png"];
    api.getList.mockResolvedValueOnce({
      ...state,
      items: state.items.map((item, index) => ({
        ...item,
        image: { path: references[index]!, sourceUrl: null },
      })),
    });
    await controller.initialize();
    const urls = [...root.querySelectorAll<HTMLImageElement>(".item-image")].map((image) =>
      image.getAttribute("src"),
    );
    expect(urls).toEqual([
      "pairrank-image://localhost/c21901c2-7226-43e9-8548-c17f286c17e2.png",
      "asset://localhost/%2Flegacy%2Fimages%2Fold.png",
    ]);
  });

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

  it("preserves item drafts when returning from settings to the same list", async () => {
    const { root, controller } = setup();
    await controller.initialize();
    const input = root.querySelector("textarea");
    if (!(input instanceof HTMLTextAreaElement)) throw new Error("Missing bulk input");
    input.value = "あとで登録する果物\nぶどう";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await click(root, controller, '[data-view="settings"]');
    await click(root, controller, '[data-action="select-list"][data-id="1"]');
    const restored = root.querySelector("textarea");
    if (!(restored instanceof HTMLTextAreaElement)) throw new Error("Missing restored bulk input");
    expect(restored.value).toBe("あとで登録する果物\nぶどう");
  });

  it.each(["existing list", "new list"] as const)(
    "keeps navigation to the %s available while reading credentials on the settings screen",
    async (destination) => {
      const { root, controller, api } = setup();
      if (destination === "new list") api.listSummaries.mockResolvedValueOnce([]);
      await controller.initialize();
      let finish: () => void = () => {};
      api.searchSettings.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = () =>
              resolve({
                braveConfigured: false,
                ollamaConfigured: true,
                defaultProvider: "ollama",
              });
          }),
      );
      const reading = controller.navigate("settings");
      const navigation =
        destination === "existing list"
          ? '[data-action="select-list"][data-id="1"]'
          : '[data-action="create-list"]';
      expect(button(root, navigation).disabled).toBe(false);
      expect(button(root, '[data-form="save-key"] button[type="submit"]').disabled).toBe(true);
      await click(root, controller, navigation);
      expect(controller.state.busy).toBe(false);
      finish();
      await reading;
      expect(controller.state.settings).toBeNull();
      if (destination === "existing list") {
        expect(controller.state.view).toBe("items");
        expect(root.querySelector("h1")?.textContent).toBe("好きな果物");
      } else {
        expect(controller.state.modal).toEqual({ kind: "create-list" });
        expect(root.querySelector("dialog")).not.toBeNull();
      }
    },
  );

  it.each([
    ["pending", "button"],
    ["failed", "Escape"],
  ] as const)(
    "recovers %s settings after cancelling list creation with %s and restores focus and drafts",
    async (initial, dismissal) => {
      const { root, controller, api } = setup();
      await controller.initialize();
      controller.state.drafts.braveKey = "unsaved key";
      let finishOld: () => void = () => {};
      if (initial === "pending") {
        api.searchSettings.mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finishOld = () =>
                resolve({
                  braveConfigured: false,
                  ollamaConfigured: false,
                  defaultProvider: "brave",
                });
            }),
        );
      } else api.searchSettings.mockRejectedValueOnce(new Error("initial settings read failed"));
      const firstRead = controller.navigate("settings");
      if (initial === "failed") await firstRead;
      const opener = '[data-action="create-list"]';
      await click(root, controller, opener);
      let finishCurrent: () => void = () => {};
      api.searchSettings.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishCurrent = () =>
              resolve({
                braveConfigured: true,
                ollamaConfigured: false,
                defaultProvider: "brave",
              });
          }),
      );
      if (dismissal === "button") button(root, '[data-action="close-modal"]').click();
      else {
        const dialog = root.querySelector("dialog");
        if (!dialog) throw new Error("Missing create-list dialog");
        dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
      }
      expect(root.querySelector("dialog")).toBeNull();
      expect(controller.state.readPending).toBe("settings");
      expect(button(root, '[data-action="select-list"][data-id="1"]').disabled).toBe(false);
      finishCurrent();
      await settle(controller);
      finishOld();
      await firstRead;
      expect(root.querySelector(".settings-card .badge")?.textContent).toBe("設定済み");
      expect(document.activeElement).toBe(button(root, opener));
      const key = root.querySelector("#braveKey");
      if (!(key instanceof HTMLInputElement)) throw new Error("Missing API key input");
      expect(key.value).toBe("unsaved key");
      expect(root.querySelector('[role="alert"]')).toBeNull();
    },
  );

  it("allows leaving the settings reload after modal cancellation without losing navigation focus or drafts", async () => {
    const { root, controller, api } = setup();
    await controller.initialize();
    controller.state.drafts.items = "unsaved item";
    api.searchSettings.mockRejectedValueOnce(new Error("initial settings read failed"));
    await controller.navigate("settings");
    await click(root, controller, '[data-action="create-list"]');
    let failRead: () => void = () => {};
    api.searchSettings.mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          failRead = () => reject(new Error("abandoned reload failed"));
        }),
    );
    button(root, '[data-action="close-modal"]').click();
    expect(controller.state.readPending).toBe("settings");
    const navigation = '[data-action="select-list"][data-id="1"]';
    await click(root, controller, navigation);
    failRead();
    await Promise.resolve();
    expect(controller.state.view).toBe("items");
    expect(controller.state.busy).toBe(false);
    expect(root.querySelector("dialog")).toBeNull();
    expect(root.querySelector('[role="alert"]')).toBeNull();
    expect(document.activeElement).toBe(button(root, navigation));
    const input = root.querySelector("textarea");
    if (!(input instanceof HTMLTextAreaElement)) throw new Error("Missing bulk input");
    expect(input.value).toBe("unsaved item");
  });

  it("protects a credential write and enables navigation during its post-write refresh", async () => {
    const { root, controller, api } = setup();
    await controller.initialize();
    await controller.navigate("settings");
    controller.state.drafts.braveKey = "saved-key";
    let finishWrite: () => void = () => {};
    api.setApiKey.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishWrite = resolve;
        }),
    );
    let failRead: (error: Error) => void = () => {};
    api.searchSettings.mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          failRead = reject;
        }),
    );
    const saving = controller.saveKey("brave");
    const navigation = '[data-action="select-list"][data-id="1"]';
    expect(button(root, navigation).disabled).toBe(true);
    await controller.navigate("items");
    expect(controller.state.view).toBe("settings");
    finishWrite();
    await vi.waitFor(() => {
      expect(root.textContent).toContain("APIキーを保存しました。");
      expect(button(root, navigation).disabled).toBe(false);
      expect(button(root, '[data-form="save-key"] button[type="submit"]').disabled).toBe(true);
    });
    await click(root, controller, navigation);
    expect(controller.state.view).toBe("items");
    failRead(new Error("old post-write read failed"));
    await saving;
    expect(root.querySelector("h1")?.textContent).toBe("好きな果物");
    expect(root.querySelector('[role="alert"]')).toBeNull();
    expect(controller.state.settings?.braveConfigured).toBe(true);
    expect(controller.state.busy).toBe(false);
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
    expect(document.activeElement).toBe(button(root, '.tabs [data-view="compare"]'));
    pressKeyAtFocus("1");
    await settle(controller);
    expect(api.answer).toHaveBeenNthCalledWith(1, pair, "a_strong");

    const answer = button(root, '[data-answer="equal"]');
    answer.focus();
    answer.click();
    await settle(controller);
    expect(answer.isConnected).toBe(false);
    expect(document.activeElement).toBe(button(root, '[data-answer="equal"]'));
    pressKeyAtFocus("2");
    await settle(controller);
    expect(api.answer).toHaveBeenNthCalledWith(2, { ...pair, revision: 6 }, "equal");
    expect(api.answer).toHaveBeenNthCalledWith(3, { ...pair, revision: 7 }, "a_weak");

    await controller.openModal({ kind: "rename-list" });
    const input = root.querySelector("#name-input");
    if (!(input instanceof HTMLInputElement)) throw new Error("Missing modal input");
    input.focus();
    pressKeyAtFocus("5");
    button(root, '[data-action="close-modal"]').focus();
    pressKeyAtFocus("5");
    expect(api.answer).toHaveBeenCalledTimes(3);
  });

  it("keeps focus on the clicked view button when another button still has native focus", async () => {
    const { root, controller } = setup();
    await controller.initialize();
    button(root, '.tabs [data-view="items"]').focus();
    await click(root, controller, '.tabs [data-view="ranking"]');
    expect(document.activeElement).toBe(button(root, '.tabs [data-view="ranking"]'));
  });

  it.each(["items", "credentials"] as const)(
    "preserves %s input focus and selection through a retry after an invalid form submission",
    async (formKind) => {
      const { root, controller, api } = setup();
      await controller.initialize();
      if (formKind === "credentials") await controller.navigate("settings");
      const selector = formKind === "items" ? "#item-names" : "#braveKey";
      const input = root.querySelector(selector);
      if (!(input instanceof HTMLInputElement || input instanceof HTMLTextAreaElement)) {
        throw new Error("Missing form input");
      }
      const form = input.form;
      const submit = form?.querySelector("button[type=submit]");
      if (!(submit instanceof HTMLButtonElement)) throw new Error("Missing form submit button");
      submit.focus();
      submit.click();
      expect(api.addItems).not.toHaveBeenCalled();
      expect(api.setApiKey).not.toHaveBeenCalled();
      input.focus();
      input.value = "編集して再試行";
      input.setSelectionRange(2, 5);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      let fail: (reason: Error) => void = () => {};
      const pending = new Promise<never>((_resolve, reject) => {
        fail = reject;
      });
      if (formKind === "items") api.addItems.mockReturnValueOnce(pending);
      else api.setApiKey.mockReturnValueOnce(pending);
      form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      expect(controller.state.busy).toBe(true);
      fail(new Error("保存に失敗しました"));
      await settle(controller);
      const restored = root.querySelector(selector);
      if (!(restored instanceof HTMLInputElement || restored instanceof HTMLTextAreaElement)) {
        throw new Error("Missing restored input");
      }
      expect(document.activeElement).toBe(restored);
      expect(restored.value).toBe("編集して再試行");
      expect([restored.selectionStart, restored.selectionEnd]).toEqual([2, 5]);
    },
  );

  it.each(["input", "button"] as const)(
    "does not transfer form %s focus to a different list after concurrent deletion",
    async (origin) => {
      const { root, controller, api, state } = setup();
      await controller.initialize();
      const input = root.querySelector("textarea");
      if (!(input instanceof HTMLTextAreaElement)) throw new Error("Missing item input");
      input.value = "新しい項目";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      const second = { ...state, id: 2, name: "別のリスト" };
      api.listSummaries.mockResolvedValue([
        { id: 2, name: second.name, itemCount: 2, comparisonCount: 0, converged: false },
      ]);
      api.getList.mockResolvedValueOnce(second);
      if (origin === "input") {
        input.focus();
        input.form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      } else {
        const submit = button(root, '[data-form="add-items"] button[type="submit"]');
        submit.focus();
        submit.click();
      }
      await settle(controller);
      expect(controller.state.active?.id).toBe(2);
      expect(document.activeElement).toBe(root);
    },
  );

  it.each(["sidebar", "settings", "form", "answer"] as const)(
    "restores the %s button after an asynchronous operation",
    async (operation) => {
      const { root, controller, api, state, pair } = setup();
      const second = { ...state, id: 2, name: "別のリスト" };
      api.listSummaries.mockResolvedValue([
        { id: 1, name: state.name, itemCount: 2, comparisonCount: 0, converged: false },
        { id: 2, name: second.name, itemCount: 2, comparisonCount: 0, converged: false },
      ]);
      await controller.initialize();
      let finish: () => void = () => {};
      const pending = new Promise<void>((resolve) => {
        finish = resolve;
      });
      let selector: string;
      if (operation === "sidebar") {
        selector = '[data-action="select-list"][data-id="2"]';
        api.getList.mockImplementationOnce(async () => {
          await pending;
          return second;
        });
      } else if (operation === "settings") {
        selector = '[data-view="settings"]';
        api.searchSettings.mockImplementationOnce(async () => {
          await pending;
          return { braveConfigured: false, ollamaConfigured: false, defaultProvider: "brave" };
        });
      } else if (operation === "form") {
        selector = '[data-form="add-items"] button[type="submit"]';
        const input = root.querySelector("textarea");
        if (!(input instanceof HTMLTextAreaElement)) throw new Error("Missing item input");
        input.value = "新しい項目";
        input.dispatchEvent(new Event("input", { bubbles: true }));
        api.addItems.mockImplementationOnce(async () => {
          await pending;
          return state;
        });
      } else {
        await controller.startComparison();
        selector = '[data-answer="b_weak"]';
        api.answer.mockImplementationOnce(async () => {
          await pending;
          return { ...state, revision: 6, comparisonCount: 1 };
        });
        api.nextPair.mockResolvedValueOnce({ ...pair, revision: 6 });
      }
      const before = button(root, selector);
      before.focus();
      before.click();
      expect(controller.state.busy).toBe(true);
      expect(before.isConnected).toBe(false);
      expect(button(root, selector).disabled).toBe(operation !== "settings");
      finish();
      await settle(controller);
      expect(document.activeElement).toBe(button(root, selector));
      expect(button(root, selector).disabled).toBe(false);
    },
  );

  it("uses a safe focus fallback when answering settles the list and removes the answer button", async () => {
    const { root, controller, api, state } = setup();
    await controller.initialize();
    await controller.startComparison();
    api.answer.mockResolvedValueOnce({
      ...state,
      revision: 6,
      convergence: { ...state.convergence, converged: true },
    });
    await click(root, controller, '[data-answer="equal"]');
    expect(controller.state.view).toBe("ranking");
    expect(root.querySelector("[data-answer]")).toBeNull();
    expect(document.activeElement).toBe(root);
  });

  it.each([
    ['[data-action="delete-item"][data-id="10"]', '[data-action="confirm-delete"]', "deleteItem"],
    ['[data-action="delete-list"]', '[data-action="confirm-delete"]', "deleteList"],
    ['[data-action="image"][data-id="10"]', '[data-action="no-image"]', "removeImage"],
    ['[data-action="image"][data-id="10"]', '[data-action="local-image"]', "setLocalImage"],
    [
      '[data-action="rename-item"][data-id="10"]',
      '[data-form="save-name"] button[type="submit"]',
      "renameItem",
    ],
    [
      '[data-action="image"][data-id="10"]',
      '[data-form="search-images"] button[type="submit"]',
      "searchImages",
    ],
  ] as const)("keeps focus in %s on %s when %s fails", async (opener, retrySelector, method) => {
    const { root, controller, api, state } = setup();
    if (method === "removeImage") api.getList.mockResolvedValueOnce(withLocalImage(state));
    api.searchSettings.mockResolvedValue({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    await controller.initialize();
    await click(root, controller, opener);
    const modal = controller.state.modal;
    api[method].mockRejectedValueOnce(new Error("操作に失敗しました"));
    const retry = button(root, retrySelector);
    retry.focus();
    retry.click();
    await settle(controller);
    expect(controller.state.modal).toEqual(modal);
    expect(root.querySelector('[role="alert"]')?.textContent).toBe("操作に失敗しました");
    expect(document.activeElement).toBe(button(root, retrySelector));
  });

  it("does not transfer an action's focus to a newly opened dialog for another item", async () => {
    const { root, controller } = setup();
    await controller.initialize();
    await click(root, controller, '[data-action="delete-item"][data-id="10"]');
    button(root, '[data-action="confirm-delete"]').focus();
    await controller.openModal({ kind: "delete-item", itemId: 11 });
    expect(root.querySelector("dialog")?.textContent).toContain("みかん");
    expect(document.activeElement).not.toBe(button(root, '[data-action="confirm-delete"]'));
  });

  it("shows whitespace-only name validation in the dialog and keeps the focused submit button", async () => {
    const { root, controller, api } = setup();
    await controller.initialize();
    await click(root, controller, '[data-action="create-list"]');
    const input = root.querySelector("#name-input");
    if (!(input instanceof HTMLInputElement)) throw new Error("Missing name input");
    input.value = "   ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    const selector = '[data-form="save-name"] button[type="submit"]';
    const submit = button(root, selector);
    submit.focus();
    submit.click();
    await settle(controller);
    expect(root.querySelector('dialog [role="alert"]')?.textContent).toBe(
      "名前を入力してください。",
    );
    expect(document.activeElement).toBe(button(root, selector));
    expect(api.createList).not.toHaveBeenCalled();
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

  it.each([
    ["create list", '[data-action="create-list"]', "save"],
    ["rename list", '[data-action="rename-list"]', "save"],
    ["rename item", '[data-action="rename-item"][data-id="10"]', "save"],
    ["image", '[data-action="image"][data-id="10"]', "image"],
    ["delete list", '[data-action="delete-list"]', "Escape"],
    ["delete item", '[data-action="delete-item"][data-id="10"]', "cancel"],
  ] as const)(
    "returns focus to the %s dialog's opener after it closes",
    async (_name, opener, completion) => {
      const { root, controller, api, state } = setup();
      if (completion === "image") {
        api.getList.mockResolvedValueOnce(withLocalImage(state));
        api.removeImage.mockResolvedValueOnce({ ...state, revision: state.revision + 1 });
      }
      await controller.initialize();
      await click(root, controller, opener);
      if (completion === "save") {
        controller.state.drafts.name = "保存する名前";
        await controller.saveName();
      } else if (completion === "image") {
        await click(root, controller, '[data-action="no-image"]');
      } else if (completion === "Escape") {
        const dialog = root.querySelector("dialog");
        if (!dialog) throw new Error("Missing deletion dialog");
        dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
      } else {
        await click(root, controller, '[data-action="close-modal"]');
      }
      expect(root.querySelector("dialog")).toBeNull();
      expect(document.activeElement).toBe(button(root, opener));
    },
  );

  it.each(["item", "list"] as const)(
    "uses a safe focus fallback when the originating %s is deleted",
    async (deleted) => {
      const { root, controller, api, state } = setup();
      await controller.initialize();
      if (deleted === "item") {
        await click(root, controller, '[data-action="delete-item"][data-id="10"]');
        api.deleteItem.mockResolvedValueOnce({
          ...state,
          items: state.items.filter((item) => item.id !== 10),
        });
      } else {
        await click(root, controller, '[data-action="delete-list"]');
        const remaining = { ...state, id: 2, name: "残りのリスト", items: [] };
        api.listSummaries.mockResolvedValueOnce([
          { id: 2, name: remaining.name, itemCount: 0, comparisonCount: 0, converged: false },
        ]);
        api.getList.mockResolvedValueOnce(remaining);
      }
      await click(root, controller, '[data-action="confirm-delete"]');
      expect(root.querySelector("dialog")).toBeNull();
      expect(document.activeElement).toBe(root);
    },
  );

  it.each(["cancel", "save"] as const)(
    "tracks the welcome create button separately from the sidebar when closing with %s",
    async (completion) => {
      const { root, controller, api } = setup();
      api.listSummaries.mockResolvedValueOnce([]);
      await controller.initialize();
      const selector = '.welcome [data-action="create-list"]';
      await click(root, controller, selector);
      if (completion === "save") {
        controller.state.drafts.name = "新しいリスト";
        await controller.saveName();
        expect(root.querySelector(selector)).toBeNull();
        expect(document.activeElement).toBe(root);
      } else {
        await click(root, controller, '[data-action="close-modal"]');
        expect(document.activeElement).toBe(button(root, selector));
      }
    },
  );

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

  it.each([
    ["brave", "keyring read failed"],
    ["ollama", "keyring read failed"],
    ["brave", ""],
  ] as const)(
    "keeps the healthy provider usable when the %s key status cannot be read",
    async (unreadable, readError) => {
      const { root, controller, api } = setup();
      const healthy = unreadable === "brave" ? "ollama" : "brave";
      api.searchSettings.mockResolvedValue({
        braveConfigured: healthy === "brave",
        ollamaConfigured: healthy === "ollama",
        defaultProvider: healthy,
        errors: { [unreadable]: readError },
      });
      await controller.initialize();
      await controller.navigate("settings");
      const cards = [...root.querySelectorAll(".settings-card")];
      expect(cards[unreadable === "brave" ? 0 : 1]?.textContent).toContain("確認できません");
      const errorMessage = readError || "資格情報ストアから読み取れませんでした。";
      expect(cards[unreadable === "brave" ? 0 : 1]?.textContent).toContain(errorMessage);
      expect(cards[healthy === "brave" ? 0 : 1]?.textContent).toContain("設定済み");
      await controller.navigate("items");
      await controller.openModal({ kind: "image", itemId: 10 });
      const provider = root.querySelector<HTMLSelectElement>("#search-provider");
      expect(provider?.value).toBe(healthy);
      expect(button(root, '[data-form="search-images"] button[type="submit"]').disabled).toBe(
        false,
      );
      if (!provider) throw new Error("Missing provider selector");
      provider.value = unreadable;
      provider.dispatchEvent(new Event("change", { bubbles: true }));
      expect(button(root, '[data-form="search-images"] button[type="submit"]').disabled).toBe(true);
      expect(root.querySelector("dialog")?.textContent).toContain(errorMessage);
    },
  );

  it.each(["brave", "ollama"] as const)(
    "allows image search with a saved %s key even when settings reads keep failing",
    async (provider) => {
      const { root, controller, api } = setup();
      api.searchSettings.mockRejectedValue(new Error("keyring read failed"));
      await controller.initialize();
      await click(root, controller, '[data-view="settings"]');
      const input = root.querySelector(`[data-draft="${provider}Key"]`);
      if (!(input instanceof HTMLInputElement)) throw new Error("Missing API key input");
      input.value = "saved-key";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      const form = input.form;
      if (!form) throw new Error("Missing API key form");
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await settle(controller);
      expect(
        root
          .querySelector(`[data-form="save-key"][data-provider="${provider}"]`)
          ?.closest(".settings-card")
          ?.querySelector(".badge")?.textContent,
      ).toBe("設定済み");
      expect(root.querySelector('[role="alert"]')?.textContent).toContain(
        "設定状態を再取得できませんでした",
      );

      await controller.navigate("items");
      await click(root, controller, '[data-action="image"][data-id="10"]');
      expect(button(root, '[data-form="search-images"] button[type="submit"]').disabled).toBe(
        false,
      );
      const search = root.querySelector('[data-form="search-images"]');
      if (!search) throw new Error("Missing image search form");
      search.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await settle(controller);
      expect(api.searchImages).toHaveBeenCalledExactlyOnceWith(provider, "りんご");

      const select = root.querySelector("#search-provider");
      if (!(select instanceof HTMLSelectElement)) throw new Error("Missing search provider");
      select.value = provider === "brave" ? "ollama" : "brave";
      select.dispatchEvent(new Event("change", { bubbles: true }));
      expect(button(root, '[data-form="search-images"] button[type="submit"]').disabled).toBe(true);
    },
  );

  it("shows whitespace API-key validation, retains the form, and permits a corrected save", async () => {
    const { root, controller, api } = setup();
    await controller.initialize();
    await controller.navigate("settings");
    const input = root.querySelector("#braveKey");
    if (!(input instanceof HTMLInputElement)) throw new Error("Missing API key input");
    input.value = "   ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    const selector = '[data-form="save-key"][data-provider="brave"] button[type="submit"]';
    const submit = button(root, selector);
    submit.focus();
    submit.click();
    await settle(controller);
    expect(root.querySelector('[role="alert"]')?.textContent).toBe(
      "APIキーの形式が正しくありません。",
    );
    expect(document.activeElement).toBe(button(root, selector));
    const restored = root.querySelector("#braveKey");
    if (!(restored instanceof HTMLInputElement)) throw new Error("Missing restored API key input");
    expect(restored.value).toBe("   ");
    expect(api.setApiKey).not.toHaveBeenCalled();
    restored.value = "saved-key";
    restored.dispatchEvent(new Event("input", { bubbles: true }));
    api.searchSettings.mockResolvedValueOnce({
      braveConfigured: true,
      ollamaConfigured: false,
      defaultProvider: "brave",
    });
    await click(root, controller, selector);
    expect(api.setApiKey).toHaveBeenCalledExactlyOnceWith("brave", "saved-key");
    expect(root.querySelector('[role="alert"]')).toBeNull();
    expect(controller.state.settings?.braveConfigured).toBe(true);
  });

  it("disables the settings button until the image dialog's credential read finishes", async () => {
    const { root, controller, api } = setup();
    await controller.initialize();
    let finish: () => void = () => {};
    api.searchSettings.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = () =>
            resolve({ braveConfigured: false, ollamaConfigured: false, defaultProvider: "brave" });
        }),
    );
    const reading = controller.openModal({ kind: "image", itemId: 10 });
    const selector = 'dialog [data-view="settings"]';
    expect(button(root, selector).disabled).toBe(true);
    button(root, selector).click();
    expect(controller.state.modal).toEqual({ kind: "image", itemId: 10 });
    finish();
    await reading;
    expect(button(root, selector).disabled).toBe(false);
    await click(root, controller, selector);
    expect(controller.state.view).toBe("settings");
    expect(root.querySelector("dialog")).toBeNull();
  });

  it("does not offer an image removal mutation when the item already has no image", async () => {
    const { root, controller, api, state } = setup();
    await controller.initialize();
    await click(root, controller, '[data-action="image"][data-id="10"]');
    const remove = button(root, '[data-action="no-image"]');
    expect(remove.disabled).toBe(true);
    remove.click();
    expect(api.removeImage).not.toHaveBeenCalled();
    expect(controller.state.active).toEqual(state);
    expect(controller.state.modal).toEqual({ kind: "image", itemId: 10 });
  });

  it("allows local images and no image while search API keys are unconfigured", async () => {
    const { root, controller, api, state } = setup();
    api.setLocalImage.mockResolvedValueOnce(
      withLocalImage({ ...state, revision: state.revision + 1 }),
    );
    api.removeImage.mockResolvedValueOnce({ ...state, revision: state.revision + 2 });
    await controller.initialize();
    await click(root, controller, '[data-action="image"][data-id="10"]');
    expect(button(root, '[data-form="search-images"] button[type="submit"]').disabled).toBe(true);
    expect(button(root, '[data-action="local-image"]').disabled).toBe(false);
    expect(button(root, '[data-action="no-image"]').disabled).toBe(true);
    await click(root, controller, '[data-action="local-image"]');
    expect(api.setLocalImage).toHaveBeenCalledExactlyOnceWith(1, 10);
    await click(root, controller, '[data-action="image"][data-id="10"]');
    expect(button(root, '[data-action="no-image"]').disabled).toBe(false);
    await click(root, controller, '[data-action="no-image"]');
    expect(api.removeImage).toHaveBeenCalledExactlyOnceWith(1, 10);
    expect(controller.state.active?.items[0]?.image).toBeNull();
    await click(root, controller, '[data-action="image"][data-id="10"]');
    expect(button(root, '[data-action="no-image"]').disabled).toBe(true);
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

  it.each(["input", "button"] as const)(
    "shows whitespace query validation while preserving %s focus and permits retry",
    async (origin) => {
      const { root, controller, api } = setup();
      api.searchSettings.mockResolvedValue({
        braveConfigured: true,
        ollamaConfigured: false,
        defaultProvider: "brave",
      });
      await controller.initialize();
      await click(root, controller, '[data-action="image"][data-id="10"]');
      const input = root.querySelector("#image-query");
      if (!(input instanceof HTMLInputElement)) throw new Error("Missing query input");
      input.value = "   ";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      const submitSelector = '[data-form="search-images"] button[type="submit"]';
      if (origin === "input") {
        input.focus();
        input.setSelectionRange(1, 2);
        input.form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      } else {
        const submit = button(root, submitSelector);
        submit.focus();
        submit.click();
      }
      await settle(controller);
      expect(root.querySelector('dialog [role="alert"]')?.textContent).toBe(
        "検索語は1〜400文字、50語以内で入力してください。",
      );
      expect(api.searchImages).not.toHaveBeenCalled();
      const restored = root.querySelector("#image-query");
      if (!(restored instanceof HTMLInputElement)) throw new Error("Missing restored query");
      expect(restored.value).toBe("   ");
      expect(document.activeElement).toBe(
        origin === "input" ? restored : button(root, submitSelector),
      );
      if (origin === "input")
        expect([restored.selectionStart, restored.selectionEnd]).toEqual([1, 2]);
      restored.value = "りんご";
      restored.dispatchEvent(new Event("input", { bubbles: true }));
      await click(root, controller, submitSelector);
      expect(api.searchImages).toHaveBeenCalledExactlyOnceWith("brave", "りんご");
      expect(root.querySelector('dialog [role="alert"]')).toBeNull();
    },
  );

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
    const retryProvider = root.querySelector("#search-provider");
    if (!(retryProvider instanceof HTMLSelectElement)) throw new Error("Missing provider");
    retryProvider.value = "brave";
    retryProvider.dispatchEvent(new Event("change", { bubbles: true }));
    expect(controller.state.error).toBe("");
    expect(root.querySelector('[role="alert"]')).toBeNull();
    expect(controller.state.searched).toBe(false);
    expect(button(root, '[data-form="search-images"] button[type="submit"]').disabled).toBe(false);
  });

  it.each([
    ["image search", "close button"],
    ["image search", "Escape"],
    ["settings lookup", "close button"],
    ["settings lookup", "Escape"],
  ] as const)(
    "dismisses a pending %s with %s and ignores its late failure",
    async (request, dismissal) => {
      const { root, controller, api, state } = setup();
      api.searchSettings.mockResolvedValue({
        braveConfigured: true,
        ollamaConfigured: false,
        defaultProvider: "brave",
      });
      await controller.initialize();
      let rejectRead: ((error: Error) => void) | undefined;
      const read = new Promise<never>((_, reject) => {
        rejectRead = reject;
      });
      let pending: Promise<void>;
      if (request === "settings lookup") {
        api.searchSettings.mockReturnValueOnce(read);
        pending = controller.openModal({ kind: "image", itemId: 10 });
      } else {
        await click(root, controller, '[data-action="image"][data-id="10"]');
        api.searchImages.mockReturnValueOnce(read);
        pending = controller.searchImages();
      }
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
      if (!rejectRead) throw new Error("The read was not started");
      rejectRead(new Error("古い読み込みに失敗しました"));
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
