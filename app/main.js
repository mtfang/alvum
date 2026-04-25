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

const { app, Tray, Menu, BrowserWindow, ipcMain, shell, screen, systemPreferences, Notification, nativeImage } = require('electron');
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
//   • Packaged builds put the binary at Contents/Resources/bin/alvum.
//   • Dev runs (`npm start` from the repo) use the Cargo target dir.
//   • A user-installed binary at ~/.alvum/runtime/Alvum.app/Contents/MacOS/alvum
//     is accepted as a final fallback for transitional installs.
function resolveBinary() {
  const packaged = path.join(process.resourcesPath || '', 'bin', 'alvum');
  const dev = path.resolve(__dirname, '..', 'target', 'release', 'alvum');
  const legacy = path.join(ALVUM_ROOT, 'runtime', 'Alvum.app', 'Contents', 'MacOS', 'alvum');
  for (const candidate of [packaged, dev, legacy]) {
    if (candidate && fs.existsSync(candidate)) return candidate;
  }
  return null;
}

let tray = null;
let captureProc = null;
let captureStartedAt = null;
let briefingProc = null;
let briefingStartedAt = null;

const BRIEFINGS_DIR = path.join(ALVUM_ROOT, 'generated', 'briefings');
const BRIEFING_LOG = path.join(LOG_DIR, 'briefing.out');
const BRIEFING_ERR = path.join(LOG_DIR, 'briefing.err');

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

