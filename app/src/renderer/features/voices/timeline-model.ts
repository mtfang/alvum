import type { VoiceSampleLike, VoiceScrubIndexEntry, VoiceTimeline } from './types';

const ACTIVE_AUDIO_GAP_MS = 5 * 60 * 1000;

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

export function voiceSampleCenterMs(sample: VoiceSampleLike): number {
  const startMs = voiceSampleInstantMs(sample, Number(sample && sample.start_secs || 0));
  const endMs = voiceSampleInstantMs(sample, Number(sample && sample.end_secs || sample && sample.start_secs || 0));
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs)) return Number.NaN;
  return (startMs + Math.max(startMs, endMs)) / 2;
}

export function voiceSampleInstantMs(sample: VoiceSampleLike, offsetSecs: number): number {
  const parsed = Date.parse(String(sample && sample.ts || ''));
  if (!Number.isFinite(parsed)) return Number.NaN;
  return parsed + Math.max(0, Number(offsetSecs) || 0) * 1000;
}

export function voiceTimelineOffsetForMs(
  ms: number,
  audioSegments: VoiceTimeline['audioSegments'],
): number {
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

export function voiceTimelineMsAtOffset(timeline: VoiceTimeline, offset: number): number {
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

export function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(1, value));
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
    .map((sample) => voiceSampleInstantMs(sample, Number(sample.start_secs || 0)))
    .filter(Number.isFinite) as number[];
  const ends = turns
    .map((sample) => voiceSampleInstantMs(sample, Number(sample.end_secs || sample.start_secs || 0)))
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
      const startMs = voiceSampleInstantMs(sample, startSecs);
      const endMs = voiceSampleInstantMs(sample, endSecs);
      if (!Number.isFinite(startMs) || !Number.isFinite(endMs)) return null;
      const startOffset = voiceTimelineOffsetForMs(startMs, audioSegments);
      const endOffset = voiceTimelineOffsetForMs(Math.max(startMs, endMs), audioSegments);
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
        offset: voiceTimelineOffsetForMs(centerMs, audioSegments),
      };
    })
    .filter(Boolean)
    .sort((a, b) => a!.offset - b!.offset || a!.centerMs - b!.centerMs) as VoiceScrubIndexEntry[];
}

function timelineAudioSegments(turns: VoiceSampleLike[]): VoiceTimeline['audioSegments'] {
  const intervals = turns
    .map((sample) => {
      const startMs = voiceSampleInstantMs(sample, Number(sample.start_secs || 0));
      const endMs = voiceSampleInstantMs(sample, Number(sample.end_secs || sample.start_secs || 0));
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

function clockLabel(date: Date): string {
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}
