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
  const progressTail = createTailState();

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
  const eventsTail = createTailState();

  function createTailState() {
    return {
      cursor: 0,
      mtimeMs: 0,
      pending: '',
    };
  }

  function resetTailState(state, stat = null) {
    state.cursor = stat ? stat.size : 0;
    state.mtimeMs = stat ? stat.mtimeMs : 0;
    state.pending = '';
  }

  function readTailChunk(file, state, label) {
    let stat;
    try {
      stat = fs.statSync(file);
    } catch {
      return null;
    }
    if (stat.size === state.cursor && stat.mtimeMs === state.mtimeMs) return null;
    if (stat.size < state.cursor || stat.size === state.cursor) resetTailState(state);

    const start = state.cursor;
    const len = stat.size - start;
    if (len <= 0) {
      state.mtimeMs = stat.mtimeMs;
      return null;
    }

    try {
      const fd = fs.openSync(file, 'r');
      try {
        const buf = Buffer.alloc(len);
        fs.readSync(fd, buf, 0, len, start);
        state.cursor = stat.size;
        state.mtimeMs = stat.mtimeMs;
        return { chunk: buf.toString('utf8'), start };
      } finally {
        fs.closeSync(fd);
      }
    } catch (e) {
      appendShellLog(`[${label}] read failed: ${e.message}`);
      return null;
    }
  }

  function completeTailLines(file, state, label) {
    const read = readTailChunk(file, state, label);
    if (!read) return null;
    const text = `${state.pending}${read.chunk}`;
    const parts = text.split(/\n/);
    state.pending = text.endsWith('\n') ? '' : (parts.pop() || '');
    return {
      lines: parts.map((line) => line.replace(/\r$/, '')).filter((line) => line.trim()),
      start: read.start,
    };
  }

  function parseJsonlFile(file, state, label, decorate = (evt) => evt, retried = false) {
    const result = completeTailLines(file, state, label);
    if (!result) return [];
    const events = [];
    for (const line of result.lines) {
      let parsed;
      try {
        parsed = JSON.parse(line);
      } catch (e) {
        if (!retried && result.start > 0 && !line.trimStart().startsWith('{')) {
          resetTailState(state);
          return parseJsonlFile(file, state, label, decorate, true);
        }
        appendShellLog(`[${label}] bad JSON: ${e.message} line=${line}`);
        continue;
      }
      events.push(decorate(parsed));
    }
    return events;
  }

  function resetBriefingWatchers(run = null) {
    if (run) {
      run.progressTail = createTailState();
      run.eventsTail = createTailState();
      return;
    }
    // Reset the progress cursor BEFORE spawning so we don't race the
    // pipeline's progress::init() (truncate) followed by the first
    // progress::report() (write back to ~original size) — both can
    // happen within one 500-ms poll, leaving stat.size == cursor and
    // pollProgress thinking nothing changed.
    resetTailState(progressTail);
    // Same reset for the richer pipeline-events stream — same race for
    // the same reason (events::init() truncates at run start).
    resetTailState(eventsTail);
  }

  function startProgressWatcher() {
    fs.mkdirSync(path.dirname(PROGRESS_FILE), { recursive: true });
    if (fs.existsSync(PROGRESS_FILE)) {
      const s = fs.statSync(PROGRESS_FILE);
      resetTailState(progressTail, s);
    }
    setInterval(pollProgress, PROGRESS_POLL_MS);
  }

  function pollProgress() {
    for (const run of getRuns()) pollBriefingRunProgress(run);

    // Send only the latest line — the popover renders a single state, not
    // a sequence, so older events in the same poll are redundant.
    const events = parseJsonlFile(PROGRESS_FILE, progressTail, 'progress');
    const evt = events[events.length - 1];
    if (!evt) return;
    appendShellLog(`[progress] → ${evt.stage} ${evt.current}/${evt.total}`);
    sendToPopover('alvum:progress', evt);
  }

  function pollBriefingRunProgress(run) {
    if (!run.progressTail) run.progressTail = createTailState();
    const events = parseJsonlFile(
      run.progressFile,
      run.progressTail,
      `progress:${run.date}`,
      (evt) => ({ ...evt, briefingDate: run.date }),
    );
    const evt = events[events.length - 1];
    if (!evt) return;
    run.progress = evt;
    appendShellLog(`[progress:${run.date}] → ${evt.stage} ${evt.current}/${evt.total}`);
    sendToPopover('alvum:progress', evt);
  }

  function startEventsWatcher() {
    fs.mkdirSync(path.dirname(EVENTS_FILE), { recursive: true });
    if (fs.existsSync(EVENTS_FILE)) {
      const s = fs.statSync(EVENTS_FILE);
      resetTailState(eventsTail, s);
    }
    setInterval(pollEvents, EVENTS_POLL_MS);
  }

  function pollEvents() {
    for (const run of getRuns()) pollBriefingRunEvents(run);

    const events = parseJsonlFile(EVENTS_FILE, eventsTail, 'events');
    for (const evt of events) {
      recordProviderEvent(evt);
      sendToPopover('alvum:event', evt);
    }
  }

  function pollBriefingRunEvents(run) {
    if (!run.eventsTail) run.eventsTail = createTailState();
    const events = parseJsonlFile(
      run.eventsFile,
      run.eventsTail,
      `events:${run.date}`,
      (evt) => ({ ...evt, briefingDate: run.date }),
    );
    for (const evt of events) {
      recordProviderEvent(evt);
      sendToPopover('alvum:event', evt);
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
