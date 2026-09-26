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

export type View = "items" | "compare" | "ranking" | "tier" | "settings";
type ReadKind = "image" | "settings";
export type Modal =
  | { kind: "create-list" }
  | { kind: "rename-list" }
  | { kind: "delete-list" }
  | { kind: "rename-item"; itemId: number }
  | { kind: "delete-item"; itemId: number }
  | { kind: "tags"; itemId?: number }
  | { kind: "image"; itemId: number }
  | { kind: "bulk-image" }
  | null;
export type BulkImageRun = {
  total: number;
  done: number;
  registered: number;
  skipped: number;
  stopping: boolean;
  stopped: boolean;
  running: boolean;
};
export type AppState = {
  lists: ListSummary[];
  active: ListState | null;
  pair: PairProposal | null;
  view: View;
  selectedTagId: number | null;
  busy: boolean;
  initialized: boolean;
  fatal: boolean;
  error: string;
  notice: string;
  modal: Modal;
  tagDraft: string;
  tagEditingId: number | null;
  tagDeletingId: number | null;
  settings: SearchSettings | null;
  bulkSettings: SearchSettings | null;
  bulkSettingsLoading: boolean;
  bulkProvider: SearchProvider | null;
  bulkRun: BulkImageRun | null;
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
    selectedTagId: null,
    busy: false,
    initialized: false,
    fatal: false,
    error: "",
    notice: "",
    modal: null,
    tagDraft: "",
    tagEditingId: null,
    tagDeletingId: null,
    settings: null,
    bulkSettings: null,
    bulkSettingsLoading: false,
    bulkProvider: null,
    bulkRun: null,
    candidates: [],
    searched: false,
    readPending: null,
    provider: "brave",
    drafts: { name: "", items: "", query: "", braveKey: "", ollamaKey: "" },
  };

  private readRequest: symbol | null = null;
  private settingsRead: Promise<SearchSettings> | null = null;
  private bulkSettingsRequest: symbol | null = null;

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
    this.state.selectedTagId = null;
    this.state.lists = await this.api.listSummaries();
    for (let attempt = 0; attempt < 3; attempt += 1) {
      const first = this.state.lists[0];
      if (!first) return null;
      try {
        const list = await this.api.getList(first.id);
        this.updateListSummary(list);
        return list;
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
    let selected = false;
    this.cancelRead("settings");
    await this.perform(async () => {
      const list = await this.api
        .getList(id)
        .catch((error: unknown) => this.handleListError(id, error));
      if (list.id !== this.state.active?.id) this.state.drafts.items = "";
      this.acceptCommittedList(list);
      this.state.modal = null;
      this.state.view = list.convergence.converged ? "ranking" : "items";
      selected = true;
    });
    if (selected && this.state.view === "items") void this.refreshBulkSettings();
  }

  selectTag(tagId: number | null): void {
    if (tagId !== null && !this.state.active?.tags.some((tag) => tag.id === tagId)) return;
    this.state.selectedTagId = tagId;
    this.changed();
  }

  async navigate(view: View): Promise<void> {
    if (view === "settings" && this.state.readPending === "settings") return;
    this.cancelRead("settings");
    if (this.state.busy) return;
    if (view === "settings") {
      this.bulkSettingsRequest = null;
      this.state.bulkSettingsLoading = false;
    }
    if (view === "compare") {
      await this.startComparison();
      return;
    }
    this.state.view = view;
    this.state.modal = null;
    this.state.error = "";
    if (view === "settings") await this.loadSettings("settings");
    else {
      this.changed();
      if (view === "items") void this.refreshBulkSettings();
    }
  }

  availableBulkProviders(): SearchProvider[] {
    const settings = this.state.bulkSettings;
    if (!settings) return [];
    return (["brave", "ollama"] as const).filter(
      (provider) =>
        (provider === "brave" ? settings.braveConfigured : settings.ollamaConfigured) &&
        settings.errors?.[provider] === undefined,
    );
  }

  async refreshBulkSettings(): Promise<void> {
    const request = Symbol();
    this.bulkSettingsRequest = request;
    this.state.bulkSettings = null;
    this.state.bulkSettingsLoading = true;
    this.changed();
    try {
      if (this.settingsRead) await this.settingsRead.catch(() => undefined);
      if (this.bulkSettingsRequest !== request) return;
      const reading = this.api.searchSettings();
      this.settingsRead = reading;
      try {
        const settings = await reading;
        if (this.bulkSettingsRequest === request) this.state.bulkSettings = settings;
      } finally {
        if (this.settingsRead === reading) this.settingsRead = null;
      }
    } catch {
      // An unknown credential status must keep bulk registration disabled.
    } finally {
      if (this.bulkSettingsRequest === request) {
        this.state.bulkSettingsLoading = false;
        this.changed();
      }
    }
  }

  openBulkImages(): void {
    if (this.state.busy || this.state.view !== "items" || !this.state.active) return;
    const missing = this.state.active.items.filter((item) => !item.image);
    const providers = this.availableBulkProviders();
    if (!missing.length || !providers.length || this.state.bulkSettingsLoading) return;
    this.state.bulkProvider = providers.length === 1 ? providers[0]! : null;
    this.state.bulkRun = null;
    this.state.error = "";
    this.state.modal = { kind: "bulk-image" };
    this.changed();
  }

  stopBulkImages(): void {
    const run = this.state.bulkRun;
    if (!run?.running || run.stopping) return;
    run.stopping = true;
    this.changed();
  }

  async runBulkImages(): Promise<void> {
    const list = this.state.active;
    const provider = this.state.bulkProvider;
    if (
      this.state.busy ||
      !list ||
      this.state.modal?.kind !== "bulk-image" ||
      !provider ||
      !this.availableBulkProviders().includes(provider)
    )
      return;
    const itemIds = list.items.filter((item) => !item.image).map((item) => item.id);
    if (!itemIds.length) return;
    const run: BulkImageRun = {
      total: itemIds.length,
      done: 0,
      registered: 0,
      skipped: 0,
      stopping: false,
      stopped: false,
      running: true,
    };
    this.state.bulkRun = run;
    this.state.busy = true;
    this.state.error = "";
    this.state.notice = "";
    this.changed();
    try {
      for (const itemId of itemIds) {
        if (run.stopping) break;
        const outcome = await this.api.autoRegisterImage(list.id, itemId, provider);
        if (outcome !== "registered" && outcome !== "skipped" && outcome !== "unavailable")
          throw new Error("画像登録の結果を確認できませんでした。");
        run.done += 1;
        if (outcome === "registered") run.registered += 1;
        else run.skipped += 1;
        this.changed();
      }
    } catch (error) {
      this.state.error = errorMessage(error);
    } finally {
      run.stopped = run.stopping && run.done < run.total;
      run.running = false;
      try {
        const latest = await this.api
          .getList(list.id)
          .catch((error: unknown) => this.handleListError(list.id, error));
        await this.acceptList(latest);
      } catch (error) {
        this.state.error = [
          this.state.error,
          `リストを再取得できませんでした: ${errorMessage(error)}`,
        ]
          .filter(Boolean)
          .join(" ");
      }
      this.state.notice = `画像の一括登録${run.stopped ? "を停止" : this.state.error ? "が中断" : "が完了"}しました。${run.done} / ${run.total} 件を処理し、${run.registered} 件を登録、${run.skipped} 件を見送りました。`;
      this.state.busy = false;
      this.changed();
    }
  }

  private async loadSettings(
    kind: ReadKind,
    messages: { notice?: string; errorPrefix?: string } = {},
  ): Promise<void> {
    await this.performRead(
      kind,
      async () => {
        const request = this.readRequest;
        // UI cancellation leaves the native worker holding the single settings-read slot.
        if (this.settingsRead) await this.settingsRead.catch(() => undefined);
        if (this.readRequest !== request) return null;
        const reading = this.api.searchSettings();
        this.settingsRead = reading;
        try {
          return await reading;
        } finally {
          if (this.settingsRead === reading) this.settingsRead = null;
        }
      },
      (settings) => {
        if (settings) this.acceptSettings(settings);
      },
      messages,
    );
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
    this.state.tagDraft = "";
    this.state.tagEditingId = null;
    this.state.tagDeletingId = null;
    const item =
      "itemId" in modal
        ? this.state.active?.items.find((entry) => entry.id === modal.itemId)
        : undefined;
    this.state.drafts.name =
      modal.kind === "rename-list" ? (this.state.active?.name ?? "") : (item?.name ?? "");
    this.state.drafts.query = item?.name ?? "";
    this.state.candidates = [];
    this.state.searched = false;
    if (modal.kind === "image") await this.loadSettings("image");
    else this.changed();
  }

  closeModal(): void {
    if (this.state.busy && this.state.readPending !== "image") return;
    const wasBulkImages = this.state.modal?.kind === "bulk-image";
    const reloadSettings =
      this.state.modal !== null && this.state.view === "settings" && this.state.settings === null;
    this.cancelRead("image");
    this.state.modal = null;
    this.state.bulkRun = null;
    if (!wasBulkImages) this.state.error = "";
    this.changed();
    if (reloadSettings) void this.navigate("settings");
  }

  private acceptCommittedList(list: ListState): void {
    // Keep committed state usable even if a later sidebar refresh fails.
    if (
      this.state.active?.id !== list.id ||
      (this.state.selectedTagId !== null &&
        !list.tags.some((tag) => tag.id === this.state.selectedTagId))
    )
      this.state.selectedTagId = null;
    this.state.active = list;
    this.state.pair = null;
    const modal = this.state.modal;
    if (
      modal?.kind === "tags" &&
      modal.itemId !== undefined &&
      !list.items.some((item) => item.id === modal.itemId)
    ) {
      this.state.modal = null;
      this.state.tagDraft = "";
      this.state.tagEditingId = null;
      this.state.tagDeletingId = null;
    }
    this.updateListSummary(list);
  }

  editTag(tagId: number | null): void {
    if (this.state.modal?.kind !== "tags" || this.state.busy) return;
    const tag = this.state.active?.tags.find((entry) => entry.id === tagId);
    this.state.tagEditingId = tag?.id ?? null;
    this.state.tagDeletingId = null;
    this.state.tagDraft = tag?.name ?? "";
    this.state.error = "";
    this.changed();
  }

  deleteTagPrompt(tagId: number | null): void {
    if (this.state.modal?.kind !== "tags" || this.state.busy) return;
    this.state.tagDeletingId = this.state.active?.tags.some((tag) => tag.id === tagId)
      ? tagId
      : null;
    this.state.tagEditingId = null;
    this.state.error = "";
    this.changed();
  }

  async saveTag(): Promise<void> {
    const list = this.state.active;
    const modal = this.state.modal;
    if (!list || modal?.kind !== "tags") return;
    const name = this.state.tagDraft.trim();
    const editingId = this.state.tagEditingId;
    if (editingId !== null && name === list.tags.find((tag) => tag.id === editingId)?.name) {
      this.editTag(null);
      return;
    }
    await this.perform(async () => {
      if (!name) throw new Error("タグ名を入力してください。");
      let result: ListState;
      if (editingId === null) {
        result = await this.api.createTag(list.id, name, modal.itemId).catch((error: unknown) => {
          if (errorMessage(error) === "同じ名前のタグが既にあります。")
            return this.handleTagError(list.id, error);
          if (modal.itemId === undefined) return this.handleListError(list.id, error);
          return this.handleItemError(list.id, modal.itemId, error);
        });
      } else {
        result = await this.api
          .renameTag(list.id, editingId, name)
          .catch((error: unknown) => this.handleTagError(list.id, error));
      }
      this.acceptCommittedList(result);
      this.state.tagDraft = "";
      this.state.tagEditingId = null;
    });
  }

  async confirmTagDelete(): Promise<void> {
    const list = this.state.active;
    const tagId = this.state.tagDeletingId;
    if (!list || this.state.modal?.kind !== "tags" || tagId === null) return;
    await this.perform(async () => {
      const result = await this.api
        .deleteTag(list.id, tagId)
        .catch((error: unknown) => this.handleTagError(list.id, error));
      this.acceptCommittedList(result);
      this.state.tagDeletingId = null;
    });
  }

  async toggleItemTag(tagId: number, checked: boolean): Promise<void> {
    const list = this.state.active;
    const modal = this.state.modal;
    if (
      !list ||
      modal?.kind !== "tags" ||
      modal.itemId === undefined ||
      !list.tags.some((tag) => tag.id === tagId)
    )
      return;
    const item = list.items.find((entry) => entry.id === modal.itemId);
    if (!item) return;
    await this.perform(async () => {
      const result = await this.api
        .setItemTag(list.id, item.id, tagId, checked)
        .catch((error: unknown) => {
          if (errorMessage(error) === "タグが見つかりません。")
            return this.handleTagError(list.id, error);
          return this.handleItemError(list.id, item.id, error);
        });
      this.acceptCommittedList(result);
    });
  }

  private updateListSummary(list: ListState): void {
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
    if (
      !modal ||
      (modal.kind !== "create-list" && modal.kind !== "rename-list" && modal.kind !== "rename-item")
    )
      return;
    if (
      (modal.kind === "rename-list" && name === list?.name) ||
      (modal.kind === "rename-item" &&
        name === list?.items.find((item) => item.id === modal.itemId)?.name)
    ) {
      this.closeModal();
      return;
    }
    await this.perform(async () => {
      if (!name) throw new Error("名前を入力してください。");
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
    if (this.state.modal === null && this.state.active && this.state.view === "items")
      void this.refreshBulkSettings();
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
    if (this.state.modal === null && this.state.active && this.state.view === "items")
      void this.refreshBulkSettings();
  }

  async addItems(): Promise<void> {
    const list = this.state.active;
    const names = this.state.drafts.items
      .split(/\r?\n/)
      .map((name) => name.trim())
      .filter(Boolean);
    if (!list) return;
    await this.perform(async () => {
      if (names.length === 0) throw new Error("項目名を入力してください。");
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
    if (this.state.active && this.state.view === "items" && !this.state.bulkSettingsLoading)
      void this.refreshBulkSettings();
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
      if (this.state.active && this.state.view === "items") void this.refreshBulkSettings();
    }
    throw error;
  }

  private async handleTagError(listId: number, error: unknown): Promise<never> {
    const message = errorMessage(error);
    if (message !== "タグが見つかりません。" && message !== "同じ名前のタグが既にあります。")
      return this.handleListError(listId, error);
    if (message === "タグが見つかりません。") {
      this.state.tagDraft = "";
      this.state.tagEditingId = null;
      this.state.tagDeletingId = null;
    }
    const current = await this.api
      .getList(listId)
      .catch((reloadError: unknown) => this.handleListError(listId, reloadError));
    this.acceptCommittedList(current);
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
    if (this.state.busy || this.state.modal?.kind !== "image") return;
    if (!query) {
      this.state.error = "検索語は1〜400文字、50語以内で入力してください。";
      this.changed();
      return;
    }
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
    let committed = false;
    await this.perform(async () => {
      if (!remove && !key) throw new Error("APIキーの形式が正しくありません。");
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
      await this.loadSettings("settings", {
        notice: this.state.notice,
        errorPrefix: "設定状態を再取得できませんでした: ",
      });
    }
  }
}
