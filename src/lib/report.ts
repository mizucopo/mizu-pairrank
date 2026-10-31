import { escapeHtml } from "./html.js";
import { matchesSelectedTags } from "./tag-filter.js";
import { tierRows } from "./tier.js";
import type { TierRow } from "./tier.js";
import type { Item, ListState, ReportView } from "./types.js";

export type ReportEntry = {
  name: string;
  rank: number;
  image: string | null;
  mu: number;
  sigma: number;
  comparisonCount: number;
};

export type ReportTier = { label: TierRow["label"]; entries: ReportEntry[] };

export type Report = {
  title: string;
  comparisonCount: number;
  itemCount: number;
  matchingCount: number;
  view: ReportView;
  selectedTagNames: string[];
  entries: ReportEntry[];
  tiers: ReportTier[];
};

export const tierPalette: Record<TierRow["label"], { background: string; color: string }> = {
  S: { background: "#f5ebd8", color: "#8d682e" },
  A: { background: "#f7e1d6", color: "#9b4b2c" },
  B: { background: "#e4efdb", color: "#496c32" },
  C: { background: "#dcecf4", color: "#2b617a" },
  D: { background: "#e9e2f4", color: "#68508a" },
};

export function matchingItems(items: readonly Item[], selectedTagIds: readonly number[]): Item[] {
  return items.filter((item) => matchesSelectedTags(item.tagIds, selectedTagIds));
}

export function createReport(
  list: ListState,
  view: ReportView,
  selectedTagIds: readonly number[],
  images: Readonly<Record<string, string>> = {},
): Report {
  const visible = matchingItems(list.items, selectedTagIds);
  const entries = visible.map((item, index): ReportEntry => {
    const image = images[String(item.id)];
    return {
      name: item.name,
      rank: index + 1,
      // Export only native PNG data; local references and source URLs never enter a report.
      image: image && /^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/.test(image) ? image : null,
      mu: item.rating.mu,
      sigma: item.rating.sigma,
      comparisonCount: item.comparisonCount,
    };
  });
  const byId = new Map(visible.map((item, index) => [item.id, entries[index]!]));
  return {
    title: list.name,
    comparisonCount: list.comparisonCount,
    itemCount: list.items.length,
    matchingCount: entries.length,
    view,
    selectedTagNames: list.tags
      .filter((tag) => selectedTagIds.includes(tag.id))
      .map((tag) => tag.name),
    entries,
    // Classification always uses the full list's range, then applies the visible filter.
    tiers: tierRows(list.items).map(({ label, entries: tierEntries }) => ({
      label,
      entries: tierEntries.flatMap(({ item }) => {
        const entry = byId.get(item.id);
        return entry ? [entry] : [];
      }),
    })),
  };
}

const e = escapeHtml;
const fixed = (value: number): string => value.toFixed(2);

function picture(entry: ReportEntry): string {
  return entry.image
    ? `<img class="item-image" src="${e(entry.image)}" alt="${e(entry.name)}" />`
    : '<span class="item-image placeholder" aria-hidden="true">—</span>';
}

function rankingHtml(report: Report): string {
  return `<table class="ranking"><caption>あなたのランキング</caption><colgroup><col class="rank" /><col class="image" /><col /><col class="rating" /><col class="rating" /><col class="count" /></colgroup><thead><tr><th scope="col">順位</th><th scope="col">画像</th><th scope="col">項目</th><th scope="col">評価</th><th scope="col">σ</th><th scope="col">比較</th></tr></thead><tbody>${report.entries
    .map(
      (entry) =>
        `<tr><th scope="row">${entry.rank}</th><td>${picture(entry)}</td><td class="name">${e(entry.name)}</td><td>${fixed(entry.mu)}</td><td>${fixed(entry.sigma)}</td><td>${entry.comparisonCount} 回</td></tr>`,
    )
    .join(
      "",
    )}</tbody></table><p class="note">推定評価の高い順。評価が同じ場合は登録順です。タグ別の順位は表示項目で振り直しています。</p>`;
}

