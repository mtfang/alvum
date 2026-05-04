import type { VoicePlaybackBlock, VoicePlaybackSample, VoiceSampleLike } from './types';
import { voiceSampleInstantMs } from './timeline-model';

export interface VoiceSampleAudioResult {
  ok?: boolean;
  sample_id?: string;
  url?: string;
  start_secs?: number;
  end_secs?: number;
  mime?: string | null;
  error?: string;
}

export interface VoiceAudioLike {
  currentTime: number;
  paused?: boolean;
  play(): Promise<unknown>;
  pause(): void;
}

export interface VoicePlaybackControllerDeps {
  voiceSampleAudio(sampleId: string): Promise<VoiceSampleAudioResult>;
  createAudio(url: string): VoiceAudioLike;
  setInterval(fn: () => void, ms: number): unknown;
  clearInterval(handle: unknown): void;
  onState(state: VoicePlaybackControllerState): void;
  onPosition(positionMs: number, block: VoicePlaybackBlock, options: { expandEditor: boolean }): void;
  onBlockEnded?(nextMs: number, expandEditor: boolean): void;
  notify(message: string, level?: 'info' | 'warning' | 'error', heading?: string): void;
  tickMs?: number;
}

export interface VoicePlaybackControllerState {
  starting: boolean;
  playing: boolean;
  expandEditor: boolean;
}

interface VoiceResolvedPlaybackSample extends VoicePlaybackSample {
  audio: VoiceAudioLike;
  audioStart: number;
  audioEnd: number;
}

interface VoicePlaybackSession {
  generation: number;
  block: VoicePlaybackBlock;
  entries: VoiceResolvedPlaybackSample[];
  expandEditor: boolean;
  timer: unknown;
}

export class VoicePlaybackController {
  private playback: VoicePlaybackSession | null = null;
  private starting = false;
  private expandEditor = false;
  private generation = 0;

  constructor(private readonly deps: VoicePlaybackControllerDeps) {}

  isPlaying(): boolean {
    return !!this.playback;
  }

  isStarting(): boolean {
    return this.starting;
  }

  setExpandEditor(expandEditor: boolean): void {
    this.expandEditor = expandEditor === true;
    if (this.playback) this.playback.expandEditor = this.expandEditor;
    this.emitState();
  }

  state(): VoicePlaybackControllerState {
    return {
      starting: this.starting,
      playing: !!this.playback,
      expandEditor: this.playback ? this.playback.expandEditor : this.expandEditor,
    };
  }

  async playBlock(block: VoicePlaybackBlock, options: { expandEditor?: boolean } = {}): Promise<void> {
    if (!block || !block.samples || !block.samples.length) return;
    const expandEditor = options.expandEditor === true;
    this.stop();
    this.starting = true;
    this.expandEditor = expandEditor;
    this.emitState();
    const generation = this.generation;
    try {
      const resolved = await Promise.all(block.samples.map((entry) => this.resolveEntry(entry, block, generation)));
      if (generation !== this.generation) return;
      const entries = resolved.filter(Boolean) as VoiceResolvedPlaybackSample[];
      if (!entries.length) {
        this.deps.notify('Sample audio unavailable.', 'warning', 'Voices');
        return;
      }
      const playback: VoicePlaybackSession = {
        generation,
        block,
        entries,
        expandEditor,
        timer: null,
      };
      this.playback = playback;
      this.starting = false;
      this.deps.onPosition(block.startMs, block, { expandEditor });
      this.emitState();
      const started = await Promise.allSettled(entries.map((entry) => entry.audio.play()));
      if (this.playback !== playback) {
        entries.forEach((entry) => entry.audio.pause());
        return;
      }
      playback.entries = entries.filter((_entry, index) => started[index].status === 'fulfilled');
      entries.forEach((entry, index) => {
        if (started[index].status !== 'fulfilled') entry.audio.pause();
      });
      if (!playback.entries.length) {
        this.deps.notify('Audio playback was blocked.', 'warning', 'Voices');
        this.stop();
        return;
      }
      playback.timer = this.deps.setInterval(() => this.tick(playback), this.deps.tickMs || 120);
      this.tick(playback);
    } catch (err) {
      this.deps.notify(errorMessage(err), 'warning', 'Voices');
    } finally {
      if (generation === this.generation) {
        this.starting = false;
        this.emitState();
      }
    }
  }

