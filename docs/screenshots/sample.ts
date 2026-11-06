// Documentation-only fixture: render the production UI without touching storage or credentials.
import type { AppState, View } from "../../src/lib/controller.js";
import type { ListState } from "../../src/lib/types.js";
import { renderApp } from "../../src/lib/view.js";

const names = [
  "いちごタルト",
  "チーズケーキ",
  "プリン",
  "抹茶アイス",
  "アップルパイ",
  "チョコアイス",
];
const images = [
  {
    path: "/docs/screenshots/assets/strawberry-tart.jpg",
    sourceUrl: "https://commons.wikimedia.org/wiki/File:Strawberry_tart,_June_2025_UK.jpg",
  },
  {
    path: "/docs/screenshots/assets/cheesecake.jpg",
    sourceUrl: "https://commons.wikimedia.org/wiki/File:Cheesecake001.jpg",
  },
  {
    path: "/docs/screenshots/assets/pudding.jpg",
    sourceUrl: "https://commons.wikimedia.org/wiki/File:Putchin_Pudding_001.jpg",
  },
  {
    path: "/docs/screenshots/assets/matcha-ice-cream.jpg",
    sourceUrl: "https://commons.wikimedia.org/wiki/File:Handmade_Matcha_ice_cream_-_Rawlab.jpg",
  },
  {
    path: "/docs/screenshots/assets/apple-pie.jpg",
    sourceUrl: "https://commons.wikimedia.org/wiki/File:Fruit_Pie.jpg",
  },
  {
    path: "/docs/screenshots/assets/chocolate-ice-cream.jpg",
    sourceUrl:
      "https://commons.wikimedia.org/wiki/File:Dark_chocolate_chip_ice_cream_cone_-_August_2025_-_Sarah_Stierch_(cropped).jpg",
  },
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
    image: images[index]!,
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
