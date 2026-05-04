const assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const { createBriefingService } = require('../main/briefing-service');
const { createArtifactStore } = require('../main/briefing/artifacts');
const { createBriefingRunStore } = require('../main/briefing/run-store');
const { createBriefingWatchers } = require('../main/briefing/watchers');
const { createProviderService } = require('../main/provider-service');
const { createSpeakerService } = require('../main/speaker-service');
const runtimeModule = require('../main/runtime');
const { createSynthesisScheduler } = require('../main/synthesis-scheduler');
const { createUpdateService } = require('../main/update-service');

function readRendererSources(dir) {
  return fs.readdirSync(dir, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => {
      const file = path.join(dir, entry.name);
      if (entry.isDirectory()) return readRendererSources(file);
      if (!/\.(ts|css)$/.test(entry.name)) return [];
      return fs.readFileSync(file, 'utf8');
    });
}

function readJsSources(dir) {
  return fs.readdirSync(dir, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => {
      const file = path.join(dir, entry.name);
      if (entry.isDirectory()) return readJsSources(file);
      if (!/\.js$/.test(entry.name)) return [];
      return [fs.readFileSync(file, 'utf8')];
    });
}

function readMainSources(dir) {
  const rootFile = path.join(dir, 'main.js');
  const moduleDir = path.join(dir, 'main');
  return [fs.readFileSync(rootFile, 'utf8')].concat(readJsSources(moduleDir));
}

const rawHtml = fs.readFileSync(path.join(__dirname, '..', 'popover.html'), 'utf8');
const popoverCss = fs.readFileSync(path.join(__dirname, '..', 'src', 'renderer', 'styles', 'popover.css'), 'utf8');
const rendererSources = readRendererSources(path.join(__dirname, '..', 'src', 'renderer')).join('\n');
const html = `${rawHtml}\n${rendererSources}`;
const main = readMainSources(path.join(__dirname, '..')).join('\n');
const preload = fs.readFileSync(path.join(__dirname, '..', 'popover-preload.js'), 'utf8');
const briefingScript = fs.readFileSync(path.join(__dirname, '..', '..', 'scripts', 'briefing.sh'), 'utf8');
const installScript = fs.readFileSync(path.join(__dirname, '..', '..', 'scripts', 'install.sh'), 'utf8');
const launchdBriefing = fs.readFileSync(path.join(__dirname, '..', '..', 'launchd', 'com.alvum.briefing.plist'), 'utf8');
const wakeSchedulerScript = fs.readFileSync(path.join(__dirname, '..', '..', 'scripts', 'wake-scheduler.sh'), 'utf8');
const pipelineCargo = fs.readFileSync(path.join(__dirname, '..', '..', 'crates', 'alvum-pipeline', 'Cargo.toml'), 'utf8');
const pipelineExtract = fs.readFileSync(path.join(__dirname, '..', '..', 'crates', 'alvum-pipeline', 'src', 'extract.rs'), 'utf8');
const pipelineLlm = fs.readFileSync(path.join(__dirname, '..', '..', 'crates', 'alvum-pipeline', 'src', 'llm.rs'), 'utf8');
const coreProgress = fs.readFileSync(path.join(__dirname, '..', '..', 'crates', 'alvum-core', 'src', 'progress.rs'), 'utf8');

function scriptTomlSection(source, section) {
  const escaped = section.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = source.match(new RegExp(`\\[${escaped}\\]([\\s\\S]*?)(?=\\n\\[|\\nEOF|$)`));
  return match ? match[1] : '';
}

function writeSchedulerConfig(file, values) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, [
    '[scheduler.synthesis]',
    `enabled = ${values.enabled === true}`,
    `time = "${values.time || '07:00'}"`,
    `policy = "${values.policy || 'completed_days'}"`,
    `setup_completed = ${values.setup_completed === true}`,
    `last_auto_run_date = "${values.last_auto_run_date || ''}"`,
    '',
  ].join('\n'));
}

function schedulerConfigRunner(file, values) {
  return async (args) => {
    assert.equal(args[0], 'config-set');
    assert.match(args[1], /^scheduler\.synthesis\./);
    const key = args[1].replace(/^scheduler\.synthesis\./, '');
    if (key === 'enabled' || key === 'setup_completed') values[key] = args[2] === 'true';
    else values[key] = args[2];
    writeSchedulerConfig(file, values);
    return { ok: true, stdout: '' };
  };
}

function launchctlSpawn() {
  const child = new EventEmitter();
  process.nextTick(() => child.emit('close', 0));
  return child;
}

