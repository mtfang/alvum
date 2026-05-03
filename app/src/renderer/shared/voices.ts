export interface VoiceGateSetting {
  key?: string;
  value?: unknown;
}

export interface VoiceGateProcessor {
  component?: string;
  settings?: VoiceGateSetting[];
}

export interface VoiceGateConnector {
  id?: string;
  component_id?: string;
  package_id?: string;
  connector_id?: string;
  processor_controls?: VoiceGateProcessor[];
}

export interface VoiceGateProfile {
  interests?: Array<Record<string, unknown>>;
}

export interface VoiceSampleLike {
  sample_id?: string;
  cluster_id?: string;
  text?: string;
  source?: string;
  ts?: string;
  start_secs?: number;
  end_secs?: number;
  media_path?: string | null;
  linked_interest_id?: string | null;
  linked_interest?: Record<string, unknown> | null;
  person_candidates?: Array<Record<string, unknown>>;
  quality_flags?: string[];
  context_interests?: Array<Record<string, unknown>>;
}

export interface VoiceGateSummary {
  visible: boolean;
  mode: string;
  diarizationReady: boolean;
  enabledPeople: number;
  pendingReviewCount: number;
  linkedPersonCount: number;
  recentEvidenceDay: string | null;
}

export interface VoiceTimeline {
  days: string[];
  defaultDay: string | null;
  selectedDay: string | null;
  sources: string[];
  people: Array<{ id: string; name: string; count: number }>;
  turns: VoiceSampleLike[];
  visibleTurns: VoiceSampleLike[];
  totalTurnCount: number;
  visibleStart: number;
  visibleLimit: number;
  hasMoreTurns: boolean;
  timeRange: { startMs: number; endMs: number; startLabel: string; endLabel: string } | null;
  audioSegments: Array<{
    startMs: number;
    endMs: number;
    startOffset: number;
    endOffset: number;
    label: string;
  }>;
  timeTicks: Array<{ label: string; offset: number }>;
  activitySpans: Array<{
    source: string;
    start_secs: number;
    end_secs: number;
    startMs: number;
    endMs: number;
    startOffset: number;
    endOffset: number;
  }>;
  scrubIndex: VoiceScrubIndexEntry[];
}

export interface VoiceScrubIndexEntry {
  sample_id: string;
  turnIndex: number;
  centerMs: number;
  offset: number;
}

export interface VoicePlaybackSample {
  source: string;
  sample: VoiceSampleLike;
  startMs: number;
  endMs: number;
  offsetSecs: number;
  audioEndSecs?: number;
}

export interface VoicePlaybackBlock {
  startMs: number;
  endMs: number;
  offset: number;
  samples: VoicePlaybackSample[];
  selectionSamples?: VoiceSampleLike[];
}

const ACTIVE_AUDIO_GAP_MS = 5 * 60 * 1000;

export function voiceGateSummary(
  connectorSummary: { connectors?: VoiceGateConnector[] } | null | undefined,
  synthesisProfile: VoiceGateProfile | null | undefined,
  samples: VoiceSampleLike[] = [],
): VoiceGateSummary {
  const settings = audioProcessorSettings(connectorSummary);
  const mode = stringSetting(settings, 'mode') || 'local';
  const diarizationReady = mode === 'provider'
    ? true
    : mode === 'local' && booleanSetting(settings, 'diarization_enabled', true);
  const people = enabledTrackedPeople(synthesisProfile);
  const reviewable = samples.filter((sample) => sample && !isIgnoredVoiceSample(sample));
  const linkedIds = new Set(
    reviewable
      .map((sample) => sample.linked_interest_id)
      .filter(Boolean)
      .map(String),
  );
  const recentEvidenceDay = reviewable
    .map(sampleDay)
    .filter(Boolean)
    .sort((a, b) => String(b).localeCompare(String(a)))[0] || null;
  return {
    visible: mode !== 'off' && diarizationReady && people.length > 0,
    mode,
    diarizationReady,
    enabledPeople: people.length,
    pendingReviewCount: reviewable.filter((sample) => !sample.linked_interest_id).length,
    linkedPersonCount: linkedIds.size,
    recentEvidenceDay,
  };
}

