import pdfMake from "pdfmake/build/pdfmake";
import type { Content, TableCell, TDocumentDefinitions } from "pdfmake/interfaces";
import { splitGraphemes } from "unicode-segmenter/grapheme";

import emojiDataUrl from "../assets/NotoEmoji.ttf?inline";
import emojiCoverage from "../assets/NotoEmoji.ttf.coverage.txt?raw";
import fontDataUrl from "../assets/NotoSansJP-Regular.otf?inline";
import fontCoverage from "../assets/NotoSansJP-Regular.otf.coverage.txt?raw";
import { tierPalette } from "./report.js";
import type { Report, ReportEntry } from "./report.js";

const fontName = "NotoSansJP";
const fontFile = "NotoSansJP-Regular.otf";
const emojiName = "NotoEmoji";
const emojiFile = "NotoEmoji.ttf";
const fontBase64 = fontDataUrl.slice(fontDataUrl.indexOf(",") + 1);
const emojiBase64 = emojiDataUrl.slice(emojiDataUrl.indexOf(",") + 1);
pdfMake.addVirtualFileSystem({ [fontFile]: fontBase64, [emojiFile]: emojiBase64 });
pdfMake.addFonts({
  [fontName]: { normal: fontFile, bold: fontFile, italics: fontFile, bolditalics: fontFile },
  [emojiName]: { normal: emojiFile, bold: emojiFile, italics: emojiFile, bolditalics: emojiFile },
});

function coverage(text: string): [number, number][] {
  return text
    .trim()
    .split(",")
    .map((range) => {
      const [start, end] = range.split("-");
      return [Number.parseInt(start!, 16), Number.parseInt(end!, 16)];
    });
}

const japaneseRanges = coverage(fontCoverage);
const emojiRanges = coverage(emojiCoverage);

function covered(code: number, ranges: readonly [number, number][]): boolean {
  let low = 0;
  let high = ranges.length - 1;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    const [start, end] = ranges[middle]!;
    if (code < start) high = middle - 1;
    else if (code > end) low = middle + 1;
    else return true;
  }
  return false;
}

type TextRun = {
  text: string;
  font: string;
  wordBreak: "normal" | "break-all";
  noWrap: boolean;
};

function textRuns(text: string): TextRun[] {
  const runs: TextRun[] = [];
  for (const segment of splitGraphemes(text)) {
    const points = Array.from(segment, (character) => character.codePointAt(0)!);
    const layoutControl = (code: number): boolean => [9, 10, 13].includes(code);
    const emojiControl = (code: number): boolean => [0x200d, 0xfe0e, 0xfe0f].includes(code);
    let font: string;
    if (points.every((code) => layoutControl(code) || covered(code, japaneseRanges))) {
      font = fontName;
    } else if (points.every((code) => emojiControl(code) || covered(code, emojiRanges))) {
      font = emojiName;
    } else {
      throw new Error(
        `PDFで表示できない文字があります: ${segment}。HTML形式でエクスポートしてください。`,
      );
    }
    // pdfmake's break-all and emergency wrapping split UTF-16 code units.
    // Only single-unit graphemes can use break-all; keep larger graphemes
    // indivisible so surrogate pairs, combining marks, and ZWJ emoji survive.
    const noWrap = segment.length > 1 && segment !== "\r\n";
    const wordBreak = noWrap || segment === "\r\n" ? "normal" : "break-all";
    const last = runs[runs.length - 1];
    if (!noWrap && last?.font === font && !last.noWrap && last.wordBreak === wordBreak) {
      last.text += segment;
    } else {
      runs.push({ text: segment, font, wordBreak, noWrap });
      if (noWrap) {
        // An empty inline stops pdfmake rejoining adjacent graphemes into an
        // unbreakable word. It adds no character to the saved PDF text.
        runs.push({ text: "", font, wordBreak: "normal", noWrap: false });
      }
    }
  }
  return runs;
}

function image(entry: ReportEntry): Content {
  return entry.image
    ? { image: entry.image, fit: [44, 44], alignment: "center" }
    : { text: "—", alignment: "center", color: "#899181" };
}