async function waitFor(predicate, label) {
  for (let i = 0; i < 50; i += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  assert.fail(label || 'condition not met');
}

function deferred() {
  const pending = {};
  pending.promise = new Promise((resolve, reject) => {
    pending.resolve = resolve;
    pending.reject = reject;
  });
  return pending;
}

test('popover shell loads bundled renderer assets', () => {
  assert.match(rawHtml, /<link rel="stylesheet" href="\.\/renderer-dist\/popover\.css">/);
  assert.match(rawHtml, /<script src="\.\/renderer-dist\/popover\.js"><\/script>/);
  assert.doesNotMatch(rawHtml, /<style>/);
  assert.doesNotMatch(rawHtml, /<script>\s/);
  assert.match(rendererSources, /appContext/);
  assert.match(rendererSources, /interface FeatureModule/);
  assert.match(rendererSources, /export function installMockAlvum/);
});

test('popover header exposes the current app version from update state', () => {
  assert.match(rawHtml, /id="version-label" class="version-label" hidden/);
  assert.match(html, /function renderVersionLabel\(\)/);
  assert.match(html, /updateState\.currentVersion/);
  assert.match(html, /label\.textContent = version \? `v\$\{version\}` : ''/);
  assert.match(html, /renderVersionLabel\(\)/);
});

test('updates panel exposes a manual check that bypasses scheduled throttling', () => {
  assert.match(preload, /updateCheck:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:update-check'\)/);
  assert.match(main, /ipcMain\.handle\('alvum:update-check', \(\) =>\s+update\.checkForUpdates\(true\)\)/);
  assert.match(rawHtml, /id="update-check-now" type="button">Check now<\/button>/);
  assert.match(html, /Auto-checks once per day; Check now bypasses the daily throttle\./);
  assert.match(html, /window\.alvum\.updateCheck\(\)/);
  assert.match(html, /state\.status === 'checking' \|\| state\.status === 'downloading' \|\| state\.status === 'installing'/);
  assert.match(html, /update-panel-actions'\)\.className = `footer-buttons \$\{ready \? 'two' : 'single'\}`/);
  assert.match(html, /try \{[\s\S]*?window\.alvum\.updateInstall\(\)[\s\S]*?\} catch \(err\) \{/);
  assert.match(main, /if \(app\.isQuitting\) return;[\s\S]*?e\.preventDefault\?\.\(\);/);
});

test('update install recovers when quitAndInstall throws or does not quit', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-updates-'));
  const updateStateFile = path.join(root, 'update-check.json');
  const logs = [];
  const broadcasts = [];
  const app = {
    isPackaged: true,
    isQuitting: false,
    getVersion: () => '0.1.7',
  };
  const updater = new EventEmitter();
  updater.setFeedURL = () => {};
  updater.quitAndInstall = () => {
    throw new Error('install failed');
  };
  const service = createUpdateService({
    app,
    autoUpdater: updater,
    updaterLoadError: null,
    fs,
    path,
    UPDATE_FEED: { provider: 'github', owner: 'mtfang', repo: 'alvum' },
    UPDATE_STATE_FILE: updateStateFile,
    UPDATE_STARTUP_DELAY_MS: 1,
    UPDATE_CHECK_INTERVAL_MS: 1000,
    UPDATE_INSTALL_TIMEOUT_MS: 5,
    appendShellLog: (line) => logs.push(line),
    notify: () => {},
    broadcastState: () => broadcasts.push(Date.now()),
  });

  service.setUpdateState({ status: 'downloaded', latestVersion: '0.1.10' });
  const failed = service.installDownloadedUpdate();
  assert.equal(failed.ok, false);
  assert.equal(failed.state.status, 'downloaded');
  assert.equal(app.isQuitting, false);
  assert.match(failed.error, /install failed/);

  updater.quitAndInstall = () => {};
  service.setUpdateState({ status: 'downloaded', latestVersion: '0.1.10', error: null });
  const started = service.installDownloadedUpdate();
  assert.equal(started.ok, true);
  assert.equal(started.state.status, 'installing');
  assert.equal(app.isQuitting, true);

  await waitFor(() => service.updateSnapshot().status === 'downloaded', 'install fallback did not restore downloaded state');
  assert.equal(app.isQuitting, false);
  assert.match(service.updateSnapshot().error, /Restart did not begin/);
  assert.equal(logs.some((line) => line.includes('quitAndInstall did not quit')), true);
  assert.ok(broadcasts.length > 0);
});

test('main menu is ordered capture connectors providers synthesis with quiet labels', () => {
  const main = html.match(/<section class="view" data-view="main">([\s\S]*?)<\/section>/)[1];
  const capture = html.indexOf('id="capture-summary"');
  const connectors = html.indexOf('id="extension-summary"');
  const providers = html.indexOf('id="provider-summary"');
  const synthesis = html.indexOf('id="briefing-summary"');
  assert.ok(capture > -1, 'capture summary exists');
  assert.ok(connectors > capture, 'connectors follows capture');
  assert.ok(providers > connectors, 'providers follows connectors');
  assert.ok(synthesis > providers, 'synthesis follows providers');

  assert.match(main, /id="capture-summary" class="summary-row drill-row" role="button" tabindex="0"/);
  assert.match(main, /id="capture-summary"[\s\S]*?<span class="action-hint" aria-hidden="true">›<\/span>/);
  assert.doesNotMatch(main, /id="capture-stats"/);
  assert.doesNotMatch(main, /id="extension-label"/);
  assert.doesNotMatch(main, /id="provider-label"/);
  assert.doesNotMatch(main, /id="briefing-label"/);
  assert.doesNotMatch(main, /id="briefing-latest"/);
  assert.doesNotMatch(main, /603 files/);
  assert.doesNotMatch(main, /4\/4 connectors/);
  assert.doesNotMatch(main, /connected<\/div>/);
  assert.doesNotMatch(main, />Ready<\/div>/);
});

test('capture directory action lives inside the capture subpane', () => {
  const main = html.match(/<section class="view" data-view="main">([\s\S]*?)<\/section>/)[1];
  const capturePane = html.match(/<section class="view" data-view="capture" hidden>([\s\S]*?)<\/section>/)[1];
  assert.doesNotMatch(main, /id="open-capture-dir"/);
  assert.match(capturePane, /id="open-capture-dir"/);
  assert.match(html, /\$\('capture-summary'\)\.onclick = \(e\) =>/);
  assert.match(html, /setView\('capture'\)/);
});

test('capture pane is read-only status and leaves source changes to connectors', () => {
  const capturePane = html.match(/<section class="view" data-view="capture" hidden>([\s\S]*?)<\/section>/)[1];
  assert.match(capturePane, /id="capture-inputs-title">Sources<\/span>/);
  assert.doesNotMatch(capturePane, /id="capture-inputs-refresh"/);
  assert.match(html, /function renderCaptureInputList/);
  assert.match(html, /row\.className = 'input-row status-only-row'/);
  assert.match(html, /state\.className = `state-badge \$\{input\.enabled \? 'on' : ''\}`/);
  assert.doesNotMatch(html, /captureInputParent = activeView === 'extensions' \? 'extensions' : 'capture'/);
  assert.doesNotMatch(html, /\$\('capture-inputs-refresh'\)\.onclick/);
});

test('briefing surface is renamed to synthesis in visible menu and actions', () => {
  assert.match(html, /<div class="label">Synthesis<\/div>/);
  assert.match(html, /briefing: 'Synthesis'/);
  assert.match(html, /'briefing-reader': 'Synthesis'/);
  assert.match(html, /'decision-graph': 'Decision Graph'/);
  assert.match(html, /generate\.textContent = queuedForDay\s+\? 'Queued'\s+: \(day\.hasBriefing \? 'Resynthesize' : \(day\.status === 'failed' \? 'Retry' : 'Synthesize'\)\)/);
  assert.match(html, /view\.textContent = 'View Synthesis'/);
  assert.match(html, /title\.textContent = runningForDay\s+\? `\$\{cancelingForDay \? 'Canceling' : 'Synthesizing'\}/);
  assert.match(html, /'day': 'Compose synthesis'/);
  assert.doesNotMatch(html, />Briefing<\/div>/);
  assert.doesNotMatch(html, />Generate briefing</);
  assert.doesNotMatch(html, />Regenerate</);
  assert.doesNotMatch(html, />View briefing</);
});

test('failed synthesis actions expose retry and keep diagnostics inside details view', () => {
  assert.match(html, /else if \(day\.status === 'failed'\) \{[\s\S]*?details\.textContent = 'View details'[\s\S]*?generate\.classList\.remove\('full-row'\);[\s\S]*?actions\.append\(generate, details, manageVoices\);/);
  assert.doesNotMatch(html, /copy\.textContent = 'Copy diagnostics'/);
  assert.doesNotMatch(html, /openLogs\.textContent = 'Open logs'/);
  assert.match(rawHtml, /id="briefing-log-copy" type="button">Copy details<\/button>/);
  assert.doesNotMatch(rawHtml, />Copy log<\/button>/);
});

test('tracked voices move speaker identity review under synthesis tracked items', () => {
  assert.match(rawHtml, /data-view="profile-voices-list"/);
  assert.match(rawHtml, /data-view="profile-voice-detail"/);
  assert.match(html, /'profile-voices-list': 'Voices'/);
  assert.match(html, /'profile-voice-detail': 'Link Voice'/);
  assert.match(html, /function renderProfileVoices/);
  assert.match(html, /function renderProfileVoiceDetail/);
  assert.match(html, /setView\('profile-voices-list'\)/);
  assert.match(html, /Link voice/);
  assert.match(html, /Create tracked person/);
  assert.match(html, /placeholder = 'Tracked person name'/);
  assert.match(html, /Merge cluster/);
  assert.match(html, /Review queue/);
  assert.match(html, /Link clip/);
  assert.match(html, /Move clip/);
  assert.match(html, /New cluster from clip/);
  assert.match(html, /Ignore clip/);
  assert.match(html, /ignored_by_user/);
  assert.match(html, /Context mentioned nearby/);
  assert.doesNotMatch(html, /window\.prompt\('Tracked person name'/);
  assert.match(preload, /speakerLinkSample:\s+\(sampleId, interestId\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-link-sample', sampleId, interestId\)/);
  assert.match(preload, /speakerMoveSample:\s+\(sampleId, clusterId\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-move-sample', sampleId, clusterId\)/);
  assert.match(preload, /speakerIgnoreSample:\s+\(sampleId\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-ignore-sample', sampleId\)/);
  assert.match(preload, /speakerUnlinkSample:\s+\(sampleId\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-unlink-sample', sampleId\)/);
  assert.match(preload, /voiceSampleAudio:\s+\(sampleId\)\s+=>\s+ipcRenderer\.invoke\('alvum:voice-sample-audio', sampleId\)/);
  assert.match(preload, /speakerLink:\s+\(id, interestId\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-link', id, interestId\)/);
  assert.match(preload, /speakerUnlink:\s+\(id\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-unlink', id\)/);
  assert.match(preload, /speakerUnlinkInterest:\s+\(interestId\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-unlink-interest', interestId\)/);
  assert.match(html, /speakerUnlinkInterest:\s+async \(interestId\)/);
  assert.match(preload, /speakerSampleAudio:\s+\(id, sampleIndex\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-sample-audio', id, sampleIndex\)/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-link'/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-link-sample'/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-move-sample'/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-ignore-sample'/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-unlink-sample'/);
  assert.match(main, /ipcMain\.handle\('alvum:voice-sample-audio'/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-unlink'/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-unlink-interest'/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-sample-audio'/);
});

test('voices are managed from the selected synthesis day instead of the top-level menu', () => {
  const mainMenu = html.match(/<section class="view" data-view="main">([\s\S]*?)<\/section>/)[1];
  assert.doesNotMatch(mainMenu, /id="voices-summary"/);
  assert.match(rawHtml, /data-view="voices"/);
  assert.doesNotMatch(rawHtml, /id="voices-day-tabs"/);
  assert.match(rawHtml, /id="voices-source-filters"/);
  assert.match(html, /let selectedVoicePeople = null/);
  assert.match(html, /selectedPeople: voiceFilterSelectionValues\(selectedVoicePeople\)/);
  assert.match(html, /let voiceFilterMenuOpen = false/);
  assert.match(html, /renderVoiceFilterMenu\(sourceFilters, timeline\)/);
  assert.match(html, /function renderVoiceFilterMenu\(parent, timeline\)/);
  assert.match(html, /appendVoiceFilterMenuSection\(panel, 'Sources', sourceOptions, selectedVoiceSources, \(id\) => \{/);
  assert.match(html, /appendVoiceFilterMenuSection\(panel, 'People', peopleOptions, selectedVoicePeople, \(id\) => \{/);
  assert.match(html, /checkbox\.type = 'checkbox'/);
  assert.match(html, /checkbox\.checked = selected == null \|\| selected\.has\(option\.id\)/);
  assert.match(html, /'No sources'/);
  assert.match(html, /'Unassigned'/);
  assert.match(rendererSources, /\.voice-filter-menu/);
  assert.match(rendererSources, /\.voice-filter-panel/);
  assert.match(rendererSources, /\.voice-filter-option/);
  assert.match(rendererSources, /import '\.\/styles\/voices\.css'/);
  assert.ok(fs.existsSync(path.join(__dirname, '..', 'src', 'renderer', 'styles', 'voices.css')));
  assert.doesNotMatch(popoverCss, /\.voice-filter-menu|\.voices-timeline-shell|\.voice-turn-row/);
  assert.doesNotMatch(html, /function appendVoiceFilterRow/);
  assert.doesNotMatch(rendererSources, /\.voice-filter-scroll/);
  assert.doesNotMatch(rawHtml, /id="voices-selection-panel"/);
  assert.doesNotMatch(rendererSources, /\.voice-assignment-drawer/);
  assert.match(rawHtml, /id="voices-timeline-shell"/);
  assert.match(rawHtml, /id="voices-playback-controls"/);
  assert.match(rawHtml, /id="voices-playback-prev"/);
  assert.match(rawHtml, /id="voices-playback-toggle"/);
  assert.match(rawHtml, /id="voices-playback-next"/);
  assert.match(rawHtml, /id="voices-playback-prev"[^>]*aria-label="Previous voice block"[^>]*>‹<\/button>/);
  assert.match(rawHtml, /id="voices-playback-toggle"[^>]*aria-label="Play voice timeline"[^>]*>▶<\/button>/);
  assert.match(rawHtml, /id="voices-playback-next"[^>]*aria-label="Next voice block"[^>]*>›<\/button>/);
  assert.match(html, /toggle\.textContent = starting \? '…' : \(playing \? '⏸' : '▶'\)/);
  assert.ok(
    rawHtml.indexOf('id="voices-playback-controls"') < rawHtml.indexOf('id="voices-ruler-labels"'),
    'playback controls should be centered above the scrub timeline',
  );
  assert.match(rawHtml, /id="voices-timeline-actions"/);
  assert.match(rawHtml, /id="voices-ruler-labels"/);
  assert.match(rawHtml, /id="voices-waveform"/);
  assert.match(rawHtml, /id="voices-time-column"/);
  assert.match(rawHtml, /id="voices-turns"/);
  assert.match(rawHtml, /id="voices-load-more"/);
  assert.doesNotMatch(html, /function renderVoicesSummaryCard/);
  assert.doesNotMatch(html, /\$\('voices-summary'\)/);
  assert.match(html, /manageVoices\.textContent = 'Manage voices'/);
  assert.match(html, /function openVoicesForDate\(date\)/);
  assert.match(html, /selectedVoicesDay = date/);
  assert.match(html, /voices: 'Voices'/);
  assert.match(html, /if \(view === 'voices'\) return 'briefing'/);
  assert.match(html, /buildVoiceTimeline/);
  assert.doesNotMatch(html, /function renderVoiceDayTabs/);
  assert.match(html, /timeline\.visibleTurns/);
  assert.match(html, /timeline\.timeTicks/);
  assert.match(html, /voiceTimelineActionsForSample/);
  assert.doesNotMatch(html, /Fix diarization/);
  assert.doesNotMatch(html, /voice-fix-actions/);
  assert.doesNotMatch(html, /function appendSplitEditor/);
  assert.doesNotMatch(html, /window\.alvum\.speakerSplitSample\(sample\.sample_id/);
  assert.match(html, /selectedVoiceSampleId/);
  assert.doesNotMatch(html, /voiceMultiSelectMode/);
  assert.match(html, /voiceScrubberOffset/);
  assert.match(html, /expandedVoiceSampleId/);
  assert.match(html, /renderVoiceTimelineActions/);
  assert.match(html, /selectVoiceSample/);
  assert.match(html, /updateVoiceSelectionUi/);
  assert.match(html, /function scrubVoiceTimeline/);
  assert.match(html, /function toggleVoiceTimelinePlayback/);
  assert.match(html, /function startVoiceTimelinePlayback/);
  assert.match(html, /function playVoiceTimelineBlock/);
  assert.match(rendererSources, /class VoicePlaybackController/);
  assert.match(rendererSources, /export function voiceAudioPlaybackBounds/);
  assert.match(html, /const bounds = voiceAudioPlaybackBounds\(entry\.sample, result\)/);
  assert.match(html, /const bounds = voiceAudioPlaybackBounds\(sample, result\)/);
  assert.match(html, /voiceAudioCurrentTimeForTimelineMs\(entry\.sample, bounds, block\.startMs\)/);
  assert.doesNotMatch(html, /mediaClockShift/);
  assert.match(html, /function skipVoiceTimelinePlayback/);
  assert.match(html, /voiceTimelineContinuousPlaybackBlock\(activeVoiceTimeline, voiceScrubberOffset\)/);
  assert.match(html, /if \(!voiceTimelinePlaybackActive\(\) && !voiceTimelinePlaybackStarting\(\)\) syncVoiceScrubberToSelection\(timeline\)/);
  assert.doesNotMatch(html, /activeVoicePlayback|voicePlaybackStarting|voicePlaybackGeneration|voicePlaybackExpandsEditor/);
  assert.match(html, /const expandEditor = !!expandedVoiceSampleId/);
  assert.match(html, /playVoiceTimelineBlock\(block, \{ expandEditor \}\)/);
  const playbackSyncFn = html.match(/function syncVoicePlaybackPosition[\s\S]*?function stopVoiceTimelinePlayback/)[0];
  assert.match(playbackSyncFn, /options\.expandEditor === true/);
  assert.match(playbackSyncFn, /expandEditor/);
  assert.doesNotMatch(playbackSyncFn, /expandEditor:\s*true/);
  assert.match(html, /document\.addEventListener\('keydown'[\s\S]*?toggleVoiceTimelinePlayback/);
  assert.match(html, /document\.addEventListener\('keydown'[\s\S]*?isPreviousVoicePlaybackKey\(e\)[\s\S]*?skipVoiceTimelinePlayback\(-1\)/);
  assert.match(html, /document\.addEventListener\('keydown'[\s\S]*?isNextVoicePlaybackKey\(e\)[\s\S]*?skipVoiceTimelinePlayback\(1\)/);
  assert.match(html, /function isPreviousVoicePlaybackKey/);
  assert.match(html, /key === 'ArrowLeft' \|\| key === 'Left'/);
  assert.match(html, /function isNextVoicePlaybackKey/);
  assert.match(html, /key === 'ArrowRight' \|\| key === 'Right'/);
  assert.doesNotMatch(html, /e\.key === 'Tab'|e\.code === 'Tab'/);
  assert.match(html, /function isEditableKeyboardTarget/);
  assert.doesNotMatch(html, /input, textarea, select, button, \[contenteditable="true"\]/);
  assert.match(html, /function nearestVoiceSample/);
  assert.match(html, /nearestVoiceTimelineSample\(timeline, offset\)/);
  assert.match(rendererSources, /function selectVoiceTimelineSampleState/);
  assert.match(html, /selectVoiceTimelineSampleState\(/);
  assert.match(html, /function renderVoiceVisibleTurns/);
  const scrubFn = html.match(/function scrubVoiceTimeline[\s\S]*?function finishVoiceScrub/)[0];
  assert.match(scrubFn, /requestAnimationFrame\(applyPendingVoiceScrub\)/);
  assert.doesNotMatch(scrubFn, /selectVoiceSample|scrollIntoView|renderVoicesTimeline/);
  const finishScrubFn = html.match(/function finishVoiceScrub[\s\S]*?function applyPendingVoiceScrub/)[0];
  assert.match(finishScrubFn, /selectVoiceSample\(sampleId, \{ keepScrubber: true, syncWindow: true \}\)/);
  assert.doesNotMatch(finishScrubFn, /scroll:\s*true|scrollVoiceSampleIntoView|renderVoicesTimeline|visibleVoiceTurnLimit/);
  const applyScrubFn = html.match(/function applyPendingVoiceScrub[\s\S]*?function nearestVoiceSample/)[0];
  assert.match(applyScrubFn, /selectVoiceSample\(sample\.sample_id, \{ keepScrubber: true, syncWindow: true \}\)/);
  assert.doesNotMatch(applyScrubFn, /scrollVoiceSampleIntoView|scroll:\s*true/);
  assert.match(rendererSources, /next\.visibleLimit = Math\.max\(next\.visibleLimit, sampleIndex \+ 1\)/);
  assert.match(html, /setPointerCapture/);
  assert.match(html, /scrollIntoView/);
  assert.match(html, /className = 'voice-scrub-label'/);
  assert.match(html, /setAttribute\('role', 'slider'\)/);
  assert.doesNotMatch(html, /type = voiceMultiSelectMode \? 'checkbox' : 'radio'/);
  assert.doesNotMatch(html, /name = 'voice-turn-selection'/);
  assert.doesNotMatch(html, /selector\.type = 'checkbox'/);
  assert.match(html, /className = 'voice-play-button'/);
  assert.match(html, /aria-label', 'Play clip'/);
  assert.match(html, /textContent = '▶'/);
  assert.match(html, /className = 'voice-assignment-controls'/);
  assert.match(html, /className = 'voice-assignment-confirm'/);
  assert.match(html, /aria-label', 'Confirm suggested assignment'/);
  assert.match(html, /className = 'voice-assignment-unassign'/);
  assert.match(html, /aria-label', 'Unassign voice'/);
  assert.match(html, /icon\.className = 'voice-unassign-icon'/);
  assert.match(html, /unassignVoiceSample\(sample, unassign\)/);
  assert.doesNotMatch(html, /unassign\.textContent = '×'/);
  assert.match(html, /className = 'voice-assignment-edit'/);
  assert.match(html, /aria-label', 'Edit voice assignment'/);
  assert.match(html, /className = 'voice-inline-editor'/);
  assert.match(html, /function appendVoiceInlineEditor/);
  assert.match(html, /function voiceModelForInterest/);
  assert.match(html, /speakerSummary\.voice_models/);
  assert.match(html, /className = 'voice-assignment-evidence'/);
  assert.match(html, /Low confidence/);
  const assignmentEvidenceFn = html.match(/function voiceAssignmentEvidenceForPerson[\s\S]*?function appendVoiceSample/)[0];
  assert.match(assignmentEvidenceFn, /voiceAssignmentConfidenceLabel/);
  assert.doesNotMatch(assignmentEvidenceFn, /candidateEvidenceDetail|voiceModelEvidenceDetail|verified sample|source|holdout|margin|tight voice model|moderate voice model|broad voice model/);
  assert.match(html, /function quickAddVoicePerson/);
  assert.match(html, /placeholder = 'New tracked person'/);
  assert.match(html, /quickAdd\.textContent = 'Add and assign'/);
  assert.doesNotMatch(html, /textContent = 'Suggested assignment'/);
  assert.doesNotMatch(html, /textContent = 'Confirm suggestion'/);
  assert.match(html, /candidateMatchLabel/);
  assert.match(html, /candidateEvidenceDetail/);
  assert.doesNotMatch(html, /appendVoiceModelMetrics/);
  assert.doesNotMatch(html, /voiceModelItems/);
  assert.doesNotMatch(html, /predictor`/);
  assert.doesNotMatch(html, /confidence ·/);
  assert.doesNotMatch(html, /% confidence/);
  assert.doesNotMatch(html, /\$\{score\} match/);
  assert.doesNotMatch(html, /function renderVoiceSelectionPanel/);
  assert.doesNotMatch(html, /renderVoiceSelectionPanel\(selectionPanel, activeVoiceTimeline\)/);
  assert.match(html, /Load \$/);
  assert.match(html, /Extracted text/);
  assert.match(html, /Unassigned/);
  assert.match(html, /voice-assignment-chip/);
  assert.match(html, /assigned/);
  assert.doesNotMatch(html, /\$\{voiceAssignmentLabel\(sample\)\} · \$\{voiceAssignmentState\(sample\)\}/);
  assert.doesNotMatch(html, /label\.textContent = 'Extracted text'/);
  assert.doesNotMatch(html, /sample\.cluster_id,\s*day \? displayDate\(day\) : null/s);
});

test('audio connector detail links to tracked voices instead of rendering speaker cards', () => {
  assert.match(html, /Review voices/);
  assert.match(html, /Tracked voice identities are managed in Voices/);
  assert.doesNotMatch(html, /placeholder = 'Speaker label'/);
  assert.doesNotMatch(html, /textContent = 'Identity actions'/);
});

test('speaker sample audio resolver only exposes captured Alvum audio', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-speaker-samples-'));
  const captureDir = path.join(root, 'capture');
  const samplePath = path.join(captureDir, '2026-04-26', 'audio', 'mic', '09-42-00.wav');
  const legacyPath = path.join(captureDir, '2026-04-26', 'audio', 'mic', '10-12-14.wav');
  const outsidePath = path.join(root, 'outside.wav');
  fs.mkdirSync(path.dirname(samplePath), { recursive: true });
  fs.writeFileSync(samplePath, 'mock-audio');
  fs.writeFileSync(legacyPath, 'legacy-audio');
  fs.writeFileSync(outsidePath, 'outside');
  const listFor = (mediaPath) => ({
    ok: true,
    speakers: [{
      speaker_id: 'spk_local_michael',
      samples: [{ media_path: mediaPath, start_secs: 1, end_secs: 3, mime: 'audio/wav' }],
    }],
  });

  const allowed = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: captureDir,
    runAlvumJson: async () => listFor(samplePath),
  });
  const playable = await allowed.speakerSampleAudio('spk_local_michael', 0);
  assert.equal(playable.ok, true);
  assert.match(playable.url, /^file:/);
  assert.equal(playable.start_secs, 1);
  assert.equal(playable.end_secs, 3);

  const legacyTs = new Date(2026, 3, 26, 10, 12, 14).toISOString();
  const legacy = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: captureDir,
    runAlvumJson: async () => ({
      ok: true,
      speakers: [{
        speaker_id: 'spk_local_michael',
        samples: [{ source: 'audio-mic', ts: legacyTs, start_secs: 4, end_secs: 7, mime: 'audio/wav' }],
      }],
    }),
  });
  const legacyPlayable = await legacy.speakerSampleAudio('spk_local_michael', 0);
  assert.equal(legacyPlayable.ok, true);
  assert.match(legacyPlayable.url, /10-12-14\.wav$/);
  assert.equal(legacyPlayable.start_secs, 4);

  const denied = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: captureDir,
    runAlvumJson: async () => listFor(outsidePath),
  });
  assert.deepEqual(await denied.speakerSampleAudio('spk_local_michael', 0), {
    ok: false,
    error: 'sample audio path is outside Alvum capture storage',
  });
});

