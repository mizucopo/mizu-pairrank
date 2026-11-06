// Documentation-only fixture: render the production UI without touching storage or credentials.
import type { AppState, View } from "../../src/lib/controller.js";
import type { ListState } from "../../src/lib/types.js";
import { renderApp } from "../../src/lib/view.js";

const names = [
  "いちごタルト",
  "チーズケーキ",
  "プリン",
  "抹茶パフェ",
  "アップルパイ",
  "チョコアイス",
];
const list: ListState = {
  id: 1,
  name: "お気に入りのおやつ",
  revision: 1,
  comparisonWindow: [1, 12],
  items: names.map((name, index) => ({
    id: index + 1,
    listId: 1,
    name,
    image: null,
    rating: { mu: 34 - index * 3, sigma: 3.5 },
    comparisonCount: 4,
    tagIds: [index < 2 || index === 4 ? 1 : 2],
  })),
  tags: [
    { id: 1, listId: 1, name: "焼き菓子" },
    { id: 2, listId: 1, name: "ひんやり" },
  ],
  comparisonCount: 12,
  convergence: {
    converged: false,
    maxSigma: 3.5,
    maxRankSpan: null,
    observedAnswers: 12,
    requiredAnswers: 20,
  },
};
const requestedView = new URLSearchParams(location.search).get("view") ?? "items";
const views: View[] = ["items", "compare", "ranking", "tier"];
const view = views.find((candidate) => candidate === requestedView);
if (!view) throw new Error(`Unsupported screenshot view: ${requestedView}`);
const state: AppState = {
  lists: [{ id: 1, name: list.name, itemCount: 6, comparisonCount: 12, converged: false }],
  active: list,
  pair: { listId: 1, revision: 1, a: list.items[0]!, b: list.items[1]! },
  comparisonPaused: false,
  canUndoAnswer: false,
  view,
  selectedTagIds: [],
  busy: false,
  initialized: true,
  fatal: false,
  error: "",
  notice: "",
  modal: null,
  exportIncludeImages: false,
  reportFormat: "pdf",
  tagDraft: "",
  tagEditingId: null,
  tagDeletingId: null,
  settings: null,
  checkedProviders: [],
  bulkProvider: null,
  bulkRun: null,
  candidates: [],
  searched: false,
  readPending: null,
  provider: "brave",
  openaiAuthMode: "api",
  openaiModels: [],
  imageSearchReturn: null,
  drafts: {
    name: "",
    items: "",
    query: "",
    braveKey: "",
    ollamaKey: "",
    openaiKey: "",
    openaiModel: "",
  },
};
const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("Screenshot root not found");
root.innerHTML = renderApp(state, (path) => path);
