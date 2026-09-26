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
    const tier = span === 0 ? 3 : Math.min(4, Math.floor((5 * (highest - item.rating.mu)) / span));
    rows[tier]?.entries.push({ item, rank: index + 1 });
  });
  return rows;
}