export function enabledTrackedPeople(profile: VoiceGateProfile | null | undefined): Array<Record<string, unknown>> {
  return (Array.isArray(profile && profile.interests) ? profile!.interests! : [])
    .filter((interest) => interest && interest.enabled !== false)
    .filter((interest) => String(interest.type || interest.interest_type || '').toLowerCase() === 'person');
}

export function buildVoiceTimeline(samples: VoiceSampleLike[], options: {
  selectedDay?: string | null;
  selectedSources?: string[];
  selectedPeople?: string[];
  visibleStart?: number;
  visibleLimit?: number;
} = {}): VoiceTimeline {
  const reviewable = (Array.isArray(samples) ? samples : [])
    .filter((sample) => sample && !isIgnoredVoiceSample(sample));
  const days = uniqueSorted(
    reviewable.map(sampleDay).filter(Boolean) as string[],
    (a, b) => b.localeCompare(a),
  );
  const sources = uniqueSorted(
    reviewable.map((sample) => sample.source).filter(Boolean).map(String),
    (a, b) => a.localeCompare(b),
  );
  const people = timelinePeople(reviewable);
  const defaultDay = days[0] || null;
  const requestedDay = typeof options.selectedDay === 'string' && options.selectedDay.trim()
    ? options.selectedDay.trim()
    : null;
  const selectedDay = requestedDay || defaultDay;
  const sourceFilterActive = Array.isArray(options.selectedSources);
  const selectedSources = sourceFilterActive
    ? new Set(options.selectedSources!.map(String))
    : null;
  const peopleFilterActive = Array.isArray(options.selectedPeople);
  const selectedPeople = peopleFilterActive
    ? new Set(options.selectedPeople!.map(String))
    : null;
  const turns = reviewable
    .filter((sample) => !selectedDay || sampleDay(sample) === selectedDay)
    .filter((sample) => !sourceFilterActive || selectedSources!.has(String(sample.source || '')))
    .filter((sample) => !peopleFilterActive || sampleMatchesPeopleFilter(sample, selectedPeople!))
    .slice()
    .sort((a, b) =>
      String(a.ts || '').localeCompare(String(b.ts || ''))
      || Number(a.start_secs || 0) - Number(b.start_secs || 0)
      || String(a.sample_id || '').localeCompare(String(b.sample_id || '')));
  const visibleLimit = Number.isFinite(Number(options.visibleLimit)) && Number(options.visibleLimit) > 0
    ? Math.floor(Number(options.visibleLimit))
    : turns.length;
  const visibleWindow = voiceTimelineVisibleWindow(turns, options.visibleStart, visibleLimit);
  const timeRange = timelineTimeRange(turns);
  const audioSegments = timelineAudioSegments(turns);
  const scrubIndex = timelineScrubIndex(turns, timeRange, audioSegments);
  return {
    days,
    defaultDay,
    selectedDay,
    sources,
    people,
    turns,
    visibleTurns: visibleWindow.visibleTurns,
    totalTurnCount: turns.length,
    visibleStart: visibleWindow.visibleStart,
    visibleLimit: visibleWindow.visibleLimit,
    hasMoreTurns: visibleWindow.hasMoreTurns,
    timeRange,
    audioSegments,
    timeTicks: timelineTicks(timeRange, audioSegments),
    activitySpans: timelineActivitySpans(turns, timeRange, audioSegments),
    scrubIndex,
  };
}

export function voiceTimelineVisibleWindow(
  turns: VoiceSampleLike[],
  visibleStart: unknown,
  visibleLimit: unknown,
): Pick<VoiceTimeline, 'visibleTurns' | 'visibleStart' | 'visibleLimit' | 'hasMoreTurns'> {
  const total = Array.isArray(turns) ? turns.length : 0;
  const limit = Number.isFinite(Number(visibleLimit)) && Number(visibleLimit) > 0
    ? Math.floor(Number(visibleLimit))
    : total;
  const maxStart = Math.max(0, total - limit);
  const start = Math.max(0, Math.min(maxStart, Math.floor(Number(visibleStart) || 0)));
  const visibleTurns = turns.slice(start, start + limit);
  return {
    visibleTurns,
    visibleStart: start,
    visibleLimit: limit,
    hasMoreTurns: start + visibleTurns.length < total,
  };
}

