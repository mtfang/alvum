function createBriefingRunStore({
  fs,
  path,
  crypto,
  shell,
  BRIEFINGS_DIR,
  appendShellLog,
  readTail,
  providerDiagnosticSnapshot,
  validDateStamp,
}) {
  function briefingFailurePath(date) {
    return path.join(BRIEFINGS_DIR, date, 'briefing.failed.json');
  }

  function briefingRunsDir(date) {
    return path.join(BRIEFINGS_DIR, date, 'runs');
  }

  function createRunId() {
    const stamp = new Date().toISOString().replace(/[-:.TZ]/g, '').slice(0, 14);
    return `${stamp}-${crypto.randomBytes(3).toString('hex')}`;
  }

  function readBriefingFailure(date) {
    try {
      const file = briefingFailurePath(date);
      if (!fs.existsSync(file)) return null;
      return JSON.parse(fs.readFileSync(file, 'utf8'));
    } catch {
      return { reason: 'previous generation failed' };
    }
  }

  function clearBriefingFailure(date) {
    try {
      const file = briefingFailurePath(date);
      if (fs.existsSync(file)) fs.unlinkSync(file);
    } catch {
      // Failure status is advisory; a stale marker should not break generation.
    }
  }

  function writeBriefingFailure(date, reason) {
    try {
      const dir = path.join(BRIEFINGS_DIR, date);
      fs.mkdirSync(dir, { recursive: true });
      const details = reason && typeof reason === 'object'
        ? { ...reason }
        : { reason: String(reason || 'generation failed') };
      fs.writeFileSync(briefingFailurePath(date), JSON.stringify({
        date,
        ...details,
        reason: details.reason || 'generation failed',
        failedAt: new Date().toISOString(),
      }, null, 2));
    } catch (e) {
      appendShellLog(`[briefing] failed to write failure marker for ${date}: ${e.message}`);
    }
  }

  function readJsonLines(file, maxBytes = 512 * 1024) {
    return readTail(file, maxBytes)
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        try {
          return JSON.parse(line);
        } catch {
          return null;
        }
      })
      .filter(Boolean);
  }

  function latestBriefingRunInfo(date) {
    try {
      const dir = briefingRunsDir(date);
      if (!fs.existsSync(dir)) return null;
      const runs = fs.readdirSync(dir)
        .map((runId) => {
          const runDir = path.join(dir, runId);
          const statusPath = path.join(runDir, 'status.json');
          try {
            const stat = fs.statSync(statusPath);
            const status = JSON.parse(fs.readFileSync(statusPath, 'utf8'));
            return { run_id: runId, run_dir: runDir, status_path: statusPath, mtimeMs: stat.mtimeMs, ...status };
          } catch {
            return null;
          }
        })
        .filter(Boolean)
        .sort((a, b) => b.mtimeMs - a.mtimeMs);
      return runs[0] || null;
    } catch {
      return null;
    }
  }

  function writeRunStatus(run, patch) {
    if (!run || !run.statusFile) return;
    run.status = {
      ...(run.status || {}),
      ...patch,
      updated_at: new Date().toISOString(),
    };
    try {
      fs.mkdirSync(path.dirname(run.statusFile), { recursive: true });
      fs.writeFileSync(run.statusFile, JSON.stringify(run.status, null, 2));
    } catch (e) {
      appendShellLog(`[briefing] failed to write run status: ${e.message}`);
    }
  }

  function summarizeRunDiagnostics(run, reason, code, signal, durationMs) {
    const progress = readJsonLines(run.progressFile);
    const events = readJsonLines(run.eventsFile);
    const lastProgress = progress[progress.length - 1] || null;
    const lastStageEvent = [...events].reverse().find((event) => event.stage);
    const lastError = [...events].reverse().find((event) => event.kind === 'error');
    const lastParseFailure = [...events].reverse().find((event) => event.kind === 'llm_parse_failed');
    return {
      reason,
      run_id: run.runId,
      run_dir: run.runDir,
      code,
      signal,
      duration_ms: durationMs,
      last_stage: lastProgress?.stage || lastStageEvent?.stage || null,
      last_pipeline_error: lastError ? {
        source: lastError.source || null,
        message: lastError.message || null,
      } : null,
      last_parse_failure: lastParseFailure ? {
        call_site: lastParseFailure.call_site || null,
        preview: lastParseFailure.preview || null,
      } : null,
      provider_state: providerDiagnosticSnapshot(),
      stderr_tail: readTail(run.stderrLog, 24 * 1024),
    };
  }

  function readJsonFileIfPresent(file) {
    if (!fs.existsSync(file)) return null;
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  }

  function briefingRunLog(date) {
    if (!validDateStamp(date)) {
      return { ok: false, error: 'invalid date' };
    }
    const latestRun = latestBriefingRunInfo(date);
    if (!latestRun) {
      const failure = readBriefingFailure(date);
      if (failure) {
        const sections = [
          `Date: ${date}`,
          'Status: failed',
          failure.reason ? `Reason: ${failure.reason}` : null,
          failure.run_id ? `Run: ${failure.run_id}` : null,
          failure.run_dir ? `Run dir: ${failure.run_dir}` : null,
          failure.failedAt ? `Failed: ${failure.failedAt}` : null,
          failure.last_stage ? `Last stage: ${failure.last_stage}` : null,
          failure.last_pipeline_error ? `Pipeline error: ${JSON.stringify(failure.last_pipeline_error, null, 2)}` : null,
          failure.stderr_tail ? `\nStderr:\n${failure.stderr_tail}` : null,
        ].filter(Boolean);
        return {
          ok: true,
          date,
          run: {
            run_id: failure.run_id || null,
            run_dir: failure.run_dir || null,
            status: 'failed',
            reason: failure.reason || null,
            last_stage: failure.last_stage || null,
            started_at: null,
            completed_at: failure.failedAt || null,
          },
          files: {
            failure: briefingFailurePath(date),
          },
          text: sections.join('\n'),
        };
      }
      return { ok: true, date, text: '', run: null };
    }
    const runDir = latestRun.run_dir;
    const statusPath = path.join(runDir, 'status.json');
    const progressPath = path.join(runDir, 'progress.jsonl');
    const eventsPath = path.join(runDir, 'events.jsonl');
    const stdoutPath = path.join(runDir, 'stdout.log');
    const stderrPath = path.join(runDir, 'stderr.log');
    let status = latestRun;
    try {
      status = readJsonFileIfPresent(statusPath) || latestRun;
    } catch (e) {
      status = { ...latestRun, status: 'unknown', reason: `could not parse status.json: ${e.message}` };
    }
    const eventLines = readTail(eventsPath, 160 * 1024)
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    const progressLines = readTail(progressPath, 80 * 1024)
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    const stderr = readTail(stderrPath, 48 * 1024).trim();
    const stdout = readTail(stdoutPath, 32 * 1024).trim();
    const sections = [
      `Run ${status.run_id || latestRun.run_id}`,
      `Date: ${date}`,
      `Status: ${status.status || 'unknown'}`,
      status.reason ? `Reason: ${status.reason}` : null,
      status.last_stage ? `Last stage: ${status.last_stage}` : null,
      status.completed_at ? `Completed: ${status.completed_at}` : null,
      status.provider_state ? `Providers: ${JSON.stringify(status.provider_state, null, 2)}` : null,
      eventLines.length ? `\nEvents:\n${eventLines.join('\n')}` : null,
      progressLines.length ? `\nProgress:\n${progressLines.join('\n')}` : null,
      stderr ? `\nStderr:\n${stderr}` : null,
      stdout ? `\nStdout:\n${stdout}` : null,
    ].filter(Boolean);
    return {
      ok: true,
      date,
      run: {
        run_id: latestRun.run_id,
        run_dir: runDir,
        status: status.status || latestRun.status || 'unknown',
        reason: status.reason || null,
        last_stage: status.last_stage || null,
        started_at: status.started_at || null,
        completed_at: status.completed_at || null,
      },
      files: {
        status: statusPath,
        progress: progressPath,
        events: eventsPath,
        stdout: stdoutPath,
        stderr: stderrPath,
      },
      text: sections.join('\n'),
    };
  }

  async function openBriefingRunLogs(date) {
    if (!validDateStamp(date)) {
      return { ok: false, error: 'invalid date' };
    }
    const latestRun = latestBriefingRunInfo(date);
    if (!latestRun || !latestRun.run_dir) {
      return { ok: false, error: 'no persisted run logs for this date' };
    }
    const error = await shell.openPath(latestRun.run_dir);
    if (error) return { ok: false, path: latestRun.run_dir, error };
    return { ok: true, path: latestRun.run_dir };
  }

  return {
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
  };
}

module.exports = { createBriefingRunStore };