  stop(): void {
    this.generation += 1;
    this.starting = false;
    if (this.playback) {
      if (this.playback.timer) this.deps.clearInterval(this.playback.timer);
      this.playback.entries.forEach((entry) => entry.audio.pause());
    }
    this.playback = null;
    this.expandEditor = false;
    this.emitState();
  }

  private async resolveEntry(
    entry: VoicePlaybackSample,
    block: VoicePlaybackBlock,
    generation: number,
  ): Promise<VoiceResolvedPlaybackSample | null> {
    const sampleId = entry.sample && entry.sample.sample_id;
    if (!sampleId) return null;
    const result = await this.deps.voiceSampleAudio(sampleId);
    if (generation !== this.generation) return null;
    if (!result || result.ok === false || !result.url) return null;
    const bounds = voiceAudioPlaybackBounds(entry.sample, result);
    if (!bounds) return null;
    const audio = this.deps.createAudio(result.url);
    const requestedAudioEnd = Number(entry.audioEndSecs);
    const audioEnd = Number.isFinite(requestedAudioEnd) && requestedAudioEnd > bounds.audioStart
      ? requestedAudioEnd
      : bounds.audioEnd;
    const currentTime = voiceAudioCurrentTimeForTimelineMs(entry.sample, bounds, block.startMs);
    if (!(audioEnd > currentTime)) return null;
    audio.currentTime = Math.max(0, currentTime);
    return {
      ...entry,
      audio,
      audioStart: bounds.audioStart,
      audioEnd,
    };
  }

  private tick(playback: VoicePlaybackSession): void {
    if (!playback || this.playback !== playback) return;
    const entries = playback.entries || [];
    if (!entries.length) {
      this.stop();
      return;
    }
    const positions = entries.map((entry) =>
      entry.startMs + Math.max(0, entry.audio.currentTime - entry.audioStart) * 1000);
    const positionMs = Math.min(
      playback.block.endMs,
      Math.max(playback.block.startMs, ...positions.filter(Number.isFinite)),
    );
    const expandEditor = playback.expandEditor === true;
    this.deps.onPosition(positionMs, playback.block, { expandEditor });
    const finished = entries.every((entry) => {
      const blockEndAudioSec = entry.audioStart + Math.max(0, (playback.block.endMs - entry.startMs) / 1000);
      return entry.audio.paused || entry.audio.currentTime >= Math.min(entry.audioEnd, blockEndAudioSec) - 0.05;
    })
      || positionMs >= playback.block.endMs - 75;
    if (!finished) return;
    const nextMs = playback.block.endMs + 1;
    this.stop();
    if (Number.isFinite(nextMs) && nextMs > playback.block.startMs) {
      this.deps.onBlockEnded?.(nextMs, expandEditor);
    }
  }

  private emitState(): void {
    this.deps.onState(this.state());
  }
}

export function voiceAudioPlaybackBounds(
  sample: VoiceSampleLike,
  result: VoiceSampleAudioResult,
): { audioStart: number; audioEnd: number; sampleStartMs: number } | null {
  const requestedId = String(sample && sample.sample_id || '');
  const resultId = String(result && result.sample_id || '');
  if (requestedId && resultId && requestedId !== resultId) return null;
  const audioStart = Number.isFinite(Number(result && result.start_secs))
    ? Number(result.start_secs)
    : Number(sample && sample.start_secs || 0);
  const audioEnd = Number.isFinite(Number(result && result.end_secs))
    ? Number(result.end_secs)
    : Number(sample && sample.end_secs || audioStart);
  const sampleStartMs = voiceSampleInstantMs(sample, Number(sample && sample.start_secs || 0));
  if (!Number.isFinite(audioStart) || !Number.isFinite(audioEnd) || !(audioEnd > audioStart)) return null;
  return { audioStart, audioEnd, sampleStartMs };
}

export function voiceAudioCurrentTimeForTimelineMs(
  _sample: VoiceSampleLike,
  bounds: { audioStart: number; sampleStartMs: number } | null,
  positionMs: number,
): number {
  if (!bounds) return 0;
  if (!Number.isFinite(positionMs) || !Number.isFinite(bounds.sampleStartMs)) return bounds.audioStart;
  return bounds.audioStart + Math.max(0, (positionMs - bounds.sampleStartMs) / 1000);
}

function errorMessage(err: unknown): string {
  if (err instanceof Error && err.message) return err.message;
  return String(err || 'Voice playback failed.');
}
