import type {
  AppApi,
  ImageCandidate,
  ListState,
  ListSummary,
  PairProposal,
  Preference,
  SearchProvider,
  SearchSettings,
} from "./types.js";

export type View = "items" | "compare" | "ranking" | "settings";
type ReadKind = "image" | "settings";
export type Modal =
  | { kind: "create-list" }
  | { kind: "rename-list" }
  | { kind: "delete-list" }
  | { kind: "rename-item"; itemId: number }
  | { kind: "delete-item"; itemId: number }
  | { kind: "image"; itemId: number }
  | null;
export type AppState = {
  lists: ListSummary[];
  active: ListState | null;
  pair: PairProposal | null;
  view: View;
  busy: boolean;
  initialized: boolean;
  fatal: boolean;
  error: string;
  notice: string;
  modal: Modal;
  settings: SearchSettings | null;
  candidates: ImageCandidate[];
  searched: boolean;
  readPending: ReadKind | null;
  provider: SearchProvider;
  drafts: { name: string; items: string; query: string; braveKey: string; ollamaKey: string };
};

export const answers: { value: Preference; label: string; key: string }[] = [
  { value: "a_strong", label: "Aが大好き", key: "1" },
  { value: "a_weak", label: "Aが好き", key: "2" },
  { value: "equal", label: "同じ", key: "3" },
  { value: "b_weak", label: "Bが好き", key: "4" },
  { value: "b_strong", label: "Bが大好き", key: "5" },
];

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const comparisonRetryMessage = "リストが更新されています。もう一度比較を開始してください。";

export class AppController {
  readonly state: AppState = {
    lists: [],
    active: null,
    pair: null,
    view: "items",
    busy: false,
    initialized: false,
    fatal: false,
    error: "",
    notice: "",
    modal: null,
    settings: null,
    candidates: [],
    searched: false,
    readPending: null,
    provider: "brave",
    drafts: { name: "", items: "", query: "", braveKey: "", ollamaKey: "" },
  };

  private readRequest: symbol | null = null;

  constructor(
    private readonly api: AppApi,
    private readonly changed: () => void,
  ) {}

  private async perform(action: () => Promise<void>): Promise<void> {
    if (this.state.busy) return;
    this.state.busy = true;
    this.state.error = "";
    this.state.notice = "";
    this.changed();
    try {
      await action();
    } catch (error) {
      this.state.error = errorMessage(error);
    } finally {
      this.state.busy = false;
      this.changed();
    }
  }

  async initialize(): Promise<void> {
    await this.perform(async () => {
      try {
        this.state.active = await this.loadFirstAvailableList();
        this.state.view = this.state.active?.convergence.converged ? "ranking" : "items";
        this.state.initialized = true;
      } catch (error) {
        this.state.fatal = true;
        throw error;
      }
    });
  }

  private async loadFirstAvailableList(): Promise<ListState | null> {
    this.state.lists = await this.api.listSummaries();
    for (let attempt = 0; attempt < 3; attempt += 1) {
      const first = this.state.lists[0];
      if (!first) return null;
      try {
        return await this.api.getList(first.id);
      } catch (error) {
        // Another window may delete the list after the summaries were read.
        if (errorMessage(error) !== "リストが見つかりません。") throw error;
        this.state.lists = await this.api.listSummaries();
      }
    }
    if (this.state.lists.length > 0) {
      this.state.notice = "リストが変更されました。リストを選択するか、新しく作成してください。";
    }
    return null;
  }

  async selectList(id: number): Promise<void> {
    this.cancelRead("settings");
    await this.perform(async () => {
      const list = await this.api
        .getList(id)
        .catch((error: unknown) => this.handleListError(id, error));
      if (list.id !== this.state.active?.id) this.state.drafts.items = "";
      this.state.active = list;
      this.state.pair = null;
      this.state.modal = null;
      this.state.view = list.convergence.converged ? "ranking" : "items";
    });
  }

