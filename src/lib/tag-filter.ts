export function matchesSelectedTags(
  itemTagIds: readonly number[],
  selectedTagIds: readonly number[],
): boolean {
  return selectedTagIds.every((id) => itemTagIds.includes(id));
}
