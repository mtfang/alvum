function createSynthesisScheduler({
  fs,
  path,
  spawn,
  powerMonitor,
  appBundlePath,
  ALVUM_ROOT,
  CONFIG_FILE,
  LAUNCHAGENTS_DIR,
  LAUNCHD_LABEL,
  LAUNCHD_PLIST,
  appendShellLog,
  notify,
  runAlvumText,
  alvumSpawnEnv,
  briefing,
  broadcastState,
}) {
  const CHECK_INTERVAL_MS = 60 * 1000;
  const WAKE_CHECK_DELAY_MS = 10 * 1000;
  const DEFAULT_SCHEDULE = {
    enabled: false,
    time: '07:00',
    policy: 'completed_days',
    setup_completed: false,
    last_auto_run_date: '',
  };

  let timer = null;
  let queue = [];
  let runningDate = null;
  let processing = false;
  let lastError = null;

  function parseFlatTomlSections(text) {
    const sections = {};
    let current = null;
    for (const raw of String(text || '').split(/\r?\n/)) {
      const line = raw.trim();
      if (!line || line.startsWith('#')) continue;
      const section = line.match(/^\[([^\]]+)\]$/);
      if (section) {
        current = section[1];
        sections[current] = sections[current] || {};
        continue;
      }
      if (!current) continue;
      const kv = line.match(/^([A-Za-z0-9_-]+)\s*=\s*(.+)$/);
      if (!kv) continue;
      let value = kv[2].trim();
      if (value === 'true') value = true;
      else if (value === 'false') value = false;
      else value = value.replace(/^"(.*)"$/, '$1');
      sections[current][kv[1]] = value;
    }
    return sections;
  }

  function validTime(value) {
    return /^([01]\d|2[0-3]):[0-5]\d$/.test(String(value || ''));
  }

  function setupCompletedFromHistory() {
    try {
      return !!(briefing && typeof briefing.latestBriefingInfo === 'function' && briefing.latestBriefingInfo());
    } catch {
      return false;
    }
  }

  function localDateStamp(date = new Date()) {
    const y = date.getFullYear();
    const m = String(date.getMonth() + 1).padStart(2, '0');
    const d = String(date.getDate()).padStart(2, '0');
    return `${y}-${m}-${d}`;
  }

  function localTimeString(date = new Date()) {
    return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
  }

  function escapeXml(value) {
    return String(value == null ? '' : value)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&apos;');
  }

  function readSchedule() {
    let section = {};
    try {
      const text = fs.existsSync(CONFIG_FILE) ? fs.readFileSync(CONFIG_FILE, 'utf8') : '';
      section = parseFlatTomlSections(text)['scheduler.synthesis'] || {};
    } catch {
      section = {};
    }
    const time = validTime(section.time) ? String(section.time) : DEFAULT_SCHEDULE.time;
    const policy = section.policy === 'completed_days' ? 'completed_days' : DEFAULT_SCHEDULE.policy;
    return {
      enabled: section.enabled === true,
      time,
      policy,
      setup_completed: section.setup_completed === true || setupCompletedFromHistory(),
      last_auto_run_date: typeof section.last_auto_run_date === 'string' ? section.last_auto_run_date : '',
    };
  }

  async function setConfigValue(key, value) {
    const result = await runAlvumText(['config-set', key, String(value)], 5000);
    if (!result.ok) throw new Error(result.error || result.stdout || `failed to set ${key}`);
    return result;
  }

  async function saveSchedule(patch = {}) {
    const previous = readSchedule();
    const next = {
      ...previous,
      ...patch,
    };
    next.setup_completed = !!next.setup_completed;
    next.enabled = next.setup_completed ? !!next.enabled : false;
    next.policy = next.policy === 'completed_days' ? 'completed_days' : DEFAULT_SCHEDULE.policy;
    if (!validTime(next.time)) next.time = DEFAULT_SCHEDULE.time;
    if (typeof next.last_auto_run_date !== 'string') next.last_auto_run_date = '';

    await setConfigValue('scheduler.synthesis.enabled', next.enabled);
    await setConfigValue('scheduler.synthesis.time', next.time);
    await setConfigValue('scheduler.synthesis.policy', next.policy);
    await setConfigValue('scheduler.synthesis.setup_completed', next.setup_completed);
    await setConfigValue('scheduler.synthesis.last_auto_run_date', next.last_auto_run_date);
    await configureLaunchd(next);
    broadcastState();
    return { ok: true, schedule: scheduleSnapshot() };
  }

  function dueDates() {
    const schedule = readSchedule();
    if (schedule.policy !== 'completed_days') return [];
    return briefing.pendingBriefingCatchup().dates || [];
  }

  function scheduleSnapshot() {
    const schedule = readSchedule();
    return {
      ...schedule,
      setup_pending: !schedule.setup_completed,
      due_dates: dueDates(),
      queued_dates: queue.slice(),
      running_date: runningDate,
      last_error: lastError,
    };
  }

  function launchdPlist(schedule = readSchedule()) {
    const [hour, minute] = schedule.time.split(':').map((part) => Number(part));
    const script = path.join(process.resourcesPath || path.join(ALVUM_ROOT, 'runtime'), 'scripts', 'wake-scheduler.sh');
    const fallbackScript = path.resolve(__dirname, '..', '..', 'scripts', 'wake-scheduler.sh');
    const scriptPath = fs.existsSync(script) ? script : fallbackScript;
    const bundlePath = appBundlePath();
    const launchIntentFile = path.join(ALVUM_ROOT, 'runtime', 'launch-intent.json');
    return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>${escapeXml(scriptPath)}</string>
  </array>
  <key>StartCalendarInterval</key>
  <dict>
    <key>Hour</key><integer>${hour}</integer>
    <key>Minute</key><integer>${minute}</integer>
  </dict>
  <key>RunAtLoad</key><false/>
  <key>StandardOutPath</key><string>${escapeXml(path.join(ALVUM_ROOT, 'runtime', 'logs', 'briefing.out'))}</string>
  <key>StandardErrorPath</key><string>${escapeXml(path.join(ALVUM_ROOT, 'runtime', 'logs', 'briefing.err'))}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>${escapeXml(process.env.PATH || '/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin')}</string>
    <key>ALVUM_APP_BUNDLE</key><string>${escapeXml(bundlePath)}</string>
    <key>ALVUM_LAUNCH_INTENT_FILE</key><string>${escapeXml(launchIntentFile)}</string>
  </dict>
</dict>
</plist>
`;
  }

  async function configureLaunchd(schedule = readSchedule()) {
    try {
      fs.mkdirSync(LAUNCHAGENTS_DIR, { recursive: true });
      if (!schedule.enabled || !schedule.setup_completed) {
        await new Promise((resolve) => {
          const bootout = spawn('launchctl', ['bootout', `gui/${process.getuid()}`, LAUNCHD_PLIST], {
            env: alvumSpawnEnv(),
            stdio: 'ignore',
          });
          bootout.on('close', () => resolve());
          bootout.on('error', () => resolve());
        });
        try {
          if (fs.existsSync(LAUNCHD_PLIST)) fs.unlinkSync(LAUNCHD_PLIST);
        } catch (e) {
          appendShellLog(`[scheduler] failed to remove disabled launchd plist: ${e.message}`);
        }
        return;
      }
      fs.writeFileSync(LAUNCHD_PLIST, launchdPlist(schedule));
      await new Promise((resolve) => {
        const bootout = spawn('launchctl', ['bootout', `gui/${process.getuid()}`, LAUNCHD_PLIST], {
          env: alvumSpawnEnv(),
          stdio: 'ignore',
        });
        bootout.on('close', () => resolve());
        bootout.on('error', () => resolve());
      });
      await new Promise((resolve) => {
        const bootstrap = spawn('launchctl', ['bootstrap', `gui/${process.getuid()}`, LAUNCHD_PLIST], {
          env: alvumSpawnEnv(),
          stdio: 'ignore',
        });
        bootstrap.on('close', (code) => {
          if (code !== 0) appendShellLog(`[scheduler] launchd bootstrap exited code=${code}`);
          resolve();
        });
        bootstrap.on('error', (e) => {
          appendShellLog(`[scheduler] launchd bootstrap failed: ${e.message}`);
          resolve();
        });
      });
    } catch (e) {
      appendShellLog(`[scheduler] failed to configure launchd: ${e.message}`);
    }
  }

  function enqueueDates(dates) {
    for (const date of dates) {
      if (date === runningDate || queue.includes(date)) continue;
      queue.push(date);
    }
    queue.sort();
  }

  async function runDue(options = {}) {
    const schedule = readSchedule();
    if (!options.ignoreEnabled && !schedule.enabled) {
      return { ok: false, status: 'disabled', schedule: scheduleSnapshot() };
    }
    const dates = dueDates();
    enqueueDates(dates);
    appendShellLog(`[scheduler] ${options.reason || 'manual'} due=${dates.join(',') || 'none'}`);
    processQueue();
    broadcastState();
    return { ok: true, status: queue.length || runningDate ? 'queued' : 'idle', dates, schedule: scheduleSnapshot() };
  }

  async function processQueue() {
    if (processing || runningDate || briefing.isBriefingRunning()) return;
    const date = queue.shift();
    if (!date) {
      broadcastState();
      return;
    }
    processing = true;
    lastError = null;
    try {
      const result = await briefing.generateBriefingForDate(date, { source: 'scheduler' });
      if (result && result.ok) {
        runningDate = date;
        appendShellLog(`[scheduler] started ${date}`);
      } else {
        const error = result && result.error ? result.error : 'could not start synthesis';
        lastError = `${date}: ${error}`;
        appendShellLog(`[scheduler] blocked ${date}: ${error}`);
        if (result && result.setupTarget) {
          queue = [];
        } else {
          setTimeout(() => processQueue(), 0);
        }
      }
    } catch (e) {
      lastError = `${date}: ${e.message}`;
      appendShellLog(`[scheduler] failed ${date}: ${e.stack || e}`);
      setTimeout(() => processQueue(), 0);
    } finally {
      processing = false;
      broadcastState();
    }
  }

  async function maybeRunDue(reason) {
    const schedule = readSchedule();
    if (!schedule.enabled || !schedule.setup_completed) return { ok: false, status: 'disabled', schedule: scheduleSnapshot() };
    const today = localDateStamp();
    if (schedule.last_auto_run_date === today) return { ok: true, status: 'already_checked', schedule: scheduleSnapshot() };
    if (localTimeString() < schedule.time) return { ok: true, status: 'not_due', schedule: scheduleSnapshot() };
    await saveSchedule({ last_auto_run_date: today });
    return runDue({ reason });
  }

  async function handleBriefingRunFinished(event) {
    if (event && event.date === runningDate) {
      runningDate = null;
      if (event.canceled) {
        queue = [];
        lastError = null;
      } else {
        if (!event.ok) lastError = `${event.date}: ${event.reason || 'synthesis failed'}`;
        setTimeout(() => processQueue(), 0);
      }
    }
    if (event && event.ok && event.source !== 'scheduler') {
      const schedule = readSchedule();
      if (!schedule.setup_completed && !schedule.enabled) {
        appendShellLog('[scheduler] first manual synthesis succeeded; enabling default schedule');
        await saveSchedule({ enabled: true, setup_completed: true, time: DEFAULT_SCHEDULE.time, policy: DEFAULT_SCHEDULE.policy });
        await runDue({ reason: 'setup-complete' });
      } else if (!schedule.setup_completed) {
        await saveSchedule({ setup_completed: true });
      }
    }
    broadcastState();
  }

  function start(launchIntent = {}) {
    configureLaunchd();
    if (timer) clearInterval(timer);
    timer = setInterval(() => maybeRunDue('timer'), CHECK_INTERVAL_MS);
    setTimeout(() => maybeRunDue(launchIntent.run_synthesis_due ? 'launchd' : 'startup'), 5000);
    if (powerMonitor) {
      powerMonitor.on('resume', () => setTimeout(() => maybeRunDue('wake'), WAKE_CHECK_DELAY_MS));
      powerMonitor.on('unlock-screen', () => setTimeout(() => maybeRunDue('unlock'), WAKE_CHECK_DELAY_MS));
    }
  }

  function shutdown() {
    if (timer) clearInterval(timer);
    timer = null;
  }

  return {
    readSchedule,
    saveSchedule,
    scheduleSnapshot,
    runDue,
    maybeRunDue,
    handleBriefingRunFinished,
    start,
    shutdown,
  };
}

module.exports = { createSynthesisScheduler };
