import { localizeBackendError } from "./backend-i18n.js";
import { t } from "./i18n.js";

export type BulkImageRun = {
  total: number;
  done: number;
  registered: number;
  skipped: number;
  stopping: boolean;
  stopped: boolean;
  running: boolean;
};

type BulkImageState = {
  bulkRun: BulkImageRun | null;
  busy: boolean;
  error: string;
  notice: string;
};

type BulkImageDependencies = {
  registerImage: (itemId: number) => Promise<unknown>;
  refreshList: () => Promise<void>;
  changed: () => void;
};

function errorMessage(error: unknown): string {
  return localizeBackendError(t(error instanceof Error ? error.message : String(error)));
}

export function stopBulkImageRegistration(state: BulkImageState, changed: () => void): void {
  const run = state.bulkRun;
  if (!run?.running || run.stopping) return;
  run.stopping = true;
  changed();
}

export async function runBulkImageRegistration(
  state: BulkImageState,
  itemIds: readonly number[],
  { registerImage, refreshList, changed }: BulkImageDependencies,
): Promise<void> {
  const run: BulkImageRun = {
    total: itemIds.length,
    done: 0,
    registered: 0,
    skipped: 0,
    stopping: false,
    stopped: false,
    running: true,
  };
  state.bulkRun = run;
  state.busy = true;
  state.error = "";
  state.notice = "";
  changed();
  try {
    for (const itemId of itemIds) {
      if (run.stopping) break;
      const outcome = await registerImage(itemId);
      if (outcome !== "registered" && outcome !== "skipped" && outcome !== "unavailable")
        throw new Error(t("画像登録の結果を確認できませんでした。"));
      run.done += 1;
      if (outcome === "registered") run.registered += 1;
      else run.skipped += 1;
      changed();
    }
  } catch (error) {
    state.error = errorMessage(error);
  } finally {
    run.stopped = run.stopping && run.done < run.total;
    run.running = false;
    // Keep the app busy until committed images and list summaries have been refreshed.
    try {
      await refreshList();
    } catch (error) {
      state.error = [
        state.error,
        t("リストを再取得できませんでした: {error}", { error: errorMessage(error) }),
      ]
        .filter(Boolean)
        .join(" ");
    }
    let message =
      "画像の一括登録が完了しました。{done} / {total} 件を処理し、{registered} 件を登録、{skipped} 件を見送りました。";
    if (run.stopped)
      message =
        "画像の一括登録を停止しました。{done} / {total} 件を処理し、{registered} 件を登録、{skipped} 件を見送りました。";
    else if (state.error)
      message =
        "画像の一括登録が中断しました。{done} / {total} 件を処理し、{registered} 件を登録、{skipped} 件を見送りました。";
    state.notice = t(message, {
      done: run.done,
      total: run.total,
      registered: run.registered,
      skipped: run.skipped,
    });
    state.busy = false;
    changed();
  }
}
