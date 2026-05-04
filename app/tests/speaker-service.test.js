const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const { createSpeakerService } = require('../main/speaker-service');

test('speaker list normalizes top-level summary arrays', async () => {
  const service = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: os.tmpdir(),
    runAlvumJson: async () => ({
      ok: true,
      path: '/tmp/speakers.json',
      speakers: { speaker_id: 'not-an-array' },
      clusters: null,
      samples: 'not-an-array',
      voice_models: { linked_interest: { id: 'person_a' } },
    }),
  });

  assert.deepEqual(await service.speakerList(), {
    ok: true,
    path: '/tmp/speakers.json',
    speakers: [],
    clusters: [],
    samples: [],
    voice_models: [],
    error: null,
  });
});

test('speaker samples normalizes voice sample summary item shapes', async () => {
  const service = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: os.tmpdir(),
    runAlvumJson: async () => ({
      ok: true,
      samples: [{
        sample_id: 42,
        cluster_id: 7,
        text: 123,
        source: 'audio-mic',
        ts: '2026-05-03T09:10:11.000Z',
        start_secs: '1.25',
        end_secs: 'bad-number',
        media_path: 99,
        mime: 88,
        linked_interest_id: 13,
        linked_interest: 'not-an-object',
        person_candidates: { id: 'person_a' },
        context_interests: null,
        ignored_by_user: true,
      }, null],
    }),
  });

  assert.deepEqual(await service.speakerSamples(), {
    ok: true,
    path: null,
    speakers: [],
    clusters: [],
    samples: [{
      sample_id: '42',
      cluster_id: '7',
      text: '123',
      source: 'audio-mic',
      ts: '2026-05-03T09:10:11.000Z',
      start_secs: 1.25,
      end_secs: 0,
      media_path: '99',
      mime: '88',
      linked_interest_id: '13',
      linked_interest: null,
      person_candidates: [],
      context_interests: [],
      quality_flags: [],
      assignment_source: null,
      ignored_by_user: true,
    }],
    voice_models: [],
    error: null,
  });
});

test('sample audio responses normalize playback fields and keep capture path boundary', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-speaker-service-'));
  const captureDir = path.join(root, 'capture');
  const samplePath = path.join(captureDir, '2026-05-03', 'audio', 'mic', '09-10-11.wav');
  const outsidePath = path.join(root, 'outside.wav');
  fs.mkdirSync(path.dirname(samplePath), { recursive: true });
  fs.writeFileSync(samplePath, 'mock-audio');
  fs.writeFileSync(outsidePath, 'outside-audio');

  const service = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: captureDir,
    runAlvumJson: async (args) => {
      if (args[1] === 'list') {
        return {
          ok: true,
          speakers: [{
            speaker_id: 'speaker_a',
            samples: [{
              sample_id: 'nested_sample',
              media_path: samplePath,
              start_secs: '2.5',
              end_secs: 'bad-number',
              mime: 37,
            }],
          }],
        };
      }
      return {
        ok: true,
        samples: [{
          sample_id: 'sample_a',
          cluster_id: 'speaker_a',
          media_path: samplePath,
          start_secs: '2.5',
          end_secs: 'bad-number',
          mime: 37,
        }, {
          sample_id: 'sample_escape',
          cluster_id: 'speaker_a',
          media_path: outsidePath,
        }],
      };
    },
  });

  const speakerAudio = await service.speakerSampleAudio('speaker_a', 0);
  assert.equal(speakerAudio.ok, true);
  assert.match(speakerAudio.url, /^file:/);
  assert.equal(speakerAudio.sample_id, 'nested_sample');
  assert.equal(speakerAudio.start_secs, 2.5);
  assert.equal(speakerAudio.end_secs, 0);
  assert.equal(speakerAudio.mime, '37');

  const voiceAudio = await service.voiceSampleAudio('sample_a');
  assert.equal(voiceAudio.ok, true);
  assert.match(voiceAudio.url, /^file:/);
  assert.equal(voiceAudio.sample_id, 'sample_a');
  assert.equal(voiceAudio.start_secs, 2.5);
  assert.equal(voiceAudio.end_secs, 0);
  assert.equal(voiceAudio.mime, '37');

  assert.deepEqual(await service.voiceSampleAudio('sample_escape'), {
    ok: false,
    error: 'sample audio path is outside Alvum capture storage',
  });
});

test('speaker unlink interest delegates to the batch registry command', async () => {
  const calls = [];
  const service = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: os.tmpdir(),
    runAlvumJson: async (args) => {
      calls.push(args);
      return { ok: true, speakers: [], samples: [] };
    },
  });

  assert.equal((await service.speakerUnlinkInterest('person_michael')).ok, true);
  assert.deepEqual(calls[0], ['speakers', 'unlink-interest', 'person_michael', '--json']);
});
