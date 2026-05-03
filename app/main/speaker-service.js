const { pathToFileURL } = require('url');

function createSpeakerService({ runAlvumJson, fs, path, CAPTURE_DIR }) {
  async function speakerList() {
    const data = await runAlvumJson(['speakers', 'list', '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], error: data && data.error ? data.error : 'speaker list failed' };
    }
    return data;
  }

  async function speakerLink(id, interestId) {
    const data = await runAlvumJson(['speakers', 'link', String(id || ''), String(interestId || ''), '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], error: data && data.error ? data.error : 'speaker link failed' };
    }
    return data;
  }

  async function speakerSamples() {
    const data = await runAlvumJson(['speakers', 'samples', '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], samples: [], error: data && data.error ? data.error : 'speaker samples failed' };
    }
    return data;
  }

  async function speakerLinkSample(sampleId, interestId) {
    const data = await runAlvumJson(['speakers', 'link-sample', String(sampleId || ''), String(interestId || ''), '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], samples: [], error: data && data.error ? data.error : 'speaker sample link failed' };
    }
    return data;
  }

  async function speakerMoveSample(sampleId, clusterId) {
    const data = await runAlvumJson(['speakers', 'move-sample', String(sampleId || ''), String(clusterId || ''), '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], samples: [], error: data && data.error ? data.error : 'speaker sample move failed' };
    }
    return data;
  }

  async function speakerIgnoreSample(sampleId) {
    const data = await runAlvumJson(['speakers', 'ignore-sample', String(sampleId || ''), '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], samples: [], error: data && data.error ? data.error : 'speaker sample ignore failed' };
    }
    return data;
  }

  async function speakerSplit(clusterId, sampleIds) {
    const args = ['speakers', 'split', String(clusterId || '')];
    for (const sampleId of Array.isArray(sampleIds) ? sampleIds : []) {
      args.push('--sample', String(sampleId || ''));
    }
    args.push('--json');
    const data = await runAlvumJson(args, 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], samples: [], error: data && data.error ? data.error : 'speaker split failed' };
    }
    return data;
  }

  async function speakerRecluster() {
    const data = await runAlvumJson(['speakers', 'recluster', '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], samples: [], error: data && data.error ? data.error : 'speaker recluster failed' };
    }
    return data;
  }

  async function speakerUnlink(id) {
    const data = await runAlvumJson(['speakers', 'unlink', String(id || ''), '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], error: data && data.error ? data.error : 'speaker unlink failed' };
    }
    return data;
  }

  async function speakerRename(id, label) {
    const data = await runAlvumJson(['speakers', 'rename', String(id || ''), String(label || ''), '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], error: data && data.error ? data.error : 'speaker rename failed' };
    }
    return data;
  }

  async function speakerMerge(sourceId, targetId) {
    const data = await runAlvumJson(['speakers', 'merge', String(sourceId || ''), String(targetId || ''), '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], error: data && data.error ? data.error : 'speaker merge failed' };
    }
    return data;
  }

  async function speakerForget(id) {
    const data = await runAlvumJson(['speakers', 'forget', String(id || ''), '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], error: data && data.error ? data.error : 'speaker forget failed' };
    }
    return data;
  }

  async function speakerReset() {
    const data = await runAlvumJson(['speakers', 'reset', '--json'], 5000);
    if (!data || data.error) {
      return { ok: false, speakers: [], error: data && data.error ? data.error : 'speaker reset failed' };
    }
    return data;
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
      url: pathToFileURL(resolved.path).toString(),
      start_secs: Number(sample.start_secs || 0),
      end_secs: Number(sample.end_secs || 0),
      mime: sample.mime || null,
    };
  }

  function resolveSample(sample) {
    if (!sample) return { ok: false, error: 'sample audio is unavailable' };
    const direct = resolveSamplePath(sample.media_path);
    if (direct.ok || sample.media_path) return direct;
    return resolveSamplePath(pathForSampleTimestamp(sample));
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
