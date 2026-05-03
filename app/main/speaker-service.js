const { pathToFileURL } = require('url');

function createSpeakerService({ runAlvumJson, fs, path, CAPTURE_DIR, broadcastState = () => {} }) {
  let mutationQueue = Promise.resolve();

  function commandFailed(data) {
    return !data || data.error || data.ok === false;
  }

  function failure(data, fallback, shape = {}) {
    return {
      ok: false,
      ...shape,
      error: data && data.error ? data.error : fallback,
    };
  }

  function runMutation(args, fallback) {
    const run = mutationQueue.then(async () => {
      const data = await runAlvumJson(args, 5000);
      if (commandFailed(data)) return failure(data, fallback);
      broadcastState();
      return data;
    });
    mutationQueue = run.catch(() => {});
    return run;
  }

  async function speakerList() {
    const data = await runAlvumJson(['speakers', 'list', '--json'], 5000);
    if (commandFailed(data)) return failure(data, 'speaker list failed', { speakers: [] });
    return data;
  }

  async function speakerLink(id, interestId) {
    return runMutation(['speakers', 'link', String(id || ''), String(interestId || ''), '--json'], 'speaker link failed');
  }

  async function speakerSamples() {
    const data = await runAlvumJson(['speakers', 'samples', '--json'], 5000);
    if (commandFailed(data)) return failure(data, 'speaker samples failed', { speakers: [], samples: [] });
    return data;
  }

  async function speakerLinkSample(sampleId, interestId) {
    return runMutation(['speakers', 'link-sample', String(sampleId || ''), String(interestId || ''), '--json'], 'speaker sample link failed');
  }

  async function speakerMoveSample(sampleId, clusterId) {
    return runMutation(['speakers', 'move-sample', String(sampleId || ''), String(clusterId || ''), '--json'], 'speaker sample move failed');
  }

  async function speakerIgnoreSample(sampleId) {
    return runMutation(['speakers', 'ignore-sample', String(sampleId || ''), '--json'], 'speaker sample ignore failed');
  }

  async function speakerUnlinkSample(sampleId) {
    return runMutation(['speakers', 'unlink-sample', String(sampleId || ''), '--json'], 'speaker sample unlink failed');
  }

  async function speakerSplitSample(sampleId, payload = {}) {
    return runMutation([
      'speakers',
      'split-sample',
      String(sampleId || ''),
      '--at',
      String(payload.at ?? ''),
      '--left-text',
      String(payload.leftText || ''),
      '--right-text',
      String(payload.rightText || ''),
      '--json',
    ], 'speaker sample split failed');
  }

  async function speakerSplit(clusterId, sampleIds) {
    const args = ['speakers', 'split', String(clusterId || '')];
    for (const sampleId of Array.isArray(sampleIds) ? sampleIds : []) {
      args.push('--sample', String(sampleId || ''));
    }
    args.push('--json');
    return runMutation(args, 'speaker split failed');
  }

  async function speakerRecluster() {
    return runMutation(['speakers', 'recluster', '--json'], 'speaker recluster failed');
  }

  async function speakerUnlink(id) {
    return runMutation(['speakers', 'unlink', String(id || ''), '--json'], 'speaker unlink failed');
  }

  async function speakerRename(id, label) {
    return runMutation(['speakers', 'rename', String(id || ''), String(label || ''), '--json'], 'speaker rename failed');
  }

  async function speakerMerge(sourceId, targetId) {
    return runMutation(['speakers', 'merge', String(sourceId || ''), String(targetId || ''), '--json'], 'speaker merge failed');
  }

  async function speakerForget(id) {
    return runMutation(['speakers', 'forget', String(id || ''), '--json'], 'speaker forget failed');
  }

  async function speakerReset() {
    return runMutation(['speakers', 'reset', '--json'], 'speaker reset failed');
  }

  async function speakerSampleAudio(id, sampleIndex) {
    const data = await speakerList();
    if (!data || data.ok === false) return data;
    const speaker = Array.isArray(data.speakers)
      ? data.speakers.find((item) => item && item.speaker_id === String(id || ''))
      : null;
    const index = Number(sampleIndex);
    const sample = speaker && Array.isArray(speaker.samples) && Number.isInteger(index)
      ? speaker.samples[index]
      : null;
    const resolved = resolveSample(sample);
    if (!resolved.ok) return resolved;
    return {
      ok: true,
      url: pathToFileURL(resolved.path).toString(),
      start_secs: Number(sample.start_secs || 0),
      end_secs: Number(sample.end_secs || 0),
      mime: sample.mime || null,
    };
  }

  async function voiceSampleAudio(sampleId) {
    const data = await speakerSamples();
    if (!data || data.ok === false) return data;
    const sample = Array.isArray(data.samples)
      ? data.samples.find((item) => item && item.sample_id === String(sampleId || ''))
      : null;
    const resolved = resolveSample(sample);
    if (!resolved.ok) return resolved;
    return {
      ok: true,
      sample_id: sample.sample_id,
      url: pathToFileURL(resolved.path).toString(),
      start_secs: Number(sample.start_secs || 0),
      end_secs: Number(sample.end_secs || 0),
      mime: sample.mime || null,
    };
  }

  function resolveSample(sample) {
    if (!sample) return { ok: false, error: 'sample audio is unavailable' };
    const direct = resolveSamplePath(sample.media_path);
    if (direct.ok) return direct;
    if (sample.media_path) return direct;
    const fallback = resolveSamplePath(pathForSampleTimestamp(sample));
    if (!fallback.ok) return fallback;
    return fallback;
  }

  function resolveSamplePath(mediaPath) {
    if (!mediaPath || !fs || !path || !CAPTURE_DIR) {
      return { ok: false, error: 'sample audio is unavailable' };
    }
    const raw = String(mediaPath);
    const candidate = path.isAbsolute(raw)
      ? raw
      : path.resolve(CAPTURE_DIR, raw.replace(/^capture[\\/]/, ''));
    let realCapture;
    let realCandidate;
    try {
      realCapture = fs.realpathSync(CAPTURE_DIR);
      realCandidate = fs.realpathSync(candidate);
    } catch {
      return { ok: false, error: 'sample audio file is missing' };
    }
    const relative = path.relative(realCapture, realCandidate);
    if (relative.startsWith('..') || path.isAbsolute(relative)) {
      return { ok: false, error: 'sample audio path is outside Alvum capture storage' };
    }
    return { ok: true, path: realCandidate };
  }

  function pathForSampleTimestamp(sample) {
    if (!sample || !sample.ts || !sample.source || !path || !CAPTURE_DIR) return null;
    const timestamp = new Date(String(sample.ts));
    if (Number.isNaN(timestamp.getTime())) return null;
    const sourceDir = audioSourceDir(sample.source);
    if (!sourceDir) return null;
    const date = [
      timestamp.getFullYear(),
      String(timestamp.getMonth() + 1).padStart(2, '0'),
      String(timestamp.getDate()).padStart(2, '0'),
    ].join('-');
    const time = [
      String(timestamp.getHours()).padStart(2, '0'),
      String(timestamp.getMinutes()).padStart(2, '0'),
      String(timestamp.getSeconds()).padStart(2, '0'),
    ].join('-');
    return path.join(CAPTURE_DIR, date, 'audio', sourceDir, `${time}.wav`);
  }

  function audioSourceDir(source) {
    const normalized = String(source || '').toLowerCase();
    if (normalized.includes('system')) return 'system';
    if (normalized.includes('mic') || normalized.includes('microphone')) return 'mic';
    return null;
  }

  return {
    speakerList,
    speakerSamples,
    speakerLink,
    speakerLinkSample,
    speakerMoveSample,
    speakerIgnoreSample,
    speakerUnlinkSample,
    speakerSplitSample,
    speakerSplit,
    speakerRecluster,
    speakerUnlink,
    speakerRename,
    speakerMerge,
    speakerForget,
    speakerReset,
    speakerSampleAudio,
    voiceSampleAudio,
  };
}

module.exports = { createSpeakerService };
