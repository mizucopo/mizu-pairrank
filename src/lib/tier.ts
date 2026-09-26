import type { Item } from "./types.js";

const labels = ["S", "A", "B", "C", "D"] as const;

export type TierRow = {
  label: (typeof labels)[number];
  entries: { item: Item; rank: number }[];
};

function decimalParts(value: number): { coefficient: bigint; exponent: number } {
  const [mantissa = "0", power = "0"] = value.toString().split("e");
  const places = mantissa.split(".")[1]?.length ?? 0;
  return {
    coefficient: BigInt(mantissa.replace(".", "")),
    exponent: Number(power) - places,
  };
}

export function tierRows(items: readonly Item[]): TierRow[] {
  const rows: TierRow[] = labels.map((label) => ({ label, entries: [] }));
  if (items.length === 0) return rows;

  let highest = -Infinity;
  let lowest = Infinity;
  let smallestExponent = Infinity;
  const decimalValues = items.map((item) => {
    highest = Math.max(highest, item.rating.mu);
    lowest = Math.min(lowest, item.rating.mu);
    const parts = decimalParts(item.rating.mu);
    smallestExponent = Math.min(smallestExponent, parts.exponent);
    return parts;
  });
  const scores = decimalValues.map(
    ({ coefficient, exponent }) => coefficient * 10n ** BigInt(exponent - smallestExponent),
  );
  const lowestScore = scores.reduce((current, score) => (score < current ? score : current));
  const highestScore = scores.reduce((current, score) => (score > current ? score : current));
  const decimalSpan = highestScore - lowestScore;
  const span = highest - lowest;
  items.forEach((item, index) => {
    let tier = span === 0 ? 3 : 0;
    if (span !== 0) {
      const decimalPosition = (scores[index]! - lowestScore) * BigInt(labels.length);
      const binaryPosition = (item.rating.mu - lowest) / span;
      for (let boundary = 1; boundary < labels.length; boundary += 1) {
        const lowerBands = labels.length - boundary;
        // Decimal JSON scores and their binary representations can round on opposite sides.
        if (
          decimalPosition <= decimalSpan * BigInt(lowerBands) ||
          binaryPosition <= lowerBands / labels.length
        ) {
          tier = boundary;
        }
      }
    }
    rows[tier]?.entries.push({ item, rank: index + 1 });
  });
  return rows;
}