test('voice sample playback and split actions stay on sample ids and Alvum-owned paths', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-voice-sample-actions-'));
  const captureDir = path.join(root, 'capture');
  const samplePath = path.join(captureDir, '2026-05-02', 'audio', 'mic', '09-00-00.wav');
  const outsidePath = path.join(root, 'outside.wav');
  fs.mkdirSync(path.dirname(samplePath), { recursive: true });
  fs.writeFileSync(samplePath, 'mock-audio');
  fs.writeFileSync(outsidePath, 'outside-audio');
  const calls = [];
  const service = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: captureDir,
    runAlvumJson: async (args) => {
      calls.push(args);
      if (args[1] === 'samples') {
        return {
          ok: true,
          samples: [{
            sample_id: 'vsm_owned',
            cluster_id: 'spk_local_michael',
            media_path: samplePath,
            start_secs: 1,
            end_secs: 3,
            mime: 'audio/wav',
          }],
        };
      }
      return { ok: true, samples: [] };
    },
  });

  const playable = await service.voiceSampleAudio('vsm_owned');
  assert.equal(playable.ok, true);
  assert.match(playable.url, /^file:/);
  assert.deepEqual(await service.speakerSplitSample('vsm_owned', {
    at: 2,
    leftText: 'left',
    rightText: 'right',
  }), { ok: true, path: null, speakers: [], clusters: [], samples: [], voice_models: [], error: null });
  assert.deepEqual(calls.at(-1), [
    'speakers',
    'split-sample',
    'vsm_owned',
    '--at',
    '2',
    '--left-text',
    'left',
    '--right-text',
    'right',
    '--json',
  ]);

  const denied = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: captureDir,
    runAlvumJson: async () => ({
      ok: true,
      samples: [{ sample_id: 'vsm_escape', media_path: outsidePath }],
    }),
  });
  assert.deepEqual(await denied.voiceSampleAudio('vsm_escape'), {
    ok: false,
    error: 'sample audio path is outside Alvum capture storage',
  });
});

test('timestamp fallback sample audio keeps segment offsets relative to resolved media', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-voice-sample-fractional-'));
  const captureDir = path.join(root, 'capture');
  const samplePath = path.join(captureDir, '2026-05-02', 'audio', 'mic', '09-00-00.wav');
  fs.mkdirSync(path.dirname(samplePath), { recursive: true });
  fs.writeFileSync(samplePath, 'mock-audio');
  const service = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: captureDir,
    runAlvumJson: async () => ({
      ok: true,
      samples: [{
        sample_id: 'vsm_fractional',
        source: 'audio-mic',
        ts: '2026-05-02T09:00:00.755',
        start_secs: 1,
        end_secs: 3,
        mime: 'audio/wav',
      }],
    }),
  });

  const playable = await service.voiceSampleAudio('vsm_fractional');

  assert.equal(playable.ok, true);
  assert.equal(playable.sample_id, 'vsm_fractional');
  assert.equal(playable.start_secs, 1);
  assert.equal(playable.end_secs, 3);
});

test('voice registry mutations are serialized and broadcast only after successful writes', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-speaker-mutations-'));
  const pending = [];
  const calls = [];
  const broadcasts = [];
  const service = createSpeakerService({
    fs,
    path,
    CAPTURE_DIR: root,
    broadcastState: () => broadcasts.push(calls.length),
    runAlvumJson: async (args) => {
      const request = deferred();
      calls.push(args);
      pending.push(request);
      return request.promise;
    },
  });

  const first = service.speakerLinkSample('vsm_a', 'person_a');
  const second = service.speakerMoveSample('vsm_b', 'speaker_b');
  const third = service.speakerUnlinkSample('vsm_c');
  await waitFor(() => calls.length === 1, 'first speaker mutation did not start');
  assert.deepEqual(calls[0], ['speakers', 'link-sample', 'vsm_a', 'person_a', '--json']);
  assert.equal(pending.length, 1);

  pending.shift().resolve({ ok: true, speakers: [{ speaker_id: 'speaker_a' }], samples: [] });
  const firstResult = await first;
  assert.equal(firstResult.ok, true);
  assert.deepEqual(firstResult.speakers.map((speaker) => speaker.speaker_id), ['speaker_a']);
  assert.deepEqual(firstResult.samples, []);
  await waitFor(() => calls.length === 2, 'second speaker mutation did not start after first');
  assert.deepEqual(calls[1], ['speakers', 'move-sample', 'vsm_b', 'speaker_b', '--json']);
  assert.deepEqual(broadcasts, [1]);

  pending.shift().resolve({ ok: false, error: 'registry locked' });
  assert.deepEqual(await second, { ok: false, path: null, speakers: [], clusters: [], samples: [], voice_models: [], error: 'registry locked' });
  assert.deepEqual(broadcasts, [1]);
  await waitFor(() => calls.length === 3, 'third speaker mutation did not start after second');
  assert.deepEqual(calls[2], ['speakers', 'unlink-sample', 'vsm_c', '--json']);

  pending.shift().resolve({ ok: true, speakers: [{ speaker_id: 'speaker_c' }], samples: [] });
  const thirdResult = await third;
  assert.equal(thirdResult.ok, true);
  assert.deepEqual(thirdResult.speakers.map((speaker) => speaker.speaker_id), ['speaker_c']);
  assert.deepEqual(thirdResult.samples, []);
  assert.deepEqual(broadcasts, [1, 3]);
});

test('failed voice registry mutations preserve the rendered speaker cache', () => {
  assert.match(html, /if \(result && result\.ok !== false && Array\.isArray\(result\.speakers\)\) \{/);
  assert.doesNotMatch(html, /if \(result && Array\.isArray\(result\.speakers\)\) \{\s*speakerSummary = result;/);
});

test('synthesis calendar surfaces stale voice markers as resynthesis prompts', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-stale-voices-'));
  const briefings = path.join(root, 'briefings');
  const dayDir = path.join(briefings, '2026-05-02');
  fs.mkdirSync(dayDir, { recursive: true });
  fs.writeFileSync(path.join(dayDir, 'briefing.md'), '# Existing synthesis');
  fs.writeFileSync(path.join(dayDir, 'voice.stale.json'), JSON.stringify({
    date: '2026-05-02',
    kind: 'voice_identity',
    sample_id: 'vsm_owned',
    marked_at: '2026-05-03T08:00:00Z',
  }));
  const calendar = require('../main/briefing/calendar').createBriefingCalendar({
    fs,
    path,
    BRIEFINGS_DIR: briefings,
    todayStamp: () => '2026-05-03',
    artifactSummaryForDate: () => ({ files: 4, bytes: 100, summary: '4 files · 100 B' }),
    readBriefingFailure: () => null,
    latestBriefingRunInfo: () => null,
  });

  const day = calendar.briefingDayInfo('2026-05-02');
  assert.equal(day.hasBriefing, true);
  assert.equal(day.status, 'success');
  assert.equal(day.staleVoice, true);
  assert.equal(day.staleVoiceMarker.sample_id, 'vsm_owned');
  assert.match(html, /day\.staleVoice/);
  assert.match(html, /Voice labels changed/);
  assert.match(html, /dot\.classList\.add\('stale-voice'\)/);
});

test('synthesis exposes per-day decision graph artifacts', () => {
  assert.match(main, /function readDecisionGraphForDate\(date\)/);
  assert.match(main, /path\.join\(dir, 'decisions\.jsonl'\)/);
  assert.match(main, /path\.join\(dir, 'tree', 'L4-edges\.jsonl'\)/);
  assert.match(main, /path\.join\(dir, 'tree', 'L4-domains\.jsonl'\)/);
  assert.match(main, /path\.join\(dir, 'synthesis-profile\.snapshot\.json'\)/);
  assert.match(main, /function fallbackDecisionGraphEdges\(decisions\)/);
  assert.match(main, /derived_from_decisions: true/);
  assert.match(main, /ipcMain\.handle\('alvum:decision-graph-date'/);
  assert.match(preload, /decisionGraphDate:\s+\(date\)\s+=>\s+ipcRenderer\.invoke\('alvum:decision-graph-date', date\)/);
  assert.match(html, /data-view="decision-graph"/);
  assert.match(html, /id="decision-graph-canvas"/);
  assert.match(html, /id="decision-graph-hover"/);
  assert.match(html, /id="decision-graph-legend"/);
  assert.ok(
    html.indexOf('id="decision-graph-title"') < html.indexOf('id="decision-graph-canvas"'),
    'decision graph title should render before the graph canvas',
  );
  assert.match(html, /graph\.textContent = 'Decision graph'/);
  assert.match(html, /graph\.onclick = \(\) => openDecisionGraphView\(day\.date\)/);
  assert.match(html, /function decisionGraphSvg\(data, selectedId\)/);
  assert.match(html, /function graphComponents\(decisions, edges\)/);
  assert.match(html, /function decisionGraphLaneCount\(component, componentEdges\)/);
  assert.match(html, /function decisionGraphLaneOrder\(laneCount\)/);
  assert.match(html, /function decisionGraphLaneY\(top, bottom, lane, laneCount\)/);
  assert.match(html, /function decisionGraphEdgeBend\(edge, index, from, to\)/);
  assert.match(html, /value\.incoming > 1 \|\| value\.outgoing > 2/);
  assert.match(html, /function relaxDecisionGraphNodes\(nodes, bounds\)/);
  assert.match(html, /function layoutDecisionGraph\(data\)/);
  assert.match(html, /targetY: laneTargetY/);
  assert.match(html, /decisionGraphEdgeBend\(edge, index, from, to\)/);
  assert.match(html, /summary: \{ decision_count: 10, edge_count: 9, domain_count: 3 \}/);
  assert.match(html, /effects: \['dec_002', 'dec_003', 'dec_004'\]/);
  assert.match(html, /from_id: 'dec_003', to_id: 'dec_008'/);
  assert.match(html, /from_id: 'dec_004', to_id: 'dec_010'/);
  assert.match(html, /isolated\.length >= 10/);
  assert.match(html, /decision-graph-component-band/);
  assert.match(html, /function renderDecisionGraphLegend\(data\)/);
  assert.match(html, /function showDecisionGraphHover\(decision, event = null\)/);
  assert.match(html, /className = 'decision-graph-selected'/);
  assert.match(html, /grid-template-columns: minmax\(0, 1fr\) minmax\(0, 1fr\)/);
  assert.match(html, /className = 'decision-graph-link-groups'/);
  assert.match(html, /className = 'decision-graph-link-group'/);
  assert.match(html, /className = 'decision-graph-link-chip'/);
  assert.match(html, /function selectDecisionGraphNode\(id\)/);
  assert.match(html, /button\.onclick = \(\) => selectDecisionGraphNode\(decision\.id\)/);
  assert.match(html, /Previous/);
  assert.match(html, /Next/);
  assert.doesNotMatch(html, /if \(!rows\.length\) return/);
  assert.doesNotMatch(html, /No previous nodes/);
  assert.doesNotMatch(html, /No next nodes/);
  assert.match(html, /node\.addEventListener\('mouseenter'/);
  assert.match(html, /node\.addEventListener\('mousemove'/);
  assert.match(html, /function renderDecisionGraphView\(\)/);
  assert.match(html, /window\.alvum\.decisionGraphDate\(date\)/);
  assert.match(html, /if \(view === 'decision-graph'\) return 'briefing'/);
  assert.doesNotMatch(html, /\["Career", "Health", "Family", "Creative", "Finances"\] as const/);
  assert.doesNotMatch(html, /function graphDomains\(data\)/);
  assert.doesNotMatch(html, /laneY/);
});

test('synthesis progress logs persist per processed day', () => {
  assert.match(briefingScript, /run_dir="\$out_dir\/runs\/\$run_id"/);
  assert.match(briefingScript, /progress_file="\$run_dir\/progress\.jsonl"/);
  assert.match(briefingScript, /events_file="\$run_dir\/events\.jsonl"/);
  assert.match(briefingScript, /stdout_log="\$run_dir\/stdout\.log"/);
  assert.match(briefingScript, /stderr_log="\$run_dir\/stderr\.log"/);
  assert.match(briefingScript, /status_file="\$run_dir\/status\.json"/);
  assert.match(briefingScript, /ALVUM_PROGRESS_FILE="\$progress_file" ALVUM_PIPELINE_EVENTS_FILE="\$events_file" "\$ALVUM_BIN" extract/);
  assert.match(briefingScript, /write_failure_marker "\$date" "\$out_dir" "\$run_id" "\$run_dir" "\$reason" "\$code" "\$stderr_log"/);
});

test('synthesis event tailer buffers partial lines and recovers rewritten files', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-watchers-'));
  const runtime = path.join(root, 'runtime');
  fs.mkdirSync(runtime, { recursive: true });
  const eventsFile = path.join(runtime, 'pipeline.events');
  const events = [];
  const sends = [];
  const logs = [];
  const watchers = createBriefingWatchers({
    fs,
    path,
    ALVUM_ROOT: root,
    appendShellLog: (line) => logs.push(line),
    recordProviderEvent: (event) => events.push(event),
    sendToPopover: (channel, payload) => sends.push({ channel, payload }),
    getRuns: () => [],
  });

  fs.writeFileSync(eventsFile, '{"ts":1,"kind":"stage_enter"');
  watchers.pollEvents();
  assert.equal(events.length, 0);
  assert.equal(logs.some((line) => line.includes('bad JSON')), false);

  fs.appendFileSync(eventsFile, ',"stage":"gather"}\n');
  watchers.pollEvents();
  assert.equal(events.length, 1);
  assert.equal(events[0].stage, 'gather');
  assert.equal(sends[0].channel, 'alvum:event');

  fs.writeFileSync(eventsFile, '{"ts":2,"kind":"stage_enter","stage":"domain-correlate","detail":"rewritten file grew past the previous cursor"}\n');
  watchers.pollEvents();
  assert.equal(events.length, 2);
  assert.equal(events[1].ts, 2);
  assert.equal(events[1].stage, 'domain-correlate');
  assert.equal(logs.some((line) => line.includes('bad JSON')), false);
});

test('scripted catch-up runs are registered as per-day live runs', () => {
  assert.match(briefingScript, /emit_run_marker/);
  assert.match(briefingScript, /\[alvum-run\]/);
  assert.match(briefingScript, /"progress_file":"%s"/);
  assert.match(main, /const SCRIPT_RUN_MARKER = '\[alvum-run\]'/);
  assert.match(main, /function handleScriptRunStart\(parentRun, marker\)/);
  assert.match(main, /briefingRuns\.set\(date, trackedRun\)/);
  assert.match(main, /consumeScriptRunMarkers\(run, chunk\)/);
  assert.match(main, /finishUnclosedScriptRuns\(run, code, signal\)/);
});

test('scripted catch-up marker attaches live progress to the processed day', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-briefing-service-'));
  const runtime = path.join(root, 'runtime');
  const briefings = path.join(root, 'briefings');
  const capture = path.join(root, 'capture');
  fs.mkdirSync(runtime, { recursive: true });
  fs.mkdirSync(briefings, { recursive: true });
  fs.mkdirSync(capture, { recursive: true });

  let spawned = null;
  const progressEvents = [];
  const service = createBriefingService({
    fs,
    path,
    crypto: require('node:crypto'),
    shell: { openPath: async () => '' },
    spawn: () => {
      spawned = new EventEmitter();
      spawned.stdout = new EventEmitter();
      spawned.stderr = new EventEmitter();
      spawned.pid = 42;
      return spawned;
    },
    ALVUM_ROOT: root,
    BRIEFINGS_DIR: briefings,
    CAPTURE_DIR: capture,
    BRIEFING_LOG: path.join(runtime, 'briefing.log'),
    BRIEFING_ERR: path.join(runtime, 'briefing.err'),
    appendShellLog: () => {},
    notify: () => {},
    resolveScript: () => '/tmp/briefing.sh',
    resolveBinary: () => '/tmp/alvum',
    alvumSpawnEnv: (env) => env,
    ensureLogDir: () => fs.mkdirSync(runtime, { recursive: true }),
    readTail: (file) => {
      try {
        return fs.readFileSync(file, 'utf8');
      } catch {
        return '';
      }
    },
    providerDiagnosticSnapshot: () => ({}),
    providerProbeSummary: async () => ({ providers: [] }),
    providerSelectableForAuto: () => true,
    refreshProviderWatch: () => {},
    recordProviderEvent: () => {},
    broadcastState: () => {},
    rebuildTrayMenu: () => {},
    sendToPopover: (channel, payload) => {
      if (channel === 'alvum:progress') progressEvents.push(payload);
    },
  });

  assert.equal(service.startBriefingProcess('/bin/bash', ['/tmp/briefing.sh'], 'Briefing').ok, true);

  const date = '2026-04-29';
  const runDir = path.join(briefings, date, 'runs', 'script-run');
  fs.mkdirSync(runDir, { recursive: true });
  const marker = {
    event: 'start',
    date,
    run_id: 'script-run',
    run_dir: runDir,
    label: `Briefing ${date}`,
    progress_file: path.join(runDir, 'progress.jsonl'),
    events_file: path.join(runDir, 'events.jsonl'),
    stdout_log: path.join(runDir, 'stdout.log'),
    stderr_log: path.join(runDir, 'stderr.log'),
    status_file: path.join(runDir, 'status.json'),
    expected_briefing: path.join(briefings, date, 'briefing.md'),
    started_at: '2026-04-29T08:00:00.000Z',
  };
  spawned.stdout.emit('data', Buffer.from(`[alvum-run] ${JSON.stringify(marker)}\n`));
  assert.equal(service.briefingRunSnapshot()[date].label, `Briefing ${date}`);

  fs.writeFileSync(marker.progress_file, '{"stage":"thread","current":1,"total":2}\n');
  service.pollProgress();
  assert.equal(progressEvents.length, 1);
  assert.equal(progressEvents[0].briefingDate, date);
  assert.equal(progressEvents[0].stage, 'thread');

  spawned.stdout.emit('data', Buffer.from(`[alvum-run] ${JSON.stringify({ ...marker, event: 'finish', code: 0, duration_ms: 1000 })}\n`));
  assert.equal(service.briefingRunSnapshot()[date], undefined);
  spawned.emit('close', 0, null);
});

test('canceling a running synthesis kills the process group and records a canceled run', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-briefing-cancel-'));
  const runtime = path.join(root, 'runtime');
  const briefings = path.join(root, 'briefings');
  const capture = path.join(root, 'capture');
  fs.mkdirSync(runtime, { recursive: true });
  fs.mkdirSync(briefings, { recursive: true });
  fs.mkdirSync(capture, { recursive: true });

  let spawned = null;
  let killedWith = null;
  const finished = [];
  const service = createBriefingService({
    fs,
    path,
    crypto: require('node:crypto'),
    shell: { openPath: async () => '' },
    spawn: () => {
      spawned = new EventEmitter();
      spawned.stdout = new EventEmitter();
      spawned.stderr = new EventEmitter();
      spawned.pid = 4242;
      spawned.killed = false;
      spawned.kill = (signal) => {
        killedWith = signal;
        spawned.killed = true;
        return true;
      };
      return spawned;
    },
    ALVUM_ROOT: root,
    BRIEFINGS_DIR: briefings,
    CAPTURE_DIR: capture,
    BRIEFING_LOG: path.join(runtime, 'briefing.log'),
    BRIEFING_ERR: path.join(runtime, 'briefing.err'),
    appendShellLog: () => {},
    notify: () => {},
    resolveScript: () => '/tmp/briefing.sh',
    resolveBinary: () => '/tmp/alvum',
    alvumSpawnEnv: (env) => env,
    ensureLogDir: () => fs.mkdirSync(runtime, { recursive: true }),
    readTail: (file) => {
      try {
        return fs.readFileSync(file, 'utf8');
      } catch {
        return '';
      }
    },
    providerDiagnosticSnapshot: () => ({}),
    providerProbeSummary: async () => ({ providers: [] }),
    providerSelectableForAuto: () => true,
    refreshProviderWatch: () => {},
    recordProviderEvent: () => {},
    broadcastState: () => {},
    rebuildTrayMenu: () => {},
    sendToPopover: () => {},
    onRunFinished: (event) => finished.push(event),
  });

  const date = '2026-04-29';
  const started = service.startBriefingProcess('/tmp/alvum', ['extract'], `Briefing ${date}`, date);
  assert.equal(started.ok, true);

  const canceled = service.cancelBriefingForDate(date);
  assert.equal(canceled.ok, true);
  assert.equal(canceled.status, 'canceling');
  assert.equal(killedWith, 'SIGTERM');
  assert.equal(service.briefingRunSnapshot()[date].canceling, true);

  spawned.emit('close', null, 'SIGTERM');

  assert.equal(service.briefingRunSnapshot()[date], undefined);
  assert.equal(finished.length, 1);
  assert.equal(finished[0].canceled, true);
  assert.equal(finished[0].ok, false);
  const log = service.briefingRunLog(date);
  assert.equal(log.ok, true);
  assert.equal(log.run.status, 'canceled');
  assert.match(log.text, /Status: canceled/);
  assert.equal(fs.existsSync(path.join(briefings, date, 'briefing.failed.json')), false);
});

test('successful synthesis clears stale voice markers for the regenerated day', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-briefing-stale-clear-'));
  const runtime = path.join(root, 'runtime');
  const briefings = path.join(root, 'briefings');
  const capture = path.join(root, 'capture');
  const date = '2026-05-02';
  const dayDir = path.join(briefings, date);
  fs.mkdirSync(runtime, { recursive: true });
  fs.mkdirSync(dayDir, { recursive: true });
  fs.mkdirSync(capture, { recursive: true });
  const staleMarker = path.join(dayDir, 'voice.stale.json');
  fs.writeFileSync(staleMarker, JSON.stringify({ date, kind: 'voice_identity' }));

  let spawned = null;
  const service = createBriefingService({
    fs,
    path,
    crypto: require('node:crypto'),
    shell: { openPath: async () => '' },
    spawn: () => {
      spawned = new EventEmitter();
      spawned.stdout = new EventEmitter();
      spawned.stderr = new EventEmitter();
      spawned.pid = 777;
      return spawned;
    },
    ALVUM_ROOT: root,
    BRIEFINGS_DIR: briefings,
    CAPTURE_DIR: capture,
    BRIEFING_LOG: path.join(runtime, 'briefing.log'),
    BRIEFING_ERR: path.join(runtime, 'briefing.err'),
    appendShellLog: () => {},
    notify: () => {},
    resolveScript: () => '/tmp/briefing.sh',
    resolveBinary: () => '/tmp/alvum',
    alvumSpawnEnv: (env) => env,
    ensureLogDir: () => fs.mkdirSync(runtime, { recursive: true }),
    readTail: () => '',
    providerDiagnosticSnapshot: () => ({}),
    providerProbeSummary: async () => ({ providers: [] }),
    providerSelectableForAuto: () => true,
    refreshProviderWatch: () => {},
    recordProviderEvent: () => {},
    broadcastState: () => {},
    rebuildTrayMenu: () => {},
    sendToPopover: () => {},
  });

  assert.equal(service.startBriefingProcess('/tmp/alvum', ['extract'], `Briefing ${date}`, date).ok, true);
  fs.writeFileSync(path.join(dayDir, 'briefing.md'), '# Regenerated synthesis');
  spawned.emit('close', 0, null);

  assert.equal(fs.existsSync(staleMarker), false);
  assert.match(briefingScript, /rm -f "\$out_dir\/voice\.stale\.json"/);
});

