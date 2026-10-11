import { readFileSync } from "node:fs";

import { Window } from "happy-dom";
import pdfMake from "pdfmake/build/pdfmake";
import { decodePDFRawStream, PDFArray, PDFDict, PDFDocument, PDFName, PDFRawStream } from "pdf-lib";
import { describe, expect, it, vi } from "vitest";

import { createReport, matchingItems, reportHtml, reportPdf } from "../../src/lib/report.js";
import type { Item, ListState } from "../../src/lib/types.js";

function item(id: number, mu = 25, tagIds: number[] = []): Item {
  return {
    id,
    listId: 77,
    name: `日本語の項目${String(id).padStart(3, "0")}`,
    image: { path: "/private/internal-image.png", sourceUrl: "https://private.invalid/source" },
    rating: { mu, sigma: 2.35 },
    comparisonCount: 7,
    tagIds,
  };
}

function list(items: Item[]): ListState {
  return {
    id: 77,
    name: "私の好きなもの",
    revision: 12345,
    comparisonWindow: [31, 99],
    items,
    tags: [
      { id: 10, listId: 77, name: "果物" },
      { id: 20, listId: 77, name: "赤色" },
    ],
    comparisonCount: 19,
    convergence: {
      converged: false,
      maxSigma: 8,
      maxRankSpan: 2,
      observedAnswers: 19,
      requiredAnswers: 20,
    },
  };
}

const png = `data:image/png;base64,${readFileSync(new URL("../../src-tauri/icons/32x32.png", import.meta.url)).toString("base64")}`;

function htmlDocument(html: string): Window["document"] {
  const window = new Window();
  window.document.write(html);
  return window.document;
}

function decodeStream(stream: PDFRawStream): string {
  return new TextDecoder().decode(decodePDFRawStream(stream).decode());
}

// Read the generated document's embedded Unicode maps and drawing operations.
// This checks actual saved bytes through an independent PDF parser.
function pageTexts(pdf: PDFDocument): string[] {
  return pdf.getPages().map((page) => {
    const fonts = page.node.Resources()?.lookup(PDFName.of("Font"), PDFDict);
    const maps = new Map<string, Map<number, string>>();
    for (const [name, reference] of fonts?.entries() ?? []) {
      const font = pdf.context.lookup(reference, PDFDict);
      const unicode = pdf.context.lookup(font.get(PDFName.of("ToUnicode")));
      if (!(unicode instanceof PDFRawStream)) throw new Error("Missing embedded Unicode map");
      const cmap = decodeStream(unicode);
      const glyphs = new Map<number, string>();
      for (const range of cmap.matchAll(/<([\da-f]+)>\s+<[\da-f]+>\s+\[([^\]]*)\]/gi)) {
        const start = Number.parseInt(range[1]!, 16);
        const characters = Array.from(range[2]!.matchAll(/<([\da-f\s]*)>/gi));
        characters.forEach((character, index) => {
          const hex = character[1]!.replace(/\s/g, "");
          const codes = hex.match(/.{4}/g) ?? [];
          glyphs.set(
            start + index,
            String.fromCharCode(...codes.map((code) => Number.parseInt(code, 16))),
          );
        });
      }
      maps.set(name.asString().slice(1), glyphs);
    }
    const contents = page.node.Contents();
    const streams =
      contents instanceof PDFArray
        ? contents.asArray().map((reference) => {
            const stream = pdf.context.lookup(reference);
            if (!(stream instanceof PDFRawStream)) throw new Error("Missing PDF page content");
            return stream;
          })
        : contents instanceof PDFRawStream
          ? [contents]
          : [];
    let fontName = "";
    let text = "";
    for (const stream of streams) {
      const operations = decodeStream(stream);
      for (const operation of operations.matchAll(
        /\/(F\d+)\s+[\d.]+\s+Tf|\[([^\]]*)\]\s+TJ|<([\da-f]+)>\s+Tj/gi,
      )) {
        if (operation[1]) {
          fontName = operation[1];
          continue;
        }
        const parts = operation[3]
          ? [operation[3]]
          : Array.from(operation[2]!.matchAll(/<([\da-f]+)>/gi), (part) => part[1]!);
        for (const part of parts) {
          for (const code of part.match(/.{4}/g) ?? []) {
            text += maps.get(fontName)?.get(Number.parseInt(code, 16)) ?? "\ufffd";
          }
        }
      }
    }
    return text;
  });
}