export function voiceTimelineVisibleStartForIndex(
  sampleIndex: unknown,
  totalTurnCount: unknown,
  visibleLimit: unknown,
): number {
  const total = Math.max(0, Math.floor(Number(totalTurnCount) || 0));
  if (!total) return 0;
  const limit = Number.isFinite(Number(visibleLimit)) && Number(visibleLimit) > 0
    ? Math.floor(Number(visibleLimit))
    : total;
  const index = Math.max(0, Math.min(total - 1, Math.floor(Number(sampleIndex) || 0)));
  const maxStart = Math.max(0, total - limit);
  return Math.max(0, Math.min(maxStart, index - Math.floor(limit / 2)));
}

export function nearestVoiceTimelineSample(
  timeline: VoiceTimeline | null | undefined,
  offset: number,
): VoiceSampleLike | null {
  const index = Array.isArray(timeline && timeline.scrubIndex) ? timeline!.scrubIndex : [];
  if (!timeline || !index.length) return null;
  const target = clamp01(Number(offset));
  let low = 0;
  let high = index.length - 1;
  while (low < high) {
    const mid = Math.floor((low + high) / 2);
    if (index[mid].offset < target) low = mid + 1;
    else high = mid;
  }
  const right = index[low];
  const left = index[Math.max(0, low - 1)];
  const chosen = Math.abs((left ? left.offset : right.offset) - target) <= Math.abs(right.offset - target)
    ? left
    : right;
  return timeline.turns[chosen.turnIndex] || null;
}

export function voiceTimelinePlaybackBlock(
  timeline: VoiceTimeline | null | undefined,
  offset: number,
): VoicePlaybackBlock {
  const empty = { startMs: Number.NaN, endMs: Number.NaN, offset: clamp01(Number(offset)), samples: [] };
  if (!timeline || !timeline.timeRange || !Array.isArray(timeline.turns) || !timeline.turns.length) return empty;
  const ranges = timeline.turns
    .map((sample) => {
      const source = String(sample && sample.source || '');
      const startMs = sampleInstantMs(sample, Number(sample && sample.start_secs || 0));
      const endMs = sampleInstantMs(sample, Number(sample && sample.end_secs || sample && sample.start_secs || 0));
      if (!source || !Number.isFinite(startMs) || !Number.isFinite(endMs) || endMs <= startMs) return null;
      return { source, sample, startMs, endMs };
    })
    .filter(Boolean)
    .sort((a, b) => a!.startMs - b!.startMs || a!.endMs - b!.endMs) as Array<{
      source: string;
      sample: VoiceSampleLike;
      startMs: number;
      endMs: number;
    }>;
  if (!ranges.length) return empty;

  const requestedMs = timelineMsAtOffset(timeline, offset);
  const targetMs = ranges.some((range) => range.startMs <= requestedMs && range.endMs > requestedMs)
    ? requestedMs
    : Math.min(...ranges.filter((range) => range.startMs >= requestedMs).map((range) => range.startMs));
  if (!Number.isFinite(targetMs)) return empty;

  const activeBySource = new Map<string, typeof ranges[number]>();
  for (const range of ranges) {
    if (range.startMs > targetMs || range.endMs <= targetMs) continue;
    const previous = activeBySource.get(range.source);
    if (!previous || range.startMs > previous.startMs || (range.startMs === previous.startMs && range.endMs < previous.endMs)) {
      activeBySource.set(range.source, range);
    }
  }
  const samples = [...activeBySource.values()]
    .sort((a, b) => a.source.localeCompare(b.source))
    .map((range) => ({
      source: range.source,
      sample: range.sample,
      startMs: range.startMs,
      endMs: range.endMs,
      offsetSecs: Math.max(0, (targetMs - range.startMs) / 1000),
    }));
  if (!samples.length) return empty;
  const endMs = Math.min(...samples.map((sample) => sample.endMs));
  return {
    startMs: targetMs,
    endMs: Math.max(targetMs, endMs),
    offset: timelineOffsetForMs(targetMs, timeline.audioSegments),
    samples,
  };
}