test('synthesis exposes live and persisted progress logs by day', () => {
  assert.match(html, /function appendProgressLog\(progress\)/);
  assert.match(html, /appendProgressLog\(p\)/);
  assert.match(html, /Live events:\\n\$\{liveRows\.join\('\\n'\)\}/);
  assert.match(html, /Persisted run log:/);
  assert.match(html, /loadPersistedBriefingLog\(date, true\)/);
  assert.match(html, /if \(runningForDay\) \{[\s\S]*?progressLog\.textContent = 'Progress log'[\s\S]*?openBriefingLogView\(day\.date\)/);
});

test('running synthesis can be canceled without becoming a failed day', () => {
  assert.match(preload, /cancelBriefingDate: \(date\) => ipcRenderer\.invoke\('alvum:cancel-briefing-date', date\)/);
  assert.match(main, /ipcMain\.handle\('alvum:cancel-briefing-date', \(_e, date\) => briefing\.cancelBriefingForDate\(date\)\)/);
  assert.match(main, /detached: true/);
  assert.match(main, /process\.kill\(-proc\.pid, signal\)/);
  assert.match(pipelineLlm, /Command::new\("claude"\)[\s\S]*?\.kill_on_drop\(true\)/);
  assert.match(pipelineLlm, /Command::new\("codex"\)[\s\S]*?\.kill_on_drop\(true\)/);
  assert.match(main, /status: 'canceled'/);
  assert.match(main, /canceled: true/);
  assert.match(main, /if \(event\.canceled\) \{[\s\S]*?queue = \[\]/);
  assert.match(html, /function cancelBriefingDateFromUi\(date\)/);
  assert.match(html, /window\.alvum\.cancelBriefingDate\(date\)/);
  assert.match(html, /cancel\.textContent = cancelingForDay \? 'Canceling\.\.\.' : 'Cancel'/);
  assert.match(html, /if \(day\.status === 'canceled'\) return 'Synthesis canceled'/);
});

test('synthesis progress log surfaces provider stop metadata', () => {
  assert.match(html, /evt\.stop_reason/);
  assert.match(html, /evt\.content_block_kinds/);
  assert.match(html, /content_block_kinds\.join\('\+'\)/);
  assert.match(main, /last_stop_reason/);
  assert.match(main, /last_content_block_kinds/);
  assert.match(html, /last_stop_reason/);
  assert.match(html, /last_content_block_kinds/);
  assert.match(html, /stats\.last_token_source === 'bedrock'/);
});

test('synthesis progress tracks direct retry through tree stages', () => {
  assert.match(html, /const STAGES = \['gather', 'process', 'thread', 'cluster', 'cluster-correlate', 'domain', 'domain-correlate', 'day', 'knowledge'\]/);
  assert.match(html, /'gather': 'Gather refs'/);
  assert.match(html, /'process': 'Process media'/);
  assert.match(html, /'day': 'Compose synthesis'/);
  assert.match(html, /stageLabel\(progress\.stage\)/);
  assert.match(html, /function progressLabel\(progress\)/);
  assert.match(html, /label\.textContent = progressLabel\(progress\)/);
  assert.match(html, /meta\.textContent = `\$\{cancelingForDay \? 'Canceling' : 'Synthesizing'\} \$\{pct\}% · \$\{progressLabel\(progress\)\}`/);
  assert.match(html, /const previousProgressByDate = progressByDate;/);
  assert.match(html, /run\.progress \|\| previousProgressByDate\[date\] \|\| null/);
  for (const stage of ['STAGE_CLUSTER', 'STAGE_CLUSTER_CORRELATE', 'STAGE_DOMAIN', 'STAGE_DOMAIN_CORRELATE', 'STAGE_DAY', 'STAGE_KNOWLEDGE']) {
    assert.match(coreProgress + pipelineExtract, new RegExp(stage));
  }
  assert.match(pipelineExtract, /progress::report\(\s*crate::progress::STAGE_DAY,\s*0,\s*1\s*\)/);
});

test('synthesis failure details fall back to failure markers when run files are absent', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-run-store-'));
  const store = createBriefingRunStore({
    fs,
    path,
    crypto: require('node:crypto'),
    shell: { openPath: async () => '' },
    BRIEFINGS_DIR: root,
    appendShellLog: () => {},
    readTail: (file) => {
      try {
        return fs.readFileSync(file, 'utf8');
      } catch {
        return '';
      }
    },
    providerDiagnosticSnapshot: () => ({}),
    validDateStamp: (value) => /^\d{4}-\d{2}-\d{2}$/.test(value || ''),
  });
  store.writeBriefingFailure('2026-04-25', {
    reason: 'code 1',
    run_id: 'run-1',
    run_dir: '/tmp/run-1',
    stderr_tail: 'traceable failure',
  });
  const log = store.briefingRunLog('2026-04-25');
  assert.equal(log.ok, true);
  assert.equal(log.run.status, 'failed');
  assert.match(log.text, /Reason: code 1/);
  assert.match(log.text, /traceable failure/);
});

test('synthesis customization lives under synthesis and uses profile IPC', () => {
  assert.match(preload, /synthesisProfile:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:synthesis-profile'\)/);
  assert.match(preload, /synthesisProfileSave:\s+\(profile\)\s+=>\s+ipcRenderer\.invoke\('alvum:synthesis-profile-save', profile\)/);
  assert.match(preload, /synthesisProfileSuggestions:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:synthesis-profile-suggestions'\)/);
  assert.match(preload, /synthesisProfilePromote:\s+\(id\)\s+=>\s+ipcRenderer\.invoke\('alvum:synthesis-profile-promote', id\)/);
  assert.match(preload, /synthesisProfileIgnore:\s+\(id\)\s+=>\s+ipcRenderer\.invoke\('alvum:synthesis-profile-ignore', id\)/);
  assert.match(main, /ipcMain\.handle\('alvum:synthesis-profile'/);
  assert.match(main, /\['profile', 'show', '--json'\]/);
  assert.match(main, /\['profile', 'save', '--json', JSON\.stringify\(profile\)\]/);
  assert.match(main, /\['profile', 'suggestions', '--json'\]/);
  const customizeButton = html.indexOf('id="synthesis-customize"');
  const calendarShell = html.indexOf('class="calendar-shell"');
  const selectedDayCard = html.indexOf('id="selected-date-actions"');
  assert.ok(customizeButton > -1, 'customize synthesis button exists');
  assert.ok(calendarShell < selectedDayCard, 'calendar appears before selected day card');
  assert.ok(selectedDayCard < customizeButton, 'customize button appears below the selected day card');
  assert.match(html, /id="synthesis-customize" type="button">Customize<\/button>/);
  assert.doesNotMatch(html, /id="synthesis-customize" class="primary"/);
  const customizeRule = html.match(/\.synthesis-customize-row button \{([\s\S]*?)\n  \}/)[1];
  assert.doesNotMatch(customizeRule, /min-height/);
  assert.match(html, /data-view="synthesis-profile"/);
  assert.match(html, /data-view="profile-intentions-list"/);
  assert.match(html, /data-view="profile-intention-detail"/);
  assert.match(html, /data-view="profile-domains-list"/);
  assert.match(html, /data-view="profile-domain-detail"/);
  assert.match(html, /data-view="profile-interests-list"/);
  assert.match(html, /data-view="profile-interest-detail"/);
  assert.doesNotMatch(html, /data-view="profile-detected-list"/);
  assert.doesNotMatch(html, /id="profile-suggestions"/);
  assert.match(html, /data-view="profile-writing-detail"/);
  assert.match(html, /data-view="profile-advanced-detail"/);
  const profileIndex = html.match(/<section class="view" data-view="synthesis-profile" hidden>([\s\S]*?)<\/section>/)[1];
  assert.match(profileIndex, /<span>Profile sections<\/span>/);
  assert.match(profileIndex, /id="profile-menu"/);
  assert.doesNotMatch(profileIndex, /profile-intention-add/);
  assert.doesNotMatch(profileIndex, /profile-domain-add/);
  assert.doesNotMatch(profileIndex, /profile-interest-add/);
  assert.doesNotMatch(profileIndex, /profile-advanced/);
  assert.match(html, /<span>Intentions<\/span>/);
  assert.match(html, /id="profile-intention-add"/);
  assert.match(html, /id="profile-intentions"/);
  assert.match(html, /id="profile-intentions-save"/);
  assert.match(html, /<span>Domains<\/span>/);
  assert.match(html, /id="profile-domain-add"/);
  assert.match(html, /id="profile-domains-save"/);
  assert.match(html, /<span>Tracked<\/span>/);
  assert.match(html, /id="profile-interest-add"/);
  assert.match(html, /id="profile-interests-save"/);
  assert.doesNotMatch(html, /<span>Detected<\/span>/);
  assert.match(html, /<span>Writing<\/span>/);
  assert.match(html, /<span>Advanced<\/span>/);
  assert.match(html, /Add extra guidance for how Alvum should write your synthesis/);
  assert.match(html, /still stay grounded in your data/);
  assert.match(html, /id="profile-intention-detail-remove"/);
  assert.match(html, /id="profile-domain-detail-remove"/);
  assert.match(html, /id="profile-interest-detail-remove"/);
  assert.doesNotMatch(html, /profile-.*reload/);
  assert.match(html, /\$\('synthesis-customize'\)\.onclick = \(\) => setView\('synthesis-profile'\)/);
  assert.match(html, /\$\('profile-intention-add'\)\.onclick = \(\) =>/);
  assert.match(html, /if \(view === 'synthesis-profile'\) return 'briefing'/);
  assert.match(html, /if \(view === 'profile-intentions-list'\) return 'synthesis-profile'/);
  assert.match(html, /if \(view === 'profile-intention-detail'\) return 'profile-intentions-list'/);
  assert.match(html, /if \(view === 'profile-domains-list'\) return 'synthesis-profile'/);
  assert.match(html, /if \(view === 'profile-domain-detail'\) return 'profile-domains-list'/);
  assert.match(html, /if \(view === 'profile-interests-list'\) return 'synthesis-profile'/);
  assert.match(html, /if \(view === 'profile-interest-detail'\) return 'profile-interests-list'/);
  assert.doesNotMatch(html, /if \(view === 'profile-detected-list'\)/);
  assert.match(html, /if \(view === 'profile-writing-detail'\) return 'synthesis-profile'/);
  assert.match(html, /if \(view === 'profile-advanced-detail'\) return 'synthesis-profile'/);
  assert.match(html, /profileMenuRow\(\s*'Intentions'/);
  assert.match(html, /setView\('profile-intentions-list'\)/);
  assert.match(html, /profileMenuRow\(\s*'Domains'/);
  assert.match(html, /setView\('profile-domains-list'\)/);
  assert.match(html, /profileMenuRow\(\s*'Tracked'/);
  assert.match(html, /setView\('profile-interests-list'\)/);
  assert.doesNotMatch(html, /profileMenuRow\(\s*'Detected'/);
  assert.doesNotMatch(html, /setView\('profile-detected-list'\)/);
  assert.match(html, /function profileTrackedSummary/);
  assert.match(html, /setView\('profile-writing-detail'\)/);
  assert.match(html, /setView\('profile-advanced-detail'\)/);
  assert.match(html, /function renderProfileIntentions/);
  assert.match(html, /function renderProfileIntentionDetail/);
  assert.match(html, /function renderProfileDomainDetail/);
  assert.match(html, /function renderProfileInterestDetail/);
  assert.match(html, /function renderProfileWriting/);
  assert.match(html, /function profilePrioritySelect/);
  assert.match(html, /function profileDomainSelect/);
  assert.match(html, /function enabledProfileDomainCount/);
  assert.match(html, /Keep at least one synthesis domain enabled/);
  assert.match(html, /id: makeProfileId\('intention', synthesisProfile\.intentions\)/);
  assert.match(html, /Mission', 'Ambition', 'Goal', 'Habit', 'Commitment/);
  assert.match(html, /window\.alvum\.synthesisProfileSave\(synthesisProfile\)/);
  assert.match(html, /await unlinkVoiceAssignmentsForDeletedInterest\(interest\)/);
  assert.match(html, /window\.alvum\.speakerUnlinkInterest\(interest\.id\)/);
  assert.match(html, /window\.alvum\.synthesisProfilePromote\(suggestion\.id\)/);
  assert.match(html, /window\.alvum\.synthesisProfileIgnore\(suggestion\.id\)/);
  const intentionListRenderer = html.match(/function renderProfileIntentions\(\) \{([\s\S]*?)\n\s+function renderProfileDomains\(\)/)[1];
  assert.match(intentionListRenderer, /setView\('profile-intention-detail'\)/);
  assert.doesNotMatch(intentionListRenderer, /profileInput\('ID'/);
  assert.doesNotMatch(intentionListRenderer, /profileInput\('Aliases'/);
  assert.doesNotMatch(intentionListRenderer, /profileInput\('Cadence'/);
  assert.doesNotMatch(intentionListRenderer, /profileTextareaField\('Nudge'/);
  assert.doesNotMatch(intentionListRenderer, /profileTextareaField\('Notes'/);
  const intentionDetailRenderer = html.match(/function renderProfileIntentionDetail\(\) \{([\s\S]*?)\n\s+function renderProfileDomainDetail\(\)/)[1];
  assert.match(intentionDetailRenderer, /profilePrioritySelect\('Priority'/);
  assert.match(intentionDetailRenderer, /profileDomainSelect\('Domain'/);
  assert.doesNotMatch(intentionDetailRenderer, /profileInput\('ID'/);
  assert.doesNotMatch(intentionDetailRenderer, /profileInput\('Aliases'/);
  assert.doesNotMatch(intentionDetailRenderer, /profileInput\('Cadence'/);
  assert.doesNotMatch(intentionDetailRenderer, /profileTextareaField\('Nudge'/);
  assert.doesNotMatch(intentionDetailRenderer, /profileTextareaField\('Notes'/);
  const interestListRenderer = html.match(/function renderProfileInterests\(\) \{([\s\S]*?)\n\s+function renderProfileInterestDetail\(\)/)[1];
  assert.match(interestListRenderer, /setView\('profile-interest-detail'\)/);
  assert.match(interestListRenderer, /synthesisProfileSuggestions/);
  assert.match(interestListRenderer, /Track/);
  assert.match(interestListRenderer, /Ignore/);
  assert.doesNotMatch(interestListRenderer, /profileInput\('ID'/);
  assert.doesNotMatch(interestListRenderer, /profileInput\('Aliases'/);
  const interestDetailRenderer = html.match(/function renderProfileInterestDetail\(\) \{([\s\S]*?)\n\s+function renderProfileWriting\(\)/)[1];
  assert.match(interestDetailRenderer, /profileSelect\('Type'/);
  assert.match(interestDetailRenderer, /profilePrioritySelect\('Priority'/);
  assert.match(interestDetailRenderer, /profileTextareaField\('Description'/);
  assert.doesNotMatch(interestDetailRenderer, /profileInput\('ID'/);
  assert.doesNotMatch(interestDetailRenderer, /profileInput\('Aliases'/);
  assert.doesNotMatch(interestDetailRenderer, /profileTextareaField\('Notes'/);
  assert.doesNotMatch(html, /function renderProfileSuggestions/);
  const writingRenderer = html.match(/function renderProfileWriting\(\) \{([\s\S]*?)\n\s+function renderProfileAdvanced\(\)/)[1];
  assert.match(writingRenderer, /profileSelect\('Detail'/);
  assert.match(writingRenderer, /profileSelect\('Tone'/);
  assert.match(writingRenderer, /profileTextareaField\('Daily Briefing Outline'/);
  assert.doesNotMatch(writingRenderer, /profileInput\('Sections'/);
  assert.doesNotMatch(writingRenderer, /profileInput\('Emphasize'/);
  assert.doesNotMatch(writingRenderer, /profileInput\('Mute'/);
  const mainView = html.match(/<section class="view" data-view="main">([\s\S]*?)<\/section>/)[1];
  assert.doesNotMatch(mainView, /Customize/);
});

test('popover caps long views so overflow scrolls inside the menu', () => {
  assert.match(html, /--popover-max-height: 640px/);
  assert.match(html, /--popover-view-max-height/);
  assert.match(html, /function applyViewScrollLimit\(\)/);
  assert.match(html, /function cappedViewHeight\(height\)/);
  assert.match(html, /return Math\.min\(fullHeight, popoverMaxHeight\(\)\)/);
  assert.match(html, /nextContentHeight = nextEl\.scrollHeight \|\| nextEl\.offsetHeight/);
});

test('connectors is the single management surface for capture sources and packages', () => {
  assert.match(html, /<div class="label">Connectors<\/div>/);
  assert.match(html, /id="extension-enabled-badge"/);
  assert.match(html, /<span>Installed connectors<\/span>/);
  assert.match(html, /id="extension-diagnose"/);
  assert.match(html, /id="extension-add"/);
  assert.match(html, /data-view="connector-add"/);
  assert.match(html, /id="connector-add-core-list"/);
  assert.match(html, /id="connector-add-external-stub"/);
  assert.match(html, /<span id="extension-detail-capture-title">Capture<\/span>/);
  assert.match(html, /<div id="extension-detail-capture-controls" class="button-grid"><\/div>/);
  assert.match(html, /<span id="extension-detail-processor-title">Processor<\/span>/);
  assert.match(html, /<div id="extension-detail-processor-controls" class="button-grid"><\/div>/);

  assert.doesNotMatch(html, /connector-capture-inputs-list/);
  assert.doesNotMatch(html, /connector-capture-inputs-refresh/);
  assert.doesNotMatch(html, /id="open-extensions-dir"/);
  assert.doesNotMatch(html, />Open folder</);
  assert.doesNotMatch(html, /id="extension-doctor"/);
  assert.doesNotMatch(html, /id="extension-detail-doctor"/);
  assert.doesNotMatch(html, /id="extension-detail-toggle"/);
  assert.doesNotMatch(html, /id="extension-detail-title"/);
  assert.doesNotMatch(html, /id="extension-detail-meta"/);
  assert.doesNotMatch(html, /id="extension-detail-dot"/);
  assert.doesNotMatch(html, /extension-detail-messages/);
  assert.doesNotMatch(html, /No external connectors installed/);
  assert.doesNotMatch(html, /<div class="label">Extensions<\/div>/);
  assert.doesNotMatch(html, /<span>Extension packages<\/span>/);
});

test('main menu shows enabled connector and provider count badges', () => {
  const main = html.match(/<section class="view" data-view="main">([\s\S]*?)<\/section>/)[1];
  assert.match(main, /id="extension-enabled-badge" class="summary-badge"/);
  assert.match(main, /id="provider-enabled-badge" class="summary-badge"/);
  assert.match(html, /\.summary-badge,\n  \.state-badge \{/);
  assert.match(html, /function enabledConnectorCount\(\)/);
  assert.match(html, /function enabledProviderCount\(\)/);
  assert.match(html, /function renderMainBadges\(\)/);
  assert.match(html, /connectorBadge\.textContent = String\(enabledConnectorCount\(\)\)/);
  assert.match(html, /providerBadge\.textContent = String\(enabledProviderCount\(\)\)/);
  assert.match(html, /renderMainBadges\(\);[\s\S]{0,160}return extensionSummary;/);
  assert.match(html, /if \(s\.providerSummary\) applyProviderSummary\(s\.providerSummary\);/);
});

test('popover uses the connector contract and keeps internals out of the menu', () => {
  assert.match(preload, /connectorList:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:connector-list'\)/);
  assert.match(preload, /doctor:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:doctor'\)/);
  assert.match(main, /\['connectors', 'list', '--json'\]/);
  assert.match(main, /\['connectors', enabled \? 'enable' : 'disable', id\]/);
  assert.match(main, /\['doctor', '--json'\]/);
  assert.match(main, /settings: settingsFor\(sections, \['capture\.audio-mic'\]\)/);
  assert.match(main, /settings: settingsFor\(sections, \['capture\.audio-system'\]\)/);
  assert.match(main, /settings: settingsFor\(sections, \['capture\.screen'\]\)/);
  assert.doesNotMatch(main, /settings: settingsFor\(sections, \['capture\.audio-mic', 'connectors\.audio'\]\)/);
  assert.doesNotMatch(main, /settings: settingsFor\(sections, \['capture\.audio-system', 'connectors\.audio'\]\)/);
  assert.doesNotMatch(main, /settings: settingsFor\(sections, \['capture\.screen', 'connectors\.screen'\]\)/);
  assert.match(html, /window\.alvum\.connectorList\(\)/);
  assert.match(html, /function connectorListStatusLabel/);
  assert.match(html, /meta\.textContent = connectorListStatusLabel\(ext\)/);
  assert.doesNotMatch(preload, /connectorDoctor:/);
  assert.doesNotMatch(main, /alvum:connector-doctor/);
  assert.doesNotMatch(html, /function componentRows/);
  assert.doesNotMatch(html, /Route \$\{index \+ 1\}/);
  assert.doesNotMatch(html, /Analysis lens \$\{analysis\.id/);
  assert.doesNotMatch(html, /<span>Components<\/span>/);
  assert.doesNotMatch(html, /extension-detail-settings/);
});

test('connector detail separates capture controls from processor settings', () => {
  assert.doesNotMatch(html, /function connectorBulkActionText/);
  assert.doesNotMatch(html, /function connectorBulkNextEnabled/);
  assert.doesNotMatch(html, /Turn all/);
  assert.doesNotMatch(html, /connectorSourceStatusLabel\(ext\).*source/);
  assert.doesNotMatch(html, /\$\{control\.enabled \? 'On' : 'Off'\} ·/);
  assert.doesNotMatch(html, /extension-detail-source-title/);
  assert.doesNotMatch(html, /extension-detail-source-controls/);
  assert.match(html, /processor_controls/);
  assert.match(html, /function renderConnectorCaptureControls/);
  assert.match(html, /function renderConnectorProcessorControls/);
  assert.match(html, /Whisper model/);
  assert.match(html, /Recognition method/);
});

test('connector detail owns child source toggles', () => {
  assert.match(html, /source_controls/);
  assert.match(html, /function renderConnectorCaptureControls/);
  assert.match(html, /window\.alvum\.toggleCaptureInput\(control\.id\)/);
  assert.match(html, /captureInputParent = 'extension-detail'/);
});

test('connector input settings are editable without a redundant input toggle pane', () => {
  assert.match(preload, /captureInputSetSetting:\s+\(id, key, value\)\s+=>\s+ipcRenderer\.invoke\('alvum:set-capture-input-setting', id, key, value\)/);
  assert.match(preload, /chooseDirectory:\s+\(defaultPath\)\s+=>\s+ipcRenderer\.invoke\('alvum:choose-directory', defaultPath\)/);
  assert.match(main, /ipcMain\.handle\('alvum:set-capture-input-setting', \(_e, id, key, value\) =>/);
  assert.match(main, /ipcMain\.handle\('alvum:choose-directory', \(_e, defaultPath\) =>/);
  assert.match(main, /function captureInputConfigSection/);
  assert.match(main, /async function chooseDirectory/);
  assert.match(main, /setCaptureInputSetting/);
  assert.match(main, /\['config-set', `\$\{section\}\.\$\{key\}`, String\(value\)\]/);
  assert.match(html, /id="capture-input-summary"/);
  assert.match(html, /captureInputParent === 'extension-detail'/);
  assert.match(html, /summary\.hidden = connectorScoped/);
  assert.match(html, /className = 'settings-row editable-setting-row'/);
  assert.match(html, /window\.alvum\.captureInputSetSetting\(input\.id, key, nextValue\)/);
  assert.match(html, /renderEditableSettingRow/);
});

test('settings use typed controls and avoid cramped multi-column text entry', () => {
  const block = html.match(/\.editable-setting-row \{([\s\S]*?)\n  \}/)[1];
  assert.match(block, /grid-template-columns: 1fr;/);
  assert.match(block, /align-items: stretch;/);
  assert.match(html, /\.setting-control-row/);
  assert.match(html, /function settingControlKind/);
  assert.match(html, /if \(key === 'since'\) return 'datetime';/);
  assert.match(html, /if \(key === 'session_dir' \|\| key\.endsWith\('_dir'\)\) return 'directory';/);
  assert.match(html, /editor\.type = 'datetime-local'/);
  assert.match(html, /window\.alvum\.chooseDirectory\(String\(value \|\| ''\)\)/);
  assert.match(html, /browse\.textContent = 'Browse'/);
  assert.match(html, /document\.createElement\('select'\)/);
});

test('processor settings expose enum options and write through processor config', () => {
  assert.match(preload, /connectorProcessorSetSetting:\s+\(component, key, value\)\s+=>\s+ipcRenderer\.invoke\('alvum:set-connector-processor-setting', component, key, value\)/);
  assert.match(main, /function processorConfigSection/);
  assert.match(main, /ipcMain\.handle\('alvum:set-connector-processor-setting', \(_e, component, key, value\) =>/);
  assert.match(main, /\['config-set', `\$\{section\}\.\$\{key\}`, String\(value\)\]/);
  assert.match(html, /options: \[/);
  assert.match(html, /value: 'ocr', label: 'OCR'/);
  assert.match(html, /function renderProcessorSettingRow/);
  assert.match(html, /window\.alvum\.connectorProcessorSetSetting\(control\.component, setting\.key, nextValue\)/);
  assert.match(html, /diarization_enabled/);
  assert.match(html, /speaker_registry/);
});

test('audio processor settings distinguish local transcription from provider transcription', () => {
  assert.match(html, /function processorSettingsForMode/);
  assert.match(html, /isAudioProcessorControl\(control\)/);
  assert.match(html, /mode === 'provider'[\s\S]*setting\.key === 'provider'/);
  assert.match(html, /mode === 'local'[\s\S]*LOCAL_AUDIO_PROCESSOR_SETTING_KEYS\.has\(String\(setting\.key \|\| ''\)\)/);
  assert.match(html, /Local Whisper \+ speaker IDs/);
  assert.match(html, /Provider diarized transcription/);
  assert.match(html, /Used only when audio processing mode is Provider/);
});

test('audio connector exposes tracked voice registry management', () => {
  assert.match(rawHtml, /id="extension-detail-speakers-section" hidden/);
  assert.match(rawHtml, /id="extension-detail-speakers" class="button-grid"/);
  assert.match(preload, /speakerList:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-list'\)/);
  assert.match(preload, /speakerLink:\s+\(id, interestId\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-link', id, interestId\)/);
  assert.match(preload, /speakerUnlink:\s+\(id\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-unlink', id\)/);
  assert.match(preload, /speakerMerge:\s+\(sourceId, targetId\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-merge', sourceId, targetId\)/);
  assert.match(preload, /speakerForget:\s+\(id\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-forget', id\)/);
  assert.match(preload, /speakerReset:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-reset'\)/);
  assert.match(preload, /speakerSampleAudio:\s+\(id, sampleIndex\)\s+=>\s+ipcRenderer\.invoke\('alvum:speaker-sample-audio', id, sampleIndex\)/);
  assert.match(main, /\['speakers', 'list', '--json'\]/);
  assert.match(main, /\['speakers', 'link', String\(id \|\| ''\), String\(interestId \|\| ''\), '--json'\]/);
  assert.match(main, /\['speakers', 'unlink', String\(id \|\| ''\), '--json'\]/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-merge'/);
  assert.match(main, /ipcMain\.handle\('alvum:speaker-sample-audio'/);
  assert.match(main, /sample audio path is outside Alvum capture storage/);
  assert.match(main, /\['speakers', 'reset', '--json'\]/);
  assert.match(main, /speaker = createSpeakerService\(\{[\s\S]*broadcastState,[\s\S]*\}\);/);
  assert.match(html, /function renderSpeakerManagement/);
  assert.match(html, /isAudioConnector\(ext\)/);
  assert.match(html, /window\.alvum\.speakerLink\(speaker\.speaker_id, interestId\)/);
  assert.match(html, /window\.alvum\.speakerUnlink\(speaker\.speaker_id\)/);
  assert.match(html, /window\.alvum\.speakerMerge\(sourceId, targetId\)/);
  assert.match(html, /window\.alvum\.speakerForget\(speakerId\)/);
  assert.match(html, /window\.alvum\.speakerReset\(\)/);
  assert.match(html, /window\.alvum\.speakerSampleAudio\(speaker\.speaker_id, sampleIndex\)/);
  assert.match(html, /Review voices/);
});

test('add connector view lists core connectors and external install stub', () => {
  assert.match(html, /<span>Core connectors<\/span>/);
  assert.match(html, /<span>External connectors<\/span>/);
  assert.match(html, /Local folder · Git URL · npm package/);
  assert.match(html, /function renderAddConnector/);
  assert.match(html, /connector\.kind === 'core'/);
  assert.match(html, /window\.alvum\.connectorSetEnabled\(connector\.id, true\)/);
  assert.match(html, /\$\('extension-add'\)\.onclick = \(\) => setView\('connector-add'\)/);
  assert.match(html, /if \(activeView === 'connector-add'\) renderAddConnector\(\)/);
  assert.match(html, /if \(view === 'connector-add'\) return 'extensions'/);
  assert.doesNotMatch(html, /window\.alvum\.openExtensionsDir\(\)/);
});

test('diagnose uses global menu notifications instead of persistent cards', () => {
  assert.match(html, /id="menu-notification"/);
  assert.match(html, /function showMenuNotification/);
  assert.match(html, /function doctorSummaryText/);
  assert.match(html, /function doctorNotificationLevel/);
  assert.match(html, /showMenuNotification\(doctorSummaryText\(result\), doctorNotificationLevel\(result\)\)/);
  assert.doesNotMatch(html, /showMenuNotification\('Connectors refreshed\.'\)/);
  assert.doesNotMatch(html, /showMenuNotification\('Opened the connector package folder\.'\)/);
  assert.doesNotMatch(html, /showMenuNotification\(`\$\{control\.label \|\| control\.id\}/);
  assert.doesNotMatch(html, /showMenuNotification\(`\$\{ext\.display_name \|\| ext\.id\}/);
  assert.doesNotMatch(html, /window\.alvum\.connectorDoctor\(\)/);
  assert.doesNotMatch(html, /extensionDoctorSummary/);
  assert.doesNotMatch(html, /function appendExtensionActionRow/);
  assert.doesNotMatch(html, /extensionActionMessage/);
});

test('provider runtime and watcher use the app spawn environment', () => {
  assert.match(main, /function alvumSpawnEnv/);
  assert.match(main, /\.local['"],\s*['"]bin/);
  assert.match(main, /\/opt\/homebrew\/bin/);
  assert.match(main, /\/usr\/local\/bin/);
  assert.match(main, /const PROVIDER_BACKGROUND_TEST_TIMEOUT_MS = 30000;/);
  assert.match(main, /const PROVIDER_MANUAL_TEST_TIMEOUT_MS = 120000;/);
  assert.match(main, /env: alvumSpawnEnv\(\{ RUST_LOG:/);
  assert.match(main, /spawn\(bin, args, \{ stdio: \['ignore', 'pipe', 'pipe'\], env: alvumSpawnEnv\(\) \}\)/);
  assert.match(main, /\['providers', 'test', '--provider', entry\.name, '--timeout-secs', PROVIDER_BACKGROUND_TEST_TIMEOUT_SECS\], PROVIDER_BACKGROUND_TEST_TIMEOUT_MS/);
  assert.match(main, /\['providers', 'test', '--provider', name, '--timeout-secs', PROVIDER_MANUAL_TEST_TIMEOUT_SECS\], PROVIDER_MANUAL_TEST_TIMEOUT_MS/);
  assert.match(main, /const PROVIDER_WATCH_MS/);
  assert.match(main, /let providerProbeCacheLive = false;/);
  assert.match(main, /function startProviderWatcher/);
  assert.match(main, /function notifyProviderIssues/);
  assert.match(main, /!liveProbe \|\| providerProbeCacheLive/);
  assert.match(main, /providerProbeCacheLive = !!liveProbe;/);
  assert.match(main, /providerSummary: provider\.providerProbeSnapshot\(\)/);
  assert.match(main, /refreshProviderWatch\(true\);/);
  assert.match(main, /setInterval\(\(\) => refreshProviderWatch\(!!currentProviderIssue\), PROVIDER_WATCH_MS\)/);
  assert.match(main, /startProviderWatcher\(\)/);
});

test('bedrock provider compiles AWS SDK HTTPS client support', () => {
  assert.match(pipelineCargo, /aws-config = \{[^\n]*"default-https-client"/);
  assert.match(pipelineCargo, /aws-sdk-bedrockruntime = \{[^\n]*"default-https-client"/);
});

test('app spawn environment preserves credential helper PATH entries', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-path-'));
  const configFile = path.join(root, 'config.toml');
  const originalPath = process.env.PATH;
  const originalExtraPath = process.env.ALVUM_EXTRA_PATH;
  const originalDisableShellPath = process.env.ALVUM_DISABLE_LOGIN_SHELL_PATH;
  try {
    fs.writeFileSync(configFile, [
      '[providers.bedrock]',
      'extra_path = "/isengard/bin:/company/aws/bin"',
      '',
      '[providers.claude-cli]',
      'extra_path = ["/claude/backend/bin"]',
      '',
    ].join('\n'));
    process.env.PATH = ['/usr/bin', '/bin'].join(path.delimiter);
    process.env.ALVUM_EXTRA_PATH = ['/company/bin', '/opt/company/bin'].join(path.delimiter);
    process.env.ALVUM_DISABLE_LOGIN_SHELL_PATH = '1';

    const env = { PATH: runtimeModule.buildAlvumPath(['/per-run/bin', '/usr/bin'].join(path.delimiter), configFile) };
    const entries = env.PATH.split(path.delimiter);

    assert.equal(entries[0], '/per-run/bin');
    assert.ok(entries.includes('/company/bin'));
    assert.ok(entries.includes('/opt/company/bin'));
    assert.ok(entries.includes('/isengard/bin'));
    assert.ok(entries.includes('/company/aws/bin'));
    assert.ok(entries.includes('/claude/backend/bin'));
    assert.ok(entries.includes(path.join(os.homedir(), 'bin')));
    assert.ok(entries.includes('/opt/amazon/bin'));
    assert.ok(entries.includes('/usr/bin'));
    assert.equal(entries.indexOf('/usr/bin'), entries.lastIndexOf('/usr/bin'));
  } finally {
    if (originalPath == null) delete process.env.PATH;
    else process.env.PATH = originalPath;
    if (originalExtraPath == null) delete process.env.ALVUM_EXTRA_PATH;
    else process.env.ALVUM_EXTRA_PATH = originalExtraPath;
    if (originalDisableShellPath == null) delete process.env.ALVUM_DISABLE_LOGIN_SHELL_PATH;
    else process.env.ALVUM_DISABLE_LOGIN_SHELL_PATH = originalDisableShellPath;
  }
});

test('provider auto selection skips providers with failed live pings', () => {
  assert.match(main, /function providerSelectableForAuto\(provider\)/);
  assert.match(main, /return !!\(provider\.test && provider\.test\.ok\);/);
  assert.match(main, /function applyProviderAutoSelection\(summary\)/);
  assert.match(main, /if \(\(summary\.configured \|\| 'auto'\) !== 'auto'\) return summary;/);
  assert.match(main, /auto_resolved: autoResolved/);
  assert.match(main, /active: provider\.name === autoResolved/);
  assert.match(main, /const result = applyProviderAutoSelection\(\{/);
  assert.match(main, /const nextSummary = applyProviderAutoSelection\(\{/);
  assert.match(html, /function autoResolvedProviderName\(providers\)/);
  assert.match(html, /find\(\(provider\) => provider\.enabled !== false && providerIsWorking\(provider\)\)/);
  assert.doesNotMatch(html, /find\(\(provider\) => provider\.enabled !== false && provider\.available\)/);
});

test('providers page manages enabled providers with add and remove', () => {
  assert.match(preload, /providerSetEnabled:\s+\(name, enabled\)\s+=>\s+ipcRenderer\.invoke\('alvum:provider-set-enabled', name, enabled\)/);
  assert.match(preload, /providerSetup:\s+\(name, action\)\s+=>\s+ipcRenderer\.invoke\('alvum:provider-setup', name, action\)/);
  assert.doesNotMatch(preload, /providerProbeSummary/);
  assert.match(main, /ipcMain\.handle\('alvum:provider-set-enabled'/);
  assert.match(main, /ipcMain\.handle\('alvum:provider-setup'/);
  assert.match(main, /function providerTest\(name\)/);
  assert.match(main, /ipcMain\.handle\('alvum:provider-test', \(_e, name\) =>\s+provider\.providerTest\(name\)\)/);
  assert.doesNotMatch(main, /alvum:provider-probe-summary/);
  assert.match(main, /\['providers', enabled \? 'enable' : 'disable', name\]/);
  assert.match(main, /function providerSetup/);
  assert.match(main, /Terminal/);
  assert.match(html, /data-view="provider-add"/);
  assert.match(html, /Available providers/);
  assert.match(html, /id="provider-add"/);
  assert.match(html, /id="provider-add-list"/);
  assert.match(html, /id="provider-detail-primary"/);
  assert.match(html, /function renderProviderAdd/);
  assert.match(html, /function configuredProviders/);
  assert.match(html, /function providerCatalogEntries/);
  assert.match(html, /function providerPrimaryAction/);
  assert.match(html, /function providerIsWorking/);
  assert.match(html, /function providerCanRemove/);
  assert.match(html, /function providerCatalogActionLabel/);
  assert.match(html, /function mergeProviderSummary/);
  assert.match(html, /function setProviderEnabledLocal/);
  assert.match(html, /function setProviderActiveLocal/);
  assert.match(html, /function updateProviderFromActionResult/);
  assert.match(html, /function runProviderPrimaryAction/);
  assert.match(html, /let providerProbeLoading = false;/);
  assert.match(html, /let providerProbeError = null;/);
  assert.match(html, /function appendProviderStateRow/);
  assert.match(html, /Loading providers/);
  assert.match(html, /Could not load providers/);
  assert.match(html, /No configured providers/);
  assert.match(html, /built-in provider catalog/);
  assert.match(html, /if \(s\.providerSummary\) applyProviderSummary\(s\.providerSummary\);/);
  assert.match(html, /id="provider-detail-check" type="button">Ping<\/button>/);
  assert.match(html, /\$\('provider-detail-check'\)\.textContent = 'Pinging\.\.\.'/);
  assert.doesNotMatch(html, /provider-probe-refresh/);
  assert.doesNotMatch(html, /refreshProviderProbe/);
  assert.doesNotMatch(html, /providerProbeSummary/);
  assert.match(html, /filter\(\(provider\) => provider\.enabled === false\)/);
  assert.match(html, /All known providers are configured/);
  assert.match(html, /Use auto/);
  assert.doesNotMatch(html, /Disable provider/);
  assert.match(html, /Add provider/);
  assert.match(html, /label: 'Use'/);
  assert.match(html, /Set up provider/);
  assert.match(html, /window\.alvum\.providerSetEnabled\(provider\.name, false\)/);
  assert.match(html, /window\.alvum\.providerSetEnabled\(provider\.name, true\)/);
  assert.match(html, /window\.alvum\.providerSetActive\('auto'\)/);
  assert.match(html, /window\.alvum\.providerSetup\(provider\.name\)/);
  assert.match(html, /if \(!providerIsWorking\(provider\)\) \{/);
  assert.match(html, /setProviderEnabledLocal\(provider\.name, true\)/);
  assert.match(html, /setProviderEnabledLocal\(provider\.name, false\)/);
  assert.match(html, /setProviderActiveLocal\(provider\.name\)/);
  assert.match(html, /setView\('provider-detail'\)/);
  assert.match(html, /let providerDetailParent = 'providers';/);
  assert.match(html, /if \(view === 'provider-detail'\) return providerDetailParent;/);
  assert.match(html, /providerDetailParent = 'provider-add';/);
  assert.match(html, /providerDetailParent = 'providers';/);
  assert.match(html, /provider\.setup_kind === 'instructions'/);
  assert.match(html, /invalidateProviderModelLoad\(result\.provider\)/);
  assert.match(html, /name === 'claude-cli'[\s\S]*?CLI default[\s\S]*?Sonnet[\s\S]*?Opus/);
  assert.match(html, /name === 'claude-cli'[\s\S]*?options_by_modality[\s\S]*?image: cliDefaultOptions[\s\S]*?audio: cliDefaultOptions/);
  assert.doesNotMatch(html, /claude login/);
  const providerPrimaryAction = html.match(/function providerPrimaryAction\(provider\) \{([\s\S]*?)\n  \}/)[1];
  assert.ok(
    providerPrimaryAction.indexOf('if (provider.active)') < providerPrimaryAction.indexOf('if (!providerIsWorking(provider))'),
    'active providers should expose Use auto even when unavailable',
  );
  assert.match(html, /\$\('provider-add'\)\.onclick = \(\) => setView\('provider-add'\)/);
  assert.match(html, /if \(view === 'provider-add'\) return 'providers'/);
  assert.doesNotMatch(html, /id="provider-detail-use"/);
  assert.doesNotMatch(html, />Use provider<\/button>/);
});

test('provider setup actions are rendered and resolved safely in main', async () => {
  assert.match(html, /function providerSetupActions\(provider\)/);
  assert.match(html, /Setup actions/);
  assert.match(html, /runProviderSetupAction\(provider, action\.id/);
  assert.match(html, /focusProviderConfigField\(result\.focus_key\)/);
  assert.match(html, /result\.refresh_models/);
  assert.match(html, /renderProviderConfigGroups/);
  assert.match(html, /Resolved invoke target/);
  assert.match(html, /Max output tokens/);
  assert.match(html, /provider\.resolved_model_max_output_tokens/);
  assert.match(main, /function providerSetupActionById/);
  assert.match(main, /case 'bedrock_refresh_catalog'/);
  assert.match(main, /case 'aws_sts'/);
  assert.match(main, /providers', 'identity', '--provider', 'bedrock'/);
  assert.match(main, /case 'open_claude_config'/);
  assert.match(main, /case 'edit_extra_path'/);
  assert.match(main, /shell\.openPath/);
  assert.match(main, /export PATH=\$\{shellArg\(env\.PATH\)\}/);

  const openedPaths = [];
  const openedUrls = [];
  const terminalCommands = [];
  const providerCommands = [];
  const fakeSpawn = (_command, _args) => {
    const child = new EventEmitter();
    child.stderr = new EventEmitter();
    if (Array.isArray(_args)) {
      terminalCommands.push(_args.join(' '));
    }
    process.nextTick(() => child.emit('close', 0));
    return child;
  };
  const provider = createProviderService({
    fs,
    path,
    shell: {
      openPath: async (target) => {
        openedPaths.push(target);
        return '';
      },
      openExternal: async (target) => {
        openedUrls.push(target);
      },
    },
    spawn: fakeSpawn,
    PROVIDER_HEALTH_FILE: path.join(os.tmpdir(), `provider-health-${Date.now()}.json`),
    appendShellLog: () => {},
    notify: () => {},
    runAlvumJson: async (args) => {
      providerCommands.push(args);
      if (args[0] === 'providers' && args[1] === 'identity') {
        return { ok: true, provider: 'bedrock', account: '123456789012', arn: 'arn:aws:iam::123456789012:user/test' };
      }
      if (args[0] === 'providers' && args[1] === 'list') {
        return {
          configured: 'auto',
          providers: [{
            name: 'bedrock',
            display_name: 'AWS Bedrock',
            enabled: true,
            available: true,
            setup_kind: 'inline',
            setup_actions: [
              { id: 'open_aws_config', label: 'Open AWS config', kind: 'folder', detail: 'Open ~/.aws.' },
              { id: 'bedrock_refresh_catalog', label: 'Refresh catalog', kind: 'inline', detail: 'Refresh Bedrock catalog.' },
              { id: 'aws_sts', label: 'Check identity', kind: 'inline', detail: 'Run SDK STS.' },
              { id: 'aws_sso_login', label: 'Refresh SSO login', kind: 'terminal', detail: 'Run AWS SSO login.' },
              { id: 'edit_extra_path', label: 'Set helper PATH', kind: 'inline', detail: 'Set helper PATH.' },
              { id: 'bedrock_list_models', label: 'List with AWS CLI', kind: 'terminal', detail: 'Optional AWS CLI fallback.' },
            ],
            config_fields: [
              { key: 'aws_profile', value: 'dev', configured: true },
              { key: 'aws_region', value: 'us-west-2', configured: true },
              { key: 'text_model', value: 'anthropic.claude-sonnet-4-5-20250929-v1:0', configured: true },
            ],
          }],
        };
      }
      return { ok: true };
    },
    alvumSpawnEnv: () => ({ PATH: '/isengard/bin:/usr/bin' }),
    connectorList: async () => ({ connectors: [] }),
    broadcastState: () => {},
  });

  await provider.providerProbeSummary(true, false);
  const openResult = await provider.providerSetup('bedrock', 'open_aws_config');
  assert.equal(openResult.ok, true);
  assert.equal(openResult.action, 'folder');
  assert.ok(openedPaths.some((target) => target.endsWith('.aws')));

  const stsResult = await provider.providerSetup('bedrock', 'aws_sts');
  assert.equal(stsResult.ok, true);
  assert.equal(stsResult.action, 'provider_command');
  assert.deepEqual(providerCommands.find((args) => args[1] === 'identity'), ['providers', 'identity', '--provider', 'bedrock']);

  const pathResult = await provider.providerSetup('bedrock', 'edit_extra_path');
  assert.equal(pathResult.ok, true);
  assert.equal(pathResult.action, 'inline');
  assert.equal(pathResult.focus_key, 'extra_path');

  const refreshResult = await provider.providerSetup('bedrock', 'bedrock_refresh_catalog');
  assert.equal(refreshResult.ok, true);
  assert.equal(refreshResult.action, 'inline');
  assert.equal(refreshResult.refresh_models, true);

  await provider.providerSetup('bedrock', 'bedrock_list_models');
  assert.match(terminalCommands.join('\n'), /aws bedrock list-foundation-models --profile 'dev' --region 'us-west-2'/);
  assert.match(terminalCommands.join('\n'), /export PATH='\/isengard\/bin:\/usr\/bin':\\?"\$PATH\\?";/);

  await provider.providerSetup('bedrock', 'aws_sso_login');
  assert.match(terminalCommands.join('\n'), /aws sso login --profile 'dev'/);

  const unknownResult = await provider.providerSetup('bedrock', 'rm -rf ~');
  assert.equal(unknownResult.ok, false);
  assert.match(unknownResult.error, /unknown setup action/);
  assert.deepEqual(openedUrls, []);
});

test('app-triggered synthesis uses configured provider instead of hard-coded auto', () => {
  const dateFunction = main.match(/function generateBriefingForDate\(date, options = \{\}\) \{([\s\S]*?)\n\s+\}/)[1];
  assert.doesNotMatch(dateFunction, /'--provider',\s*'auto'/);
});

test('manual resynthesis reprocesses refs instead of using stale processed sidecar', () => {
  const start = main.indexOf('async function generateBriefingForDate(date, options = {})');
  const end = main.indexOf('\n  function openBriefingForDate', start);
  const dateFunction = main.slice(start, end);
  assert.match(dateFunction, /if \(resume\) args\.push\('--resume'\);\s+else args\.push\('--no-skip-processed'\);/);
});

test('permission-blocked connectors surface actionable status and settings', () => {
  assert.match(preload, /openPermissionSettings:\s+\(permission\)\s+=>\s+ipcRenderer\.invoke\('alvum:open-permission-settings', permission\)/);
  assert.match(main, /function capturePermissionStatus/);
  assert.match(main, /function startPermissionWatcher/);
  assert.match(main, /function annotateConnectorPermissions/);
  assert.match(main, /Permissions restored/);
  assert.match(main, /reconcileCaptureProcess\(\{ userInitiated: false \}\)/);
  assert.match(main, /ipcMain\.handle\('alvum:open-permission-settings'/);
  assert.match(main, /systemPreferences\.askForMediaAccess\('microphone'\)/);
  assert.match(main, /Privacy_ScreenCapture/);
  assert.match(main, /permission_issues/);
  assert.match(html, /function permissionIssueText/);
  assert.match(html, /function handlePermissionIssues/);
  assert.match(html, /Permission needed/);
  assert.match(html, /window\.alvum\.openPermissionSettings\(issue\.permission\)/);
  assert.match(html, /control\.blocked_permissions/);
  assert.match(html, /input\.blocked_permissions/);
});

test('fresh launch is privacy-first and capture starts only from enabled sources', () => {
  assert.match(main, /function reconcileCaptureProcess/);
  assert.match(main, /function consumeLaunchIntent/);
  assert.match(main, /enabledCaptureInputs\(\)/);
  assert.match(main, /status: 'no_enabled_sources'/);
  assert.match(main, /status: 'blocked_permissions'/);
  assert.match(main, /sectionEnabled\(sections, 'capture\.audio-mic', false\)/);
  assert.match(main, /sectionEnabled\(sections, 'capture\.audio-system', false\)/);
  assert.match(main, /sectionEnabled\(sections, 'capture\.screen', false\)/);
  const readyBlock = main.match(/app\.whenReady\(\)\.then\(\(\) => \{([\s\S]*?)\n\}\);/)[1];
  assert.doesNotMatch(readyBlock, /requestPermissions\(\)/);
  assert.doesNotMatch(readyBlock, /startCapture\(\)/);
  assert.match(readyBlock, /runtime\.consumeLaunchIntent\(\)/);
  assert.match(readyBlock, /launchIntent\.skip_capture_autostart \|\| launchIntent\.skipCaptureAutostart/);
  assert.match(readyBlock, /startup auto-start skipped by launch intent/);
  assert.match(readyBlock, /reconcileCaptureProcess\(\{ userInitiated: false \}\)/);
  assert.match(main, /fs\.closeSync\(out\);/);
  assert.match(main, /fs\.closeSync\(err\);/);
});

test('whisper install is exposed through preload and connector readiness', () => {
  assert.match(preload, /installWhisperModel:\s+\(variant\)\s+=>\s+ipcRenderer\.invoke\('alvum:install-whisper-model', variant\)/);
  assert.match(main, /ipcMain\.handle\('alvum:install-whisper-model'/);
  assert.match(main, /\['models', 'install', 'whisper', '--variant', variant\]/);
  assert.match(html, /function audioProcessorReadiness/);
  assert.match(html, /waiting_on_install/);
  assert.match(html, /function installWhisperModelFromUi/);
  assert.match(html, /whisperVariantFromSelectedModel\(\)/);
  assert.match(html, /window\.alvum\.installWhisperModel\(variant\)/);
  assert.match(html, /large-v3/);
  assert.match(html, /large-v3-turbo/);
  assert.match(html, /large-v3-turbo-q5_0/);
  assert.match(html, /readiness\.action\.kind === 'install_whisper'/);
});

test('pyannote install is exposed through preload and audio readiness', () => {
  assert.match(preload, /installPyannote:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:install-pyannote'\)/);
  assert.match(preload, /openPyannoteTerms:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:open-pyannote-terms'\)/);
  assert.match(main, /ipcMain\.handle\('alvum:install-pyannote'/);
  assert.match(main, /ipcMain\.handle\('alvum:open-pyannote-terms'/);
  assert.match(main, /\['models', 'install', 'pyannote', '--variant', 'community-1'\]/);
  assert.match(html, /function installPyannoteFromUi/);
  assert.match(html, /window\.alvum\.installPyannote\(\)/);
  assert.match(html, /function renderPyannoteAccessCard/);
  assert.match(html, /window\.alvum\.openPyannoteTerms\(\)/);
  assert.match(html, /pyannote_hf_token/);
  assert.match(html, /Save token and retry/);
  assert.match(html, /result\.detail \|\| result\.error \|\| 'Pyannote install failed'/);
  assert.match(html, /waiting_on_diarization_install/);
  assert.match(html, /requires_huggingface_access/);
  assert.match(html, /tokenConfigured/);
  assert.match(html, /readiness\.action\.kind === 'install_pyannote'/);
});

test('setup checklist actions stay contained in narrow popovers', () => {
  assert.match(html, /row\.className = 'settings-row setup-checklist-row'/);
  assert.match(html, /text\.className = 'setup-checklist-copy'/);
  assert.match(html, /button\.className = 'setup-checklist-action'/);
  assert.match(html, /\.setup-checklist \{[\s\S]*?width: 100%;[\s\S]*?min-width: 0;/);
  assert.match(html, /\.setup-checklist-row \{[\s\S]*?grid-template-columns: minmax\(0, 1fr\);[\s\S]*?overflow: hidden;/);
  assert.match(html, /\.setup-checklist-action \{[\s\S]*?width: 100%;[\s\S]*?min-width: 0;/);
  assert.match(html, /\.settings-row \{[\s\S]*?grid-template-columns: minmax\(0, 1fr\) auto;/);
  assert.match(html, /\.settings-row > :first-child \{[\s\S]*?min-width: 0;/);
});

test('setup first synthesis targets only completed capture days', () => {
  assert.match(html, /function firstSynthesisTarget\(\)/);
  assert.match(html, /currentState\.briefingCatchupDates/);
  assert.match(html, /target\.hasCapture[\s\S]*?!target\.hasBriefing[\s\S]*?target\.date < today/);
  assert.match(html, /const hasSuccessfulSynthesis = !!\(currentState\.latestBriefing && currentState\.latestBriefing\.date\)/);
  assert.match(html, /const needsFirstSynthesis = !schedule\.setup_completed && !hasSuccessfulSynthesis/);
  assert.match(html, /const hasAnyCaptureData = !!synthesisTarget \|\| Number\(currentState\.captureStats/);
  assert.match(html, /if \(synthesisTarget && needsFirstSynthesis\)/);
  assert.match(html, /onAction: \(\) => openSynthesisForDate\(synthesisTarget\.date\)/);
  assert.doesNotMatch(html, /find\(\(target\) => target\.hasCapture && !target\.hasBriefing\)/);
});

test('scheduler catchup ignores capture days that contain only empty folders', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-artifacts-'));
  const capture = path.join(root, 'capture');
  const briefings = path.join(root, 'briefings');
  fs.mkdirSync(path.join(capture, '2026-04-27', 'screen'), { recursive: true });
  fs.mkdirSync(path.join(capture, '2026-04-28', 'audio', 'mic'), { recursive: true });
  fs.writeFileSync(path.join(capture, '2026-04-28', 'audio', 'mic', 'chunk.wav'), 'audio');
  fs.mkdirSync(path.join(capture, '2026-04-29', 'screen'), { recursive: true });
  fs.writeFileSync(path.join(capture, '2026-04-29', 'screen', 'capture.png'), 'png');
  fs.mkdirSync(path.join(briefings, '2026-04-29'), { recursive: true });
  fs.writeFileSync(path.join(briefings, '2026-04-29', 'briefing.md'), '# done');

  const artifacts = createArtifactStore({
    fs,
    path,
    CAPTURE_DIR: capture,
    BRIEFINGS_DIR: briefings,
    todayStamp: () => '2026-04-30',
    dateAddDays: (stamp) => stamp,
  });

  assert.deepEqual(artifacts.pendingBriefingCatchup().dates, ['2026-04-28']);
});

test('launchd wakes Electron scheduler instead of running synthesis directly', () => {
  assert.match(launchdBriefing, /scripts\/wake-scheduler\.sh/);
  assert.match(launchdBriefing, /ALVUM_APP_BUNDLE/);
  assert.match(launchdBriefing, /ALVUM_LAUNCH_INTENT_FILE/);
  assert.doesNotMatch(launchdBriefing, /scripts\/briefing\.sh/);
  assert.match(wakeSchedulerScript, /"run_synthesis_due":true/);
  assert.match(wakeSchedulerScript, /open -gj "\$bundle"/);
  assert.doesNotMatch(wakeSchedulerScript, /\$ALVUM_BIN.*extract/);
});

test('installer writes privacy-first onboarding config and leaves scheduling to Electron', () => {
  assert.doesNotMatch(installScript, /command -v claude/);
  assert.match(installScript, /ALVUM_INSTALL_WHISPER/);
  assert.doesNotMatch(installScript, /ALVUM_SKIP_WHISPER/);
  assert.doesNotMatch(installScript, /install_plist/);
  assert.match(installScript, /unload_plist "\$ALVUM_LAUNCHAGENTS\/\$ALVUM_BRIEFING_LABEL\.plist"/);
  assert.doesNotMatch(installScript, /Install the menu-bar plugin/);
  assert.match(installScript, /ALVUM_INSTALL_SWIFTBAR/);

  const screenConnector = scriptTomlSection(installScript, 'connectors.screen');
  assert.match(screenConnector, /enabled = true/);
  assert.doesNotMatch(screenConnector, /vision/);

  const audioConnector = scriptTomlSection(installScript, 'connectors.audio');
  assert.match(audioConnector, /enabled = true/);
  assert.doesNotMatch(audioConnector, /whisper_model/);

  const screenProcessor = scriptTomlSection(installScript, 'processors.screen');
  assert.match(screenProcessor, /mode = "ocr"/);

  const audioProcessor = scriptTomlSection(installScript, 'processors.audio');
  assert.match(audioProcessor, /mode = "local"/);
  assert.match(audioProcessor, /whisper_model = "\$ALVUM_MODELS_DIR\/ggml-base\.en\.bin"/);
  assert.match(audioProcessor, /whisper_language = "en"/);
  assert.match(audioProcessor, /diarization_enabled = "true"/);
  assert.match(audioProcessor, /diarization_model = "pyannote-local"/);
  assert.match(audioProcessor, /pyannote_command = ""/);
  assert.match(audioProcessor, /speaker_registry = "\$ALVUM_ROOT\/runtime\/speakers\.json"/);

  const schedule = scriptTomlSection(installScript, 'scheduler.synthesis');
  assert.match(schedule, /enabled = false/);
  assert.match(schedule, /time = "07:00"/);
  assert.match(schedule, /policy = "completed_days"/);
  assert.match(schedule, /setup_completed = false/);
});

test('synthesis schedule is app-owned and exposed through customize UI', () => {
  assert.match(main, /createSynthesisScheduler/);
  assert.match(main, /synthesisSchedule: scheduler \? scheduler\.scheduleSnapshot\(\) : null/);
  assert.match(main, /onRunFinished: \(\.\.\.args\) => scheduler && scheduler\.handleBriefingRunFinished\(\.\.\.args\)/);
  assert.match(main, /scheduler\.start\(launchIntent\)/);
  assert.match(preload, /synthesisSchedule:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:synthesis-schedule'\)/);
  assert.match(preload, /synthesisScheduleSave:\s+\(patch\)\s+=>\s+ipcRenderer\.invoke\('alvum:synthesis-schedule-save', patch\)/);
  assert.match(preload, /synthesisScheduleRunDue:\s+\(\)\s+=>\s+ipcRenderer\.invoke\('alvum:synthesis-schedule-run-due'\)/);
  assert.match(main, /ipcMain\.handle\('alvum:synthesis-schedule'/);
  assert.match(main, /ipcMain\.handle\('alvum:synthesis-schedule-save'/);
  assert.match(main, /ipcMain\.handle\('alvum:synthesis-schedule-run-due'/);
  assert.match(rawHtml, /data-view="profile-schedule-detail"/);
  assert.match(rawHtml, /id="profile-schedule-save"/);
  assert.match(rawHtml, /id="profile-schedule-run-due"/);
  assert.match(html, /profileMenuRow\(\s*'Schedule'/);
  assert.match(html, /function renderProfileSchedule\(\)/);
  assert.match(html, /Automatic synthesis/);
  assert.match(html, /Completed days only/);
  assert.match(html, /synthesisScheduleSummary/);
  assert.match(html, /Queued for synthesis/);
  assert.match(html, /calendar-dot \$\{queuedForDay \? 'queued'/);
  assert.match(html, /if \(view === 'profile-schedule-detail'\) return 'synthesis-profile'/);
});

test('first successful manual synthesis enables the default daily schedule', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-scheduler-'));
  const configFile = path.join(root, 'runtime', 'config.toml');
  const launchAgents = path.join(root, 'LaunchAgents');
  const plist = path.join(launchAgents, 'com.alvum.briefing.plist');
  const config = {};
  writeSchedulerConfig(configFile, config);
  const broadcasts = [];
  const logs = [];
  const scheduler = createSynthesisScheduler({
    fs,
    path,
    spawn: launchctlSpawn,
    powerMonitor: null,
    appBundlePath: () => '/Applications/Alvum.app',
    ALVUM_ROOT: root,
    CONFIG_FILE: configFile,
    LAUNCHAGENTS_DIR: launchAgents,
    LAUNCHD_LABEL: 'com.alvum.briefing',
    LAUNCHD_PLIST: plist,
    appendShellLog: (line) => logs.push(line),
    notify: () => {},
    runAlvumText: schedulerConfigRunner(configFile, config),
    alvumSpawnEnv: () => ({}),
    briefing: {
      pendingBriefingCatchup: () => ({ dates: [] }),
      isBriefingRunning: () => false,
      generateBriefingForDate: async () => ({ ok: true }),
    },
    broadcastState: () => broadcasts.push(Date.now()),
  });

  assert.equal(scheduler.scheduleSnapshot().setup_pending, true);
  fs.mkdirSync(launchAgents, { recursive: true });
  fs.writeFileSync(plist, launchdBriefing);
  await scheduler.saveSchedule({ enabled: false });
  assert.equal(fs.existsSync(plist), false);

  await scheduler.handleBriefingRunFinished({ date: '2026-04-29', ok: true, source: 'manual' });

  const saved = scheduler.readSchedule();
  assert.equal(saved.enabled, true);
  assert.equal(saved.setup_completed, true);
  assert.equal(saved.time, '07:00');
  assert.equal(saved.policy, 'completed_days');
  assert.match(fs.readFileSync(plist, 'utf8'), /wake-scheduler\.sh/);
  assert.equal(logs.some((line) => line.includes('first manual synthesis succeeded')), true);
  assert.ok(broadcasts.length > 0);
});

test('existing synthesis output proves scheduler setup for migrated profiles', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-scheduler-'));
  const configFile = path.join(root, 'runtime', 'config.toml');
  const config = {
    enabled: false,
    time: '07:00',
    policy: 'completed_days',
    setup_completed: false,
    last_auto_run_date: '',
  };
  writeSchedulerConfig(configFile, config);
  const scheduler = createSynthesisScheduler({
    fs,
    path,
    spawn: launchctlSpawn,
    powerMonitor: null,
    appBundlePath: () => '/Applications/Alvum.app',
    ALVUM_ROOT: root,
    CONFIG_FILE: configFile,
    LAUNCHAGENTS_DIR: path.join(root, 'LaunchAgents'),
    LAUNCHD_LABEL: 'com.alvum.briefing',
    LAUNCHD_PLIST: path.join(root, 'LaunchAgents', 'com.alvum.briefing.plist'),
    appendShellLog: () => {},
    notify: () => {},
    runAlvumText: schedulerConfigRunner(configFile, config),
    alvumSpawnEnv: () => ({}),
    briefing: {
      pendingBriefingCatchup: () => ({ dates: ['2026-04-29'] }),
      latestBriefingInfo: () => ({ date: '2026-04-27', path: path.join(root, 'briefings', '2026-04-27', 'briefing.md') }),
      isBriefingRunning: () => false,
      generateBriefingForDate: async () => ({ ok: true }),
    },
    broadcastState: () => {},
  });

  assert.equal(scheduler.readSchedule().setup_completed, true);
  assert.equal(scheduler.scheduleSnapshot().setup_pending, false);
  assert.equal(scheduler.scheduleSnapshot().enabled, false);
});

test('scheduler queues completed days oldest-to-newest and continues after failure', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alvum-scheduler-'));
  const configFile = path.join(root, 'runtime', 'config.toml');
  const config = {
    enabled: true,
    time: '00:00',
    policy: 'completed_days',
    setup_completed: true,
    last_auto_run_date: '',
  };
  writeSchedulerConfig(configFile, config);
  const started = [];
  const scheduler = createSynthesisScheduler({
    fs,
    path,
    spawn: launchctlSpawn,
    powerMonitor: null,
    appBundlePath: () => '/Applications/Alvum.app',
    ALVUM_ROOT: root,
    CONFIG_FILE: configFile,
    LAUNCHAGENTS_DIR: path.join(root, 'LaunchAgents'),
    LAUNCHD_LABEL: 'com.alvum.briefing',
    LAUNCHD_PLIST: path.join(root, 'LaunchAgents', 'com.alvum.briefing.plist'),
    appendShellLog: () => {},
    notify: () => {},
    runAlvumText: schedulerConfigRunner(configFile, config),
    alvumSpawnEnv: () => ({}),
    briefing: {
      pendingBriefingCatchup: () => ({ dates: ['2026-04-27', '2026-04-28'] }),
      isBriefingRunning: () => false,
      generateBriefingForDate: async (date, options) => {
        started.push({ date, source: options.source });
        return { ok: true };
      },
    },
    broadcastState: () => {},
  });

  await scheduler.runDue({ reason: 'test', ignoreEnabled: true });
  await waitFor(() => started.length === 1, 'first queued day did not start');
  assert.deepEqual(started[0], { date: '2026-04-27', source: 'scheduler' });
  assert.equal(scheduler.scheduleSnapshot().running_date, '2026-04-27');
  assert.deepEqual(scheduler.scheduleSnapshot().queued_dates, ['2026-04-28']);

  await scheduler.handleBriefingRunFinished({ date: '2026-04-27', ok: false, reason: 'code 1', source: 'scheduler' });
  await waitFor(() => started.length === 2, 'second queued day did not start after failure');
  assert.deepEqual(started[1], { date: '2026-04-28', source: 'scheduler' });

  await scheduler.handleBriefingRunFinished({ date: '2026-04-28', ok: true, source: 'scheduler' });
  await waitFor(() => scheduler.scheduleSnapshot().running_date == null, 'scheduler did not drain running date');
  assert.deepEqual(scheduler.scheduleSnapshot().queued_dates, []);
});

test('provider detail renders data-type capabilities and per-modality models', () => {
  assert.match(html, /function renderProviderCapabilities/);
  assert.match(html, /provider\.selected_models/);
  assert.match(html, /capability\.provenance/);
  assert.match(html, /'Data types'/);
  assert.match(html, /\['text', 'Text'\], \['image', 'Image'\], \['audio', 'Audio'\]/);
  assert.match(html, /field\.key === 'text_model'/);
  assert.match(html, /field\.key === 'image_model'/);
  assert.match(html, /selected_models/);
  assert.match(html, /capabilities/);
});

test('ollama model catalog keeps installed text and image choices separate', () => {
  assert.match(html, /options_by_modality/);
  assert.match(html, /field\.key === 'audio_model' \? optionsByModality\.audio : optionsByModality\.text/);
  assert.match(html, /not installed/);
  assert.match(html, /No image models/);
  assert.match(html, /No audio models/);
  assert.match(html, /if \(option\.disabled\) item\.disabled = true;/);
  assert.match(html, /providerInstalledModelValues/);
  assert.match(html, /field\.key === 'model' \|\| field\.key === 'text_model' \|\| field\.key === 'image_model' \|\| field\.key === 'audio_model'/);
  assert.match(html, /providerModelInputSupport/);
  assert.match(html, /inputSupportCovers/);
  assert.match(html, /modelInputSupport\(model\)/);
  assert.match(html, /input_support/);
  assert.match(html, /labels\.join\(', '\)} input/);
  assert.match(html, /installable_model_error/);
  assert.doesNotMatch(html, /providerInstalledModelFamilies/);
  assert.doesNotMatch(html, /Small edge model; good first Ollama download for laptops/);
});

test('menu notifications overlay existing content without taking list space', () => {
  const block = html.match(/\.menu-notification \{([\s\S]*?)\n  \}/)[1];
  assert.match(block, /position: absolute;/);
  assert.match(block, /top: 34px;/);
  assert.match(block, /z-index: 20;/);
  assert.match(block, /pointer-events: none;/);
  assert.match(block, /backdrop-filter: saturate\(140%\) blur\(18px\);/);
  assert.match(block, /background: color-mix\(in srgb, var\(--surface\) 72%, transparent\);/);
});

test('menu notifications drop in then rise away after two seconds', () => {
  assert.match(html, /@keyframes menu-notification-drop/);
  assert.match(html, /@keyframes menu-notification-rise/);
  assert.match(html, /\.menu-notification\.presenting/);
  assert.match(html, /\.menu-notification\.dismissing/);
  assert.match(html, /menuNotificationDismissTimer/);
  assert.match(html, /menuNotificationHideTimer/);
  assert.match(html, /notification\.classList\.add\('presenting'\)/);
  assert.match(html, /notification\.classList\.add\('dismissing'\)/);
  assert.match(html, /}, 2000\);/);
});
