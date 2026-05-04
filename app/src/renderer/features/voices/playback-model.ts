import type { VoicePlaybackBlock, VoiceSampleLike, VoiceTimeline } from './types';
import {
  clamp01,
  voiceSampleCenterMs,
  voiceSampleInstantMs,
  voiceTimelineMsAtOffset,
  voiceTimelineOffsetForMs,
} from './timeline-model';

export function voiceTimelinePlaybackBlock(
  timeline: VoiceTimeline | null | undefined,
  offset: number,
): VoicePlaybackBlock {
  const empty = { startMs: Number.NaN, endMs: Number.NaN, offset: clamp01(Number(offset)), samples: [] };
  if (!timeline || !timeline.timeRange || !Array.isArray(timeline.turns) || !timeline.turns.length) return empty;
  const ranges = timeline.turns
    .map((sample) => {
      const source = String(sample && sample.source || '');
      const startMs = voiceSampleInstantMs(sample, Number(sample && sample.start_secs || 0));
      const endMs = voiceSampleInstantMs(sample, Number(sample && sample.end_secs || sample && sample.start_secs || 0));
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

  const requestedMs = voiceTimelineMsAtOffset(timeline, offset);
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
    offset: voiceTimelineOffsetForMs(targetMs, timeline.audioSegments),
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
        startMs: voiceSampleInstantMs(sample, Number(sample && sample.start_secs || 0)),
        endMs: voiceSampleInstantMs(sample, Number(sample && sample.end_secs || sample && sample.start_secs || 0)),
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
  const currentMs = voiceTimelineMsAtOffset(timeline, offset);
  const currentBlock = voiceTimelinePlaybackBlock(timeline, offset);
  const targetMs = direction < 0
    ? previousVoiceTimelinePlaybackBoundaryMs(boundaries, currentBlock, currentMs)
    : nextVoiceTimelinePlaybackBoundaryMs(boundaries, currentBlock, currentMs);
  if (!Number.isFinite(targetMs)) return empty;
  return voiceTimelinePlaybackBlock(timeline, voiceTimelineOffsetForMs(targetMs, timeline.audioSegments));
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
  const startMs = voiceSampleInstantMs(sample, startSecs);
  const endMs = voiceSampleInstantMs(sample, endSecs);
  if (!source || !mediaKey || !Number.isFinite(startMs) || !Number.isFinite(endMs) || endMs <= startMs) return null;
  return { source, mediaKey, sample, startMs, endMs, endSecs };
}

function voiceTimelinePlaybackBoundaryMs(timeline: VoiceTimeline): number[] {
  const boundaries = new Set<number>();
  for (const sample of timeline.turns) {
    const startMs = voiceSampleInstantMs(sample, Number(sample && sample.start_secs || 0));
    const endMs = voiceSampleInstantMs(sample, Number(sample && sample.end_secs || sample && sample.start_secs || 0));
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
