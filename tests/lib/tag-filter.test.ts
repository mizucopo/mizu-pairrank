import { describe, expect, it } from "vitest";

import { matchesSelectedTags } from "../../src/lib/tag-filter.js";

describe("matchesSelectedTags", () => {
  it.each([
    { name: "no selection", itemTagIds: [10, 20], selectedTagIds: [], expected: true },
    { name: "no tags and no selection", itemTagIds: [], selectedTagIds: [], expected: true },
    { name: "single selection", itemTagIds: [10, 20], selectedTagIds: [20], expected: true },
    {
      name: "all selected tags",
      itemTagIds: [10, 20, 30],
      selectedTagIds: [20, 10],
      expected: true,
    },
    {
      name: "only some selected tags",
      itemTagIds: [10],
      selectedTagIds: [10, 20],
      expected: false,
    },
    { name: "tagless item", itemTagIds: [], selectedTagIds: [10], expected: false },
    { name: "no matching tags", itemTagIds: [10, 20], selectedTagIds: [30], expected: false },
  ])("matches $name: $expected", ({ itemTagIds, selectedTagIds, expected }) => {
    expect(matchesSelectedTags(itemTagIds, selectedTagIds)).toBe(expected);
  });
});
