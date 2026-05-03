const assert = require('node:assert/strict');
const fs = require('node:fs');
const Module = require('node:module');
const path = require('node:path');
const test = require('node:test');
const ts = require('typescript');

function loadTsModule(relativePath) {
  const file = path.join(__dirname, '..', relativePath);
  const source = fs.readFileSync(file, 'utf8');
  const compiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
      esModuleInterop: true,
    },
    fileName: file,
  }).outputText;
  const mod = new Module(file, module);
  mod.filename = file;
  mod.paths = Module._nodeModulePaths(path.dirname(file));
  mod._compile(compiled, file);
  return mod.exports;
}

function connectorSummary(mode, diarizationEnabled = true) {
  return {
    connectors: [{
      id: 'alvum.audio/audio',
      component_id: 'alvum.audio/audio',
      package_id: 'alvum.audio',
      connector_id: 'audio',
      processor_controls: [{
        component: 'alvum.audio/whisper',
        settings: [
          { key: 'mode', value: mode },
          { key: 'diarization_enabled', value: diarizationEnabled },
        ],
      }],
    }],
  };
}

function profile(interests) {
  return { interests };
}

test('voices top-level card visibility follows audio diarization and enabled people gates', () => {
  const { voiceGateSummary } = loadTsModule('src/renderer/shared/voices.ts');
  const people = profile([
    { id: 'person_michael', type: 'person', name: 'Michael', enabled: true },
  ]);

  assert.equal(voiceGateSummary(connectorSummary('local', false), people).visible, false, 'no diarization hides Voices');
  assert.equal(voiceGateSummary(connectorSummary('local', true), profile([])).visible, false, 'no enabled tracked people hides Voices');
  assert.equal(voiceGateSummary(connectorSummary('off', true), people).visible, false, 'audio processing off hides Voices');

  const eligibleEmpty = voiceGateSummary(connectorSummary('local', true), people, []);
  assert.equal(eligibleEmpty.visible, true, 'configured local diarization and people shows Voices before samples exist');
  assert.equal(eligibleEmpty.pendingReviewCount, 0);
  assert.equal(eligibleEmpty.linkedPersonCount, 0);
  assert.equal(eligibleEmpty.recentEvidenceDay, null);

  const eligibleWithSamples = voiceGateSummary(connectorSummary('provider', false), people, [
    { sample_id: 'vsm_old', linked_interest_id: 'person_michael', ts: '2026-04-30T20:00:00Z' },
    { sample_id: 'vsm_new', linked_interest_id: null, ts: '2026-05-02T08:09:00Z' },
  ]);
  assert.equal(eligibleWithSamples.visible, true, 'provider diarized transcription shows Voices');
  assert.equal(eligibleWithSamples.pendingReviewCount, 1);
  assert.equal(eligibleWithSamples.linkedPersonCount, 1);
  assert.equal(eligibleWithSamples.recentEvidenceDay, '2026-05-02');
});

test('voice timeline model groups by day and source with chronological turns and activity spans', () => {
  const { buildVoiceTimeline } = loadTsModule('src/renderer/shared/voices.ts');
  const timeline = buildVoiceTimeline([
    { sample_id: 'vsm_b', source: 'audio-system', ts: '2026-05-02T10:30:00Z', start_secs: 9, end_secs: 12, text: 'Second', linked_interest_id: null },
    { sample_id: 'vsm_a', source: 'audio-mic', ts: '2026-05-02T09:00:00Z', start_secs: 2, end_secs: 4, text: 'First', linked_interest_id: 'person_michael' },
    { sample_id: 'vsm_c', source: 'audio-mic', ts: '2026-05-01T09:00:00Z', start_secs: 1, end_secs: 3, text: 'Older', linked_interest_id: null },
  ], { selectedSources: ['audio-mic'] });

  assert.deepEqual(timeline.days, ['2026-05-02', '2026-05-01']);
  assert.equal(timeline.defaultDay, '2026-05-02');
  assert.deepEqual(timeline.sources, ['audio-mic', 'audio-system']);
  assert.deepEqual(timeline.turns.map((turn) => turn.sample_id), ['vsm_a']);
  assert.equal(timeline.activitySpans.length, 1);
  assert.equal(timeline.activitySpans[0].source, 'audio-mic');
  assert.equal(timeline.activitySpans[0].start_secs, 2);
  assert.equal(timeline.activitySpans[0].end_secs, 4);
  assert.equal(timeline.activitySpans[0].startOffset, 0);
  assert.equal(timeline.activitySpans[0].endOffset, 1);
});

