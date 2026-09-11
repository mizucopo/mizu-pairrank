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
    provider: "brave",
    drafts: { name: "", items: "", query: "", braveKey: "", ollamaKey: "" },
  };

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
        this.state.lists = await this.api.listSummaries();
        const first = this.state.lists[0];
        if (first) {
          this.state.active = await this.api.getList(first.id);
          this.state.view = this.state.active.convergence.converged ? "ranking" : "items";
        }
        this.state.initialized = true;
      } catch (error) {
        this.state.fatal = true;
        throw error;
      }
    });
  }

  async selectList(id: number): Promise<void> {
    await this.perform(async () => {
      const list = await this.api.getList(id);
      this.state.active = list;
      this.state.pair = null;
      this.state.modal = null;
      this.state.drafts.items = "";
      this.state.view = list.convergence.converged ? "ranking" : "items";
    });
  }

  async navigate(view: View): Promise<void> {
    if (this.state.busy) return;
    if (view === "compare") {
      await this.startComparison();
      return;
    }
    this.state.view = view;
    this.state.modal = null;
    this.state.error = "";
    if (view === "settings") {
      await this.perform(async () => {
        await this.loadSettings();
      });
    } else this.changed();
  }

  private async loadSettings(): Promise<void> {
    this.state.settings = await this.api.searchSettings();
    this.state.provider = this.state.settings.defaultProvider;
  }

  async openModal(modal: Exclude<Modal, null>): Promise<void> {
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
      await this.perform(async () => {
        await this.loadSettings();
      });
    } else this.changed();
  }

  closeModal(): void {
    if (this.state.busy) return;
    this.state.modal = null;
    this.state.error = "";
    this.changed();
  }

  private async acceptList(list: ListState): Promise<void> {
    // A committed answer is consumed even if refreshing the sidebar later fails.
    this.state.active = list;
    this.state.pair = null;
    this.state.lists = await this.api.listSummaries();
  }

  async saveName(): Promise<void> {
    const modal = this.state.modal;
    const list = this.state.active;
    const name = this.state.drafts.name.trim();
    if (!modal || !name) return;
    await this.perform(async () => {
      let result: ListState;
      if (modal.kind === "create-list") result = await this.api.createList(name);
      else if (modal.kind === "rename-list" && list)
        result = await this.api.renameList(list.id, name);
      else if (modal.kind === "rename-item" && list)
        result = await this.api.renameItem(list.id, modal.itemId, name);
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
        await this.api.deleteList(list.id);
        this.state.active = null;
        this.state.pair = null;
        this.state.modal = null;
        this.state.lists = await this.api.listSummaries();
        const first = this.state.lists[0];
        if (first) this.state.active = await this.api.getList(first.id);
      } else if (modal.kind === "delete-item") {
        const result = await this.api.deleteItem(list.id, modal.itemId);
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
      const result = await this.api.addItems(list.id, names);
      this.state.drafts.items = "";
      await this.acceptList(result);
      this.state.notice = `${names.length}件の項目を追加しました。`;
    });
  }

  async startComparison(): Promise<void> {
    const list = this.state.active;
    if (!list || list.items.length < 2) return;
    await this.perform(async () => {
      if (list.convergence.converged) await this.acceptList(await this.api.resumeList(list.id));
      this.state.view = "compare";
      this.state.pair = await this.api.nextPair(list.id);
    });
  }

  async answer(preference: Preference): Promise<void> {
    const pair = this.state.pair;
    if (!pair || this.state.view !== "compare" || this.state.modal) return;
    await this.perform(async () => {
      const result = await this.api.answer(pair, preference);
      // Drop the used proposal before any later I/O; it must never be submitted twice.
      this.state.pair = null;
      this.state.active = result;
      if (result.convergence.converged) {
        this.state.view = "ranking";
        this.state.notice = "順位ほぼ確定。比較を続けることもできます。";
      }
      this.state.lists = await this.api.listSummaries();
      if (!result.convergence.converged) this.state.pair = await this.api.nextPair(result.id);
    });
  }

  async searchImages(): Promise<void> {
    const query = this.state.drafts.query.trim();
    if (!query || this.state.modal?.kind !== "image") return;
    await this.perform(async () => {
      this.state.candidates = [];
      this.state.searched = false;
      this.state.candidates = await this.api.searchImages(this.state.provider, query);
      this.state.searched = true;
    });
  }

  async changeImage(source: "local" | "none" | ImageCandidate): Promise<void> {
    const list = this.state.active;
    const modal = this.state.modal;
    if (!list || modal?.kind !== "image") return;
    await this.perform(async () => {
      const result =
        source === "local"
          ? await this.api.setLocalImage(list.id, modal.itemId)
          : source === "none"
            ? await this.api.removeImage(list.id, modal.itemId)
            : await this.api.setRemoteImage(list.id, modal.itemId, source);
      this.state.modal = null;
      await this.acceptList(result);
    });
  }

  async saveKey(provider: SearchProvider, remove = false): Promise<void> {
    const field = provider === "brave" ? "braveKey" : "ollamaKey";
    const key = remove ? "" : this.state.drafts[field].trim();
    if (!remove && !key) return;
    await this.perform(async () => {
      this.state.settings = await this.api.setApiKey(provider, key);
      this.state.provider = this.state.settings.defaultProvider;
      this.state.drafts[field] = "";
      this.state.notice = remove ? "APIキーを削除しました。" : "APIキーを保存しました。";
    });
  }
}