describe("report projection and HTML", () => {
  it("requires all tags and renumbers the authoritative incoming ranking order", () => {
    const state = list([
      item(8, 50, [10]),
      item(42, 40, [10, 20]),
      item(3, 40, [10, 20]),
      item(9, 0, [20]),
    ]);
    expect(matchingItems(state.items, [10, 20]).map((entry) => entry.id)).toEqual([42, 3]);
    const report = createReport(state, "ranking", [20, 10]);
    expect(report.entries.map(({ name, rank }) => [name, rank])).toEqual([
      [state.items[1]!.name, 1],
      [state.items[2]!.name, 2],
    ]);
    expect(report.selectedTagNames).toEqual(["果物", "赤色"]);
    expect(report.itemCount).toBe(4);
    expect(report.matchingCount).toBe(2);
    const document = htmlDocument(reportHtml(report));
    expect(Array.from(document.querySelectorAll("tbody th"), (node) => node.textContent)).toEqual([
      "1",
      "2",
    ]);
    expect(document.querySelector("tbody")?.textContent).toContain("40.00");
    expect(document.querySelector("tbody")?.textContent).toContain("2.35");
    expect(document.querySelector("tbody")?.textContent).toContain("7 回");
  });

  it("classifies tiers before filtering, preserves score boundaries, and omits tier rank labels", () => {
    const state = list([
      item(1, 50),
      item(2, 40, [10]),
      item(3, 30, [10]),
      item(4, 20, [10]),
      item(5, 10, [10]),
      item(6, 0),
    ]);
    const report = createReport(state, "tier", [10]);
    expect(
      report.tiers.map(({ label, entries }) => [label, entries.map((entry) => entry.mu)]),
    ).toEqual([
      ["S", []],
      ["A", [40]],
      ["B", [30]],
      ["C", [20]],
      ["D", [10]],
    ]);
    const document = htmlDocument(reportHtml(report));
    expect(Array.from(document.querySelectorAll(".tier h3"), (node) => node.textContent)).toEqual([
      "S",
      "A",
      "B",
      "C",
      "D",
    ]);
    expect(document.querySelectorAll(".tier-items li")).toHaveLength(4);
    expect(document.querySelectorAll(".rank-number,table")).toHaveLength(0);
  });

  it("keeps equally rated entries in C and snapshots values rather than retaining mutable items", () => {
    const state = list([item(1), item(2)]);
    const report = createReport(state, "tier", []);
    state.items[0]!.name = "変更後";
    state.items[0]!.rating.mu = 100;
    expect(report.entries[0]?.name).toBe("日本語の項目001");
    expect(report.entries[0]?.mu).toBe(25);
    expect(report.tiers[3]?.entries).toHaveLength(2);
    expect(report.tiers[0]?.entries).toHaveLength(0);
  });

  it("escapes every text field and embeds only opted-in PNG data without private references", () => {
    const unsafe =
      '</style><script>alert(1)</script><img src="https://evil.invalid" onerror="bad()">&';
    const state = list([item(1, 50, [10]), item(2, 20, [10])]);
    state.name = unsafe;
    state.items[0]!.name = unsafe;
    state.tags[0]!.name = unsafe;
    const report = createReport(state, "ranking", [10], { "1": png, "2": unsafe });
    const html = reportHtml(report);
    const document = htmlDocument(html);
    expect(document.querySelector("h1")?.textContent).toBe(unsafe);
    expect(document.querySelector("td.name")?.textContent).toBe(unsafe);
    expect(document.querySelector(".tags")?.textContent).toContain(unsafe);
    expect(document.querySelectorAll("script,link,iframe,object")).toHaveLength(0);
    expect(document.querySelectorAll("[onerror]")).toHaveLength(0);
    expect(document.querySelectorAll("img")).toHaveLength(1);
    expect(document.querySelector("img")?.getAttribute("src")).toBe(png);
    expect(
      document.querySelector('meta[http-equiv="Content-Security-Policy"]')?.getAttribute("content"),
    ).toBe("default-src 'none'; img-src data:; style-src 'unsafe-inline'");
    for (const privateValue of ["/private/internal-image.png", "private.invalid/source", "12345"]) {
      expect(html).not.toContain(privateValue);
      expect(JSON.stringify(report)).not.toContain(privateValue);
    }
    expect(
      createReport(state, "ranking", [10]).entries.every((entry) => entry.image === null),
    ).toBe(true);
  });

  it("renders empty lists and unmatched filters without table generation errors", () => {
    for (const view of ["ranking", "tier"] as const) {
      expect(
        htmlDocument(reportHtml(createReport(list([]), view, []))).querySelector(".empty")
          ?.textContent,
      ).toBe("項目はありません。");
      expect(
        htmlDocument(reportHtml(createReport(list([item(1)]), view, [10]))).querySelector(".empty")
          ?.textContent,
      ).toBe("選択したタグをすべて持つ項目はありません。");
    }
  });

  it("retains complete long names and all cards in standalone HTML", () => {
    const state = list(Array.from({ length: 90 }, (_, index) => item(index + 1)));
    state.items[0]!.name = "長い日本語の名前「折り返し」".repeat(300);
    const document = htmlDocument(reportHtml(createReport(state, "tier", [])));
    expect(document.querySelectorAll(".tier-items li")).toHaveLength(90);
    expect(document.querySelector(".tier-items .name")?.textContent).toBe(state.items[0]!.name);
  });
});