async function requestPermissions() {
  // Microphone: Electron has a direct API that wraps AVCaptureDevice.requestAccess.
  // This triggers the native TCC dialog when status is `not-determined`.
  const micStatus = systemPreferences.getMediaAccessStatus('microphone');
  console.log('[permissions] microphone status:', micStatus);
  if (micStatus !== 'granted') {
    const ok = await systemPreferences.askForMediaAccess('microphone');
    console.log('[permissions] mic grant response:', ok);
  }

  // Screen Recording: no Electron wrapper for async request. Triggering
  // CGPreflight by reading `screen` media status is the standard idiom;
  // on `not-determined` macOS renders a dialog the next time a screen
  // API is hit. SCK will re-request from the child process regardless.
  const screenStatus = systemPreferences.getMediaAccessStatus('screen');
  console.log('[permissions] screen status:', screenStatus);
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
      env: {
        ...process.env,
        RUST_LOG: process.env.RUST_LOG || 'info',
      },
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

function generateBriefing() {
  if (briefingProc) {
    appendShellLog('[briefing] already running, ignoring request');
    return;
  }
  const script = resolveScript('briefing.sh');
  if (!script) {
    notify('Alvum', 'briefing.sh not found. Missing from bundle Resources/scripts?');
    return;
  }
  // Reset the progress cursor BEFORE spawning so we don't race the
  // pipeline's progress::init() (truncate) followed by the first
  // progress::report() (write back to ~original size) — both can
  // happen within one 500-ms poll, leaving stat.size == cursor and
  // pollProgress thinking nothing changed.
  progressCursor = 0;
  progressMtimeMs = 0;
  ensureLogDir();
  const out = fs.openSync(BRIEFING_LOG, 'a');
  const err = fs.openSync(BRIEFING_ERR, 'a');
  try {
    briefingProc = spawn('/bin/bash', [script], {
      cwd: ALVUM_ROOT,
      stdio: ['ignore', out, err],
      env: { ...process.env },
      detached: false,
    });
  } catch (e) {
    appendShellLog(`[briefing] spawn threw: ${e.stack || e}`);
    notify('Alvum', `Failed to start briefing: ${e.message}`);
    return;
  }
  briefingStartedAt = new Date();
  appendShellLog(`[briefing] spawned pid=${briefingProc.pid}`);
  notify('Alvum', 'Briefing started. You\'ll get another notification when it\'s ready.');
  rebuildTrayMenu();

  briefingProc.on('error', (e) => {
    appendShellLog(`[briefing] spawn error: ${e.stack || e}`);
  });
  briefingProc.on('exit', (code, signal) => {
    const durationMs = briefingStartedAt ? Date.now() - briefingStartedAt.getTime() : 0;
    appendShellLog(`[briefing] exited code=${code} signal=${signal} duration_ms=${durationMs}`);
    briefingProc = null;
    briefingStartedAt = null;
    if (code === 0) {
      notify('Alvum', `Briefing ready (${Math.round(durationMs / 1000)}s). Click tray → Open today's briefing.`);
    } else {
      notify('Alvum', `Briefing failed (code ${code}). See ${BRIEFING_ERR}.`);
    }
    rebuildTrayMenu();
  });
}

function openTodayBriefing() {
  const file = path.join(BRIEFINGS_DIR, todayStamp(), 'briefing.md');
  if (!fs.existsSync(file)) {
    notify('Alvum', `No briefing yet for ${todayStamp()}. Run "Generate briefing now" first.`);
    return;
  }
  shell.openPath(file);
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
const POPOVER_H = 300;

function createPopover() {
  popover = new BrowserWindow({
    width: POPOVER_W,
    height: POPOVER_H,
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
    if (!popover.webContents.isDevToolsOpened()) popover.hide();
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

function togglePopover() {
  if (!popover) return;
  if (popover.isVisible()) {
    popover.hide();
    return;
  }
  // Position centered horizontally below the tray icon, clamped inside
  // the work area so we never spill off-screen on a narrow display.
  const trayBounds = tray.getBounds();
  const display = screen.getDisplayNearestPoint({
    x: Math.round(trayBounds.x + trayBounds.width / 2),
    y: trayBounds.y,
  });
  const work = display.workArea;
  let x = Math.round(trayBounds.x + trayBounds.width / 2 - POPOVER_W / 2);
  let y = Math.round(trayBounds.y + trayBounds.height + 6);
  x = Math.max(work.x + 6, Math.min(x, work.x + work.width - POPOVER_W - 6));
  y = Math.max(work.y + 6, Math.min(y, work.y + work.height - POPOVER_H - 6));
  popover.setPosition(x, y, false);
  popover.show();
  popover.focus();
}

function captureStats() {
  // Cheap on-demand counts so the popover always shows fresh numbers
  // without a long-running watcher. Failures degrade to "—".
  try {
    const dir = path.join(ALVUM_ROOT, 'capture', todayStamp());
    const wav = (sub) =>
      fs.existsSync(path.join(dir, 'audio', sub))
        ? fs.readdirSync(path.join(dir, 'audio', sub)).filter((f) => f.endsWith('.wav')).length
        : 0;
    const png =
      fs.existsSync(path.join(dir, 'screen', 'images'))
        ? fs.readdirSync(path.join(dir, 'screen', 'images')).filter((f) => f.endsWith('.png')).length
        : 0;
    return `mic ${wav('mic')} · sys ${wav('system')} · screen ${png}`;
  } catch {
    return '—';
  }
}

function broadcastState() {
  if (!popover) return;
  popover.webContents.send('alvum:state', {
    captureRunning: !!captureProc,
    captureStartedAt: captureStartedAt ? captureStartedAt.toLocaleTimeString() : null,
    briefingRunning: !!briefingProc,
    stats: captureStats(),
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

// === Provider helpers ==================================================
//
// Spawn `alvum providers list/test/set-active` and return the parsed
// JSON to the popover renderer. Each call is one-shot (no streaming
// output), so a simple promise wrapper around child_process is fine.

function runAlvumJson(args, timeoutMs) {
  return new Promise((resolve) => {
    const bin = resolveBinary();
    if (!bin) return resolve({ error: 'alvum binary not found' });
    const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'] });
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

// === IPC handlers from popover renderer ===============================
function bindIpc() {
  ipcMain.on('alvum:request-state',  () => broadcastState());
  ipcMain.on('alvum:toggle-capture', () => (captureProc ? stopCapture() : startCapture()));
  ipcMain.on('alvum:start-briefing', () => generateBriefing());
  ipcMain.on('alvum:open-briefing',  () => openTodayBriefing());
  ipcMain.on('alvum:open-briefing-log',  () => shell.openPath(BRIEFING_LOG));
  ipcMain.on('alvum:open-capture-dir',   () => shell.openPath(path.join(ALVUM_ROOT, 'capture')));
  ipcMain.on('alvum:open-shell-log',     () => shell.openPath(SHELL_LOG));
  ipcMain.on('alvum:quit',           () => app.quit());

  // Provider status / test / set-active. The renderer drives these via
  // ipcRenderer.invoke (request/response), not .send, so we use handle()
  // and return the parsed CLI JSON synchronously to the renderer.
  ipcMain.handle('alvum:provider-list', () =>
    runAlvumJson(['providers', 'list'], 5000));
  ipcMain.handle('alvum:provider-test', (_e, name, model) =>
    runAlvumJson(
      ['providers', 'test', '--provider', name, '--model', model || 'claude-sonnet-4-6'],
      120000  // a real LLM call may take 30+ s on first auth
    ));
  ipcMain.handle('alvum:provider-set-active', (_e, name) =>
    runAlvumJson(['providers', 'set-active', name], 5000));
}

app.whenReady().then(async () => {
  if (process.platform === 'darwin' && app.dock) app.dock.hide();

  await requestPermissions();

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
  startProgressWatcher();

  startCapture();
});

app.on('before-quit', () => { app.isQuitting = true; });

app.on('before-quit', () => {
  stopCapture();
});

app.on('window-all-closed', (e) => {
  // Background agent: keep running when no windows exist.
  e.preventDefault?.();
});
