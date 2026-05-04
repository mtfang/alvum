export function voiceFilterSelectionValues(selected: Set<string> | null | undefined): string[] | undefined {
  return selected == null ? undefined : [...selected];
}

export function voiceFilterSummary(
  selectedSources: Set<string> | null | undefined,
  sourceCount: number,
  selectedPeople: Set<string> | null | undefined,
  peopleCount: number,
): string {
  const sourceText = voiceFilterSummaryText(selectedSources, sourceCount, 'All sources', 'No sources', 'sources');
  const peopleText = voiceFilterSummaryText(selectedPeople, peopleCount, 'All people', 'Unassigned', 'people');
  return peopleCount ? `${sourceText} · ${peopleText}` : sourceText;
}

export function voiceFilterSummaryText(
  selected: Set<string> | null | undefined,
  total: number,
  allText: string,
  noneText: string,
  unit: string,
): string {
  if (!total || selected == null) return allText;
  if (!selected.size) return noneText;
  return `${selected.size}/${total} ${unit}`;
}

export function toggleVoiceFilterSelection(
  selected: Set<string> | null | undefined,
  ids: string[],
  id: string,
): Set<string> | null {
  const allIds = Array.isArray(ids) ? ids.map(String) : [];
  const next = selected == null ? new Set(allIds) : new Set([...selected].filter((selectedId) => allIds.includes(selectedId)));
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next.size === allIds.length ? null : next;
}