function tierHtml(report: Report): string {
  return `<h2>あなたの Tier 表</h2><div class="tiers">${report.tiers
    .map(
      ({ label, entries }) =>
        `<section class="tier"><h3 style="background:${tierPalette[label].background};color:${tierPalette[label].color}">${label}</h3>${
          entries.length
            ? `<ol class="tier-items">${entries.map((entry) => `<li>${picture(entry)}<span class="name">${e(entry.name)}</span></li>`).join("")}</ol>`
            : '<p class="tier-empty">項目なし</p>'
        }</section>`,
    )
    .join(
      "",
    )}</div><p class="note">リスト全体の評価の最高値から最低値までを5等分。各 Tier は左ほど上位です。全項目が同じ評価のときは C に表示します。</p>`;
}

export function reportHtml(report: Report): string {
  const title = report.view === "ranking" ? "ランキング" : "Tier 表";
  const tags = report.selectedTagNames.length
    ? `<p class="tags">選択したタグ: ${report.selectedTagNames.map(e).join(" / ")}（すべて一致）</p>`
    : "";
  let content: string;
  if (!report.matchingCount) {
    content = `<h2>あなたの${title}</h2><p class="empty">${report.selectedTagNames.length ? "選択したタグをすべて持つ項目はありません。" : "項目はありません。"}</p>`;
  } else {
    content = report.view === "ranking" ? rankingHtml(report) : tierHtml(report);
  }
  return `<!doctype html>
<html lang="ja">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'" />
<title>${e(report.title)} — ${title}</title>
<style>
*{box-sizing:border-box}body{margin:0;background:#faf9f5;color:#283126;font:14px/1.6 system-ui,sans-serif}main{max-width:1100px;margin:0 auto;padding:32px}h1{font-size:28px;line-height:1.4;margin:0 0 8px;overflow-wrap:anywhere;white-space:pre-wrap}h2,caption{font-size:20px;font-weight:600;text-align:left;margin:24px 0 12px}.summary,.tags,.note,.tier-empty{color:#67705f}.tags,.name{overflow-wrap:anywhere;white-space:pre-wrap}.note{font-size:12px}table{width:100%;border-collapse:collapse;table-layout:fixed;background:#fff}th,td{padding:10px 8px;border-bottom:1px solid #e0e4db;text-align:left;vertical-align:middle}thead th{background:#eef1e9}.rank{width:52px}.image{width:72px}.rating{width:70px}.count{width:76px}.item-image{display:block;width:52px;height:52px;object-fit:contain;background:#f0f1ed;border-radius:6px}.placeholder{display:grid;place-items:center;color:#899181}.tier{display:grid;grid-template-columns:58px minmax(0,1fr);border:1px solid #e0e4db;background:white}.tier+.tier{border-top:0}.tier h3{margin:0;padding:16px 4px;text-align:center;display:grid;place-items:center;font-size:20px}.tier-items{margin:0;padding:12px;list-style:none;display:grid;grid-template-columns:repeat(auto-fill,minmax(115px,1fr));gap:12px}.tier-items li{display:flex;flex-direction:column;align-items:center;gap:6px;text-align:center;min-width:0}.tier-empty{padding:12px}.empty{padding:20px;background:white}@media(max-width:600px){main{padding:16px}.rank{width:40px}.image{width:52px}.rating{width:54px}.count{width:52px}th,td{padding:6px 4px;font-size:12px}.item-image{width:40px;height:40px}}@media print{body{background:white}main{max-width:none;padding:0}thead{display:table-header-group}.tier-items li{break-inside:avoid}img{break-inside:avoid}}
</style>
</head>
<body><main><header><h1>${e(report.title)}</h1><p class="summary">${report.itemCount} 項目 · ${report.comparisonCount} 回の比較</p>${tags}<p class="summary">表示: ${report.matchingCount} 項目</p></header>${content}<p class="note">評価と「順位ほぼ確定」の判定は、タグの表示に関係なくリスト全体で行います。</p></main></body>
</html>`;
}

export async function reportPdf(report: Report): Promise<Uint8Array> {
  const { renderPdf } = await import("./report-pdf.js");
  return renderPdf(report);
}
