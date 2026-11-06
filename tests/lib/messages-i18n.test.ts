import pdfMake from "pdfmake/build/pdfmake";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AppController } from "../../src/lib/controller.js";
import { setLanguagePreference } from "../../src/lib/i18n.js";
import { reportHtml, reportPdf } from "../../src/lib/report.js";
import type { Report } from "../../src/lib/report.js";
import { renderApp } from "../../src/lib/view.js";
import type { AppApi, Item } from "../../src/lib/types.js";

const report: Report = {
  title: "私のランキング <script>",
  comparisonCount: 3,
  itemCount: 1,
  matchingCount: 1,
  view: "ranking",
  selectedTagNames: ["果物 {count} <b>"],
  entries: [{ name: "りんご", rank: 1, image: null, mu: 25, sigma: 2, comparisonCount: 3 }],
  tiers: [{ label: "C", entries: [] }],
};

afterEach(() => {
  setLanguagePreference("ja");
  vi.restoreAllMocks();
});

describe("localized messages and reports", () => {
  it("translates report labels while preserving and escaping user text", () => {
    setLanguagePreference("en");
    const html = reportHtml(report);
    expect(html).toContain('<html lang="en">');
    expect(html).toContain("Your ranking");
    expect(html).toContain("1 items · 3 comparisons");
    expect(html).toContain("Selected tags: 果物 {count} &lt;b&gt; (match all)");
    expect(html).toContain("私のランキング &lt;script&gt;");
    expect(html).toContain("りんご");
    expect(html).not.toContain("推定評価");
    setLanguagePreference("ja");
    expect(reportHtml(report)).toContain("あなたのランキング");
    expect(reportHtml(report)).toContain('<html lang="ja">');
  });

  it("translates empty and tier reports", () => {
    setLanguagePreference("en");
    expect(reportHtml({ ...report, matchingCount: 0 })).toContain(
      "No items have all the selected tags.",
    );
    const tier = reportHtml({ ...report, view: "tier" });
    expect(tier).toContain("Your tier list");
    expect(tier).toContain("No items");
  });

  it("uses the current language for controller validation without altering drafts", async () => {
    // This validation path must not call the backend.
    const controller = new AppController({} as AppApi, vi.fn());
    controller.state.modal = { kind: "create-list" };
    setLanguagePreference("en");
    await controller.saveName();
    expect(controller.state.error).toBe("Enter a name.");
    expect(controller.state.drafts.name).toBe("");
    setLanguagePreference("ja");
    await controller.saveName();
    expect(controller.state.error).toBe("名前を入力してください。");
  });

  it("renders every comparison answer in English without translating item names", () => {
    setLanguagePreference("en");
    const controller = new AppController({} as AppApi, vi.fn());
    const a: Item = {
      id: 1,
      listId: 1,
      name: "りんご",
      image: null,
      rating: { mu: 25, sigma: 2 },
      comparisonCount: 0,
      tagIds: [],
    };
    const b = { ...a, id: 2, name: "みかん" };
    Object.assign(controller.state, {
      initialized: true,
      view: "compare",
      active: {
        id: 1,
        name: "果物",
        revision: 1,
        items: [a, b],
        tags: [],
        comparisonCount: 0,
        comparisonWindow: [],
        convergence: {
          converged: false,
          maxSigma: 2,
          maxRankSpan: 0,
          observedAnswers: 0,
          requiredAnswers: 20,
        },
      },
      pair: { listId: 1, revision: 1, a, b },
    });
    const html = renderApp(controller.state, (path) => path);
    for (const label of ["Strongly prefer A", "Prefer A", "Equal", "Prefer B", "Strongly prefer B"])
      expect(html).toContain(label);
    expect(html).not.toContain("Aが大好き");
    expect(html).toContain("りんご");
    expect(html).toContain("みかん");
  });

  it("preserves backend error recovery while English is selected", async () => {
    setLanguagePreference("en");
    const api = {
      listSummaries: vi
        .fn()
        .mockResolvedValueOnce([{ id: 1 }])
        .mockResolvedValueOnce([]),
      getList: vi.fn().mockRejectedValue(new Error("リストが見つかりません。")),
    };
    const controller = new AppController(api as unknown as AppApi, vi.fn());
    await controller.initialize();
    expect(api.listSummaries).toHaveBeenCalledTimes(2);
    expect(controller.state.initialized).toBe(true);
    expect(controller.state.active).toBeNull();
    expect(controller.state.error).toBe("");
  });

  it("translates both failures when ChatGPT connection and status refresh fail", async () => {
    setLanguagePreference("en");
    const api = {
      connectChatGpt: vi
        .fn()
        .mockRejectedValue(new Error("ChatGPT認証に失敗しました (HTTP 401)。再試行してください。")),
      searchSettings: vi.fn().mockRejectedValue(new Error("データ処理に失敗しました: offline")),
    };
    const controller = new AppController(api as unknown as AppApi, vi.fn());
    controller.state.view = "settings";
    await controller.connectChatGpt();
    expect(controller.state.error).toContain(
      "ChatGPT authentication failed (HTTP 401). Try again.",
    );
    expect(controller.state.error).toContain(
      "Could not recheck the connection: Data processing failed: offline",
    );
    expect(controller.state.error).not.toMatch(/[ぁ-んァ-ヶ一-龯]/);
  });

  it("localizes PDF headings and metadata while keeping Japanese user content", async () => {
    setLanguagePreference("en");
    const createPdf = vi.spyOn(pdfMake, "createPdf");
    const bytes = await reportPdf(report);
    expect(bytes.length).toBeGreaterThan(100);
    const definition = createPdf.mock.calls[0]![0];
    expect(definition.info?.title).toBe("私のランキング <script> — Ranking");
    const content = JSON.stringify(definition.content);
    expect(content).toContain("Your ranking");
    expect(content).toContain("りんご");
    expect(content).not.toContain("あなたのランキング");
  });
});