test('voice timeline model filters by person together with source', () => {
  const { buildVoiceTimeline } = loadTsModule('src/renderer/shared/voices.ts');
  const samples = [
    { sample_id: 'mic_michael', source: 'audio-mic', ts: '2026-05-02T09:00:00Z', start_secs: 0, end_secs: 5, text: 'Michael mic', linked_interest_id: 'person_michael', linked_interest: { id: 'person_michael', name: 'Michael' } },
    { sample_id: 'system_michael_suggested', source: 'audio-system', ts: '2026-05-02T09:00:05Z', start_secs: 0, end_secs: 5, text: 'Michael system', person_candidates: [{ id: 'person_michael', name: 'Michael' }] },
    { sample_id: 'mic_christine', source: 'audio-mic', ts: '2026-05-02T09:00:10Z', start_secs: 0, end_secs: 5, text: 'Christine mic', linked_interest_id: 'person_christine', linked_interest: { id: 'person_christine', name: 'Christine' } },
    { sample_id: 'system_unassigned', source: 'audio-system', ts: '2026-05-02T09:00:15Z', start_secs: 0, end_secs: 5, text: 'Unassigned system' },
  ];

  const christineMic = buildVoiceTimeline(samples, {
    selectedDay: '2026-05-02',
    selectedSources: ['audio-mic'],
    selectedPeople: ['person_christine'],
  });
  assert.deepEqual(christineMic.people.map((person) => [person.id, person.name, person.count]), [
    ['person_christine', 'Christine', 1],
    ['person_michael', 'Michael', 2],
  ]);
  assert.deepEqual(christineMic.turns.map((turn) => turn.sample_id), ['mic_christine']);

  const suggestedMichaelSystem = buildVoiceTimeline(samples, {
    selectedDay: '2026-05-02',
    selectedSources: ['audio-system'],
    selectedPeople: ['person_michael'],
  });
  assert.deepEqual(suggestedMichaelSystem.turns.map((turn) => turn.sample_id), ['system_michael_suggested']);
  assert.deepEqual([...new Set(suggestedMichaelSystem.turns.map((turn) => turn.source))], ['audio-system']);
});

test('voice timeline model distinguishes no checked filters from all checked filters', () => {
  const { buildVoiceTimeline } = loadTsModule('src/renderer/shared/voices.ts');
  const samples = [
    { sample_id: 'assigned_michael', source: 'audio-mic', ts: '2026-05-02T09:00:00Z', start_secs: 0, end_secs: 5, text: 'Michael', linked_interest_id: 'person_michael', linked_interest: { id: 'person_michael', name: 'Michael' } },
    { sample_id: 'suggested_michael', source: 'audio-system', ts: '2026-05-02T09:00:05Z', start_secs: 0, end_secs: 5, text: 'Suggested Michael', person_candidates: [{ id: 'person_michael', name: 'Michael' }] },
    { sample_id: 'unassigned', source: 'audio-system', ts: '2026-05-02T09:00:10Z', start_secs: 0, end_secs: 5, text: 'Unassigned' },
  ];

  const noSources = buildVoiceTimeline(samples, {
    selectedDay: '2026-05-02',
    selectedSources: [],
  });
  assert.deepEqual(noSources.turns.map((turn) => turn.sample_id), []);

  const noPeople = buildVoiceTimeline(samples, {
    selectedDay: '2026-05-02',
    selectedPeople: [],
  });
  assert.deepEqual(noPeople.turns.map((turn) => turn.sample_id), ['suggested_michael', 'unassigned']);
});

test('voice timeline model exposes timestamp ticks and a progressive visible window', () => {
  const { buildVoiceTimeline } = loadTsModule('src/renderer/shared/voices.ts');
  const samples = Array.from({ length: 12 }, (_, index) => ({
    sample_id: `vsm_${index}`,
    source: index % 2 === 0 ? 'audio-mic' : 'audio-system',
    ts: `2026-05-02T09:${String(index).padStart(2, '0')}:00`,
    start_secs: 0,
    end_secs: 5,
    text: `Extracted turn ${index}`,
    linked_interest_id: index % 3 === 0 ? 'person_michael' : null,
  }));
  const timeline = buildVoiceTimeline(samples, { selectedDay: '2026-05-02', visibleLimit: 5 });

  assert.equal(timeline.totalTurnCount, 12);
  assert.equal(timeline.visibleLimit, 5);
  assert.equal(timeline.hasMoreTurns, true);
  assert.deepEqual(timeline.visibleTurns.map((turn) => turn.sample_id), ['vsm_0', 'vsm_1', 'vsm_2', 'vsm_3', 'vsm_4']);
  assert.equal(timeline.activitySpans.length, 2);
  assert.equal(timeline.timeRange.startLabel, '09:00');
  assert.equal(timeline.timeRange.endLabel, '09:11');
  assert.equal(timeline.audioSegments.length, 1);
  assert.equal(timeline.activitySpans[0].startOffset, 0);
  assert.ok(timeline.activitySpans[1].startOffset > timeline.activitySpans[0].startOffset);
  assert.ok(timeline.activitySpans[1].endOffset <= 1);
  assert.equal(timeline.timeTicks.length, 6);
  assert.equal(timeline.timeTicks[0].label, '09:00');
  assert.equal(timeline.timeTicks.at(-1).label, '09:11');
});