  async navigate(view: View): Promise<void> {
    if (view === "settings" && this.state.readPending === "settings") return;
    this.cancelRead("settings");
    if (this.state.busy) return;
    if (view === "compare") {
      await this.startComparison();
      return;
    }
    this.state.view = view;
    this.state.modal = null;
    this.state.error = "";
    if (view === "settings") {
      await this.performRead(
        "settings",
        () => this.api.searchSettings(),
        (settings) => this.acceptSettings(settings),
      );
    } else this.changed();
  }

  private acceptSettings(current: SearchSettings): void {
    const settings = { ...current };
    let hasErrors = false;
    for (const provider of ["brave", "ollama"] as const) {
      if (current.errors?.[provider] !== undefined) {
        hasErrors = true;
        const field = provider === "brave" ? "braveConfigured" : "ollamaConfigured";
        settings[field] = this.state.settings?.[field] ?? false;
      }
    }
    if (hasErrors) {
      if (settings.braveConfigured && settings.errors?.brave === undefined)
        settings.defaultProvider = "brave";
      else if (settings.ollamaConfigured && settings.errors?.ollama === undefined)
        settings.defaultProvider = "ollama";
      else
        settings.defaultProvider =
          settings.ollamaConfigured && !settings.braveConfigured ? "ollama" : "brave";
    }
    this.state.settings = settings;
    this.state.provider = settings.defaultProvider;
  }

  async openModal(modal: Exclude<Modal, null>): Promise<void> {
    this.cancelRead("settings");
    if (this.state.busy) return;
    this.state.modal = modal;
    this.state.error = "";
    const item =
      "itemId" in modal
        ? this.state.active?.items.find((entry) => entry.id === modal.itemId)
        : undefined;
    this.state.drafts.name =
      modal.kind === "rename-list" ? (this.state.active?.name ?? "") : (item?.name ?? "");
    this.state.drafts.query = item?.name ?? "";
    this.state.candidates = [];
    this.state.searched = false;
    if (modal.kind === "image") {
      await this.performRead(
        "image",
        () => this.api.searchSettings(),
        (settings) => this.acceptSettings(settings),
      );
    } else this.changed();
  }

  closeModal(): void {
    if (this.state.busy && this.state.readPending !== "image") return;
    this.cancelRead("image");
    this.state.modal = null;
    this.state.error = "";
    this.changed();
  }

  private acceptCommittedList(list: ListState): void {
    // Keep committed state usable even if a later sidebar refresh fails.
    this.state.active = list;
    this.state.pair = null;
    const summary: ListSummary = {
      id: list.id,
      name: list.name,
      itemCount: list.items.length,
      comparisonCount: list.comparisonCount,
      converged: list.convergence.converged,
    };
    this.state.lists = [...this.state.lists.filter((entry) => entry.id !== list.id), summary].sort(
      (a, b) => a.id - b.id,
    );
  }

  private async acceptList(list: ListState): Promise<void> {
    this.acceptCommittedList(list);
    await this.refreshListSummaries(list.id);
  }

  private async refreshListSummaries(listId: number): Promise<void> {
    this.state.lists = await this.api.listSummaries();
    if (!this.state.lists.some((entry) => entry.id === listId)) {
      await this.handleListError(listId, new Error("リストが見つかりません。"));
    }
  }

  async saveName(): Promise<void> {
    const modal = this.state.modal;
    const list = this.state.active;
    const name = this.state.drafts.name.trim();
    if (!modal || !name) return;
    await this.perform(async () => {
      let result: ListState;
      if (modal.kind === "create-list") {
        result = await this.api.createList(name);
        this.state.drafts.items = "";
      } else if (modal.kind === "rename-list" && list)
        result = await this.api
          .renameList(list.id, name)
          .catch((error: unknown) => this.handleListError(list.id, error));
      else if (modal.kind === "rename-item" && list)
        result = await this.api
          .renameItem(list.id, modal.itemId, name)
          .catch((error: unknown) => this.handleItemError(list.id, modal.itemId, error));
      else return;
      this.state.modal = null;
      this.state.view = "items";
      await this.acceptList(result);
    });
  }