export function voiceTimelineContinuousPlaybackBlock(
  timeline: VoiceTimeline | null | undefined,
  offset: number,
): VoicePlaybackBlock {
  const block = voiceTimelinePlaybackBlock(timeline, offset);
  if (!timeline || !Array.isArray(timeline.turns) || !timeline.turns.length || !block.samples.length) return block;
  const selectionSamples = new Map<string, VoiceSampleLike>();
  const samples = block.samples.map((entry) => {
    const mediaKey = voicePlaybackMediaKey(entry.sample);
    const source = String(entry.source || '');
    if (!mediaKey || !source) return entry;
    const related = timeline.turns
      .map((sample) => voicePlaybackMediaRange(sample))
      .filter((range): range is NonNullable<typeof range> =>
        !!range
        && range.source === source
        && range.mediaKey === mediaKey
        && range.endMs > block.startMs)
      .sort((a, b) => a.startMs - b.startMs || a.endMs - b.endMs);
    if (!related.length) return entry;
    for (const range of related) {
      if (range.sample.sample_id) selectionSamples.set(String(range.sample.sample_id), range.sample);
    }
    const endMs = Math.max(entry.endMs, ...related.map((range) => range.endMs));
    const audioEndSecs = Math.max(
      Number(entry.sample.end_secs || 0),
      ...related.map((range) => range.endSecs).filter(Number.isFinite),
    );
    return {
      ...entry,
      endMs,
      audioEndSecs,
    };
  });
  const finiteEndMs = samples
    .map((sample) => sample.endMs)
    .filter(Number.isFinite);
  return {
    ...block,
    endMs: finiteEndMs.length ? Math.max(block.startMs, Math.min(...finiteEndMs)) : block.endMs,
    samples,
    selectionSamples: [...selectionSamples.values()].sort((a, b) =>
      voiceSampleCenterMs(a) - voiceSampleCenterMs(b)
      || String(a.sample_id || '').localeCompare(String(b.sample_id || ''))),
  };
}

export function voicePlaybackSampleForPosition(
  block: VoicePlaybackBlock | null | undefined,
  ms: number,
): VoiceSampleLike | null {
  if (!block || !Number.isFinite(ms)) return null;
  const selectionSamples = Array.isArray(block.selectionSamples) ? block.selectionSamples : [];
  if (selectionSamples.length) {
    const selectionEntries = selectionSamples
      .map((sample) => ({
        sample,
        startMs: sampleInstantMs(sample, Number(sample && sample.start_secs || 0)),
        endMs: sampleInstantMs(sample, Number(sample && sample.end_secs || sample && sample.start_secs || 0)),
      }))
      .filter((entry) => Number.isFinite(entry.startMs) && Number.isFinite(entry.endMs));
    const selected = voicePlaybackEntryForPosition(selectionEntries, ms);
    if (selected) return selected.sample;
  }
  const entries = Array.isArray(block.samples)
    ? block.samples
      .map((entry) => ({
        sample: entry.sample,
        startMs: entry.startMs,
        endMs: entry.endMs,
      }))
      .filter((entry) => entry.sample && Number.isFinite(entry.startMs) && Number.isFinite(entry.endMs))
    : [];
  const selected = voicePlaybackEntryForPosition(entries, ms);
  return selected ? selected.sample : null;
}

export function voiceTimelinePlaybackStepBlock(
  timeline: VoiceTimeline | null | undefined,
  offset: number,
  direction: number,
): VoicePlaybackBlock {
  const empty = { startMs: Number.NaN, endMs: Number.NaN, offset: clamp01(Number(offset)), samples: [] };
  if (!timeline || !timeline.timeRange || !Array.isArray(timeline.turns) || !timeline.turns.length) return empty;
  const boundaries = voiceTimelinePlaybackBoundaryMs(timeline);
  if (!boundaries.length) return empty;
  const currentMs = timelineMsAtOffset(timeline, offset);
  const currentBlock = voiceTimelinePlaybackBlock(timeline, offset);
  const targetMs = direction < 0
    ? previousVoiceTimelinePlaybackBoundaryMs(boundaries, currentBlock, currentMs)
    : nextVoiceTimelinePlaybackBoundaryMs(boundaries, currentBlock, currentMs);
  if (!Number.isFinite(targetMs)) return empty;
  return voiceTimelinePlaybackBlock(timeline, timelineOffsetForMs(targetMs, timeline.audioSegments));
}