test('voice timeline model compresses capture gaps into labeled active audio chunks', () => {
  const { buildVoiceTimeline } = loadTsModule('src/renderer/shared/voices.ts');
  const timeline = buildVoiceTimeline([
    { sample_id: 'morning_a', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 0, end_secs: 30, text: 'Morning A' },
    { sample_id: 'morning_b', source: 'audio-mic', ts: '2026-05-02T09:01:00', start_secs: 0, end_secs: 30, text: 'Morning B' },
    { sample_id: 'evening_a', source: 'audio-system', ts: '2026-05-02T15:00:00', start_secs: 0, end_secs: 30, text: 'Evening A' },
    { sample_id: 'evening_b', source: 'audio-system', ts: '2026-05-02T15:01:00', start_secs: 0, end_secs: 30, text: 'Evening B' },
  ], { selectedDay: '2026-05-02' });

  assert.equal(timeline.audioSegments.length, 2);
  assert.deepEqual(timeline.audioSegments.map((segment) => segment.label), ['09:00-09:01', '15:00-15:01']);
  assert.equal(timeline.timeTicks.length, 2);
  assert.deepEqual(timeline.timeTicks.map((tick) => tick.label), ['09:00-09:01', '15:00-15:01']);
  assert.ok(timeline.activitySpans[1].startOffset < 0.75, 'second chunk should not be pushed across the whole idle-day gap');
});

test('voice timeline model preserves an explicitly selected empty day', () => {
  const { buildVoiceTimeline } = loadTsModule('src/renderer/shared/voices.ts');
  const timeline = buildVoiceTimeline([
    { sample_id: 'vsm_previous', source: 'audio-mic', ts: '2026-05-01T09:00:00', start_secs: 0, end_secs: 30, text: 'Previous day' },
  ], { selectedDay: '2026-05-02' });

  assert.equal(timeline.defaultDay, '2026-05-01');
  assert.equal(timeline.selectedDay, '2026-05-02');
  assert.equal(timeline.totalTurnCount, 0);
  assert.deepEqual(timeline.visibleTurns, []);
  assert.equal(timeline.timeRange, null);
});

test('voice timeline model builds active audio spans for ten thousand samples within the UI budget', () => {
  const { buildVoiceTimeline } = loadTsModule('src/renderer/shared/voices.ts');
  const samples = Array.from({ length: 10_000 }, (_, index) => {
    const chunk = Math.floor(index / 1000);
    const withinChunk = index % 1000;
    const minute = chunk * 90 + Math.floor(withinChunk / 20);
    const second = (withinChunk % 20) * 3;
    return {
      sample_id: `vsm_perf_${index}`,
      source: index % 2 === 0 ? 'audio-mic' : 'audio-system',
      ts: `2026-05-02T${String(8 + Math.floor(minute / 60)).padStart(2, '0')}:${String(minute % 60).padStart(2, '0')}:${String(second).padStart(2, '0')}`,
      start_secs: 0,
      end_secs: 2,
      text: `Perf turn ${index}`,
    };
  });
  const started = performance.now();
  const timeline = buildVoiceTimeline(samples, { selectedDay: '2026-05-02', visibleLimit: 24 });
  const elapsed = performance.now() - started;

  assert.equal(timeline.totalTurnCount, 10_000);
  assert.equal(timeline.audioSegments.length, 10);
  assert.ok(timeline.audioSegments.at(-1).label.startsWith('21:30-22:19'));
  assert.ok(elapsed < 500, `expected timeline model under 500ms, got ${elapsed.toFixed(1)}ms`);
  assert.ok(timeline.activitySpans.length < 400, 'horizontal timeline should render merged active stretches, not one DOM span per sample');
});

