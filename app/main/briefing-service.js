const {
  todayStamp,
  dateAddDays,
  localMidnightUtc,
  validDateStamp,
} = require('./briefing/dates');
const { createArtifactStore } = require('./briefing/artifacts');
const { createBriefingRunStore } = require('./briefing/run-store');
const { createBriefingCalendar } = require('./briefing/calendar');
const { createBriefingMarkdown } = require('./briefing/markdown');
const { createDecisionGraphReader } = require('./briefing/decision-graph');
const { createBriefingWatchers } = require('./briefing/watchers');

function createBriefingService({
  fs,
  path,
  crypto,
  shell,
  spawn,
  ALVUM_ROOT,
  BRIEFINGS_DIR,
  CAPTURE_DIR,
  BRIEFING_LOG,
  BRIEFING_ERR,
  appendShellLog,
  notify,
  resolveScript,
  resolveBinary,
  alvumSpawnEnv,
  ensureLogDir,
  readTail,
  providerDiagnosticSnapshot,
  providerProbeSummary,
  providerSelectableForAuto,
  refreshProviderWatch,
  recordProviderEvent,
  broadcastState,
  rebuildTrayMenu,
  sendToPopover,
}) {
  const briefingRuns = new Map();
  const artifacts = createArtifactStore({
    fs,
    path,
    CAPTURE_DIR,
    BRIEFINGS_DIR,
    todayStamp,
    dateAddDays,
  });
  const runStore = createBriefingRunStore({
    fs,
    path,
    crypto,
    shell,
    BRIEFINGS_DIR,
    appendShellLog,
    readTail,
    providerDiagnosticSnapshot,
    validDateStamp,
  });
  const calendar = createBriefingCalendar({
    fs,
    path,
    BRIEFINGS_DIR,
    todayStamp,
    artifactSummaryForDate: artifacts.artifactSummaryForDate,
    readBriefingFailure: runStore.readBriefingFailure,
    latestBriefingRunInfo: runStore.latestBriefingRunInfo,
  });
  const markdown = createBriefingMarkdown({
    fs,
    path,
    BRIEFINGS_DIR,
    validDateStamp,
  });
  const decisionGraph = createDecisionGraphReader({
    fs,
    path,
    BRIEFINGS_DIR,
    validDateStamp,
  });
  const watchers = createBriefingWatchers({
    fs,
    path,
    ALVUM_ROOT,
    appendShellLog,
    recordProviderEvent,
    sendToPopover,
    getRuns: () => briefingRuns.values(),
  });

  const {
    formatBytes,
    scanFileStats,
    artifactSummaryForDate,
    pendingBriefingCatchup,
    latestBriefingInfo,
    recentBriefingTargets,
    captureStats,
  } = artifacts;
  const {
    briefingFailurePath,
    briefingRunsDir,
    createRunId,
    readBriefingFailure,
    clearBriefingFailure,
    writeBriefingFailure,
    readJsonLines,
    latestBriefingRunInfo,
    writeRunStatus,
    summarizeRunDiagnostics,
    briefingRunLog,
    openBriefingRunLogs,
  } = runStore;
  const {
    briefingDayInfo,
    briefingCalendarMonth,
  } = calendar;
  const {
    renderBriefingMarkdown,
    readBriefingForDate,
  } = markdown;
  const {
    readJsonFileIfPresent,
    readJsonlFileIfPresent,
    decisionGraphDomains,
    fallbackDecisionGraphEdges,
    readDecisionGraphForDate,
  } = decisionGraph;
  const {
    resetBriefingWatchers,
    startProgressWatcher,
    pollProgress,
    pollBriefingRunProgress,
    startEventsWatcher,
    pollEvents,
    pollBriefingRunEvents,
  } = watchers;

  function briefingRunSnapshot() {
    const runs = {};
    for (const [date, run] of briefingRuns.entries()) {
      runs[date] = {
        date,
        label: run.label,
        startedAt: run.startedAt.toLocaleTimeString(),
        startedAtMs: run.startedAt.getTime(),
        progress: run.progress || null,
        lastPct: run.lastPct || 0,
      };
    }
    return runs;
  }

  function startBriefingProcess(command, args, label, targetDate = null, extraEnv = {}) {
    if (targetDate && briefingRuns.has(targetDate)) {
      appendShellLog(`[briefing] ${targetDate} already running, ignoring request`);
      return { ok: false, error: 'briefing already running for date' };
    }
    const runDate = targetDate || todayStamp();
    const runId = createRunId();
    const runDir = path.join(briefingRunsDir(runDate), runId);
    fs.mkdirSync(runDir, { recursive: true });
    const run = {
      date: runDate,
      runId,
      runDir,
      label,
      startedAt: new Date(),
      proc: null,
      progress: null,
      lastPct: 0,
      progressFile: path.join(runDir, 'progress.jsonl'),
      eventsFile: path.join(runDir, 'events.jsonl'),
      stdoutLog: path.join(runDir, 'stdout.log'),
      stderrLog: path.join(runDir, 'stderr.log'),
      statusFile: path.join(runDir, 'status.json'),
      progressCursor: 0,
      progressMtimeMs: 0,
      eventsCursor: 0,
      eventsMtimeMs: 0,
      expectedBriefing: path.join(BRIEFINGS_DIR, runDate, 'briefing.md'),
      previousBriefingMtimeMs: (() => {
        try { return fs.statSync(path.join(BRIEFINGS_DIR, runDate, 'briefing.md')).mtimeMs; } catch { return 0; }
      })(),
      status: null,
    };
    writeRunStatus(run, {
      status: 'running',
      run_id: runId,
      date: runDate,
      label,
      command,
      args,
      started_at: run.startedAt.toISOString(),
      provider_state: providerDiagnosticSnapshot(),
    });
    resetBriefingWatchers(run);
    ensureLogDir();
    const globalOut = fs.createWriteStream(BRIEFING_LOG, { flags: 'a' });
    const globalErr = fs.createWriteStream(BRIEFING_ERR, { flags: 'a' });
    const runOut = fs.createWriteStream(run.stdoutLog, { flags: 'a' });
    const runErr = fs.createWriteStream(run.stderrLog, { flags: 'a' });
    let proc;
    try {
      const env = alvumSpawnEnv({
        ...extraEnv,
        ALVUM_PROGRESS_FILE: run.progressFile,
        ALVUM_PIPELINE_EVENTS_FILE: run.eventsFile,
      });
      proc = spawn(command, args, {
        cwd: ALVUM_ROOT,
        stdio: ['ignore', 'pipe', 'pipe'],
        env,
        detached: false,
      });
      run.proc = proc;
      briefingRuns.set(runDate, run);
      proc.stdout.on('data', (chunk) => {
        globalOut.write(chunk);
        runOut.write(chunk);
      });
      proc.stderr.on('data', (chunk) => {
        globalErr.write(chunk);
        runErr.write(chunk);
      });
    } catch (e) {
      globalOut.end();
      globalErr.end();
      runOut.end();
      runErr.end();
      appendShellLog(`[briefing] spawn threw: ${e.stack || e}`);
      const diagnostics = {
        reason: e.message,
        run_id: runId,
        run_dir: runDir,
        code: null,
        signal: null,
        duration_ms: Date.now() - run.startedAt.getTime(),
      };
      writeRunStatus(run, { status: 'failed', ...diagnostics });
      writeBriefingFailure(runDate, diagnostics);
      notify('Alvum', `Failed to start briefing: ${e.message}`);
      return { ok: false, error: e.message };
    }
    appendShellLog(`[briefing] spawned pid=${proc ? proc.pid : 'unknown'} label=${label} run=${runId}`);
    notify('Alvum', `${label} started. You'll get another notification when it's ready.`);
    rebuildTrayMenu();

    proc.on('error', (e) => {
      appendShellLog(`[briefing] spawn error: ${e.stack || e}`);
    });
    proc.on('close', (code, signal) => {
      globalOut.end();
      globalErr.end();
      runOut.end();
      runErr.end();
      const finishedRun = briefingRuns.get(runDate) || run;
      const durationMs = finishedRun ? Date.now() - finishedRun.startedAt.getTime() : 0;
      appendShellLog(`[briefing] exited code=${code} signal=${signal} duration_ms=${durationMs} run=${runId}`);
      briefingRuns.delete(runDate);
      let producedBriefing = true;
      if (finishedRun && finishedRun.expectedBriefing) {
        try {
          producedBriefing = fs.statSync(finishedRun.expectedBriefing).mtimeMs > finishedRun.previousBriefingMtimeMs;
        } catch {
          producedBriefing = false;
        }
      }
      if (code === 0 && producedBriefing) {
        clearBriefingFailure(runDate);
        writeRunStatus(run, {
          status: 'success',
          code,
          signal,
          duration_ms: durationMs,
          completed_at: new Date().toISOString(),
          briefing_path: finishedRun.expectedBriefing,
        });
        notify('Alvum', `${label} ready (${Math.round(durationMs / 1000)}s).`);
      } else {
        const reason = signal ? `signal ${signal}` : (code === 0 ? 'no briefing generated' : `code ${code}`);
        const diagnostics = summarizeRunDiagnostics(run, reason, code, signal, durationMs);
        writeRunStatus(run, {
          status: 'failed',
          ...diagnostics,
          completed_at: new Date().toISOString(),
        });
        writeBriefingFailure(runDate, diagnostics);
        notify('Alvum', `${label} failed (${reason}). See ${run.stderrLog}.`);
        setTimeout(() => refreshProviderWatch(true), 0);
      }
      rebuildTrayMenu();
      broadcastState();
    });
    return { ok: true, date: runDate, run_id: runId, run_dir: runDir };
  }

  function generateBriefing() {
    const script = resolveScript('briefing.sh');
    if (!script) {
      notify('Alvum', 'briefing.sh not found. Missing from bundle Resources/scripts?');
      return { ok: false, error: 'briefing.sh not found' };
    }
    return startBriefingProcess('/bin/bash', [script], 'Briefing');
  }

  async function synthesisPreflight(date) {
    const artifactsForDate = artifactSummaryForDate(date);
    if (!artifactsForDate.files) {
      return {
        ok: false,
        error: 'No capture data is available for this date yet.',
        setupTarget: 'capture',
      };
    }

    let summary = await providerProbeSummary(false, false);
    let usable = summary && Array.isArray(summary.providers)
      ? summary.providers.some(providerSelectableForAuto)
      : false;
    if (!usable) {
      summary = await providerProbeSummary(true, true);
      usable = summary && Array.isArray(summary.providers)
        ? summary.providers.some(providerSelectableForAuto)
        : false;
    }
    if (!usable) {
      const message = summary && summary.error
        ? summary.error
        : 'No enabled provider is authenticated and ready for synthesis.';
      return {
        ok: false,
        error: message,
        setupTarget: 'providers',
        providerSummary: summary,
      };
    }
    return { ok: true, providerSummary: summary };
  }

  async function generateBriefingForDate(date) {
    if (!validDateStamp(date)) {
      return { ok: false, error: 'invalid date' };
    }
    const bin = resolveBinary();
    if (!bin) return { ok: false, error: 'alvum binary not found' };
    const preflight = await synthesisPreflight(date);
    if (!preflight.ok) {
      appendShellLog(`[briefing] preflight blocked ${date}: ${preflight.error}`);
      return preflight;
    }
    const captureDir = path.join(CAPTURE_DIR, date);
    const outDir = path.join(BRIEFINGS_DIR, date);
    fs.mkdirSync(outDir, { recursive: true });
    const briefingPath = path.join(outDir, 'briefing.md');
    const resume = !fs.existsSync(briefingPath);
    const args = [
      'extract',
      '--capture-dir', captureDir,
      '--output', outDir,
      '--since', localMidnightUtc(date),
      '--before', localMidnightUtc(dateAddDays(date, 1)),
      '--briefing-date', date,
    ];
    if (resume) args.push('--resume');
    return startBriefingProcess(bin, args, `Briefing ${date}`, date);
  }

  function openBriefingForDate(date) {
    if (!validDateStamp(date)) {
      return { ok: false, error: 'invalid date' };
    }
    const file = path.join(BRIEFINGS_DIR, date, 'briefing.md');
    if (!fs.existsSync(file)) {
      notify('Alvum', `No briefing yet for ${date}. Generate it first.`);
      return { ok: false, error: 'briefing not found' };
    }
    shell.openPath(file);
    return { ok: true };
  }

  function openTodayBriefing() {
    return openBriefingForDate(todayStamp());
  }

  function isBriefingRunning() {
    return briefingRuns.size > 0;
  }

  return {
    todayStamp,
    dateAddDays,
    localMidnightUtc,
    formatBytes,
    scanFileStats,
    artifactSummaryForDate,
    pendingBriefingCatchup,
    latestBriefingInfo,
    recentBriefingTargets,
    briefingFailurePath,
    briefingRunsDir,
    createRunId,
    readBriefingFailure,
    clearBriefingFailure,
    writeBriefingFailure,
    readJsonLines,
    latestBriefingRunInfo,
    writeRunStatus,
    summarizeRunDiagnostics,
    briefingRunLog,
    openBriefingRunLogs,
    briefingDayInfo,
    briefingCalendarMonth,
    resetBriefingWatchers,
    briefingRunSnapshot,
    startBriefingProcess,
    generateBriefing,
    synthesisPreflight,
    generateBriefingForDate,
    openBriefingForDate,
    renderBriefingMarkdown,
    readBriefingForDate,
    readJsonFileIfPresent,
    readJsonlFileIfPresent,
    decisionGraphDomains,
    fallbackDecisionGraphEdges,
    readDecisionGraphForDate,
    openTodayBriefing,
    captureStats,
    startProgressWatcher,
    pollProgress,
    pollBriefingRunProgress,
    startEventsWatcher,
    pollEvents,
    pollBriefingRunEvents,
    isBriefingRunning,
  };
}

module.exports = { createBriefingService };