export function voiceTimelineActionsForSample(
  sample: VoiceSampleLike,
  interests: Array<Record<string, unknown>>,
) {
  const assignmentTargets = enabledTrackedPeople({ interests });
  return {
    canAssign: assignmentTargets.length > 0,
    assignmentTargets,
    contextEvidence: Array.isArray(sample && sample.context_interests) ? sample.context_interests! : [],
  };
}

export function sampleDay(sample: VoiceSampleLike): string | null {
  const ts = String(sample && sample.ts || '');
  const parsed = new Date(ts);
  if (!Number.isNaN(parsed.getTime())) {
    return [
      parsed.getFullYear(),
      String(parsed.getMonth() + 1).padStart(2, '0'),
      String(parsed.getDate()).padStart(2, '0'),
    ].join('-');
  }
  const match = ts.match(/^(\d{4}-\d{2}-\d{2})/);
  return match ? match[1] : null;
}

export function isIgnoredVoiceSample(sample: VoiceSampleLike): boolean {
  return !!(sample && Array.isArray(sample.quality_flags) && sample.quality_flags.includes('ignored_by_user'));
}

function audioProcessorSettings(
  connectorSummary: { connectors?: VoiceGateConnector[] } | null | undefined,
): VoiceGateSetting[] {
  const connectors = Array.isArray(connectorSummary && connectorSummary.connectors)
    ? connectorSummary!.connectors!
    : [];
  const audio = connectors.find((connector) => connector && (
    connector.component_id === 'alvum.audio/audio'
    || (connector.package_id === 'alvum.audio' && connector.connector_id === 'audio')
    || connector.id === 'alvum.audio/audio'
  ));
  const processors = audio && Array.isArray(audio.processor_controls) ? audio.processor_controls : [];
  const processor = processors.find((control) => control && control.component === 'alvum.audio/whisper');
  return processor && Array.isArray(processor.settings) ? processor.settings : [];
}

function stringSetting(settings: VoiceGateSetting[], key: string): string {
  const setting = settings.find((item) => item && item.key === key);
  if (!setting || setting.value == null) return '';
  return String(setting.value || '').trim();
}

function booleanSetting(settings: VoiceGateSetting[], key: string, fallback: boolean): boolean {
  const setting = settings.find((item) => item && item.key === key);
  if (!setting || setting.value == null) return fallback;
  if (typeof setting.value === 'boolean') return setting.value;
  const normalized = String(setting.value).trim().toLowerCase();
  if (normalized === 'true' || normalized === '1' || normalized === 'on' || normalized === 'yes') return true;
  if (normalized === 'false' || normalized === '0' || normalized === 'off' || normalized === 'no') return false;
  return fallback;
}

function uniqueSorted(values: string[], sort: (a: string, b: string) => number): string[] {
  return [...new Set(values)].sort(sort);
}

function timelinePeople(samples: VoiceSampleLike[]): VoiceTimeline['people'] {
  const byId = new Map<string, { id: string; name: string; count: number }>();
  for (const sample of samples) {
    for (const person of samplePersonEntries(sample)) {
      const current = byId.get(person.id) || { ...person, count: 0 };
      current.count += 1;
      if (current.name === current.id && person.name !== person.id) current.name = person.name;
      byId.set(person.id, current);
    }
  }
  return [...byId.values()].sort((a, b) =>
    a.name.localeCompare(b.name)
    || a.id.localeCompare(b.id));
}

function samplePersonIds(sample: VoiceSampleLike): string[] {
  return samplePersonEntries(sample).map((person) => person.id);
}

function sampleMatchesPeopleFilter(sample: VoiceSampleLike, selectedPeople: Set<string>): boolean {
  if (!selectedPeople.size) return !sampleLinkedPersonId(sample);
  return samplePersonIds(sample).some((id) => selectedPeople.has(id));
}

function sampleLinkedPersonId(sample: VoiceSampleLike): string {
  return String(sample && sample.linked_interest_id || sample && sample.linked_interest && sample.linked_interest.id || '').trim();
}