test('voice timeline model exposes a bounded scrub index for nearest-turn lookup', () => {
  const { buildVoiceTimeline, nearestVoiceTimelineSample } = loadTsModule('src/renderer/shared/voices.ts');
  const timeline = buildVoiceTimeline([
    { sample_id: 'vsm_a', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 0, end_secs: 10, text: 'First' },
    { sample_id: 'vsm_b', source: 'audio-mic', ts: '2026-05-02T09:00:20', start_secs: 0, end_secs: 10, text: 'Second' },
    { sample_id: 'vsm_c', source: 'audio-mic', ts: '2026-05-02T09:00:40', start_secs: 0, end_secs: 10, text: 'Third' },
  ], { selectedDay: '2026-05-02' });

  assert.deepEqual(timeline.scrubIndex.map((entry) => entry.sample_id), ['vsm_a', 'vsm_b', 'vsm_c']);
  assert.equal(nearestVoiceTimelineSample(timeline, 0.01).sample_id, 'vsm_a');
  assert.equal(nearestVoiceTimelineSample(timeline, 0.50).sample_id, 'vsm_b');
  assert.equal(nearestVoiceTimelineSample(timeline, 0.99).sample_id, 'vsm_c');
});

test('voice timeline model supports a bounded visible turn window', () => {
  const { buildVoiceTimeline, voiceTimelineVisibleStartForIndex } = loadTsModule('src/renderer/shared/voices.ts');
  const samples = Array.from({ length: 8 }, (_, index) => ({
    sample_id: `vsm_${index}`,
    source: 'audio-mic',
    ts: `2026-05-02T09:${String(index).padStart(2, '0')}:00`,
    start_secs: 0,
    end_secs: 5,
    text: `Turn ${index}`,
  }));

  const start = voiceTimelineVisibleStartForIndex(6, samples.length, 3);
  const timeline = buildVoiceTimeline(samples, {
    selectedDay: '2026-05-02',
    visibleLimit: 3,
    visibleStart: start,
  });

  assert.equal(start, 5);
  assert.equal(timeline.visibleStart, 5);
  assert.deepEqual(timeline.visibleTurns.map((sample) => sample.sample_id), ['vsm_5', 'vsm_6', 'vsm_7']);
  assert.equal(timeline.hasMoreTurns, false);
});

test('voice timeline playback block starts overlapping selected sources together', () => {
  const { buildVoiceTimeline, voiceTimelinePlaybackBlock } = loadTsModule('src/renderer/shared/voices.ts');
  const timeline = buildVoiceTimeline([
    { sample_id: 'mic_a', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 0, end_secs: 8, text: 'Mic A' },
    { sample_id: 'system_a', source: 'audio-system', ts: '2026-05-02T09:00:00', start_secs: 2, end_secs: 10, text: 'System A' },
    { sample_id: 'mic_b', source: 'audio-mic', ts: '2026-05-02T09:00:20', start_secs: 0, end_secs: 6, text: 'Mic B' },
  ], { selectedDay: '2026-05-02' });

  const block = voiceTimelinePlaybackBlock(timeline, 0.20);

  assert.deepEqual(block.samples.map((entry) => entry.sample.sample_id).sort(), ['mic_a', 'system_a']);
  assert.equal(block.samples.length, 2);
  assert.ok(block.startMs >= Date.parse('2026-05-02T09:00:02'));
  assert.ok(block.endMs <= Date.parse('2026-05-02T09:00:10'));
});

test('voice timeline playback block follows the selected source filter', () => {
  const { buildVoiceTimeline, voiceTimelinePlaybackBlock } = loadTsModule('src/renderer/shared/voices.ts');
  const timeline = buildVoiceTimeline([
    { sample_id: 'mic_a', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 0, end_secs: 8, text: 'Mic A' },
    { sample_id: 'system_a', source: 'audio-system', ts: '2026-05-02T09:00:00', start_secs: 2, end_secs: 10, text: 'System A' },
  ], { selectedDay: '2026-05-02', selectedSources: ['audio-system'] });

  const block = voiceTimelinePlaybackBlock(timeline, 0);

  assert.deepEqual(block.samples.map((entry) => entry.sample.sample_id), ['system_a']);
  assert.deepEqual([...new Set(block.samples.map((entry) => entry.source))], ['audio-system']);
});

test('voice timeline playback step lands on adjacent next chunk', () => {
  const { buildVoiceTimeline, voiceTimelinePlaybackStepBlock } = loadTsModule('src/renderer/shared/voices.ts');
  const timeline = buildVoiceTimeline([
    { sample_id: 'chunk_a', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 0, end_secs: 1, text: 'A' },
    { sample_id: 'chunk_b', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 1, end_secs: 2, text: 'B' },
    { sample_id: 'chunk_c', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 2, end_secs: 3, text: 'C' },
  ], { selectedDay: '2026-05-02' });

  const next = voiceTimelinePlaybackStepBlock(timeline, 0, 1);
  assert.deepEqual(next.samples.map((entry) => entry.sample.sample_id), ['chunk_b']);

  const previous = voiceTimelinePlaybackStepBlock(timeline, next.offset, -1);
  assert.deepEqual(previous.samples.map((entry) => entry.sample.sample_id), ['chunk_a']);
});

