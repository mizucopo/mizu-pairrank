import { describe, expect, it } from "vitest";

import { tierRows } from "../../src/lib/tier.js";
import type { Item } from "../../src/lib/types.js";

function item(id: number, mu: number): Item {
  return {
    id,
    listId: 1,
    name: `項目${id}`,
    image: null,
    rating: { mu, sigma: 8 },
    comparisonCount: 0,
  };
}

describe("tierRows", () => {
  it("places score-band boundaries in the lower tier", () => {
    const rows = tierRows(
      [50, 41, 40, 31, 30, 21, 20, 11, 10, 0].map((mu, index) => item(index + 1, mu)),
    );

    expect(rows.map((row) => row.label)).toEqual(["S", "A", "B", "C", "D"]);
    expect(rows.map((row) => row.entries.map(({ item: entry }) => entry.rating.mu))).toEqual([
      [50, 41],
      [40, 31],
      [30, 21],
      [20, 11],
      [10, 0],
    ]);
  });

  it("places fractional boundaries in the lower tier without moving nearby scores", () => {
    const rows = tierRows(
      [1, 0.800001, 0.8, 0.6, 0.4, 0.2, 0].map((mu, index) => item(index + 1, mu)),
    );

    expect(rows.map((row) => row.entries.map(({ item: entry }) => entry.rating.mu))).toEqual([
      [1, 0.800001],
      [0.8],
      [0.6],
      [0.4],
      [0.2, 0],
    ]);
  });

  it("keeps equal scores together and preserves the incoming ranking order and ranks", () => {
    const rows = tierRows([item(1, 50), item(42, 40), item(7, 40), item(2, 35), item(9, 0)]);

    expect(rows[1]?.entries.map(({ item: entry, rank }) => [entry.id, rank])).toEqual([
      [42, 2],
      [7, 3],
      [2, 4],
    ]);
  });

  it("puts all equally rated items in C", () => {
    const rows = tierRows([item(1, 25), item(2, 25)]);

    expect(rows.map((row) => row.entries.map(({ item: entry }) => entry.id))).toEqual([
      [],
      [],
      [],
      [1, 2],
      [],
    ]);
  });

  it("returns all five empty tiers for an empty list", () => {
    expect(tierRows([])).toEqual([
      { label: "S", entries: [] },
      { label: "A", entries: [] },
      { label: "B", entries: [] },
      { label: "C", entries: [] },
      { label: "D", entries: [] },
    ]);
  });
});