  async confirmDelete(): Promise<void> {
    const list = this.state.active;
    const modal = this.state.modal;
    if (!list || !modal) return;
    await this.perform(async () => {
      if (modal.kind === "delete-list") {
        await this.api
          .deleteList(list.id)
          .catch((error: unknown) => this.handleListError(list.id, error));
        this.state.active = null;
        this.state.pair = null;
        this.state.modal = null;
        this.state.drafts.items = "";
        this.state.lists = this.state.lists.filter((entry) => entry.id !== list.id);
        this.state.active = await this.loadFirstAvailableList();
      } else if (modal.kind === "delete-item") {
        const result = await this.api
          .deleteItem(list.id, modal.itemId)
          .catch((error: unknown) => this.handleItemError(list.id, modal.itemId, error));
        this.state.modal = null;
        await this.acceptList(result);
      }
      this.state.view = "items";
    });
  }

  async addItems(): Promise<void> {
    const list = this.state.active;
    const names = this.state.drafts.items
      .split(/\r?\n/)
      .map((name) => name.trim())
      .filter(Boolean);
    if (!list || names.length === 0) return;
    await this.perform(async () => {
      const result = await this.api
        .addItems(list.id, names)
        .catch((error: unknown) => this.handleListError(list.id, error));
      this.state.drafts.items = "";
      await this.acceptList(result);
      this.state.notice = `${names.length}件の項目を追加しました。`;
    });
  }

  async startComparison(): Promise<void> {
    const list = this.state.active;
    if (!list) return;
    await this.perform(async () => {
      this.state.pair = null;
      this.state.view = "compare";
      for (let attempt = 0; attempt < 3; attempt += 1) {
        const current = await this.api
          .resumeList(list.id)
          .catch((error: unknown) => this.handleListError(list.id, error));
        this.acceptCommittedList(current);
        const pair = await this.api
          .nextPair(list.id)
          .catch((error: unknown) => this.handleListError(list.id, error));
        if (pair && pair.revision !== current.revision) continue;
        if (!pair && current.items.length >= 2) continue;
        await this.refreshListSummaries(list.id);
        this.state.pair = pair;
        this.state.view = pair ? "compare" : "items";
        return;
      }
      throw new Error(comparisonRetryMessage);
    });
  }

  private async handleListError(listId: number, error: unknown): Promise<never> {
    if (errorMessage(error) === "リストが見つかりません。") {
      this.state.lists = this.state.lists.filter((entry) => entry.id !== listId);
      if (this.state.active && this.state.active.id !== listId) {
        this.state.lists = await this.api.listSummaries();
        if (this.state.lists.some((entry) => entry.id === this.state.active?.id)) throw error;
      }
      this.state.pair = null;
      this.state.active = null;
      this.state.modal = null;
      this.state.candidates = [];
      this.state.searched = false;
      this.state.drafts.items = "";
      this.state.notice = "";
      this.state.view = "items";
      this.state.active = await this.loadFirstAvailableList();
      this.state.view = this.state.active?.convergence.converged ? "ranking" : "items";
    }
    throw error;
  }

  private async handleItemError(listId: number, itemId: number, error: unknown): Promise<never> {
    if (errorMessage(error) !== "項目が見つかりません。")
      return this.handleListError(listId, error);
    this.state.modal = null;
    this.state.candidates = [];
    this.state.searched = false;
    this.state.pair = null;
    this.state.view = "items";
    const list = this.state.active;
    if (list?.id === listId) {
      this.acceptCommittedList({
        ...list,
        items: list.items.filter((item) => item.id !== itemId),
        convergence: { ...list.convergence, converged: false },
      });
      const current = await this.api
        .getList(listId)
        .catch((reloadError: unknown) => this.handleListError(listId, reloadError));
      await this.acceptList(current);
    }
    throw error;
  }

