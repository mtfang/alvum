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
