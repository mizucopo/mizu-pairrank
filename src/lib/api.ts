import { invoke } from "@tauri-apps/api/core";

import type { AppApi } from "./types.js";

export const api: AppApi = {
  listSummaries: () => invoke("list_summaries"),
  getList: (listId) => invoke("get_list", { listId }),
  createList: (name) => invoke("create_list", { name }),
  renameList: (listId, name) => invoke("rename_list", { listId, name }),
  deleteList: (listId) => invoke("delete_list", { listId }),
  addItems: (listId, names) => invoke("add_items", { listId, names }),
  renameItem: (listId, itemId, name) => invoke("rename_item", { listId, itemId, name }),
  deleteItem: (listId, itemId) => invoke("delete_item", { listId, itemId }),
  resumeList: (listId) => invoke("resume_list", { listId }),
  nextPair: (listId) => invoke("next_pair", { listId }),
  answer: (pair, preference) =>
    invoke("answer", {
      listId: pair.listId,
      aId: pair.a.id,
      bId: pair.b.id,
      preference,
      expectedRevision: pair.revision,
    }),
  searchSettings: () => invoke("search_settings"),
  setApiKey: (provider, key) => invoke("set_api_key", { provider, key }),
  searchImages: (provider, query) => invoke("search_images", { provider, query }),
  setLocalImage: (listId, itemId) => invoke("set_local_image", { listId, itemId }),
  setRemoteImage: (listId, itemId, candidate) =>
    invoke("set_remote_image", { listId, itemId, candidate }),
  removeImage: (listId, itemId) => invoke("remove_image", { listId, itemId }),
};
