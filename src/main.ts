import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { api } from "./lib/api.js";
import { AppController, answers } from "./lib/controller.js";
import type { AppState } from "./lib/controller.js";
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
  function render(): void {
    const focus = document.activeElement;
    const focusKey = focus instanceof HTMLElement ? focus.dataset.focus : undefined;
    const selection =
      focus instanceof HTMLInputElement || focus instanceof HTMLTextAreaElement
        ? ([focus.selectionStart, focus.selectionEnd] as const)
        : null;
    root.innerHTML = renderApp(controller.state, assetUrl);
    const dialog = root.querySelector("dialog");
    if (dialog instanceof HTMLDialogElement) {
      dialog.showModal();
      dialog.addEventListener("cancel", (event) => {
        event.preventDefault();
        controller.closeModal();
      });
    }
    for (const element of root.querySelectorAll<HTMLElement>("[data-focus]")) {
      if (element.dataset.focus !== focusKey || !focusKey) continue;
      element.focus();
      if (
        selection &&
        (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)
      ) {
        element.setSelectionRange(selection[0], selection[1]);
      }
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
      render();
    }
  });
  root.addEventListener("submit", (event) => {
    event.preventDefault();
    const form = event.target;
    if (!(form instanceof HTMLFormElement)) return;
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
        void controller.openModal({ kind: "create-list" });
        break;
      case "rename-list":
        void controller.openModal({ kind: "rename-list" });
        break;
      case "delete-list":
        void controller.openModal({ kind: "delete-list" });
        break;
      case "rename-item":
        void controller.openModal({ kind: "rename-item", itemId: id });
        break;
      case "delete-item":
        void controller.openModal({ kind: "delete-item", itemId: id });
        break;
      case "image":
        void controller.openModal({ kind: "image", itemId: id });
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
