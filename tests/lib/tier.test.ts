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
      [1, 0.800001, 0.8000000000000002, 0.8, 0.6, 0.4, 0.2, 0].map((mu, index) =>
        item(index + 1, mu),
      ),
    );

    expect(rows.map((row) => row.entries.map(({ item: entry }) => entry.rating.mu))).toEqual([
      [1, 0.800001, 0.8000000000000002],
      [0.8],
      [0.6],
      [0.4],
      [0.2, 0],
    ]);

    const narrowRows = tierRows(
      [0.35, 0.280001, 0.28, 0.210001, 0.21, 0.140001, 0.14, 0.070001, 0.07, 0].map((mu, index) =>
        item(index + 1, mu),
      ),
    );
    expect(narrowRows.map((row) => row.entries.map(({ item: entry }) => entry.rating.mu))).toEqual([
      [0.35, 0.280001],
      [0.28, 0.210001],
      [0.21, 0.140001],
      [0.14, 0.070001],
      [0.07, 0],
    ]);

    const tinyRows = tierRows([item(1, 1e-8), item(2, 8e-9), item(3, 0)]);
    expect(tinyRows.map((row) => row.entries.map(({ item: entry }) => entry.id))).toEqual([
      [1],
      [2],
      [],
      [],
      [3],
    ]);
  });

  it("keeps the maximum in S when the rating range is only a few ULPs wide", () => {
    const rows = tierRows([item(1, 25.000000000000007), item(2, 25)]);

    expect(rows.map((row) => row.entries.map(({ item: entry }) => entry.id))).toEqual([
      [1],
      [],
      [],
      [],
      [2],
    ]);
  });

  it("keeps representable scores above a narrow boundary in the higher tier", () => {
    const ulp = 25.000000000000004 - 25;
    const rows = tierRows(
      [25 + 20 * ulp, 25 + 17 * ulp, 25 + 16 * ulp, 25].map((mu, index) => item(index + 1, mu)),
    );

    expect(rows.map((row) => row.entries.map(({ item: entry }) => entry.id))).toEqual([
      [1, 2],
      [3],
      [],
      [],
      [4],
    ]);
  });

  it("does not demote a score when its floating-point ratio rounds to a boundary", () => {
    const rows = tierRows(
      [33.550664760147896, 31.924467635513754, 25.41967913697718].map((mu, index) =>
        item(index + 1, mu),
      ),
    );

    expect(rows.map((row) => row.entries.map(({ item: entry }) => entry.id))).toEqual([
      [1, 2],
      [],
      [],
      [],
      [3],
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
