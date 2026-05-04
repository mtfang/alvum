import type { VoiceSampleLike, VoiceTimeline } from './types';
import {
  voiceSampleInstantMs,
  voiceTimelineOffsetForMs,
  voiceTimelineVisibleStartForIndex,
} from './timeline-model';

export interface VoiceTimelineViewState {
  selectedSampleId: string | null;
  expandedSampleId: string | null;
  visibleStart: number;
  visibleLimit: number;
  scrubberOffset: number;
}

export interface SelectVoiceTimelineSampleOptions {
  keepScrubber?: boolean;
  syncWindow?: boolean;
  scroll?: boolean;
  expandEditor?: boolean;
  collapseEditor?: boolean;
}

export interface SelectVoiceTimelineSampleResult {
  state: VoiceTimelineViewState;
  sampleIndex: number;
  renderRows: boolean;
  windowChanged: boolean;
  rerenderTimeline: boolean;
  scrollAfterRender: boolean;
  scroll: boolean;
}

export function reconcileVoiceTimelineViewState(
  timeline: VoiceTimeline | null | undefined,
  state: VoiceTimelineViewState,
): VoiceTimelineViewState {
  const ids = new Set((timeline && Array.isArray(timeline.turns) ? timeline.turns : [])
    .map((sample) => String(sample && sample.sample_id || ''))
    .filter(Boolean));
  return {
    ...state,
    selectedSampleId: state.selectedSampleId && ids.has(state.selectedSampleId) ? state.selectedSampleId : null,
    expandedSampleId: state.expandedSampleId && ids.has(state.expandedSampleId) ? state.expandedSampleId : null,
  };
}

export function selectVoiceTimelineSampleState(
  timeline: VoiceTimeline | null | undefined,
  state: VoiceTimelineViewState,
  sampleId: string,
  options: SelectVoiceTimelineSampleOptions = {},
): SelectVoiceTimelineSampleResult {
  const turns = timeline && Array.isArray(timeline.turns) ? timeline.turns : [];
  const sampleIndex = turns.findIndex((turn) => String(turn && turn.sample_id || '') === sampleId);
  const previousExpanded = state.expandedSampleId;
  const next: VoiceTimelineViewState = {
    selectedSampleId: sampleId,
    expandedSampleId: nextExpandedSampleId(state.expandedSampleId, sampleId, options),
    visibleStart: state.visibleStart,
    visibleLimit: state.visibleLimit,
    scrubberOffset: state.scrubberOffset,
  };
  const sample = sampleIndex >= 0 ? turns[sampleIndex] : null;
  if (!options.keepScrubber && sample && timeline) {
    next.scrubberOffset = voiceTimelineSampleScrubOffset(sample, timeline);
  }

  let windowChanged = false;
  if (options.syncWindow && timeline && sampleIndex >= 0) {
    const visibleStart = voiceTimelineVisibleStartForIndex(sampleIndex, timeline.totalTurnCount, next.visibleLimit);
    windowChanged = visibleStart !== next.visibleStart;
    next.visibleStart = visibleStart;
  }

  let rerenderTimeline = false;
  let scrollAfterRender = false;
  if (options.scroll && !options.syncWindow && sampleIndex >= next.visibleLimit) {
    next.visibleLimit = Math.max(next.visibleLimit, sampleIndex + 1);
    next.visibleStart = 0;
    rerenderTimeline = true;
    scrollAfterRender = true;
  }

  return {
    state: next,
    sampleIndex,
    renderRows: previousExpanded !== next.expandedSampleId,
    windowChanged,
    rerenderTimeline,
    scrollAfterRender,
    scroll: options.scroll === true,
  };
}

export function voiceTimelineSampleScrubOffset(sample: VoiceSampleLike, timeline: VoiceTimeline): number {
  const startMs = voiceSampleInstantMs(sample, Number(sample && sample.start_secs || 0));
  return voiceTimelineOffsetForMs(startMs, timeline.audioSegments || []);
}

function nextExpandedSampleId(
  expandedSampleId: string | null,
  sampleId: string,
  options: SelectVoiceTimelineSampleOptions,
): string | null {
  if (options.expandEditor) return sampleId;
  if (options.collapseEditor) return null;
  if (expandedSampleId && expandedSampleId !== sampleId) return null;
  return expandedSampleId || null;
}