function samplePersonEntries(sample: VoiceSampleLike): Array<{ id: string; name: string }> {
  const linkedId = sampleLinkedPersonId(sample);
  if (linkedId) {
    const linked = sample && sample.linked_interest;
    const name = String(linked && linked.name || linkedId).trim() || linkedId;
    return [{ id: linkedId, name }];
  }
  const seen = new Set<string>();
  return (Array.isArray(sample && sample.person_candidates) ? sample.person_candidates! : [])
    .map((candidate) => {
      const id = String(candidate && candidate.id || '').trim();
      if (!id || seen.has(id)) return null;
      seen.add(id);
      return { id, name: String(candidate && candidate.name || id).trim() || id };
    })
    .filter(Boolean) as Array<{ id: string; name: string }>;
}

function timelineTimeRange(turns: VoiceSampleLike[]): VoiceTimeline['timeRange'] {
  if (!turns.length) return null;
  const starts = turns
    .map((sample) => sampleInstantMs(sample, Number(sample.start_secs || 0)))
    .filter(Number.isFinite) as number[];
  const ends = turns
    .map((sample) => sampleInstantMs(sample, Number(sample.end_secs || sample.start_secs || 0)))
    .filter(Number.isFinite) as number[];
  if (!starts.length || !ends.length) return null;
  const start = Math.min(...starts);
  const end = Math.max(...ends);
  return {
    startMs: start,
    endMs: Math.max(start, end),
    startLabel: clockLabel(new Date(start)),
    endLabel: clockLabel(new Date(Math.max(start, end))),
  };
}

function timelineTicks(
  timeRange: VoiceTimeline['timeRange'],
  audioSegments: VoiceTimeline['audioSegments'],
): Array<{ label: string; offset: number }> {
  if (audioSegments.length > 1) {
    return audioSegments.map((segment) => ({
      label: segment.label,
      offset: segment.startOffset,
    }));
  }
  if (!timeRange) return [];
  const range = Math.max(1, timeRange.endMs - timeRange.startMs);
  return Array.from({ length: 6 }, (_, index) => {
    const offset = index / 5;
    return {
      label: clockLabel(new Date(timeRange.startMs + range * offset)),
      offset,
    };
  });
}

function timelineActivitySpans(
  turns: VoiceSampleLike[],
  timeRange: VoiceTimeline['timeRange'],
  audioSegments: VoiceTimeline['audioSegments'],
): VoiceTimeline['activitySpans'] {
  if (!timeRange || !audioSegments.length) return [];
  const spans = turns
    .filter((sample) => sample.source && Number.isFinite(Number(sample.start_secs)) && Number.isFinite(Number(sample.end_secs)))
    .map((sample) => {
      const startSecs = Number(sample.start_secs);
      const endSecs = Number(sample.end_secs);
      const startMs = sampleInstantMs(sample, startSecs);
      const endMs = sampleInstantMs(sample, endSecs);
      if (!Number.isFinite(startMs) || !Number.isFinite(endMs)) return null;
      const startOffset = timelineOffsetForMs(startMs, audioSegments);
      const endOffset = timelineOffsetForMs(Math.max(startMs, endMs), audioSegments);
      return {
        source: String(sample.source),
        start_secs: startSecs,
        end_secs: endSecs,
        startMs,
        endMs: Math.max(startMs, endMs),
        startOffset,
        endOffset,
      };
    })
    .filter(Boolean) as VoiceTimeline['activitySpans'];
  return mergeActivitySpans(spans);
}

function timelineScrubIndex(
  turns: VoiceSampleLike[],
  timeRange: VoiceTimeline['timeRange'],
  audioSegments: VoiceTimeline['audioSegments'],
): VoiceScrubIndexEntry[] {
  if (!timeRange) return [];
  return turns
    .map((sample, turnIndex) => {
      const centerMs = voiceSampleCenterMs(sample);
      const sampleId = String(sample.sample_id || '');
      if (!sampleId || !Number.isFinite(centerMs)) return null;
      return {
        sample_id: sampleId,
        turnIndex,
        centerMs,
        offset: timelineOffsetForMs(centerMs, audioSegments),
      };
    })
    .filter(Boolean)
    .sort((a, b) => a!.offset - b!.offset || a!.centerMs - b!.centerMs) as VoiceScrubIndexEntry[];
}

