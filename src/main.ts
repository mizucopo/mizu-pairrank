import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { api } from "./lib/api.js";
import { AppController, answers } from "./lib/controller.js";
import type { AppState, Modal } from "./lib/controller.js";
import type { AppApi, SearchProvider } from "./lib/types.js";
import { renderApp } from "./lib/view.js";

function draftField(value: string | undefined): value is keyof AppState["drafts"] {
  return (
    value === "name" ||
    value === "items" ||
    value === "query" ||
    value === "braveKey" ||
    value === "ollamaKey"
  );
}
export function mountApp(
  root: HTMLElement,
  backend: AppApi,
  assetUrl = convertFileSrc,
): AppController {
  root.tabIndex = -1;
  const controller = new AppController(backend, render);
  type ButtonFocus = {
    key: string;
    listId: number | null;
    modal: Modal;
    index: number;
  };
  let modalOpener: ButtonFocus | null = null;
  let pendingButtonFocus: ButtonFocus | null = null;
  let pendingFieldFocus: {
    key: string;
    listId: number | null;
    dialog: boolean;
    selection: readonly [number | null, number | null] | null;
  } | null = null;
  let renderedListId: number | null = null;
  let renderedModal: Modal = null;
  let dialogWasOpen = false;
  let restoreModalFocus = false;
  function buttonKey(button: HTMLButtonElement): string {
    const { action, view, answer, id, provider } = button.dataset;
    return JSON.stringify([
      action,
      view,
      answer,
      id,
      provider,
      button.form?.dataset.form,
      button.form?.dataset.provider,
    ]);
  }
  function matchingButtons(key: string): HTMLButtonElement[] {
    return Array.from(root.querySelectorAll<HTMLButtonElement>("button")).filter(
      (button) => buttonKey(button) === key,
    );
  }
  function rememberButton(button: HTMLButtonElement): ButtonFocus {
    const key = buttonKey(button);
    const isGlobal =
      button.closest(".sidebar") ||
      button.dataset.action === "create-list" ||
      button.form?.dataset.form === "save-key";
    return {
      key,
      listId: isGlobal ? null : renderedListId,
      modal: button.closest("dialog") ? renderedModal : null,
      index: matchingButtons(key).indexOf(button),
    };
  }
  function restoreButton(target: ButtonFocus | null): void {
    let button: HTMLButtonElement | undefined;
    if (
      target &&
      target.modal === controller.state.modal &&
      (target.listId === null || target.listId === controller.state.active?.id)
    ) {
      button = matchingButtons(target.key)[target.index];
    }
    if (button && !button.disabled) button.focus({ preventScroll: true });
    else {
      const fallback =
        root.querySelector<HTMLElement>('dialog [data-action="close-modal"]') ?? root;
      fallback.focus({ preventScroll: true });
    }
  }
  async function openModal(modal: Exclude<Modal, null>, button: HTMLButtonElement): Promise<void> {
    if (controller.state.busy && controller.state.readPending !== "settings") return;
    modalOpener = rememberButton(button);
    restoreModalFocus = false;
    await controller.openModal(modal);
  }
  function render(): void {
    const focus = document.activeElement;
    if (!pendingButtonFocus && focus instanceof HTMLButtonElement && root.contains(focus)) {
      pendingButtonFocus = rememberButton(focus);
      pendingFieldFocus = null;
    }
    const focusKey = focus instanceof HTMLElement ? focus.dataset.focus : undefined;
    if (!pendingButtonFocus && focusKey && root.contains(focus)) {
      pendingFieldFocus = {
        key: focusKey,
        listId: focusKey === "braveKey" || focusKey === "ollamaKey" ? null : renderedListId,
        dialog: Boolean(focus?.closest("dialog")),
        selection:
          focus instanceof HTMLInputElement || focus instanceof HTMLTextAreaElement
            ? [focus.selectionStart, focus.selectionEnd]
            : null,
      };
    }
    root.innerHTML = renderApp(controller.state, (path) =>
      assetUrl(
        path,
        /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.png$/.test(path)
          ? "pairrank-image"
          : "asset",
      ),
    );
    renderedListId = controller.state.active?.id ?? null;
    renderedModal = controller.state.modal;
    const dialog = root.querySelector("dialog");
    if (dialogWasOpen && !dialog) restoreModalFocus = true;
    dialogWasOpen = Boolean(dialog);
    if (dialog instanceof HTMLDialogElement) {
      if (pendingButtonFocus?.modal !== renderedModal) pendingButtonFocus = null;
      dialog.showModal();
      dialog.addEventListener("cancel", (event) => {
        event.preventDefault();
        controller.closeModal();
      });
    }
    if (!controller.state.busy && !restoreModalFocus && pendingFieldFocus) {
      const field = pendingFieldFocus;
      if (
        field.dialog === Boolean(dialog) &&
        (field.listId === null || field.listId === controller.state.active?.id)
      ) {
        const element = [...root.querySelectorAll<HTMLElement>("[data-focus]")].find(
          (entry) => entry.dataset.focus === field.key,
        );
        element?.focus({ preventScroll: true });
        if (
          field.selection &&
          (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)
        ) {
          element.setSelectionRange(field.selection[0], field.selection[1]);
        }
      }
    }
    if (!controller.state.busy) pendingFieldFocus = null;
    if (!controller.state.busy) {
      if (!dialog && restoreModalFocus) {
        restoreButton(modalOpener);
        modalOpener = null;
        restoreModalFocus = false;
      } else if (pendingButtonFocus) {
        restoreButton(pendingButtonFocus);
      }
      pendingButtonFocus = null;
    }
    // Rendering replaces the clicked button. Keep shortcut events inside the app.
    if (!dialog && !root.contains(document.activeElement)) {
      root.focus({ preventScroll: true });
    }
  }
  root.addEventListener("input", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)) return;
    const field = target.dataset.draft;
    if (draftField(field)) controller.state.drafts[field] = target.value;
  });
  root.addEventListener("change", (event) => {
    const target = event.target;
    if (target instanceof HTMLSelectElement && target.id === "search-provider") {
      controller.state.provider = target.value === "ollama" ? "ollama" : "brave";
      controller.state.candidates = [];
      controller.state.searched = false;
      controller.state.error = "";
      render();
    }
  });
  root.addEventListener("submit", (event) => {
    event.preventDefault();
    const form = event.target;
    if (!(form instanceof HTMLFormElement)) return;
    if (document.activeElement instanceof HTMLElement && document.activeElement.dataset.focus) {
      pendingButtonFocus = null;
    } else if (event instanceof SubmitEvent && event.submitter instanceof HTMLButtonElement) {
      pendingButtonFocus = rememberButton(event.submitter);
      pendingFieldFocus = null;
    }
    switch (form.dataset.form) {
      case "add-items":
        void controller.addItems();
        break;
      case "save-name":
        void controller.saveName();
        break;
      case "search-images":
        void controller.searchImages();
        break;
      case "save-key":
        void controller.saveKey(form.dataset.provider === "ollama" ? "ollama" : "brave");
        break;
    }
  });
  root.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const button = target.closest("button");
    if (!(button instanceof HTMLButtonElement) || button.disabled) return;
    if (!(button.form && button.type === "submit")) {
      pendingButtonFocus = rememberButton(button);
      pendingFieldFocus = null;
    }
    const answer = answers.find((entry) => entry.value === button.dataset.answer);
    if (answer) {
      void controller.answer(answer.value);
      return;
    }
    const view = button.dataset.view;
    if (view === "items" || view === "compare" || view === "ranking" || view === "settings") {
      void controller.navigate(view);
      return;
    }
    const id = Number(button.dataset.id);
    const provider: SearchProvider = button.dataset.provider === "ollama" ? "ollama" : "brave";
    switch (button.dataset.action) {
      case "select-list":
        void controller.selectList(id);
        break;
      case "create-list":
        void openModal({ kind: "create-list" }, button);
        break;
      case "rename-list":
        void openModal({ kind: "rename-list" }, button);
        break;
      case "delete-list":
        void openModal({ kind: "delete-list" }, button);
        break;
      case "rename-item":
        void openModal({ kind: "rename-item", itemId: id }, button);
        break;
      case "delete-item":
        void openModal({ kind: "delete-item", itemId: id }, button);
        break;
      case "image":
        void openModal({ kind: "image", itemId: id }, button);
        break;
      case "close-modal":
        controller.closeModal();
        break;
      case "confirm-delete":
        void controller.confirmDelete();
        break;
      case "local-image":
        void controller.changeImage("local");
        break;
      case "no-image":
        void controller.changeImage("none");
        break;
      case "choose-image": {
        const candidate = controller.state.candidates[Number(button.dataset.index)];
        if (candidate) void controller.changeImage(candidate);
        break;
      }
      case "remove-key":
        void controller.saveKey(provider, true);
        break;
    }
  });
  root.addEventListener("keydown", (event) => {
    if (event.repeat || event.altKey || event.ctrlKey || event.metaKey || controller.state.modal)
      return;
    const target = event.target;
    if (target instanceof Element && target.closest("input, textarea, select, [contenteditable]"))
      return;
    const answer = answers.find((entry) => entry.key === event.key);
    if (answer && controller.state.view === "compare") {
      event.preventDefault();
      void controller.answer(answer.value);
    }
  });
  root.addEventListener(
    "error",
    (event) => {
      const target = event.target;
      if (target instanceof HTMLImageElement) {
        target.hidden = true;
        const fallback = target.nextElementSibling;
        if (fallback instanceof HTMLElement && fallback.classList.contains("image-fallback"))
          fallback.hidden = false;
      }
    },
    true,
  );
  render();
  return controller;
}
const root = document.querySelector<HTMLElement>("#app");
if (root) {
  if (isTauri()) void mountApp(root, api).initialize();
  else
    root.innerHTML =
      '<main class="startup"><h1>pairrank</h1><p>デスクトップアプリで起動してください。</p><code>npm run tauri dev</code></main>';
}