  async answer(preference: Preference): Promise<void> {
    const pair = this.state.pair;
    if (!pair || this.state.view !== "compare" || this.state.modal) return;
    await this.perform(async () => {
      let result: ListState;
      try {
        result = await this.api.answer(pair, preference);
      } catch (error) {
        if (errorMessage(error) === "リストが更新されています。最新の比較を読み直してください。") {
          this.state.pair = null;
          throw new Error(comparisonRetryMessage, { cause: error });
        }
        return this.handleListError(pair.listId, error);
      }
      // Drop the used proposal before any later I/O; it must never be submitted twice.
      this.acceptCommittedList(result);
      if (result.convergence.converged) {
        this.state.view = "ranking";
        this.state.notice = "順位ほぼ確定。比較を続けることもできます。";
      }
      await this.refreshListSummaries(result.id);
      if (!result.convergence.converged) {
        const next = await this.api
          .nextPair(result.id)
          .catch((error: unknown) => this.handleListError(result.id, error));
        if (!next || next.revision !== result.revision) throw new Error(comparisonRetryMessage);
        this.state.pair = next;
      }
    });
  }

  async searchImages(): Promise<void> {
    const query = this.state.drafts.query.trim();
    if (this.state.busy || !query || this.state.modal?.kind !== "image") return;
    this.state.candidates = [];
    this.state.searched = false;
    await this.performRead(
      "image",
      () => this.api.searchImages(this.state.provider, query),
      (candidates) => {
        this.state.candidates = candidates;
        this.state.searched = true;
      },
    );
  }

  private cancelRead(kind: ReadKind): void {
    if (this.state.readPending !== kind) return;
    this.readRequest = null;
    this.state.readPending = null;
    this.state.busy = false;
  }

  private async performRead<T>(
    kind: ReadKind,
    load: () => Promise<T>,
    accept: (result: T) => void,
    messages: { notice?: string; errorPrefix?: string } = {},
  ): Promise<void> {
    if (this.state.busy) return;
    const request = Symbol();
    this.readRequest = request;
    this.state.busy = true;
    this.state.readPending = kind;
    this.state.error = "";
    this.state.notice = messages.notice ?? "";
    this.changed();
    try {
      const result = await load();
      if (this.readRequest === request) accept(result);
    } catch (error) {
      if (this.readRequest === request) {
        this.state.error = `${messages.errorPrefix ?? ""}${errorMessage(error)}`;
      }
    } finally {
      // A dismissed read must not update a new screen or release a later operation.
      if (this.readRequest === request) {
        this.cancelRead(kind);
        this.changed();
      }
    }
  }

  async changeImage(source: "local" | "none" | ImageCandidate): Promise<void> {
    const list = this.state.active;
    const modal = this.state.modal;
    if (!list || modal?.kind !== "image") return;
    await this.perform(async () => {
      let result: ListState | null;
      try {
        if (source === "local") result = await this.api.setLocalImage(list.id, modal.itemId);
        else if (source === "none") result = await this.api.removeImage(list.id, modal.itemId);
        else result = await this.api.setRemoteImage(list.id, modal.itemId, source);
      } catch (error) {
        return this.handleItemError(list.id, modal.itemId, error);
      }
      if (!result) return;
      this.state.modal = null;
      await this.acceptList(result);
    });
  }

  async saveKey(provider: SearchProvider, remove = false): Promise<void> {
    const field = provider === "brave" ? "braveKey" : "ollamaKey";
    const key = remove ? "" : this.state.drafts[field].trim();
    if (!remove && !key) return;
    let committed = false;
    await this.perform(async () => {
      await this.api.setApiKey(provider, key);
      this.state.drafts[field] = "";
      this.state.notice = remove ? "APIキーを削除しました。" : "APIキーを保存しました。";
      const configured = provider === "brave" ? "braveConfigured" : "ollamaConfigured";
      const settings: SearchSettings = {
        braveConfigured: false,
        ollamaConfigured: false,
        defaultProvider: "brave",
        ...this.state.settings,
        [configured]: !remove,
      };
      if (settings.errors) {
        settings.errors = { ...settings.errors };
        delete settings.errors[provider];
      }
      settings.defaultProvider =
        settings.ollamaConfigured && !settings.braveConfigured ? "ollama" : "brave";
      this.acceptSettings(settings);
      committed = true;
    });
    if (committed && this.state.view === "settings" && !this.state.modal) {
      await this.performRead(
        "settings",
        () => this.api.searchSettings(),
        (settings) => this.acceptSettings(settings),
        {
          notice: this.state.notice,
          errorPrefix: "設定状態を再取得できませんでした: ",
        },
      );
    }
  }
}