function timelineAudioSegments(turns: VoiceSampleLike[]): VoiceTimeline['audioSegments'] {
  const intervals = turns
    .map((sample) => {
      const startMs = sampleInstantMs(sample, Number(sample.start_secs || 0));
      const endMs = sampleInstantMs(sample, Number(sample.end_secs || sample.start_secs || 0));
      if (!Number.isFinite(startMs) || !Number.isFinite(endMs)) return null;
      return { startMs, endMs: Math.max(startMs + 1, endMs) };
    })
    .filter(Boolean)
    .sort((a, b) => a!.startMs - b!.startMs) as Array<{ startMs: number; endMs: number }>;
  if (!intervals.length) return [];
  const merged: Array<{ startMs: number; endMs: number }> = [];
  for (const interval of intervals) {
    const previous = merged[merged.length - 1];
    if (previous && interval.startMs - previous.endMs <= ACTIVE_AUDIO_GAP_MS) {
      previous.endMs = Math.max(previous.endMs, interval.endMs);
    } else {
      merged.push({ ...interval });
    }
  }
  const total = Math.max(1, merged.reduce((sum, segment) => sum + Math.max(1, segment.endMs - segment.startMs), 0));
  let cursor = 0;
  return merged.map((segment) => {
    const duration = Math.max(1, segment.endMs - segment.startMs);
    const startOffset = cursor / total;
    cursor += duration;
    return {
      startMs: segment.startMs,
      endMs: segment.endMs,
      startOffset,
      endOffset: cursor / total,
      label: `${clockLabel(new Date(segment.startMs))}-${clockLabel(new Date(segment.endMs))}`,
    };
  });
}

function voiceSampleCenterMs(sample: VoiceSampleLike): number {
  const startMs = sampleInstantMs(sample, Number(sample && sample.start_secs || 0));
  const endMs = sampleInstantMs(sample, Number(sample && sample.end_secs || sample && sample.start_secs || 0));
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs)) return Number.NaN;
  return (startMs + Math.max(startMs, endMs)) / 2;
}

function voicePlaybackMediaKey(sample: VoiceSampleLike): string {
  const source = String(sample && sample.source || '');
  const media = String(sample && (sample.media_path || sample.ts) || '');
  return source && media ? `${source}\0${media}` : '';
}

function voicePlaybackMediaRange(sample: VoiceSampleLike): {
  source: string;
  mediaKey: string;
  sample: VoiceSampleLike;
  startMs: number;
  endMs: number;
  endSecs: number;
} | null {
  const source = String(sample && sample.source || '');
  const mediaKey = voicePlaybackMediaKey(sample);
  const startSecs = Number(sample && sample.start_secs || 0);
  const endSecs = Number(sample && sample.end_secs || sample && sample.start_secs || 0);
  const startMs = sampleInstantMs(sample, startSecs);
  const endMs = sampleInstantMs(sample, endSecs);
  if (!source || !mediaKey || !Number.isFinite(startMs) || !Number.isFinite(endMs) || endMs <= startMs) return null;
  return { source, mediaKey, sample, startMs, endMs, endSecs };
}

function voiceTimelinePlaybackBoundaryMs(timeline: VoiceTimeline): number[] {
  const boundaries = new Set<number>();
  for (const sample of timeline.turns) {
    const startMs = sampleInstantMs(sample, Number(sample && sample.start_secs || 0));
    const endMs = sampleInstantMs(sample, Number(sample && sample.end_secs || sample && sample.start_secs || 0));
    if (Number.isFinite(startMs)) boundaries.add(Math.round(startMs));
    if (Number.isFinite(endMs) && endMs > startMs) boundaries.add(Math.round(endMs));
  }
  return [...boundaries].sort((a, b) => a - b);
}

function voicePlaybackEntryForPosition<T extends {
  sample: VoiceSampleLike;
  startMs: number;
  endMs: number;
}>(entries: T[], ms: number): T | null {
  const sorted = entries
    .filter((entry) => entry.sample && Number.isFinite(entry.startMs) && Number.isFinite(entry.endMs))
    .sort((a, b) =>
      a.startMs - b.startMs
      || a.endMs - b.endMs
      || String(a.sample.sample_id || '').localeCompare(String(b.sample.sample_id || '')));
  if (!sorted.length) return null;
  const active = sorted.filter((entry) => entry.startMs <= ms && entry.endMs > ms);
  if (active.length) {
    return active
      .slice()
      .sort((a, b) =>
        Math.abs(entryCenterMs(a) - ms) - Math.abs(entryCenterMs(b) - ms)
        || String(a.sample.sample_id || '').localeCompare(String(b.sample.sample_id || '')))[0];
  }
  const previous = sorted
    .filter((entry) => entry.startMs <= ms)
    .sort((a, b) =>
      b.startMs - a.startMs
      || b.endMs - a.endMs
      || String(a.sample.sample_id || '').localeCompare(String(b.sample.sample_id || '')))[0];
  return previous || sorted[0];
}

