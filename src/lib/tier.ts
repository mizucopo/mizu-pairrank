import type { Item } from "./types.js";

const labels = ["S", "A", "B", "C", "D"] as const;

export type TierRow = {
  label: (typeof labels)[number];
  entries: { item: Item; rank: number }[];
};

export function tierRows(items: readonly Item[]): TierRow[] {
  const rows: TierRow[] = labels.map((label) => ({ label, entries: [] }));
  let highest = -Infinity;
  let lowest = Infinity;
  for (const item of items) {
    highest = Math.max(highest, item.rating.mu);
    lowest = Math.min(lowest, item.rating.mu);
  }
  const span = highest - lowest;
  items.forEach((item, index) => {
    let tier = span === 0 ? 3 : 0;
    if (span !== 0) {
      for (let boundary = 1; boundary < labels.length; boundary += 1) {
        if (item.rating.mu <= lowest + (span * (labels.length - boundary)) / labels.length) {
          tier = boundary;
        }
      }
    }
    rows[tier]?.entries.push({ item, rank: index + 1 });
  });
  return rows;
}