describe("saved PDF output", () => {
  it.each([
    {
      description: "unbroken Latin with Japanese and joined emoji",
      title: "VeryLongUnbrokenAsciiListTitle".repeat(12),
      tag: "VeryLongUnbrokenAsciiTagName".repeat(12),
      name: `${"VeryLongUnbrokenAsciiItemName".repeat(12)}𠮷🍎👩‍👩‍👧‍👦`,
    },
    {
      description: "consecutive non-BMP Japanese graphemes",
      title: `題名${"𠮷".repeat(120)}題名終`,
      tag: `タグ${"𠮷".repeat(120)}タグ終`,
      name: `項目${"𠮷".repeat(120)}項目終`,
    },
    {
      description: "consecutive emoji graphemes",
      title: `題名${"🍎".repeat(120)}題名終`,
      tag: `タグ${"🍎".repeat(120)}タグ終`,
      name: `項目${"🍎".repeat(120)}項目終`,
    },
    {
      description: "consecutive decomposed Latin graphemes",
      title: `題名${"e\u0301".repeat(180)}題名終`,
      tag: `タグ${"e\u0301".repeat(180)}タグ終`,
      name: `項目${"e\u0301".repeat(180)}項目終`,
    },
  ])(
    "wraps $description in titles, tags, and names in both views",
    async ({ title, tag, name }) => {
      type LayoutStream = {
        _pdfMakePages: {
          pageSize: { width: number };
          items: {
            type: string;
            item: { x: number; getWidth: () => number };
          }[];
        }[];
      };
      const state = list([item(1, 25, [10])]);
      state.name = title;
      state.tags[0]!.name = tag;
      state.items[0]!.name = name;
      const createPdf = vi.spyOn(pdfMake, "createPdf");
      try {
        for (const view of ["ranking", "tier"] as const) {
          const pdf = await PDFDocument.load(await reportPdf(createReport(state, view, [10])));
          const output = createPdf.mock.results[createPdf.mock.results.length - 1]!.value;
          const stream = (await output.getStream()) as unknown as LayoutStream;
          const lines = stream["_pdfMakePages"].flatMap((page) =>
            page.items
              .filter(({ type }) => type === "line")
              .map(({ item: line }) => ({
                left: line.x,
                right: line.x + line.getWidth(),
                pageWidth: page.pageSize.width,
              })),
          );
          expect(lines.length).toBeGreaterThan(20);
          for (const line of lines) {
            expect(line.left).toBeGreaterThanOrEqual(31.99);
            expect(line.right).toBeLessThanOrEqual(line.pageWidth - 32 + 0.01);
          }
          const text = pageTexts(pdf)
            .map((page) =>
              page
                .replace(/\d+ \/ \d+$/, "")
                .replace(/^Tier C/, "")
                .replace(/^順位画像項目評価σ比較/, ""),
            )
            .join("");
          expect(text).toContain(state.name);
          expect(text).toContain(state.tags[0]!.name);
          expect(text).toContain(state.items[0]!.name);
          expect(text).not.toContain("\ufffd");
        }
      } finally {
        createPdf.mockRestore();
      }
    },
  );

  it("creates a readable PDF with embedded Japanese text and opted-in managed PNG data", async () => {
    const state = list([item(1)]);
    state.items[0]!.name = "かな漢字髙𠮷";
    const bytes = await reportPdf(createReport(state, "ranking", [], { "1": png }));
    expect(new TextDecoder().decode(bytes.slice(0, 5))).toBe("%PDF-");
    const pdf = await PDFDocument.load(bytes);
    expect(pdf.getPageCount()).toBe(1);
    expect(pageTexts(pdf)[0]).toContain("かな漢字髙𠮷");
    expect(pageTexts(pdf)[0]).toContain("私の好きなもの");
    expect(pageTexts(pdf)[0]).toContain("2.35");
    const objects = pdf.context.enumerateIndirectObjects().map(([, object]) => object);
    expect(
      objects.some(
        (object) =>
          object instanceof PDFRawStream &&
          object.dict.get(PDFName.of("Subtype")) === PDFName.of("CIDFontType0C"),
      ),
    ).toBe(true);
    expect(
      objects.some(
        (object) =>
          object instanceof PDFRawStream &&
          object.dict.get(PDFName.of("Subtype")) === PDFName.of("Image"),
      ),
    ).toBe(true);
    expect(pdf.getTitle()).toBe("私の好きなもの — ランキング");
  });

  it("paginates a long ranking and repeats its table headings while retaining every item", async () => {
    const state = list(Array.from({ length: 160 }, (_, index) => item(index + 1)));
    const pdf = await PDFDocument.load(await reportPdf(createReport(state, "ranking", [])));
    const texts = pageTexts(pdf);
    expect(texts.length).toBeGreaterThan(3);
    for (const text of texts.slice(0, -1)) expect(text).toContain("順位画像項目評価σ比較");
    for (const entry of state.items) expect(texts.join("")).toContain(entry.name);
  });

  it("flows a large single tier across pages with repeated labels and preserves long Japanese names", async () => {
    const state = list(Array.from({ length: 100 }, (_, index) => item(index + 1)));
    state.items[0]!.name = "長い日本語の名前「折り返し」".repeat(80);
    const pdf = await PDFDocument.load(await reportPdf(createReport(state, "tier", [])));
    const texts = pageTexts(pdf);
    expect(texts.length).toBeGreaterThan(2);
    expect(texts.slice(1, -1).every((text) => text.includes("Tier C"))).toBe(true);
    expect(texts.join("")).not.toContain("項目（左ほど上位）");
    for (const entry of state.items.slice(1)) expect(texts.join("")).toContain(entry.name);
    expect(texts.join("")).toContain("折り返し");
    expect(texts.join("")).not.toContain("\ufffd");
  });

  it("exports empty filtered views as valid PDFs", async () => {
    for (const view of ["ranking", "tier"] as const) {
      const pdf = await PDFDocument.load(
        await reportPdf(createReport(list([item(1)]), view, [10])),
      );
      expect(pdf.getPageCount()).toBe(1);
      expect(pageTexts(pdf)[0]).toContain("選択したタグをすべて持つ項目はありません。");
    }
  });

  it("embeds monochrome emoji alongside Japanese and preserves a joined emoji sequence", async () => {
    const state = list([item(1, 25, [10])]);
    state.name = "果物🍎";
    state.tags[0]!.name = "りんご🍎";
    state.items[0]!.name = "りんご🍎と家族👩‍👩‍👧‍👦";
    const pdf = await PDFDocument.load(await reportPdf(createReport(state, "ranking", [10])));
    const text = pageTexts(pdf).join("");
    expect(text).toContain(state.name);
    expect(text).toContain(state.tags[0]!.name);
    expect(text).toContain(state.items[0]!.name);
    expect(text).not.toContain("\ufffd");
    const fonts = pdf.getPage(0).node.Resources()?.lookup(PDFName.of("Font"), PDFDict);
    expect(fonts?.entries()).toHaveLength(2);
  });

  it("fails explicitly for uncovered characters while preserving them in HTML", async () => {
    const state = list([item(1)]);
    state.items[0]!.name = "未対応\u{10ffff}";
    const report = createReport(state, "ranking", []);
    await expect(reportPdf(report)).rejects.toThrow("HTML形式でエクスポートしてください。");
    expect(htmlDocument(reportHtml(report)).querySelector(".name")?.textContent).toBe(
      state.items[0]!.name,
    );
  });

  it("generates Unicode PDFs without Intl.Segmenter or Array.prototype.at", async () => {
    const state = list([item(1, 25, [10])]);
    state.name = "互換性確認🍎";
    state.tags[0]!.name = "分類e\u0301";
    state.items[0]!.name = "日本語𠮷りんご🍎家族👩‍👩‍👧‍👦アクセントe\u0301";
    const segmenterDescriptor = Object.getOwnPropertyDescriptor(Intl, "Segmenter")!;
    const atDescriptor = Object.getOwnPropertyDescriptor(Array.prototype, "at")!;
    const documents: Uint8Array[] = [];
    let missingApis = false;
    vi.resetModules();
    Reflect.deleteProperty(Intl, "Segmenter");
    try {
      // Vitest's module loader needs .at; remove it after loading the renderer.
      const { renderPdf } = await import("../../src/lib/report-pdf.js");
      Reflect.deleteProperty(Array.prototype, "at");
      missingApis = !("Segmenter" in Intl) && !("at" in Array.prototype);
      for (const view of ["ranking", "tier"] as const) {
        documents.push(await renderPdf(createReport(state, view, [10])));
      }
    } finally {
      Object.defineProperty(Intl, "Segmenter", segmenterDescriptor);
      // oxlint-disable-next-line no-extend-native -- Restore the native method removed for this compatibility test.
      Object.defineProperty(Array.prototype, "at", atDescriptor);
      vi.resetModules();
    }
    expect(missingApis).toBe(true);
    for (const bytes of documents) {
      const pdf = await PDFDocument.load(bytes);
      const text = pageTexts(pdf).join("");
      expect(text).toContain(state.name);
      expect(text).toContain(state.tags[0]!.name);
      expect(text).toContain(state.items[0]!.name);
      expect(text).not.toContain("\ufffd");
    }
  });
});