test('voice timeline playback step lands on source starts inside the current block', () => {
  const { buildVoiceTimeline, voiceTimelinePlaybackStepBlock } = loadTsModule('src/renderer/shared/voices.ts');
  const timeline = buildVoiceTimeline([
    { sample_id: 'mic_a', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 0, end_secs: 8, text: 'Mic A' },
    { sample_id: 'system_a', source: 'audio-system', ts: '2026-05-02T09:00:00', start_secs: 2, end_secs: 10, text: 'System A' },
    { sample_id: 'mic_b', source: 'audio-mic', ts: '2026-05-02T09:00:20', start_secs: 0, end_secs: 6, text: 'Mic B' },
  ], { selectedDay: '2026-05-02' });

  const next = voiceTimelinePlaybackStepBlock(timeline, 0, 1);

  assert.deepEqual(next.samples.map((entry) => entry.sample.sample_id).sort(), ['mic_a', 'system_a']);
});

test('voice timeline continuous playback block spans adjacent chunks in the same media file', () => {
  const { buildVoiceTimeline, voiceTimelineContinuousPlaybackBlock } = loadTsModule('src/renderer/shared/voices.ts');
  const timeline = buildVoiceTimeline([
    { sample_id: 'chunk_a', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 0, end_secs: 1, text: 'A' },
    { sample_id: 'chunk_b', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 1, end_secs: 2, text: 'B' },
    { sample_id: 'chunk_c', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 2, end_secs: 3, text: 'C' },
  ], { selectedDay: '2026-05-02' });

  const block = voiceTimelineContinuousPlaybackBlock(timeline, 0);

  assert.deepEqual(block.samples.map((entry) => entry.sample.sample_id), ['chunk_a']);
  assert.equal(block.endMs - block.startMs, 3000);
  assert.equal(block.samples[0].audioEndSecs, 3);
  assert.deepEqual(block.selectionSamples.map((sample) => sample.sample_id), ['chunk_a', 'chunk_b', 'chunk_c']);
});

test('voice playback selection does not jump to the next chunk before its start', () => {
  const { buildVoiceTimeline, voiceTimelineContinuousPlaybackBlock, voicePlaybackSampleForPosition } = loadTsModule('src/renderer/shared/voices.ts');
  assert.equal(typeof voicePlaybackSampleForPosition, 'function');
  const timeline = buildVoiceTimeline([
    { sample_id: 'chunk_a', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 0, end_secs: 1, text: 'A' },
    { sample_id: 'chunk_b', source: 'audio-mic', ts: '2026-05-02T09:00:00', start_secs: 2, end_secs: 3, text: 'B' },
  ], { selectedDay: '2026-05-02' });
  const block = voiceTimelineContinuousPlaybackBlock(timeline, 0);

  const betweenChunks = Date.parse('2026-05-02T09:00:01.700');
  const atNextChunk = Date.parse('2026-05-02T09:00:02.000');

  assert.equal(voicePlaybackSampleForPosition(block, betweenChunks).sample_id, 'chunk_a');
  assert.equal(voicePlaybackSampleForPosition(block, atNextChunk).sample_id, 'chunk_b');
});

test('voice timeline actions expose assignment targets and context evidence', () => {
  const { voiceTimelineActionsForSample } = loadTsModule('src/renderer/shared/voices.ts');
  const actions = voiceTimelineActionsForSample(
    {
      sample_id: 'vsm_a',
      cluster_id: 'spk_local_first',
      media_path: '/Users/michael/.alvum/capture/2026-05-02/audio/mic/09-00-00.wav',
      start_secs: 2,
      end_secs: 8,
      context_interests: [{ id: 'project_alvum', type: 'project', name: 'Alvum' }],
    },
    [
      { id: 'person_michael', type: 'person', name: 'Michael', enabled: true },
      { id: 'project_alvum', type: 'project', name: 'Alvum', enabled: true },
      { id: 'person_disabled', type: 'person', name: 'Disabled', enabled: false },
    ],
  );

  assert.equal(actions.canAssign, true);
  assert.deepEqual(actions.assignmentTargets.map((target) => target.id), ['person_michael']);
  assert.deepEqual(actions.contextEvidence.map((item) => item.id), ['project_alvum']);
});
