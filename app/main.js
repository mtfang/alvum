// Alvum Electron shell — the "box" the spec calls for.
//
// Responsibilities (MVP, ~200 LOC cap):
//   • Proper Cocoa app so macOS TCC prompts render and permissions
//     persist across rebuilds.
//   • Menu bar (Tray) icon with status + quick actions.
//   • Spawn `alvum capture` as a child process. Because Electron is
//     the Cocoa responsible-process and it holds the TCC grants, the
//     Rust subprocess inherits mic/screen access via the standard
//     responsible-process chain — no TCC dance in Rust.
//
// Out of scope for MVP: web UI, auto-update, Windows/Linux, packaging
// polish. Those are all in the full spec but are deferred until
// capture runs reliably through this shell.

const { app, Tray, Menu, BrowserWindow, ipcMain, shell, screen, systemPreferences, Notification, nativeImage, dialog } = require('electron');
const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

const HOME = os.homedir();
const ALVUM_ROOT = path.join(HOME, '.alvum');
const LOG_DIR = path.join(ALVUM_ROOT, 'runtime', 'logs');
const LOG_OUT = path.join(LOG_DIR, 'capture.out');
const LOG_ERR = path.join(LOG_DIR, 'capture.err');
const SHELL_LOG = path.join(LOG_DIR, 'shell.log');

function appendShellLog(line) {
  try {
    fs.mkdirSync(LOG_DIR, { recursive: true });
    fs.appendFileSync(SHELL_LOG, `${new Date().toISOString()} ${line}\n`);
  } catch (e) {
    // Last resort — original console call, before we reroute it below.
    origConsoleError('appendShellLog failed', e);
  }
}

// Route every console.* from the Electron main process into shell.log so
// logs exist even when launched via `open` (which detaches stdout). The
// Rust capture subprocess has its own out/err sinks (capture.out / .err).
const origConsoleLog = console.log.bind(console);
const origConsoleError = console.error.bind(console);
const origConsoleWarn = console.warn.bind(console);
function fmtArgs(args) {
  return args.map((a) => {
    if (typeof a === 'string') return a;
    try { return JSON.stringify(a); } catch { return String(a); }
  }).join(' ');
}
console.log = (...args) => { appendShellLog(`[log] ${fmtArgs(args)}`); origConsoleLog(...args); };
console.error = (...args) => { appendShellLog(`[err] ${fmtArgs(args)}`); origConsoleError(...args); };
console.warn = (...args) => { appendShellLog(`[warn] ${fmtArgs(args)}`); origConsoleWarn(...args); };

process.on('uncaughtException', (e) => {
  appendShellLog(`[uncaughtException] ${e && e.stack ? e.stack : e}`);
});
process.on('unhandledRejection', (reason) => {
  appendShellLog(`[unhandledRejection] ${reason && reason.stack ? reason.stack : reason}`);
});

