export type Preference = "a_strong" | "a_weak" | "equal" | "b_weak" | "b_strong";
export type SearchProvider = "brave" | "ollama";
export type Rating = { mu: number; sigma: number };
export type ImageAsset = { path: string; sourceUrl: string | null };
export type Item = {
  id: number;
  listId: number;
  name: string;
  image: ImageAsset | null;
  rating: Rating;
  comparisonCount: number;
};
export type Convergence = {
  converged: boolean;
  maxSigma: number;
  maxRankSpan: number | null;
  observedAnswers: number;
  requiredAnswers: number;
};
export type ListSummary = {
  id: number;
  name: string;
  itemCount: number;
  comparisonCount: number;
  converged: boolean;
};
export type ListState = {
  id: number;
  name: string;
  revision: number;
  items: Item[];
  comparisonCount: number;
  convergence: Convergence;
};
export type PairProposal = { listId: number; revision: number; a: Item; b: Item };
export type ImageCandidate = {
  id: string;
  title: string;
  previewUrl: string;
  sourceUrl: string;
};
export type SearchSettings = {
  braveConfigured: boolean;
  ollamaConfigured: boolean;
  defaultProvider: SearchProvider;
};
export type AppApi = {
  listSummaries: () => Promise<ListSummary[]>;
  getList: (listId: number) => Promise<ListState>;
  createList: (name: string) => Promise<ListState>;
  renameList: (listId: number, name: string) => Promise<ListState>;
  deleteList: (listId: number) => Promise<void>;
  addItems: (listId: number, names: string[]) => Promise<ListState>;
  renameItem: (listId: number, itemId: number, name: string) => Promise<ListState>;
  deleteItem: (listId: number, itemId: number) => Promise<ListState>;
  resumeList: (listId: number) => Promise<ListState>;
  nextPair: (listId: number) => Promise<PairProposal | null>;
  answer: (pair: PairProposal, preference: Preference) => Promise<ListState>;
  searchSettings: () => Promise<SearchSettings>;
  setApiKey: (provider: SearchProvider, key: string) => Promise<SearchSettings>;
  searchImages: (provider: SearchProvider, query: string) => Promise<ImageCandidate[]>;
  setLocalImage: (listId: number, itemId: number) => Promise<ListState | null>;
  setRemoteImage: (listId: number, itemId: number, candidate: ImageCandidate) => Promise<ListState>;
  removeImage: (listId: number, itemId: number) => Promise<ListState>;
};