function ranking(report: Report): Content[] {
  return [
    { text: "あなたのランキング", style: "heading" },
    {
      table: {
        headerRows: 1,
        widths: [26, 46, "*", 42, 35, 39],
        body: [
          ["順位", "画像", "項目", "評価", "σ", "比較"].map((text) => ({
            text,
            fillColor: "#eef1e9",
          })),
          ...report.entries.map((entry): TableCell[] => [
            { text: String(entry.rank) },
            image(entry),
            { text: textRuns(entry.name) },
            { text: entry.mu.toFixed(2) },
            { text: entry.sigma.toFixed(2) },
            { text: `${entry.comparisonCount} 回` },
          ]),
        ],
      },
      layout: "lightHorizontalLines",
    },
    {
      text: "推定評価の高い順。評価が同じ場合は登録順です。タグ別の順位は表示項目で振り直しています。",
      style: "note",
    },
  ];
}

function tiers(report: Report): Content[] {
  const cardsPerRow = 4;
  const tables: Content[] = [];
  for (const tier of report.tiers) {
    const body: TableCell[][] = [];
    const label = (): TableCell => ({
      text: tier.label,
      fontSize: 16,
      alignment: "center",
      fillColor: tierPalette[tier.label].background,
      color: tierPalette[tier.label].color,
    });
    if (!tier.entries.length) {
      body.push([
        label(),
        { text: "項目なし", colSpan: cardsPerRow, color: "#67705f" },
        {},
        {},
        {},
      ]);
    }
    for (let offset = 0; offset < tier.entries.length; offset += cardsPerRow) {
      const cards: TableCell[] = tier.entries.slice(offset, offset + cardsPerRow).map((entry) => ({
        stack: [image(entry), { text: textRuns(entry.name), margin: [0, 5, 0, 3] }],
        alignment: "center",
      }));
      while (cards.length < cardsPerRow) cards.push({ text: "" });
      // Repeat the label on each physical row, so a large tier flows across pages.
      body.push([label(), ...cards]);
    }
    tables.push({
      table: {
        headerRows: 1,
        widths: [36, "*", "*", "*", "*"],
        body: [
          [
            {
              text: `Tier ${tier.label}`,
              fillColor: tierPalette[tier.label].background,
              color: tierPalette[tier.label].color,
            },
            { text: "", colSpan: 4, fillColor: "#eef1e9" },
            {},
            {},
            {},
          ],
          ...body,
        ],
      },
      layout: "lightHorizontalLines",
      margin: [0, 0, 0, 8],
    });
  }
  return [
    { text: "あなたの Tier 表", style: "heading" },
    ...tables,
    {
      text: "リスト全体の評価の最高値から最低値までを5等分。全項目が同じ評価のときは C に表示します。",
      style: "note",
    },
  ];
}

export async function renderPdf(report: Report): Promise<Uint8Array> {
  const content: Content[] = [
    { text: textRuns(report.title), fontSize: 20, margin: [0, 0, 0, 8] },
    { text: `${report.itemCount} 項目 · ${report.comparisonCount} 回の比較`, color: "#67705f" },
  ];
  if (report.selectedTagNames.length) {
    content.push({
      text: textRuns(`選択したタグ: ${report.selectedTagNames.join(" / ")}（すべて一致）`),
      margin: [0, 4, 0, 0],
    });
  }
  content.push({
    text: `表示: ${report.matchingCount} 項目`,
    margin: [0, 4, 0, 8],
    color: "#67705f",
  });
  if (report.matchingCount) {
    content.push(...(report.view === "ranking" ? ranking(report) : tiers(report)));
  } else {
    content.push({
      text: report.selectedTagNames.length
        ? "選択したタグをすべて持つ項目はありません。"
        : "項目はありません。",
      margin: [0, 12, 0, 12],
    });
  }
  content.push({
    text: "評価と「順位ほぼ確定」の判定は、タグの表示に関係なくリスト全体で行います。",
    style: "note",
  });
  const definition: TDocumentDefinitions = {
    info: { title: `${report.title} — ${report.view === "ranking" ? "ランキング" : "Tier 表"}` },
    pageSize: "A4",
    pageMargins: [32, 32, 32, 38],
    defaultStyle: { font: fontName, fontSize: 10, color: "#283126" },
    styles: {
      heading: { fontSize: 14, margin: [0, 10, 0, 8] },
      note: { fontSize: 8, color: "#67705f", margin: [0, 10, 0, 0] },
    },
    content,
    footer: (page, pages) => ({ text: `${page} / ${pages}`, alignment: "center", fontSize: 8 }),
  };
  return new Uint8Array(await pdfMake.createPdf(definition).getBuffer());
}