function entryCenterMs(entry: { startMs: number; endMs: number }): number {
  return (entry.startMs + Math.max(entry.startMs, entry.endMs)) / 2;
}

function previousVoiceTimelinePlaybackBoundaryMs(
  boundaries: number[],
  currentBlock: VoicePlaybackBlock,
  currentMs: number,
): number {
  if (currentBlock.samples.length && currentMs - currentBlock.startMs > 1500) return currentBlock.startMs;
  const before = currentBlock.samples.length ? currentBlock.startMs : currentMs;
  for (let i = boundaries.length - 1; i >= 0; i -= 1) {
    if (boundaries[i] < before - 1) return boundaries[i];
  }
  return boundaries[0];
}

function nextVoiceTimelinePlaybackBoundaryMs(
  boundaries: number[],
  currentBlock: VoicePlaybackBlock,
  currentMs: number,
): number {
  const after = currentBlock.samples.length ? Math.max(currentBlock.startMs, currentMs) : currentMs;
  for (const boundary of boundaries) {
    if (boundary > after + 1) return boundary;
  }
  return Number.NaN;
}

function mergeActivitySpans(spans: VoiceTimeline['activitySpans']): VoiceTimeline['activitySpans'] {
  const sorted = spans.slice().sort((a, b) =>
    String(a.source).localeCompare(String(b.source))
    || a.startMs - b.startMs);
  const merged: VoiceTimeline['activitySpans'] = [];
  for (const span of sorted) {
    const previous = merged[merged.length - 1];
    if (previous && previous.source === span.source && span.startMs - previous.endMs <= ACTIVE_AUDIO_GAP_MS) {
      previous.end_secs = span.end_secs;
      previous.endMs = Math.max(previous.endMs, span.endMs);
      previous.endOffset = Math.max(previous.endOffset, span.endOffset);
    } else {
      merged.push({ ...span });
    }
  }
  return merged.sort((a, b) => a.startOffset - b.startOffset || String(a.source).localeCompare(String(b.source)));
}

function timelineOffsetForMs(ms: number, audioSegments: VoiceTimeline['audioSegments']): number {
  if (!audioSegments.length) return 0;
  for (const segment of audioSegments) {
    if (ms >= segment.startMs && ms <= segment.endMs) {
      const local = (ms - segment.startMs) / Math.max(1, segment.endMs - segment.startMs);
      return clamp01(segment.startOffset + local * (segment.endOffset - segment.startOffset));
    }
    if (ms < segment.startMs) return segment.startOffset;
  }
  return audioSegments[audioSegments.length - 1].endOffset;
}

function timelineMsAtOffset(timeline: VoiceTimeline, offset: number): number {
  const clamped = clamp01(Number(offset));
  const segments = Array.isArray(timeline.audioSegments) ? timeline.audioSegments : [];
  if (segments.length) {
    for (const segment of segments) {
      if (clamped >= segment.startOffset && clamped <= segment.endOffset) {
        const local = (clamped - segment.startOffset) / Math.max(0.000001, segment.endOffset - segment.startOffset);
        return segment.startMs + local * (segment.endMs - segment.startMs);
      }
      if (clamped < segment.startOffset) return segment.startMs;
    }
    return segments[segments.length - 1].endMs;
  }
  const range = Math.max(1, timeline.timeRange!.endMs - timeline.timeRange!.startMs);
  return timeline.timeRange!.startMs + range * clamped;
}

function sampleInstantMs(sample: VoiceSampleLike, offsetSecs: number): number {
  const parsed = Date.parse(String(sample && sample.ts || ''));
  if (!Number.isFinite(parsed)) return Number.NaN;
  return parsed + Math.max(0, Number(offsetSecs) || 0) * 1000;
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(1, value));
}

function clockLabel(date: Date): string {
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}
