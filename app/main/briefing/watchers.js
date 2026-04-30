function createBriefingWatchers({
  fs,
  path,
  ALVUM_ROOT,
  appendShellLog,
  recordProviderEvent,
  sendToPopover,
  getRuns,
}) {
  // === Briefing progress watcher ========================================
  //
  // The Rust pipeline appends one JSON line per stage transition to
  // ~/.alvum/runtime/briefing.progress. We poll the file (same cadence
  // as the notification queue, no fancy fs.watch dance) and forward each
  // line to the popover renderer so it can update the progress bar +
  // stage checklist in real time.
  const PROGRESS_FILE = path.join(ALVUM_ROOT, 'runtime', 'briefing.progress');
  const PROGRESS_POLL_MS = 500;
  let progressCursor = 0;
  let progressMtimeMs = 0;

  // === Pipeline events watcher ==========================================
  //
  // Companion to the progress watcher above. The Rust pipeline writes one
  // JSON line per pipeline event (stage_enter/exit, llm_call_*,
  // input_inventory, input_filtered, warning, error, …) to
  // ~/.alvum/runtime/pipeline.events. We tail it the same way and forward
  // EVERY new line to the popover renderer — events are independent, the
  // popover renders a running list rather than a single state.
  const EVENTS_FILE = path.join(ALVUM_ROOT, 'runtime', 'pipeline.events');
  const EVENTS_POLL_MS = 500;
  let eventsCursor = 0;
  let eventsMtimeMs = 0;

  function resetBriefingWatchers(run = null) {
    if (run) {
      run.progressCursor = 0;
      run.progressMtimeMs = 0;
      run.eventsCursor = 0;
      run.eventsMtimeMs = 0;
      return;
    }
    // Reset the progress cursor BEFORE spawning so we don't race the
    // pipeline's progress::init() (truncate) followed by the first
    // progress::report() (write back to ~original size) — both can
    // happen within one 500-ms poll, leaving stat.size == cursor and
    // pollProgress thinking nothing changed.
    progressCursor = 0;
    progressMtimeMs = 0;
    // Same reset for the richer pipeline-events stream — same race for
    // the same reason (events::init() truncates at run start).
    eventsCursor = 0;
    eventsMtimeMs = 0;
  }

  function startProgressWatcher() {
    fs.mkdirSync(path.dirname(PROGRESS_FILE), { recursive: true });
    if (fs.existsSync(PROGRESS_FILE)) {
      const s = fs.statSync(PROGRESS_FILE);
      progressCursor = s.size;
      progressMtimeMs = s.mtimeMs;
    }
    setInterval(pollProgress, PROGRESS_POLL_MS);
  }

  function pollProgress() {
    for (const run of getRuns()) pollBriefingRunProgress(run);

    let stat;
    try {
      stat = fs.statSync(PROGRESS_FILE);
    } catch {
      return;
    }
    // Skip only when nothing has changed AT ALL. Tracking mtime on top
    // of size catches the truncate-then-write race where the pipeline
    // truncates the file via progress::init() and then writes the first
    // event back to a similar size within one poll interval — without
    // mtime, stat.size == cursor and we'd miss the change.
    if (stat.size === progressCursor && stat.mtimeMs === progressMtimeMs) return;

    // mtime changed without a size shrink → re-read whole file. mtime
    // changed AND size shrank → ditto. Only the size-equal-and-cursor-
    // matches case is interpreted as "appended bytes since last read".
    if (stat.size <= progressCursor) progressCursor = 0;

    let chunk;
    try {
      const fd = fs.openSync(PROGRESS_FILE, 'r');
      const len = stat.size - progressCursor;
      const buf = Buffer.alloc(len);
      fs.readSync(fd, buf, 0, len, progressCursor);
      fs.closeSync(fd);
      chunk = buf.toString('utf8');
      progressCursor = stat.size;
      progressMtimeMs = stat.mtimeMs;
    } catch (e) {
      appendShellLog(`[progress] read failed: ${e.message}`);
      return;
    }

    // Send only the latest line — the popover renders a single state, not
    // a sequence, so older events in the same poll are redundant.
    const lines = chunk.split('\n').filter((l) => l.trim());
    if (!lines.length) return;
    const last = lines[lines.length - 1];
    try {
      const evt = JSON.parse(last);
      appendShellLog(`[progress] → ${evt.stage} ${evt.current}/${evt.total}`);
      sendToPopover('alvum:progress', evt);
    } catch (e) {
      appendShellLog(`[progress] bad JSON: ${e.message} line=${last}`);
    }
  }

  function pollBriefingRunProgress(run) {
    let stat;
    try {
      stat = fs.statSync(run.progressFile);
    } catch {
      return;
    }
    if (stat.size === run.progressCursor && stat.mtimeMs === run.progressMtimeMs) return;
    if (stat.size <= run.progressCursor) run.progressCursor = 0;

    let chunk;
    try {
      const fd = fs.openSync(run.progressFile, 'r');
      const len = stat.size - run.progressCursor;
      const buf = Buffer.alloc(len);
      fs.readSync(fd, buf, 0, len, run.progressCursor);
      fs.closeSync(fd);
      chunk = buf.toString('utf8');
      run.progressCursor = stat.size;
      run.progressMtimeMs = stat.mtimeMs;
    } catch (e) {
      appendShellLog(`[progress:${run.date}] read failed: ${e.message}`);
      return;
    }

    const lines = chunk.split('\n').filter((l) => l.trim());
    if (!lines.length) return;
    const last = lines[lines.length - 1];
    try {
      const evt = { ...JSON.parse(last), briefingDate: run.date };
      run.progress = evt;
      appendShellLog(`[progress:${run.date}] → ${evt.stage} ${evt.current}/${evt.total}`);
      sendToPopover('alvum:progress', evt);
    } catch (e) {
      appendShellLog(`[progress:${run.date}] bad JSON: ${e.message} line=${last}`);
    }
  }

  function startEventsWatcher() {
    fs.mkdirSync(path.dirname(EVENTS_FILE), { recursive: true });
    if (fs.existsSync(EVENTS_FILE)) {
      const s = fs.statSync(EVENTS_FILE);
      eventsCursor = s.size;
      eventsMtimeMs = s.mtimeMs;
    }
    setInterval(pollEvents, EVENTS_POLL_MS);
  }

  function pollEvents() {
    for (const run of getRuns()) pollBriefingRunEvents(run);

    let stat;
    try {
      stat = fs.statSync(EVENTS_FILE);
    } catch {
      return;
    }
    if (stat.size === eventsCursor && stat.mtimeMs === eventsMtimeMs) return;
    if (stat.size <= eventsCursor) eventsCursor = 0;

    let chunk;
    try {
      const fd = fs.openSync(EVENTS_FILE, 'r');
      const len = stat.size - eventsCursor;
      const buf = Buffer.alloc(len);
      fs.readSync(fd, buf, 0, len, eventsCursor);
      fs.closeSync(fd);
      chunk = buf.toString('utf8');
      eventsCursor = stat.size;
      eventsMtimeMs = stat.mtimeMs;
    } catch (e) {
      appendShellLog(`[events] read failed: ${e.message}`);
      return;
    }

    const lines = chunk.split('\n').filter((l) => l.trim());
    for (const line of lines) {
      let evt;
      try {
        evt = JSON.parse(line);
      } catch (e) {
        appendShellLog(`[events] bad JSON: ${e.message} line=${line}`);
        continue;
      }
      recordProviderEvent(evt);
      sendToPopover('alvum:event', evt);
    }
  }

  function pollBriefingRunEvents(run) {
    let stat;
    try {
      stat = fs.statSync(run.eventsFile);
    } catch {
      return;
    }
    if (stat.size === run.eventsCursor && stat.mtimeMs === run.eventsMtimeMs) return;
    if (stat.size <= run.eventsCursor) run.eventsCursor = 0;

    let chunk;
    try {
      const fd = fs.openSync(run.eventsFile, 'r');
      const len = stat.size - run.eventsCursor;
      const buf = Buffer.alloc(len);
      fs.readSync(fd, buf, 0, len, run.eventsCursor);
      fs.closeSync(fd);
      chunk = buf.toString('utf8');
      run.eventsCursor = stat.size;
      run.eventsMtimeMs = stat.mtimeMs;
    } catch (e) {
      appendShellLog(`[events:${run.date}] read failed: ${e.message}`);
      return;
    }

    const lines = chunk.split('\n').filter((l) => l.trim());
    for (const line of lines) {
      try {
        const evt = { ...JSON.parse(line), briefingDate: run.date };
        recordProviderEvent(evt);
        sendToPopover('alvum:event', evt);
      } catch (e) {
        appendShellLog(`[events:${run.date}] bad JSON: ${e.message} line=${line}`);
      }
    }
  }

  return {
    resetBriefingWatchers,
    startProgressWatcher,
    pollProgress,
    pollBriefingRunProgress,
    startEventsWatcher,
    pollEvents,
    pollBriefingRunEvents,
  };
}

module.exports = { createBriefingWatchers };