// Binary resolution:
//   • Packaged builds put the binary in a nested helper app so macOS
//     permission settings can resolve the Alvum icon for the TCC client.
//   • Dev runs (`npm start` from the repo) use the Cargo target dir.
//   • A user-installed binary at ~/.alvum/runtime/Alvum.app/Contents/MacOS/alvum
//     is accepted as a final fallback for transitional installs.
function resolveBinary() {
  const packagedHelper = path.join(
    process.resourcesPath || '',
    '..',
    'Helpers',
    'Alvum Capture.app',
    'Contents',
    'MacOS',
    'alvum');
  const packaged = path.join(process.resourcesPath || '', 'bin', 'alvum');
  const dev = path.resolve(__dirname, '..', 'target', 'release', 'alvum');
  const legacy = path.join(ALVUM_ROOT, 'runtime', 'Alvum.app', 'Contents', 'MacOS', 'alvum');
  for (const candidate of [packagedHelper, packaged, dev, legacy]) {
    if (candidate && fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function alvumSpawnEnv(extraEnv = {}) {
  const pathEntries = [
    path.join(HOME, '.local', 'bin'),
    path.join(HOME, '.cargo', 'bin'),
    path.join(HOME, '.bun', 'bin'),
    '/opt/homebrew/bin',
    '/opt/homebrew/sbin',
    '/usr/local/bin',
    '/usr/local/sbin',
    '/usr/bin',
    '/bin',
    '/usr/sbin',
    '/sbin',
    ...(process.env.PATH || '').split(path.delimiter),
  ].filter(Boolean);
  const PATH = [...new Set(pathEntries)].join(path.delimiter);
  return { ...process.env, PATH, ...extraEnv };
}

let tray = null;
let captureProc = null;
let captureStartedAt = null;
let permissionWatchTimer = null;
let lastPermissionStatusKey = '';
let lastPermissionBlockKey = '';
const briefingRuns = new Map();

const BRIEFINGS_DIR = path.join(ALVUM_ROOT, 'generated', 'briefings');
const CAPTURE_DIR = path.join(ALVUM_ROOT, 'capture');
const BRIEFING_LOG = path.join(LOG_DIR, 'briefing.out');
const BRIEFING_ERR = path.join(LOG_DIR, 'briefing.err');
const CONFIG_FILE = path.join(ALVUM_ROOT, 'runtime', 'config.toml');
const EXTENSIONS_DIR = path.join(ALVUM_ROOT, 'runtime', 'extensions');
const PERMISSION_SETTINGS_URLS = {
  microphone: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone',
  screen: 'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture',
};
const PERMISSION_WATCH_MS = 3000;
const SOURCE_PERMISSION_REQUIREMENTS = {
  'audio-mic': [{ permission: 'microphone', label: 'Microphone' }],
  'audio-system': [{ permission: 'screen', label: 'Screen & System Audio Recording' }],
  screen: [{ permission: 'screen', label: 'Screen Recording' }],
};

function ensureExtensionsDir() {
  fs.mkdirSync(EXTENSIONS_DIR, { recursive: true });
}

function resolveScript(name) {
  const packaged = path.join(process.resourcesPath || '', 'scripts', name);
  const dev = path.resolve(__dirname, '..', 'scripts', name);
  for (const candidate of [packaged, dev]) {
    if (candidate && fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function todayStamp() {
  // Local-day YYYY-MM-DD so it matches briefing.sh's `date +%Y-%m-%d`.
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const dd = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${dd}`;
}

function dateAddDays(stamp, days) {
  const [y, m, d] = stamp.split('-').map(Number);
  const date = new Date(y, m - 1, d + days);
  const yy = date.getFullYear();
  const mm = String(date.getMonth() + 1).padStart(2, '0');
  const dd = String(date.getDate()).padStart(2, '0');
  return `${yy}-${mm}-${dd}`;
}

function localMidnightUtc(stamp) {
  const [y, m, d] = stamp.split('-').map(Number);
  return new Date(y, m - 1, d).toISOString().replace(/\.\d{3}Z$/, 'Z');
}

function formatBytes(bytes) {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = value >= 10 || unit === 0 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

function scanFileStats(root) {
  const totals = { files: 0, bytes: 0, byExt: new Map() };
  function walk(dir) {
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (entry.name === '.DS_Store') continue;
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
        continue;
      }
      if (!entry.isFile()) continue;
      let size = 0;
      try {
        size = fs.statSync(full).size;
      } catch {
        size = 0;
      }
      const ext = (path.extname(entry.name).slice(1) || 'file').toLowerCase();
      const current = totals.byExt.get(ext) || { files: 0, bytes: 0 };
      current.files += 1;
      current.bytes += size;
      totals.byExt.set(ext, current);
      totals.files += 1;
      totals.bytes += size;
    }
  }
  walk(root);
  return totals;
}

function artifactSummaryForDate(stamp) {
  const dir = path.join(CAPTURE_DIR, stamp);
  const stats = scanFileStats(dir);
  const detail = [...stats.byExt.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([ext, v]) => `${ext}: ${v.files} files / ${formatBytes(v.bytes)}`)
    .join('\n');
  return {
    date: stamp,
    files: stats.files,
    bytes: stats.bytes,
    summary: `${stats.files} files · ${formatBytes(stats.bytes)}`,
    detail: detail || 'No capture artifacts for today',
  };
}

function pendingBriefingCatchup() {
  const today = todayStamp();
  try {
    if (!fs.existsSync(CAPTURE_DIR)) return { count: 0, dates: [] };
    const dates = fs.readdirSync(CAPTURE_DIR)
      .filter((name) => /^\d{4}-\d{2}-\d{2}$/.test(name))
      .filter((name) => name < today)
      .filter((name) => {
        const capturePath = path.join(CAPTURE_DIR, name);
        const briefingPath = path.join(BRIEFINGS_DIR, name, 'briefing.md');
        if (fs.existsSync(briefingPath)) return false;
        try {
          return fs.readdirSync(capturePath).length > 0;
        } catch {
          return false;
        }
      })
      .sort();
    return { count: dates.length, dates };
  } catch {
    return { count: 0, dates: [] };
  }
}

function latestBriefingInfo() {
  try {
    if (!fs.existsSync(BRIEFINGS_DIR)) return null;
    const entries = fs.readdirSync(BRIEFINGS_DIR)
      .filter((name) => /^\d{4}-\d{2}-\d{2}$/.test(name))
      .map((date) => {
        const file = path.join(BRIEFINGS_DIR, date, 'briefing.md');
        if (!fs.existsSync(file)) return null;
        const stat = fs.statSync(file);
        return { date, path: file, mtimeMs: stat.mtimeMs, mtime: new Date(stat.mtimeMs).toLocaleString() };
      })
      .filter(Boolean)
      .sort((a, b) => b.date.localeCompare(a.date));
    return entries[0] || null;
  } catch {
    return null;
  }
}

function recentBriefingTargets() {
  const today = todayStamp();
  const wanted = new Set([today, dateAddDays(today, -1)]);
  try {
    if (fs.existsSync(CAPTURE_DIR)) {
      for (const name of fs.readdirSync(CAPTURE_DIR)) {
        if (/^\d{4}-\d{2}-\d{2}$/.test(name)) wanted.add(name);
      }
    }
  } catch {
    // Keep the Today/Yesterday fallback list.
  }
  return [...wanted]
    .sort((a, b) => b.localeCompare(a))
    .slice(0, 10)
    .map((date) => {
      const captureDir = path.join(CAPTURE_DIR, date);
      const briefingPath = path.join(BRIEFINGS_DIR, date, 'briefing.md');
      const artifacts = artifactSummaryForDate(date);
      return {
        date,
        label: date === today ? 'Today' : (date === dateAddDays(today, -1) ? 'Yesterday' : date),
        hasCapture: artifacts.files > 0,
        hasBriefing: fs.existsSync(briefingPath),
        artifacts: artifacts.summary,
        captureDir,
      };
    });
}

function briefingFailurePath(date) {
  return path.join(BRIEFINGS_DIR, date, 'briefing.failed.json');
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
    fs.writeFileSync(briefingFailurePath(date), JSON.stringify({
      date,
      reason,
      failedAt: new Date().toISOString(),
    }, null, 2));
  } catch (e) {
    appendShellLog(`[briefing] failed to write failure marker for ${date}: ${e.message}`);
  }
}

function briefingDayInfo(date) {
  const briefingPath = path.join(BRIEFINGS_DIR, date, 'briefing.md');
  const artifacts = artifactSummaryForDate(date);
  const failure = readBriefingFailure(date);
  const hasBriefing = fs.existsSync(briefingPath);
  const hasCapture = artifacts.files > 0;
  return {
    date,
    hasCapture,
    hasBriefing,
    artifacts: artifacts.summary,
    status: hasBriefing ? 'success' : (failure ? 'failed' : (hasCapture ? 'captured' : 'empty')),
    failure,
  };
}

function briefingCalendarMonth(month) {
  const today = todayStamp();
  const monthStamp = /^\d{4}-\d{2}$/.test(month || '') ? month : today.slice(0, 7);
  const [y, m] = monthStamp.split('-').map(Number);
  const first = new Date(y, m - 1, 1);
  const start = new Date(y, m - 1, 1 - first.getDay());
  const days = [];
  for (let i = 0; i < 42; i += 1) {
    const d = new Date(start.getFullYear(), start.getMonth(), start.getDate() + i);
    const date = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
    days.push({
      ...briefingDayInfo(date),
      inMonth: date.slice(0, 7) === monthStamp,
      isToday: date === today,
    });
  }
  return {
    month: monthStamp,
    label: first.toLocaleString(undefined, { month: 'long', year: 'numeric' }),
    today,
    days,
  };
}

// Notification icon as ATTACHMENT image. Self-signed LSUIElement
// Electron apps don't get a sender-side icon (left of the toast) —
// that requires a proper Apple Developer ID signature or a compiled
// Assets.car (needs Xcode). Passing this here renders on the right
// side, which still surfaces the alvum brand on every notification.
const APP_ICON = nativeImage.createFromPath(path.join(__dirname, 'assets', 'icon.png'));

function notify(title, body) {
  try {
    new Notification({ title, body, icon: APP_ICON }).show();
  } catch (e) {
    console.error('notify failed', e);
  }
}

// External-notification queue. Out-of-process tools (capture.sh toggle,
// menu-bar.sh, briefing.sh, …) append a JSON line per notification; we
// poll the file and fan each line into the bundle's Electron Notification
// API so the system shows the alvum logo instead of the AppleScript icon
// `osascript display notification` is hard-locked to since Big Sur.
const NOTIFY_QUEUE = path.join(ALVUM_ROOT, 'runtime', 'notify.queue');
const NOTIFY_TTL_MS = 60 * 1000;          // ignore lines older than this on startup
const NOTIFY_POLL_MS = 500;
let notifyCursor = 0;

function startNotifyQueueWatcher() {
  fs.mkdirSync(path.dirname(NOTIFY_QUEUE), { recursive: true });
  // Seed the cursor at current size so a backlog of stale entries (e.g.
  // a long-running queue from before this app instance launched) doesn't
  // dump every old notification at once. The TTL filter further protects
  // against time-skewed lines that may race in during the first poll.
  if (fs.existsSync(NOTIFY_QUEUE)) {
    notifyCursor = fs.statSync(NOTIFY_QUEUE).size;
  } else {
    fs.writeFileSync(NOTIFY_QUEUE, '');
  }
  setInterval(pollNotifyQueue, NOTIFY_POLL_MS);
}

function pollNotifyQueue() {
  let stat;
  try {
    stat = fs.statSync(NOTIFY_QUEUE);
  } catch {
    return;                                // file vanished; nothing to do
  }
  if (stat.size === notifyCursor) return;
  if (stat.size < notifyCursor) {           // truncated externally; resync
    notifyCursor = 0;
  }

  let chunk;
  try {
    const fd = fs.openSync(NOTIFY_QUEUE, 'r');
    const len = stat.size - notifyCursor;
    const buf = Buffer.alloc(len);
    fs.readSync(fd, buf, 0, len, notifyCursor);
    fs.closeSync(fd);
    chunk = buf.toString('utf8');
    notifyCursor = stat.size;
  } catch (e) {
    appendShellLog(`[notify-queue] read failed: ${e.message}`);
    return;
  }

  const now = Date.now();
  for (const line of chunk.split('\n')) {
    if (!line.trim()) continue;
    let payload;
    try {
      payload = JSON.parse(line);
    } catch (e) {
      appendShellLog(`[notify-queue] bad JSON: ${e.message} line=${line}`);
      continue;
    }
    // Drop ancient lines so a long-stopped Alvum.app doesn't burst
    // weeks of notifications when it relaunches.
    if (payload.ts && now - payload.ts > NOTIFY_TTL_MS) continue;
    notify(payload.title || 'Alvum', payload.body || '');
  }
}

function capturePermissionStatus() {
  if (process.platform !== 'darwin') return {};
  return {
    microphone: systemPreferences.getMediaAccessStatus('microphone'),
    screen: systemPreferences.getMediaAccessStatus('screen'),
  };
}

function sourcePermissionRequirements(sourceId) {
  return SOURCE_PERMISSION_REQUIREMENTS[sourceId] || [];
}

function blockedPermissionsForSource(sourceId, permissions = capturePermissionStatus()) {
  return sourcePermissionRequirements(sourceId)
    .filter((requirement) => permissions[requirement.permission] !== 'granted')
    .map((requirement) => ({
      ...requirement,
      status: permissions[requirement.permission] || 'unknown',
    }));
}

function permissionIssueSummary(issues) {
  if (!issues || !issues.length) return '';
  const labelsByPermission = new Map(issues.map((issue) => [
    issue.permission || issue.label,
    issue.permission === 'screen'
      ? 'Screen & System Audio Recording'
      : (issue.label || issue.permission),
  ]));
  const labels = [...labelsByPermission.values()];
  const sourceLabels = [...new Set(issues.map((issue) => issue.source_label).filter(Boolean))];
  const target = sourceLabels.length === 1 ? sourceLabels[0] : 'Enabled connectors';
  const suffix = labels.length === 1 ? 'permission' : 'permissions';
  return `${target} blocked by ${labels.join(' and ')} ${suffix}.`;
}

let lastPermissionNotificationKey = '';
function notifyPermissionIssues(issues) {
  if (!issues || !issues.length) return;
  const key = JSON.stringify(issues.map((issue) => [
    issue.connector_id || '',
    issue.source_id || '',
    issue.permission,
    issue.status,
  ]).sort());
  if (key === lastPermissionNotificationKey) return;
  lastPermissionNotificationKey = key;
  appendShellLog(`[permissions] enabled connector blocked: ${permissionIssueSummary(issues)} (${issues.map((issue) => `${issue.permission}:${issue.status}`).join(', ')})`);
  notify('Alvum permission needed', permissionIssueSummary(issues));
}

async function openPermissionSettings(permission) {
  const key = permission === 'microphone' ? 'microphone' : 'screen';
  const url = PERMISSION_SETTINGS_URLS[key];
  try {
    await shell.openExternal(url);
    return { ok: true, permission: key, url };
  } catch (e) {
    return { ok: false, permission: key, url, error: e.message };
  }
}

async function promptForSourcePermissions(sourceIds, openSettings = true) {
  const ids = Array.isArray(sourceIds) ? sourceIds : [sourceIds];
  const needsMic = ids.some((id) => sourcePermissionRequirements(id).some((req) => req.permission === 'microphone'));
  if (needsMic) {
    const micStatus = systemPreferences.getMediaAccessStatus('microphone');
    if (micStatus !== 'granted') {
      const ok = await systemPreferences.askForMediaAccess('microphone');
      console.log('[permissions] mic grant response:', ok);
      if (!ok && openSettings) await openPermissionSettings('microphone');
    }
  }

  const permissions = capturePermissionStatus();
  const issues = ids.flatMap((id) =>
    blockedPermissionsForSource(id, permissions).map((issue) => ({
      ...issue,
      source_id: id,
    })));
  if (openSettings) {
    const screenIssue = issues.find((issue) => issue.permission === 'screen');
    const micIssue = issues.find((issue) => issue.permission === 'microphone');
    if (screenIssue) await openPermissionSettings('screen');
    else if (micIssue) await openPermissionSettings('microphone');
  }
  return issues;
}

function enabledPermissionIssues() {
  return captureInputsSummary().inputs.flatMap((input) =>
    (input.blocked_permissions || []).map((issue) => ({
      ...issue,
      source_id: input.id,
      source_label: input.label,
    })));
}

function reportEnabledPermissionBlocks() {
  notifyPermissionIssues(enabledPermissionIssues());
}

function permissionIssuesKey(issues) {
  return JSON.stringify((issues || []).map((issue) => [
    issue.source_id || '',
    issue.permission || '',
    issue.status || '',
  ]).sort());
}

function startPermissionWatcher() {
  if (permissionWatchTimer) return;
  lastPermissionStatusKey = JSON.stringify(capturePermissionStatus());
  lastPermissionBlockKey = permissionIssuesKey(enabledPermissionIssues());
  permissionWatchTimer = setInterval(() => {
    const status = capturePermissionStatus();
    const statusKey = JSON.stringify(status);
    const issues = enabledPermissionIssues();
    const blockKey = permissionIssuesKey(issues);
    const statusChanged = statusKey !== lastPermissionStatusKey;
    const blocksChanged = blockKey !== lastPermissionBlockKey;
    if (!statusChanged && !blocksChanged) return;

    appendShellLog(`[permissions] status changed: ${statusKey}`);
    const hadBlocks = lastPermissionBlockKey !== '[]';
    const hasBlocks = issues.length > 0;
    if (hasBlocks) notifyPermissionIssues(issues);
    if (hadBlocks && !hasBlocks && captureProc) {
      appendShellLog('[permissions] Permissions restored; restarting capture');
      notify('Alvum permissions restored', 'Restarting capture.');
      restartCapture();
    }
    lastPermissionStatusKey = statusKey;
    lastPermissionBlockKey = blockKey;
    broadcastState();
  }, PERMISSION_WATCH_MS);
}

function requestPermissions() {
  const micStatus = systemPreferences.getMediaAccessStatus('microphone');
  const screenStatus = systemPreferences.getMediaAccessStatus('screen');
  appendShellLog(`[permissions] microphone status: ${micStatus}`);
  appendShellLog(`[permissions] screen status: ${screenStatus}`);

  notifyPermissionIssues(enabledPermissionIssues());
}

function ensureLogDir() {
  fs.mkdirSync(LOG_DIR, { recursive: true });
}

function startCapture() {
  if (captureProc && !captureProc.killed) return;

  const bin = resolveBinary();
  appendShellLog(`[startCapture] resolveBinary → ${bin}`);
  if (!bin) {
    notify('Alvum', 'Could not locate alvum binary. Build with `cargo build --release -p alvum-cli`.');
    return;
  }

  ensureLogDir();
  const out = fs.openSync(LOG_OUT, 'a');
  const err = fs.openSync(LOG_ERR, 'a');

  try {
    captureProc = spawn(bin, ['capture'], {
      cwd: ALVUM_ROOT,
      stdio: ['ignore', out, err],
      env: alvumSpawnEnv({ RUST_LOG: process.env.RUST_LOG || 'info' }),
      detached: false,
    });
  } catch (e) {
    appendShellLog(`[startCapture] spawn threw: ${e.stack || e}`);
    notify('Alvum', `Failed to spawn capture: ${e.message}`);
    return;
  }
  captureStartedAt = new Date();
  appendShellLog(`[startCapture] spawned pid=${captureProc.pid} bin=${bin}`);

  captureProc.on('error', (e) => {
    appendShellLog(`[capture] spawn error: ${e.stack || e}`);
  });
  captureProc.on('exit', (code, signal) => {
    appendShellLog(`[capture] exited code=${code} signal=${signal}`);
    captureProc = null;
    captureStartedAt = null;
    rebuildTrayMenu();
  });

  rebuildTrayMenu();
}

function stopCapture() {
  if (!captureProc) return;
  try {
    captureProc.kill('SIGTERM');
  } catch (e) {
    console.error('[capture] SIGTERM failed', e);
  }
}

function restartCapture() {
  if (captureProc) {
    captureProc.once('exit', () => startCapture());
    stopCapture();
  } else {
    startCapture();
  }
}

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
  const run = targetDate ? {
    date: targetDate,
    label,
    startedAt: new Date(),
    proc: null,
    progress: null,
    lastPct: 0,
    progressFile: path.join(ALVUM_ROOT, 'runtime', `briefing.${targetDate}.progress`),
    eventsFile: path.join(ALVUM_ROOT, 'runtime', `pipeline.${targetDate}.events`),
    progressCursor: 0,
    progressMtimeMs: 0,
    eventsCursor: 0,
    eventsMtimeMs: 0,
    expectedBriefing: path.join(BRIEFINGS_DIR, targetDate, 'briefing.md'),
    previousBriefingMtimeMs: (() => {
      try { return fs.statSync(path.join(BRIEFINGS_DIR, targetDate, 'briefing.md')).mtimeMs; } catch { return 0; }
    })(),
  } : null;
  if (run) {
    resetBriefingWatchers(run);
  } else {
    resetBriefingWatchers();
  }
  ensureLogDir();
  const out = fs.openSync(BRIEFING_LOG, 'a');
  const err = fs.openSync(BRIEFING_ERR, 'a');
  let proc;
  try {
    const env = alvumSpawnEnv({
      ...extraEnv,
      ...(run ? {
        ALVUM_PROGRESS_FILE: run.progressFile,
        ALVUM_PIPELINE_EVENTS_FILE: run.eventsFile,
      } : {}),
    });
    proc = spawn(command, args, {
      cwd: ALVUM_ROOT,
      stdio: ['ignore', out, err],
      env,
      detached: false,
    });
    if (run) {
      run.proc = proc;
      briefingRuns.set(targetDate, run);
    }
  } catch (e) {
    appendShellLog(`[briefing] spawn threw: ${e.stack || e}`);
    if (targetDate) writeBriefingFailure(targetDate, e.message);
    notify('Alvum', `Failed to start briefing: ${e.message}`);
    return { ok: false, error: e.message };
  }
  appendShellLog(`[briefing] spawned pid=${proc ? proc.pid : 'unknown'} label=${label}`);
  notify('Alvum', `${label} started. You'll get another notification when it's ready.`);
  rebuildTrayMenu();

  proc.on('error', (e) => {
    appendShellLog(`[briefing] spawn error: ${e.stack || e}`);
  });
  proc.on('exit', (code, signal) => {
    const finishedRun = targetDate ? briefingRuns.get(targetDate) : null;
    const durationMs = finishedRun ? Date.now() - finishedRun.startedAt.getTime() : 0;
    appendShellLog(`[briefing] exited code=${code} signal=${signal} duration_ms=${durationMs}`);
    if (targetDate) briefingRuns.delete(targetDate);
    let producedBriefing = true;
    if (finishedRun && finishedRun.expectedBriefing) {
      try {
        producedBriefing = fs.statSync(finishedRun.expectedBriefing).mtimeMs > finishedRun.previousBriefingMtimeMs;
      } catch {
        producedBriefing = false;
      }
    }
    if (code === 0 && producedBriefing) {
      if (targetDate) clearBriefingFailure(targetDate);
      notify('Alvum', `${label} ready (${Math.round(durationMs / 1000)}s).`);
    } else {
      const reason = signal ? `signal ${signal}` : (code === 0 ? 'no briefing generated' : `code ${code}`);
      if (targetDate) writeBriefingFailure(targetDate, reason);
      notify('Alvum', `${label} failed (${reason}). See ${BRIEFING_ERR}.`);
      setTimeout(() => refreshProviderWatch(true), 0);
    }
    rebuildTrayMenu();
  });
  return { ok: true };
}

function generateBriefing() {
  const script = resolveScript('briefing.sh');
  if (!script) {
    notify('Alvum', 'briefing.sh not found. Missing from bundle Resources/scripts?');
    return { ok: false, error: 'briefing.sh not found' };
  }
  return startBriefingProcess('/bin/bash', [script], 'Briefing');
}

function generateBriefingForDate(date) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date || '')) {
    return { ok: false, error: 'invalid date' };
  }
  const bin = resolveBinary();
  if (!bin) return { ok: false, error: 'alvum binary not found' };
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
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date || '')) {
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

let markdownRendererPromise = null;

function markdownRenderer() {
  if (!markdownRendererPromise) {
    markdownRendererPromise = Promise.all([
      import('unified'),
      import('remark-parse'),
      import('remark-gfm'),
      import('remark-math'),
      import('remark-rehype'),
      import('rehype-sanitize'),
      import('rehype-katex'),
      import('rehype-stringify'),
    ]).then(([
      unifiedMod,
      remarkParseMod,
      remarkGfmMod,
      remarkMathMod,
      remarkRehypeMod,
      rehypeSanitizeMod,
      rehypeKatexMod,
      rehypeStringifyMod,
    ]) => unifiedMod.unified()
      .use(remarkParseMod.default)
      .use(remarkGfmMod.default)
      .use(remarkMathMod.default)
      .use(remarkRehypeMod.default)
      .use(rehypeSanitizeMod.default)
      .use(rehypeKatexMod.default, { strict: false, throwOnError: false })
      .use(rehypeStringifyMod.default));
  }
  return markdownRendererPromise;
}

async function renderBriefingMarkdown(markdown) {
  const processor = await markdownRenderer();
  const rendered = await processor.process(markdown || '');
  return String(rendered);
}

async function readBriefingForDate(date) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date || '')) {
    return { ok: false, error: 'invalid date' };
  }
  const file = path.join(BRIEFINGS_DIR, date, 'briefing.md');
  try {
    const stat = fs.statSync(file);
    const markdown = fs.readFileSync(file, 'utf8');
    return {
      ok: true,
      date,
      path: file,
      mtime: new Date(stat.mtimeMs).toLocaleString(),
      markdown,
      html: await renderBriefingMarkdown(markdown),
    };
  } catch (e) {
    return { ok: false, date, error: e.message };
  }
}

function readJsonFileIfPresent(file) {
  if (!fs.existsSync(file)) return null;
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function readJsonlFileIfPresent(file) {
  if (!fs.existsSync(file)) return { exists: false, items: [] };
  const raw = fs.readFileSync(file, 'utf8');
  const items = raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  return { exists: true, items };
}

function decisionGraphDomains(profileSnapshot, domainRows, decisions) {
  const ordered = [];
  const seen = new Set();
  const push = (id, enabled = true) => {
    const value = String(id || '').trim();
    if (!value || seen.has(value) || enabled === false) return;
    seen.add(value);
    ordered.push(value);
  };

  const profileDomains = profileSnapshot
    && profileSnapshot.profile
    && Array.isArray(profileSnapshot.profile.domains)
    ? profileSnapshot.profile.domains
    : [];
  profileDomains
    .slice()
    .sort((a, b) => Number(a.priority || 0) - Number(b.priority || 0))
    .forEach((domain) => push(domain.name || domain.id, domain.enabled));
  domainRows.forEach((domain) => push(domain.id || domain.name));
  decisions.forEach((decision) => {
    push(decision.domain);
    (decision.cross_domain || []).forEach((domain) => push(domain));
  });
  return ordered.length ? ordered : ['Career', 'Health', 'Family'];
}

function fallbackDecisionGraphEdges(decisions) {
  const edges = [];
  const seen = new Set();
  const ids = new Set(decisions.map((decision) => decision.id).filter(Boolean));
  const add = (fromId, toId, metadata = {}) => {
    if (!fromId || !toId || !ids.has(fromId) || !ids.has(toId)) return;
    const key = `${fromId}->${toId}`;
    if (seen.has(key)) return;
    seen.add(key);
    edges.push({
      from_id: fromId,
      to_id: toId,
      relation: metadata.relation || metadata.mechanism || 'caused',
      mechanism: metadata.mechanism || metadata.rationale || '',
      strength: metadata.strength || 'contributing',
      rationale: metadata.rationale || null,
      derived_from_decisions: true,
    });
  };

  decisions.forEach((decision) => {
    (decision.causes || []).forEach((cause) => {
      if (typeof cause === 'string') {
        add(cause, decision.id);
      } else if (cause && typeof cause === 'object') {
        add(cause.from_id || cause.id, cause.to_id || decision.id, cause);
      }
    });
    (decision.effects || []).forEach((effect) => {
      if (typeof effect === 'string') {
        add(decision.id, effect);
      } else if (effect && typeof effect === 'object') {
        add(effect.from_id || decision.id, effect.to_id || effect.id, effect);
      }
    });
  });
  return edges;
}

function readDecisionGraphForDate(date) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date || '')) {
    return { ok: false, error: 'invalid date' };
  }
  const dir = path.join(BRIEFINGS_DIR, date);
  const decisionsPath = path.join(dir, 'decisions.jsonl');
  const edgesPath = path.join(dir, 'tree', 'L4-edges.jsonl');
  const domainsPath = path.join(dir, 'tree', 'L4-domains.jsonl');
  const profilePath = path.join(dir, 'synthesis-profile.snapshot.json');
  try {
    const decisions = readJsonlFileIfPresent(decisionsPath);
    if (!decisions.exists) {
      return { ok: false, date, error: 'No decision artifacts found for this day.' };
    }
    const edgeRows = readJsonlFileIfPresent(edgesPath);
    const domainRows = readJsonlFileIfPresent(domainsPath);
    const profileSnapshot = readJsonFileIfPresent(profilePath);
    const fallbackEdges = edgeRows.exists ? [] : fallbackDecisionGraphEdges(decisions.items);
    const edges = edgeRows.exists ? edgeRows.items : fallbackEdges;
    const domains = decisionGraphDomains(profileSnapshot, domainRows.items, decisions.items);
    return {
      ok: true,
      date,
      paths: {
        decisions: decisionsPath,
        edges: edgeRows.exists ? edgesPath : null,
        domains: domainRows.exists ? domainsPath : null,
        profile: profileSnapshot ? profilePath : null,
      },
      decisions: decisions.items,
      edges,
      domains,
      derived_edges: fallbackEdges.length,
      summary: {
        decision_count: decisions.items.length,
        edge_count: edges.length,
        domain_count: domains.length,
      },
    };
  } catch (e) {
    return { ok: false, date, error: e.message };
  }
}

function openTodayBriefing() {
  return openBriefingForDate(todayStamp());
}

function trayIcon() {
  // Idle icon: rendered as a template image so macOS strips the source
  // colour and tints to black (light menu bar) or white (dark menu bar)
  // to match the rest of the bar. Resized to 22×22 logical.
  const diskIcon = path.join(__dirname, 'assets', 'tray-icon.png');
  if (fs.existsSync(diskIcon)) {
    const img = nativeImage.createFromPath(diskIcon).resize({ width: 22, height: 22 });
    if (!img.isEmpty()) {
      img.setTemplateImage(true);
      return img;
    }
  }
  // Last-resort placeholder so startup never fails on a missing asset.
  const placeholder = Buffer.from(
    'iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAQAAAC1+jfqAAAAL0lEQVR42mNkIAAYiVLCwMDw'
      + 'BzOBEdcwDO4ECmAEd4LFYCrATGsQgzAOXg0AAFc8Aew8p+a7AAAAAElFTkSuQmCC',
    'base64'
  );
  const img = nativeImage.createFromBuffer(placeholder);
  img.setTemplateImage(true);
  return img;
}

// Active icon: white-logo variant with a green recording dot composited
// in. Template mode strips colour, so we ship a non-template asset and
// the menu bar's natural dark substrate keeps the white legible. Falls
// back to the idle template icon if the active asset is missing on disk.
function trayIconActive() {
  const diskIcon = path.join(__dirname, 'assets', 'tray-icon-active.png');
  if (fs.existsSync(diskIcon)) {
    const img = nativeImage.createFromPath(diskIcon).resize({ width: 22, height: 22 });
    if (!img.isEmpty()) {
      // Explicitly NOT a template image — the green must survive untinted.
      img.setTemplateImage(false);
      return img;
    }
  }
  return trayIcon();
}

// Apply the right icon for the current capture state. Called on every
// state transition (start/stop/restart) and on system theme changes.
function applyTrayIcon() {
  if (!tray) return;
  tray.setImage(captureProc ? trayIconActive() : trayIcon());
}

// Right-click fallback context menu — minimal "nuclear" options so the
// user is never trapped if the popover renderer breaks. Left-click goes
// to the popover; right-click gets just Quit + log access.
function rightClickMenu() {
  return Menu.buildFromTemplate([
    { label: 'Alvum', enabled: false },
    {
      label: captureProc
        ? `● running since ${captureStartedAt.toLocaleTimeString()}`
        : '○ stopped',
      enabled: false,
    },
    { type: 'separator' },
    { label: 'Open shell log', click: () => shell.openPath(SHELL_LOG) },
    { label: 'Open briefing log', click: () => shell.openPath(BRIEFING_LOG) },
    { type: 'separator' },
    { label: 'Quit alvum', click: () => app.quit() },
  ]);
}

// Refresh the tooltip + icon. The full UI lives in the popover; the
// tray itself only carries glanceable state via icon + tooltip.
function rebuildTrayMenu() {
  const status = captureProc
    ? `● Capture running (started ${captureStartedAt.toLocaleTimeString()})`
    : '○ Capture stopped';
  tray.setToolTip(status);
  applyTrayIcon();
  broadcastState();
}

// === Popover BrowserWindow ============================================
//
// Standard menu-bar-app pattern: a frameless transparent BrowserWindow
// positioned next to the tray icon, shown on click and dismissed on
// blur. Replaces the previous ContextMenu so we can render real UI
// (progress bar, stage list, vibrancy) instead of NSMenuItem-only text.

let popover = null;
const POPOVER_W = 320;
const POPOVER_MIN_H = 300;
const POPOVER_MAX_H = 640;
let popoverHeight = POPOVER_MIN_H;
let popoverResizeTarget = POPOVER_MIN_H;
let popoverResizeTimer = null;

function createPopover() {
  popover = new BrowserWindow({
    width: POPOVER_W,
    height: popoverHeight,
    show: false,
    frame: false,
    transparent: true,
    resizable: false,
    movable: false,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    skipTaskbar: true,
    alwaysOnTop: true,
    hasShadow: true,
    vibrancy: 'menu',                 // native macOS popover translucency
    visualEffectState: 'active',
    webPreferences: {
      preload: path.join(__dirname, 'popover-preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  });
  popover.loadFile(path.join(__dirname, 'popover.html'));
  popover.on('blur', () => {
    if (popover && !popover.isDestroyed() && !popover.webContents.isDevToolsOpened()) popover.hide();
  });
  // Hide instead of close on the window's own X equivalent so we keep
  // a single instance for the lifetime of the app.
  popover.on('close', (e) => {
    if (!app.isQuitting) {
      e.preventDefault();
      popover.hide();
    }
  });
}

function popoverWorkArea() {
  const trayBounds = tray.getBounds();
  const display = screen.getDisplayNearestPoint({
    x: Math.round(trayBounds.x + trayBounds.width / 2),
    y: trayBounds.y,
  });
  return { trayBounds, work: display.workArea };
}

function clampPopoverHeight(height) {
  const { work } = popoverWorkArea();
  const screenMax = Math.max(POPOVER_MIN_H, work.height - 12);
  const requested = Number.isFinite(height) ? Math.ceil(height) : POPOVER_MIN_H;
  return Math.max(POPOVER_MIN_H, Math.min(requested, POPOVER_MAX_H, screenMax));
}

function positionPopover() {
  if (!popover || popover.isDestroyed() || !tray) return;
  const { trayBounds, work } = popoverWorkArea();
  let x = Math.round(trayBounds.x + trayBounds.width / 2 - POPOVER_W / 2);
  let y = Math.round(trayBounds.y + trayBounds.height + 6);
  x = Math.max(work.x + 6, Math.min(x, work.x + work.width - POPOVER_W - 6));
  y = Math.max(work.y + 6, Math.min(y, work.y + work.height - popoverHeight - 6));
  popover.setPosition(x, y, false);
}

function stopPopoverResizeAnimation() {
  if (!popoverResizeTimer) return;
  clearInterval(popoverResizeTimer);
  popoverResizeTimer = null;
}

function applyPopoverHeight(height) {
  if (!popover || popover.isDestroyed()) return;
  popoverHeight = height;
  popover.setSize(POPOVER_W, popoverHeight, false);
  if (popover.isVisible()) positionPopover();
}

function resizePopover(height) {
  if (!popover || popover.isDestroyed() || !tray) return;
  const nextHeight = clampPopoverHeight(height);
  if (popoverResizeTimer && nextHeight === popoverResizeTarget) return;
  popoverResizeTarget = nextHeight;
  if (nextHeight === popoverHeight) {
    stopPopoverResizeAnimation();
    if (popover.isVisible()) positionPopover();
    return;
  }
  if (!popover.isVisible()) {
    stopPopoverResizeAnimation();
    applyPopoverHeight(nextHeight);
    return;
  }
  stopPopoverResizeAnimation();
  const startHeight = popoverHeight;
  const delta = nextHeight - startHeight;
  const startedAt = Date.now();
  const durationMs = 180;
  popoverResizeTimer = setInterval(() => {
    if (!popover || popover.isDestroyed()) {
      stopPopoverResizeAnimation();
      return;
    }
    const t = Math.min(1, (Date.now() - startedAt) / durationMs);
    const eased = 1 - Math.pow(1 - t, 3);
    applyPopoverHeight(Math.round(startHeight + delta * eased));
    if (t >= 1) stopPopoverResizeAnimation();
  }, 16);
}

function togglePopover() {
  if (!popover || popover.isDestroyed()) return;
  if (popover.isVisible()) {
    popover.hide();
    return;
  }
  positionPopover();
  popover.show();
  popover.focus();
  // Tell the renderer to refresh anything that depends on the
  // outside-world state (provider availability, capture stats, etc.).
  popover.webContents.send('alvum:popover-show');
}

function captureStats() {
  // Cheap on-demand counts so the popover always shows fresh numbers
  // without a long-running watcher. Failures degrade to empty stats.
  try {
    return artifactSummaryForDate(todayStamp());
  } catch {
    return { date: todayStamp(), files: 0, bytes: 0, summary: '0 files · 0 B', detail: 'Capture stats unavailable' };
  }
}

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
    else if (/^-?\d+$/.test(value)) value = Number(value);
    else value = value.replace(/^"(.*)"$/, '$1');
    sections[current][kv[1]] = value;
  }
  return sections;
}

function loadConfigSections() {
  try {
    const text = fs.existsSync(CONFIG_FILE)
      ? fs.readFileSync(CONFIG_FILE, 'utf8')
      : '';
    return parseFlatTomlSections(text);
  } catch {
    return {};
  }
}

function sectionEnabled(sections, name, fallback = true) {
  return sections[name] && typeof sections[name].enabled === 'boolean'
    ? sections[name].enabled
    : fallback;
}

function settingsFor(sections, names) {
  const settings = {};
  for (const name of names) {
    const section = sections[name] || {};
    for (const [key, value] of Object.entries(section)) {
      if (key !== 'enabled') settings[key] = value;
    }
  }
  return settings;
}

function captureInputsSummary() {
  const sections = loadConfigSections();
  const permissions = capturePermissionStatus();
  const inputs = [
    {
      id: 'audio-mic',
      label: 'Microphone',
      kind: 'capture',
      enabled: sectionEnabled(sections, 'capture.audio-mic') && sectionEnabled(sections, 'connectors.audio'),
      detail: 'Local audio capture',
      settings: settingsFor(sections, ['capture.audio-mic']),
    },
    {
      id: 'audio-system',
      label: 'System audio',
      kind: 'capture',
      enabled: sectionEnabled(sections, 'capture.audio-system') && sectionEnabled(sections, 'connectors.audio'),
      detail: 'App and system output',
      settings: settingsFor(sections, ['capture.audio-system']),
    },
    {
      id: 'screen',
      label: 'Screen',
      kind: 'capture',
      enabled: sectionEnabled(sections, 'capture.screen') && sectionEnabled(sections, 'connectors.screen'),
      detail: 'Screenshots and OCR',
      settings: settingsFor(sections, ['capture.screen']),
    },
    {
      id: 'claude-code',
      label: 'Claude sessions',
      kind: 'session',
      enabled: sectionEnabled(sections, 'connectors.claude-code'),
      detail: 'Claude Code transcript history',
      settings: settingsFor(sections, ['connectors.claude-code']),
    },
    {
      id: 'codex',
      label: 'Codex sessions',
      kind: 'session',
      enabled: sectionEnabled(sections, 'connectors.codex'),
      detail: 'Codex transcript history',
      settings: settingsFor(sections, ['connectors.codex']),
    },
  ];
  const annotatedInputs = inputs.map((input) => {
    const blocked_permissions = input.enabled
      ? blockedPermissionsForSource(input.id, permissions)
      : [];
    return {
      ...input,
      required_permissions: sourcePermissionRequirements(input.id),
      blocked_permissions,
      blocked: blocked_permissions.length > 0,
    };
  });
  return {
    running: !!captureProc,
    enabled: annotatedInputs.filter((input) => input.enabled).length,
    total: annotatedInputs.length,
    permissions,
    inputs: annotatedInputs,
  };
}

function runAlvumText(args, timeoutMs = 5000) {
  return new Promise((resolve) => {
    const bin = resolveBinary();
    if (!bin) return resolve({ ok: false, error: 'alvum binary not found' });
    const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'], env: alvumSpawnEnv() });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => { stdout += d.toString(); });
    child.stderr.on('data', (d) => { stderr += d.toString(); });
    const timer = setTimeout(() => child.kill('SIGTERM'), timeoutMs);
    child.on('close', (code, signal) => {
      clearTimeout(timer);
      resolve({ ok: code === 0, code, signal, stdout, stderr, error: code === 0 ? null : (stderr || stdout || `code ${code}`) });
    });
    child.on('error', (e) => {
      clearTimeout(timer);
      resolve({ ok: false, error: e.message });
    });
  });
}

async function setConfigValue(key, value) {
  return runAlvumText(['config-set', key, String(value)], 5000);
}

function captureInputConfigSection(id) {
  if (id === 'audio-mic' || id === 'audio-system') return `capture.${id}`;
  if (id === 'screen') return 'capture.screen';
  if (id === 'claude-code' || id === 'codex') return `connectors.${id}`;
  return null;
}

async function chooseDirectory(defaultPath) {
  const options = {
    title: 'Choose folder',
    properties: ['openDirectory', 'createDirectory'],
  };
  if (defaultPath && typeof defaultPath === 'string') options.defaultPath = defaultPath;
  const result = popover && !popover.isDestroyed()
    ? await dialog.showOpenDialog(popover, options)
    : await dialog.showOpenDialog(options);
  if (result.canceled || !result.filePaths || !result.filePaths.length) {
    return { ok: false, canceled: true };
  }
  return { ok: true, path: result.filePaths[0] };
}

async function setCaptureInputSetting(id, key, value) {
  const section = captureInputConfigSection(id);
  if (!section) return { ok: false, error: 'unknown input' };
  if (!/^[A-Za-z0-9_-]+$/.test(String(key || ''))) {
    return { ok: false, error: 'invalid setting key' };
  }
  const result = await runAlvumText(['config-set', `${section}.${key}`, String(value)], 5000);
  if (!result.ok) return { ok: false, input: id, key, error: result.error, stdout: result.stdout };
  if (['audio-mic', 'audio-system', 'screen'].includes(id) && captureProc) restartCapture();
  const captureInputs = captureInputsSummary();
  broadcastState();
  return { ok: true, input: id, key, value, captureInputs };
}

function processorConfigSection(component) {
  if (component === 'alvum.audio/whisper') return 'processors.audio';
  if (component === 'alvum.screen/vision') return 'processors.screen';
  return null;
}

async function setConnectorProcessorSetting(component, key, value) {
  const section = processorConfigSection(component);
  if (!section) return { ok: false, error: 'unknown processor' };
  if (!/^[A-Za-z0-9_-]+$/.test(String(key || ''))) {
    return { ok: false, error: 'invalid setting key' };
  }
  const result = await runAlvumText(['config-set', `${section}.${key}`, String(value)], 5000);
  const list = await connectorList();
  broadcastState();
  return {
    ok: result.ok,
    component,
    key,
    value,
    error: result.error,
    stdout: result.stdout,
    connectors: list.connectors,
  };
}

async function toggleCaptureInput(id) {
  const current = captureInputsSummary().inputs.find((input) => input.id === id);
  if (!current) return { ok: false, error: 'unknown input' };
  const next = !current.enabled;
  const changes = [];
  if (id === 'audio-mic' || id === 'audio-system') {
    changes.push(['capture.' + id + '.enabled', next]);
    if (next) {
      changes.push(['connectors.audio.enabled', true]);
    } else {
      const other = id === 'audio-mic' ? 'audio-system' : 'audio-mic';
      const otherEnabled = captureInputsSummary().inputs.find((input) => input.id === other)?.enabled;
      if (!otherEnabled) changes.push(['connectors.audio.enabled', false]);
    }
  } else if (id === 'screen') {
    changes.push(['capture.screen.enabled', next], ['connectors.screen.enabled', next]);
  } else if (id === 'claude-code' || id === 'codex') {
    changes.push(['connectors.' + id + '.enabled', next]);
  }
  for (const [key, value] of changes) {
    const result = await setConfigValue(key, value);
    if (!result.ok) return result;
  }
  if (next && ['audio-mic', 'audio-system', 'screen'].includes(id)) {
    await promptForSourcePermissions([id], true);
  }
  if (['audio-mic', 'audio-system', 'screen'].includes(id) && captureProc) restartCapture();
  const captureInputs = captureInputsSummary();
  const permission_issues = captureInputs.inputs
    .filter((input) => input.id === id)
    .flatMap((input) => (input.blocked_permissions || []).map((issue) => ({
      ...issue,
      source_id: input.id,
      source_label: input.label,
    })));
  notifyPermissionIssues(permission_issues);
  broadcastState();
  return { ok: true, input: id, enabled: next, captureInputs, permission_issues };
}

function broadcastState() {
  if (!popover || popover.isDestroyed()) return;
  const catchup = pendingBriefingCatchup();
  const latestBriefing = latestBriefingInfo();
  const capture = captureStats();
  popover.webContents.send('alvum:state', {
    captureRunning: !!captureProc,
    captureStartedAt: captureStartedAt ? captureStartedAt.toLocaleTimeString() : null,
    briefingRunning: briefingRuns.size > 0,
    briefingRuns: briefingRunSnapshot(),
    briefingStartedAt: null,
    briefingTargetDate: null,
    briefingCatchupPending: catchup.count,
    briefingCatchupDates: catchup.dates,
    captureStats: capture,
    captureInputs: captureInputsSummary(),
    permissions: capturePermissionStatus(),
    stats: capture.summary,
    latestBriefing,
    briefingTargets: recentBriefingTargets(),
    briefingCalendar: briefingCalendarMonth(),
    providerSummary: providerProbeCache,
    providerIssue: currentProviderIssue,
  });
}

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
  for (const run of briefingRuns.values()) pollBriefingRunProgress(run);

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
  if (!lines.length || !popover) return;
  const last = lines[lines.length - 1];
  try {
    const evt = JSON.parse(last);
    appendShellLog(`[progress] → ${evt.stage} ${evt.current}/${evt.total}`);
    popover.webContents.send('alvum:progress', evt);
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
  if (!lines.length || !popover) return;
  const last = lines[lines.length - 1];
  try {
    const evt = { ...JSON.parse(last), briefingDate: run.date };
    run.progress = evt;
    appendShellLog(`[progress:${run.date}] → ${evt.stage} ${evt.current}/${evt.total}`);
    popover.webContents.send('alvum:progress', evt);
  } catch (e) {
    appendShellLog(`[progress:${run.date}] bad JSON: ${e.message} line=${last}`);
  }
}

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
  for (const run of briefingRuns.values()) pollBriefingRunEvents(run);

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

  if (!popover) return;
  const lines = chunk.split('\n').filter((l) => l.trim());
  for (const line of lines) {
    let evt;
    try {
      evt = JSON.parse(line);
    } catch (e) {
      appendShellLog(`[events] bad JSON: ${e.message} line=${line}`);
      continue;
    }
    popover.webContents.send('alvum:event', evt);
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

  if (!popover) return;
  const lines = chunk.split('\n').filter((l) => l.trim());
  for (const line of lines) {
    try {
      popover.webContents.send('alvum:event', { ...JSON.parse(line), briefingDate: run.date });
    } catch (e) {
      appendShellLog(`[events:${run.date}] bad JSON: ${e.message} line=${line}`);
    }
  }
}

// === Provider helpers ==================================================
//
// Spawn `alvum providers list/test/set-active` and return the parsed
// JSON to the popover renderer. Each call is one-shot (no streaming
// output), so a simple promise wrapper around child_process is fine.

function runAlvumJson(args, timeoutMs) {
  return new Promise((resolve) => {
    const bin = resolveBinary();
    if (!bin) return resolve({ error: 'alvum binary not found' });
    const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'], env: alvumSpawnEnv() });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => { stdout += d.toString(); });
    child.stderr.on('data', (d) => { stderr += d.toString(); });
    const timer = setTimeout(() => child.kill('SIGTERM'), timeoutMs);
    child.on('close', () => {
      clearTimeout(timer);
      try {
        resolve(JSON.parse(stdout));
      } catch (e) {
        resolve({ error: `parse failed: ${e.message}`, stdout, stderr });
      }
    });
    child.on('error', (e) => {
      clearTimeout(timer);
      resolve({ error: e.message });
    });
  });
}

let providerProbeCache = null;
let providerProbeCacheAt = 0;
let providerProbeCacheLive = false;
let providerWatchTimer = null;
let lastProviderIssueKey = '';
let currentProviderIssue = null;
const PROVIDER_PROBE_TTL_MS = 2 * 60 * 1000;
const PROVIDER_WATCH_MS = 3 * 60 * 1000;

function providerUiStatus(entry, test) {
  if (entry.enabled === false) {
    return { level: 'yellow', status: 'removed', reason: 'removed from Alvum provider list' };
  }
  if (!entry.available) {
    return { level: 'red', status: 'not_setup', reason: entry.auth_hint || 'not configured' };
  }
  if (!test) {
    return { level: 'yellow', status: 'not_checked', reason: 'not checked yet' };
  }
  if (test.ok) {
    return { level: 'green', status: test.status || 'available', reason: 'authenticated and returning tokens' };
  }
  return {
    level: 'yellow',
    status: test.status || 'unavailable',
    reason: test.error || 'probe failed',
  };
}

function providerSelectableForAuto(provider) {
  if (!provider || provider.enabled === false || !provider.available) return false;
  return provider.test ? provider.test.ok : provider.available;
}

function autoProviderName(providers) {
  const match = (providers || []).find(providerSelectableForAuto);
  return match ? match.name : null;
}

function applyProviderAutoSelection(summary) {
  if (!summary || summary.error || !Array.isArray(summary.providers)) return summary;
  if ((summary.configured || 'auto') !== 'auto') return summary;
  const autoResolved = autoProviderName(summary.providers);
  return {
    ...summary,
    auto_resolved: autoResolved,
    providers: summary.providers.map((provider) => ({
      ...provider,
      active: provider.name === autoResolved,
    })),
  };
}

async function providerProbeSummary(force = false, liveProbe = true) {
  const now = Date.now();
  if (!force
      && providerProbeCache
      && now - providerProbeCacheAt < PROVIDER_PROBE_TTL_MS
      && (!liveProbe || providerProbeCacheLive)) {
    return providerProbeCache;
  }
  const previousSummary = providerProbeCache && !providerProbeCache.error ? providerProbeCache : null;
  const previousByName = new Map(
    previousSummary && Array.isArray(previousSummary.providers)
      ? previousSummary.providers.map((provider) => [provider.name, provider])
      : [],
  );
  const data = await runAlvumJson(['providers', 'list'], 5000);
  if (!data || data.error || !Array.isArray(data.providers)) {
    const errorSummary = {
      error: (data && data.error) || 'provider list failed',
      connected: 0,
      total: 0,
      live_checked: false,
      checked_at: new Date().toISOString(),
      providers: [],
    };
    providerProbeCache = errorSummary;
    providerProbeCacheAt = Date.now();
    providerProbeCacheLive = false;
    return errorSummary;
  }
  const providers = await Promise.all(data.providers.map(async (entry) => {
    const previous = previousByName.get(entry.name);
    let test = null;
    if (liveProbe && entry.enabled !== false && entry.available) {
      test = await runAlvumJson(['providers', 'test', '--provider', entry.name], 30000);
    } else if (previous && previous.test && entry.enabled !== false && entry.available) {
      test = previous.test;
    }
    return {
      ...entry,
      test,
      usage: previous ? previous.usage : null,
      ui: providerUiStatus(entry, test),
    };
  }));
  const result = applyProviderAutoSelection({
    configured: data.configured,
    auto_resolved: data.auto_resolved,
    connected: providers.filter((p) => p.ui.level === 'green').length,
    total: providers.length,
    live_checked: !!liveProbe || !!(previousSummary && previousSummary.live_checked),
    checked_at: new Date().toISOString(),
    providers,
  });
  providerProbeCache = result;
  providerProbeCacheAt = Date.now();
  providerProbeCacheLive = providerProbeCacheLive || !!liveProbe;
  return result;
}

function providerIssues(summary) {
  if (!summary || summary.error) {
    return [{ level: 'warning', message: summary && summary.error ? summary.error : 'Provider check failed.' }];
  }
  const providers = Array.isArray(summary.providers) ? summary.providers : [];
  const enabled = providers.filter((provider) => provider.enabled !== false);
  const configured = summary.configured || 'auto';
  const usable = enabled.filter(providerSelectableForAuto);

  if (enabled.length === 0) {
    return [{ level: 'warning', message: 'No providers are enabled in Alvum.' }];
  }
  if (usable.length === 0) {
    return [{ level: 'warning', message: 'No enabled provider is currently usable.' }];
  }
  if (configured === 'auto') return [];

  const active = providers.find((provider) => provider.name === configured);
  if (!active) {
    return [{ level: 'warning', message: `Configured provider ${configured} is not recognized.` }];
  }
  if (active.enabled === false) {
    return [{ level: 'warning', message: `Configured provider ${configured} is removed from Alvum's provider list.` }];
  }
  if (!active.available) {
    return [{ level: 'warning', message: `Configured provider ${configured} is not detected; ${active.auth_hint || 'check setup'}.` }];
  }
  if (active.test && !active.test.ok) {
    return [{ level: 'warning', message: `Configured provider ${configured} failed its live check: ${active.test.status || 'unavailable'}.` }];
  }
  return [];
}

function notifyProviderIssues(issues) {
  const activeIssues = (issues || []).filter(Boolean);
  const nextIssue = activeIssues[0] || null;
  currentProviderIssue = nextIssue;
  const key = nextIssue ? `${nextIssue.level}:${nextIssue.message}` : '';
  if (!nextIssue) {
    lastProviderIssueKey = '';
    broadcastState();
    return;
  }
  if (key !== lastProviderIssueKey) {
    lastProviderIssueKey = key;
    appendShellLog(`[providers] ${nextIssue.message}`);
    notify('Alvum provider issue', nextIssue.message);
  }
  broadcastState();
}

async function refreshProviderWatch(liveProbe = false) {
  const summary = liveProbe
    ? await providerProbeSummary(true, true)
    : await providerProbeSummary(true, false);
  notifyProviderIssues(providerIssues(summary));
  return summary;
}

function startProviderWatcher() {
  if (providerWatchTimer) return;
  refreshProviderWatch(true);
  providerWatchTimer = setInterval(() => refreshProviderWatch(!!currentProviderIssue), PROVIDER_WATCH_MS);
}

async function providerTest(name) {
  const result = await runAlvumJson(['providers', 'test', '--provider', name], 30000);
  const summary = await providerProbeSummary(false, false);
  if (!summary || summary.error || !Array.isArray(summary.providers)) return result;
  const providers = summary.providers.map((provider) => {
    if (provider.name !== name) return provider;
    return {
      ...provider,
      test: result,
      ui: providerUiStatus(provider, result),
    };
  });
  const nextSummary = applyProviderAutoSelection({
    ...summary,
    connected: providers.filter((p) => p.ui.level === 'green').length,
    providers,
  });
  providerProbeCache = nextSummary;
  providerProbeCacheAt = Date.now();
  notifyProviderIssues(providerIssues(nextSummary));
  return { ...result, summary: nextSummary };
}

async function providerSetActive(name) {
  const result = await runAlvumJson(['providers', 'set-active', name], 5000);
  const summary = await refreshProviderWatch(false);
  setTimeout(() => refreshProviderWatch(true), 0);
  return { ...result, summary };
}

async function providerSetEnabled(name, enabled) {
  const result = await runAlvumJson(['providers', enabled ? 'enable' : 'disable', name], 5000);
  const summary = await refreshProviderWatch(false);
  setTimeout(() => refreshProviderWatch(true), 0);
  return { ...result, summary };
}

function providerByNameFromSummary(name) {
  const providers = providerProbeCache && Array.isArray(providerProbeCache.providers)
    ? providerProbeCache.providers
    : [];
  return providers.find((provider) => provider.name === name) || null;
}

function escapeAppleScriptString(value) {
  return String(value || '').replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

function openTerminalCommand(command) {
  return new Promise((resolve) => {
    const script = [
      'tell application "Terminal"',
      'activate',
      `do script "${escapeAppleScriptString(command)}"`,
      'end tell',
    ].join('\n');
    const child = spawn('/usr/bin/osascript', ['-e', script], {
      stdio: ['ignore', 'pipe', 'pipe'],
      env: alvumSpawnEnv(),
    });
    let stderr = '';
    child.stderr.on('data', (d) => { stderr += d.toString(); });
    child.on('close', (code) => {
      resolve({
        ok: code === 0,
        action: 'terminal',
        error: code === 0 ? null : (stderr || `osascript exited ${code}`),
      });
    });
    child.on('error', (e) => resolve({ ok: false, action: 'terminal', error: e.message }));
  });
}

async function providerSetup(name) {
  let provider = providerByNameFromSummary(name);
  if (!provider) {
    const summary = await providerProbeSummary(true, false);
    provider = Array.isArray(summary.providers)
      ? summary.providers.find((entry) => entry.name === name)
      : null;
  }
  if (!provider) return { ok: false, provider: name, error: 'unknown provider' };
  if (provider.setup_kind === 'terminal' && provider.setup_command) {
    return { provider: name, ...(await openTerminalCommand(provider.setup_command)) };
  }
  if (provider.setup_kind === 'url' && provider.setup_url) {
    try {
      await shell.openExternal(provider.setup_url);
      return { ok: true, provider: name, action: 'url', url: provider.setup_url };
    } catch (e) {
      return { ok: false, provider: name, action: 'url', url: provider.setup_url, error: e.message };
    }
  }
  return {
    ok: false,
    provider: name,
    action: 'instructions',
    error: provider.setup_hint || provider.auth_hint || 'No setup action is available.',
  };
}

function readTail(file, maxBytes = 80 * 1024) {
  try {
    if (!fs.existsSync(file)) return '';
    const stat = fs.statSync(file);
    const start = Math.max(0, stat.size - maxBytes);
    const fd = fs.openSync(file, 'r');
    const buf = Buffer.alloc(stat.size - start);
    fs.readSync(fd, buf, 0, buf.length, start);
    fs.closeSync(fd);
    return buf.toString('utf8');
  } catch (e) {
    return `Could not read log: ${e.message}`;
  }
}

function logSnapshot(kind) {
  const files = {
    shell: SHELL_LOG,
    briefing: BRIEFING_LOG,
    pipeline: EVENTS_FILE,
  };
  const file = files[kind] || SHELL_LOG;
  return { kind, file, text: readTail(file) };
}

async function openExtensionsDir() {
  try {
    ensureExtensionsDir();
    const error = await shell.openPath(EXTENSIONS_DIR);
    if (error) return { ok: false, path: EXTENSIONS_DIR, error };
    return { ok: true, path: EXTENSIONS_DIR };
  } catch (e) {
    return { ok: false, path: EXTENSIONS_DIR, error: e.message };
  }
}

async function extensionList() {
  const data = await runAlvumJson(['extensions', 'list', '--json'], 5000);
  if (!data || data.error) return { extensions: [], error: data && data.error ? data.error : 'extension list failed' };
  return {
    extensions: Array.isArray(data.extensions) ? data.extensions : [],
    core: Array.isArray(data.core) ? data.core : [],
  };
}

async function extensionSetEnabled(id, enabled) {
  const result = await runAlvumText(['extensions', enabled ? 'enable' : 'disable', id], 30000);
  const list = await extensionList();
  if (result.ok && captureProc) restartCapture();
  broadcastState();
  return { ok: result.ok, id, enabled, error: result.error, stdout: result.stdout, extensions: list.extensions, core: list.core };
}

async function extensionDoctor() {
  const data = await runAlvumJson(['extensions', 'doctor', '--json'], 30000);
  if (!data || data.error) return { extensions: [], error: data && data.error ? data.error : 'extension doctor failed' };
  return {
    extensions: Array.isArray(data.extensions) ? data.extensions : [],
  };
}

async function connectorList() {
  const data = await runAlvumJson(['connectors', 'list', '--json'], 5000);
  if (!data || data.error) return { connectors: [], error: data && data.error ? data.error : 'connector list failed' };
  return {
    connectors: annotateConnectorPermissions(Array.isArray(data.connectors) ? data.connectors : []),
  };
}

function annotateConnectorPermissions(connectors, permissions = capturePermissionStatus()) {
  return connectors.map((connector) => {
    const source_controls = Array.isArray(connector.source_controls)
      ? connector.source_controls.map((control) => {
          const blocked_permissions = control.enabled
            ? blockedPermissionsForSource(control.id, permissions)
            : [];
          return {
            ...control,
            required_permissions: sourcePermissionRequirements(control.id),
            blocked_permissions,
            blocked: blocked_permissions.length > 0,
          };
        })
      : [];
    const permission_issues = source_controls.flatMap((control) =>
      (control.blocked_permissions || []).map((issue) => ({
        ...issue,
        connector_id: connector.id,
        connector_label: connector.display_name || connector.id,
        source_id: control.id,
        source_label: control.label || control.id,
      })));
    return {
      ...connector,
      source_controls,
      permission_issues,
      blocked: permission_issues.length > 0,
    };
  });
}

async function connectorSetEnabled(id, enabled) {
  const result = await runAlvumText(['connectors', enabled ? 'enable' : 'disable', id], 30000);
  let list = await connectorList();
  let permission_issues = [];
  if (result.ok && enabled) {
    const record = list.connectors.find((connector) => connector.id === id || connector.component_id === id);
    const sourceIds = record && Array.isArray(record.source_controls)
      ? record.source_controls.filter((control) => control.enabled).map((control) => control.id)
      : [];
    if (sourceIds.length) await promptForSourcePermissions(sourceIds, true);
    list = await connectorList();
    const updated = list.connectors.find((connector) => connector.id === id || connector.component_id === id);
    permission_issues = updated && Array.isArray(updated.permission_issues)
      ? updated.permission_issues
      : [];
    notifyPermissionIssues(permission_issues);
  }
  if (result.ok && captureProc) restartCapture();
  const captureInputs = captureInputsSummary();
  broadcastState();
  return { ok: result.ok, id, enabled, error: result.error, stdout: result.stdout, connectors: list.connectors, captureInputs, permission_issues };
}

async function globalDoctor() {
  const data = await runAlvumJson(['doctor', '--json'], 30000);
  const permissionIssues = enabledPermissionIssues();
  const permissionChecks = permissionIssues.map((issue) => ({
    id: `permission.${issue.permission}.${issue.source_id || 'source'}`,
    label: issue.label || issue.permission,
    level: 'warning',
    message: `${issue.source_label || 'Enabled connector'} is enabled but ${issue.label || issue.permission} is ${issue.status}.`,
  }));
  const providerSummary = await providerProbeSummary(true, true);
  const providerChecks = providerIssues(providerSummary).map((issue, index) => ({
    id: `providers.runtime.${index}`,
    label: 'Providers',
    level: issue.level || 'warning',
    message: issue.message,
  }));
  if (!data || data.error) {
    return {
      ok: false,
      error_count: 1,
      warning_count: permissionChecks.length + providerChecks.length,
      checks: permissionChecks.concat(providerChecks),
      error: data && data.error ? data.error : 'doctor failed',
    };
  }
  const checks = Array.isArray(data.checks) ? data.checks : [];
  return {
    ok: !!data.ok && permissionChecks.length === 0 && providerChecks.length === 0,
    error_count: Number(data.error_count) || 0,
    warning_count: (Number(data.warning_count) || 0) + permissionChecks.length + providerChecks.length,
    checks: checks.concat(permissionChecks, providerChecks),
  };
}

async function synthesisProfile() {
  const data = await runAlvumJson(['profile', 'show', '--json'], 5000);
  if (!data || data.error) return { ok: false, error: data && data.error ? data.error : 'profile load failed' };
  return { ok: true, profile: data };
}

async function synthesisProfileSave(profile) {
  const result = await runAlvumText(['profile', 'save', '--json', JSON.stringify(profile)], 10000);
  const updated = await synthesisProfile();
  const suggestions = await synthesisProfileSuggestions();
  return {
    ok: result.ok,
    error: result.error,
    stdout: result.stdout,
    profile: updated.profile,
    suggestions: suggestions.suggestions || [],
  };
}

async function synthesisProfileSuggestions() {
  const data = await runAlvumJson(['profile', 'suggestions', '--json'], 10000);
  if (!data || data.error) return { ok: false, suggestions: [], error: data && data.error ? data.error : 'profile suggestions failed' };
  return { ok: true, suggestions: Array.isArray(data.suggestions) ? data.suggestions : [], knowledge_dir: data.knowledge_dir };
}

async function synthesisProfilePromote(id) {
  const result = await runAlvumText(['profile', 'promote', id], 10000);
  const profile = await synthesisProfile();
  const suggestions = await synthesisProfileSuggestions();
  return {
    ok: result.ok,
    error: result.error,
    stdout: result.stdout,
    profile: profile.profile,
    suggestions: suggestions.suggestions || [],
  };
}

async function synthesisProfileIgnore(id) {
  const result = await runAlvumText(['profile', 'ignore', id], 10000);
  const suggestions = await synthesisProfileSuggestions();
  return {
    ok: result.ok,
    error: result.error,
    stdout: result.stdout,
    suggestions: suggestions.suggestions || [],
  };
}

// === IPC handlers from popover renderer ===============================
function bindIpc() {
  ipcMain.on('alvum:request-state',  () => broadcastState());
  ipcMain.on('alvum:resize-popover', (_e, height) => resizePopover(height));
  ipcMain.on('alvum:toggle-capture', () => (captureProc ? stopCapture() : startCapture()));
  ipcMain.handle('alvum:capture-inputs', () => captureInputsSummary());
  ipcMain.handle('alvum:toggle-capture-input', (_e, id) => toggleCaptureInput(id));
  ipcMain.handle('alvum:set-capture-input-setting', (_e, id, key, value) =>
    setCaptureInputSetting(id, key, value));
  ipcMain.handle('alvum:choose-directory', (_e, defaultPath) =>
    chooseDirectory(defaultPath));
  ipcMain.on('alvum:start-briefing', () => generateBriefing());
  ipcMain.handle('alvum:start-briefing-date', (_e, date) => generateBriefingForDate(date));
  ipcMain.handle('alvum:briefing-calendar-month', (_e, month) => briefingCalendarMonth(month));
  ipcMain.on('alvum:open-briefing',  () => openTodayBriefing());
  ipcMain.handle('alvum:open-briefing-date', (_e, date) => openBriefingForDate(date));
  ipcMain.handle('alvum:read-briefing-date', (_e, date) => readBriefingForDate(date));
  ipcMain.handle('alvum:decision-graph-date', (_e, date) => readDecisionGraphForDate(date));
  ipcMain.handle('alvum:synthesis-profile', () =>
    synthesisProfile());
  ipcMain.handle('alvum:synthesis-profile-save', (_e, profile) =>
    synthesisProfileSave(profile));
  ipcMain.handle('alvum:synthesis-profile-suggestions', () =>
    synthesisProfileSuggestions());
  ipcMain.handle('alvum:synthesis-profile-promote', (_e, id) =>
    synthesisProfilePromote(id));
  ipcMain.handle('alvum:synthesis-profile-ignore', (_e, id) =>
    synthesisProfileIgnore(id));
  ipcMain.on('alvum:open-briefing-log',  () => shell.openPath(BRIEFING_LOG));
  ipcMain.on('alvum:open-capture-dir',   () => shell.openPath(path.join(ALVUM_ROOT, 'capture')));
  ipcMain.handle('alvum:open-extensions-dir', () => openExtensionsDir());
  ipcMain.on('alvum:open-shell-log',     () => shell.openPath(SHELL_LOG));
  ipcMain.handle('alvum:open-permission-settings', (_e, permission) =>
    openPermissionSettings(permission));
  ipcMain.on('alvum:quit',           () => app.quit());

  // Provider status / test / set-active. The renderer drives these via
  // ipcRenderer.invoke (request/response), not .send, so we use handle()
  // and return the parsed CLI JSON synchronously to the renderer.
  ipcMain.handle('alvum:provider-list', () =>
    runAlvumJson(['providers', 'list'], 5000));
  ipcMain.handle('alvum:provider-test', (_e, name) =>
    providerTest(name));
  ipcMain.handle('alvum:provider-set-active', (_e, name) =>
    providerSetActive(name));
  ipcMain.handle('alvum:provider-set-enabled', (_e, name, enabled) =>
    providerSetEnabled(name, !!enabled));
  ipcMain.handle('alvum:provider-setup', (_e, name) =>
    providerSetup(name));
  ipcMain.handle('alvum:log-snapshot', (_e, kind) =>
    logSnapshot(kind));
  ipcMain.handle('alvum:extension-list', () =>
    extensionList());
  ipcMain.handle('alvum:extension-set-enabled', (_e, id, enabled) =>
    extensionSetEnabled(id, !!enabled));
  ipcMain.handle('alvum:extension-doctor', () =>
    extensionDoctor());
  ipcMain.handle('alvum:connector-list', () =>
    connectorList());
  ipcMain.handle('alvum:connector-set-enabled', (_e, id, enabled) =>
    connectorSetEnabled(id, !!enabled));
  ipcMain.handle('alvum:set-connector-processor-setting', (_e, component, key, value) =>
    setConnectorProcessorSetting(component, key, value));
  ipcMain.handle('alvum:doctor', () =>
    globalDoctor());
}

app.whenReady().then(() => {
  if (process.platform === 'darwin' && app.dock) app.dock.hide();

  ensureExtensionsDir();
  requestPermissions();

  tray = new Tray(trayIcon());

  // Tray button bindings:
  //   left-click → popover (rich UI)
  //   right-click → minimal context menu (Quit, log access)
  // popUpContextMenu handles its own dismissal; popover dismisses on blur.
  bindIpc();
  createPopover();
  tray.on('click', () => togglePopover());
  tray.on('right-click', () => tray.popUpContextMenu(rightClickMenu()));

  rebuildTrayMenu();
  startNotifyQueueWatcher();
  startPermissionWatcher();
  startProviderWatcher();
  startProgressWatcher();
  startEventsWatcher();

  startCapture();
  setTimeout(reportEnabledPermissionBlocks, 1500);
});

app.on('before-quit', () => { app.isQuitting = true; });

app.on('before-quit', () => {
  stopCapture();
});

app.on('window-all-closed', (e) => {
  // Background agent: keep running when no windows exist.
  e.preventDefault?.();
});
