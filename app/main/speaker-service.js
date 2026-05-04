const { pathToFileURL } = require('url');

function createSpeakerService({ runAlvumJson, fs, path, CAPTURE_DIR, broadcastState = () => {} }) {
  let mutationQueue = Promise.resolve();

  function commandFailed(data) {
    return !data || data.error || data.ok === false;
  }

  function failure(data, fallback, shape = {}) {
    return {
      ...normalizeSpeakerSummary(shape),
      ok: false,
      error: data && data.error ? data.error : fallback,
    };
  }

  function runMutation(args, fallback) {
    const run = mutationQueue.then(async () => {
      const data = await runAlvumJson(args, 5000);
      if (commandFailed(data)) return failure(data, fallback);
      broadcastState();
      return normalizeSpeakerSummary(data);
    });
    mutationQueue = run.catch(() => {});
    return run;
  }

  async function speakerList() {
    const data = await runAlvumJson(['speakers', 'list', '--json'], 5000);
    if (commandFailed(data)) return failure(data, 'speaker list failed', { speakers: [] });
    return normalizeSpeakerSummary(data);
  }

  async function speakerLink(id, interestId) {
    return runMutation(['speakers', 'link', String(id || ''), String(interestId || ''), '--json'], 'speaker link failed');
  }

  async function speakerSamples() {
    const data = await runAlvumJson(['speakers', 'samples', '--json'], 5000);
    if (commandFailed(data)) return failure(data, 'speaker samples failed', { speakers: [], samples: [] });
    return normalizeSpeakerSummary(data);
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

  async function speakerUnlinkInterest(interestId) {
    return runMutation(['speakers', 'unlink-interest', String(interestId || ''), '--json'], 'speaker interest unlink failed');
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
    return normalizeSampleAudio(sample, resolved.path);
  }

  async function voiceSampleAudio(sampleId) {
    const data = await speakerSamples();
    if (!data || data.ok === false) return data;
    const sample = Array.isArray(data.samples)
      ? data.samples.find((item) => item && item.sample_id === String(sampleId || ''))
      : null;
    const resolved = resolveSample(sample);
    if (!resolved.ok) return resolved;
    return normalizeSampleAudio(sample, resolved.path);
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
    speakerUnlinkInterest,
    speakerRename,
    speakerMerge,
    speakerForget,
    speakerReset,
    speakerSampleAudio,
    voiceSampleAudio,
  };
}

function normalizeSpeakerSummary(data) {
  const source = isPlainObject(data) ? data : {};
  return {
    ...source,
    ok: source.ok === false ? false : true,
    path: stringOrNull(source.path),
    speakers: arrayOf(source.speakers, normalizeSpeakerSummaryItem),
    clusters: arrayOf(source.clusters, normalizeSpeakerSummaryItem),
    samples: arrayOf(source.samples, normalizeVoiceSampleSummaryItem),
    voice_models: arrayOf(source.voice_models, normalizeObject),
    error: stringOrNull(source.error),
  };
}

function normalizeSpeakerSummaryItem(item) {
  const source = isPlainObject(item) ? item : {};
  return {
    ...source,
    speaker_id: stringOrEmpty(source.speaker_id),
    label: stringOrNull(source.label),
    linked_interest_id: stringOrNull(source.linked_interest_id),
    linked_interest: normalizeInterest(source.linked_interest),
    fingerprint_count: numberOrZero(source.fingerprint_count),
    samples: arrayOf(source.samples, normalizeSpeakerSampleSummaryItem),
    person_candidates: arrayOf(source.person_candidates, normalizeCandidate),
    duplicate_candidates: arrayOf(source.duplicate_candidates, normalizeDuplicateCandidate),
    context_interests: arrayOf(source.context_interests, normalizeCandidate),
  };
}

function normalizeSpeakerSampleSummaryItem(item) {
  const source = isPlainObject(item) ? item : {};
  return {
    ...source,
    sample_id: stringOrNull(source.sample_id),
    text: stringOrEmpty(source.text),
    source: stringOrEmpty(source.source),
    ts: stringOrEmpty(source.ts),
    start_secs: numberOrZero(source.start_secs),
    end_secs: numberOrZero(source.end_secs),
    media_path: stringOrNull(source.media_path),
    mime: stringOrNull(source.mime),
  };
}

function normalizeVoiceSampleSummaryItem(item) {
  const source = normalizeSpeakerSampleSummaryItem(item);
  return {
    ...source,
    sample_id: stringOrEmpty(source.sample_id),
    cluster_id: stringOrEmpty(source.cluster_id),
    quality_flags: arrayOfValues(source.quality_flags, stringOrEmpty),
    assignment_source: stringOrNull(source.assignment_source),
    linked_interest_id: stringOrNull(source.linked_interest_id),
    linked_interest: normalizeInterest(source.linked_interest),
    person_candidates: arrayOf(source.person_candidates, normalizeCandidate),
    context_interests: arrayOf(source.context_interests, normalizeCandidate),
  };
}

function normalizeSampleAudio(sample, resolvedPath) {
  const source = normalizeSpeakerSampleSummaryItem(sample);
  return {
    ok: true,
    sample_id: source.sample_id,
    url: pathToFileURL(resolvedPath).toString(),
    start_secs: source.start_secs,
    end_secs: source.end_secs,
    mime: source.mime,
  };
}

function normalizeInterest(value) {
  if (!isPlainObject(value)) return null;
  return {
    ...value,
    id: stringOrEmpty(value.id),
    type: stringOrEmpty(value.type),
    name: stringOrEmpty(value.name),
  };
}

function normalizeCandidate(value) {
  if (!isPlainObject(value)) return {};
  return {
    ...value,
    id: stringOrEmpty(value.id),
    type: stringOrEmpty(value.type),
    name: stringOrEmpty(value.name),
    score: numberOrZero(value.score),
    reason: stringOrEmpty(value.reason),
  };
}

function normalizeDuplicateCandidate(value) {
  if (!isPlainObject(value)) return {};
  return {
    ...value,
    speaker_id: stringOrEmpty(value.speaker_id),
    label: stringOrNull(value.label),
    linked_interest_id: stringOrNull(value.linked_interest_id),
    score: numberOrZero(value.score),
  };
}

function normalizeObject(value) {
  return isPlainObject(value) ? value : {};
}

function arrayOf(value, normalize) {
  if (!Array.isArray(value)) return [];
  return value
    .filter(isPlainObject)
    .map(normalize);
}

function arrayOfValues(value, normalize) {
  if (!Array.isArray(value)) return [];
  return value.map(normalize);
}

function stringOrEmpty(value) {
  return value == null ? '' : String(value);
}

function stringOrNull(value) {
  return value == null || value === '' ? null : String(value);
}

function numberOrZero(value) {
  const number = Number(value);
  return Number.isFinite(number) ? number : 0;
}

function isPlainObject(value) {
  return value != null && typeof value === 'object' && !Array.isArray(value);
}

module.exports = { createSpeakerService };
