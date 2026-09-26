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

function binaryParts(value: number): { coefficient: bigint; exponent: number } {
  const bits = new DataView(new ArrayBuffer(8));
  bits.setFloat64(0, value);
  const raw = bits.getBigUint64(0);
  const negative = (raw & (1n << 63n)) !== 0n;
  const exponent = Number((raw >> 52n) & 0x7ffn);
  const fraction = raw & ((1n << 52n) - 1n);
  return {
    coefficient: (negative ? -1n : 1n) * (exponent === 0 ? fraction : fraction | (1n << 52n)),
    exponent: exponent === 0 ? -1074 : exponent - 1075,
  };
}

function alignedScores(
  parts: readonly { coefficient: bigint; exponent: number }[],
  radix: bigint,
): bigint[] {
  const smallestExponent = parts.reduce(
    (smallest, part) => Math.min(smallest, part.exponent),
    Infinity,
  );
  return parts.map(
    ({ coefficient, exponent }) => coefficient * radix ** BigInt(exponent - smallestExponent),
  );
}

export function tierRows(items: readonly Item[]): TierRow[] {
  const rows: TierRow[] = labels.map((label) => ({ label, entries: [] }));
  if (items.length === 0) return rows;

  const decimalValues = items.map((item) => decimalParts(item.rating.mu));
  const decimalScores = alignedScores(decimalValues, 10n);
  const binaryScores = alignedScores(
    items.map((item) => binaryParts(item.rating.mu)),
    2n,
  );
  const decimalLowest = decimalScores.reduce((current, score) =>
    score < current ? score : current,
  );
  const decimalHighest = decimalScores.reduce((current, score) =>
    score > current ? score : current,
  );
  const binaryLowest = binaryScores.reduce((current, score) => (score < current ? score : current));
  const binaryHighest = binaryScores.reduce((current, score) =>
    score > current ? score : current,
  );
  const decimalSpan = decimalHighest - decimalLowest;
  const binarySpan = binaryHighest - binaryLowest;
  items.forEach((item, index) => {
    let tier = binarySpan === 0n ? 3 : 0;
    if (binarySpan !== 0n) {
      const decimalPosition = (decimalScores[index]! - decimalLowest) * BigInt(labels.length);
      const binaryPosition = (binaryScores[index]! - binaryLowest) * BigInt(labels.length);
      for (let boundary = 1; boundary < labels.length; boundary += 1) {
        const lowerBands = labels.length - boundary;
        // Decimal JSON scores and their binary representations can round on opposite sides.
        if (
          decimalPosition <= decimalSpan * BigInt(lowerBands) ||
          binaryPosition <= binarySpan * BigInt(lowerBands)
        ) {
          tier = boundary;
        }
      }
    }
    rows[tier]?.entries.push({ item, rank: index + 1 });
  });
  return rows;
}
