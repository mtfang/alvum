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

const { app, Tray, Menu, shell, systemPreferences, Notification, nativeImage } = require('electron');
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

function notify(title, body) {
  try {
    new Notification({ title, body }).show();
  } catch (e) {
    console.error('notify failed', e);
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

function trayIcon() {
  // Fallback monochrome-template icon built in-memory so we don't need a
  // shipped asset and can't fail on read-only bundles (app.asar).
  // Real designed icons will land under assets/ later and override this.
  const diskIcon = path.join(__dirname, 'assets', 'tray-icon.png');
  if (fs.existsSync(diskIcon)) {
    const img = nativeImage.createFromPath(diskIcon);
    if (!img.isEmpty()) {
      img.setTemplateImage(true);
      return img;
    }
  }
  // 16x16 solid dot as placeholder. Template image so macOS tints it.
  const placeholder = Buffer.from(
    'iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAQAAAC1+jfqAAAAL0lEQVR42mNkIAAYiVLCwMDw'
      + 'BzOBEdcwDO4ECmAEd4LFYCrATGsQgzAOXg0AAFc8Aew8p+a7AAAAAElFTkSuQmCC',
    'base64'
  );
  const img = nativeImage.createFromBuffer(placeholder);
  img.setTemplateImage(true);
  return img;
}

function rebuildTrayMenu() {
  const status = captureProc
    ? `● Capture running (started ${captureStartedAt.toLocaleTimeString()})`
    : '○ Capture stopped';

  const menu = Menu.buildFromTemplate([
    { label: 'Alvum', enabled: false },
    { label: status, enabled: false },
    { type: 'separator' },
    {
      label: captureProc ? 'Stop capture' : 'Start capture',
      click: () => (captureProc ? stopCapture() : startCapture()),
    },
    {
      label: 'Restart capture',
      enabled: !!captureProc,
      click: () => restartCapture(),
    },
    { type: 'separator' },
    {
      label: 'Open capture dir',
      click: () => shell.openPath(path.join(ALVUM_ROOT, 'capture')),
    },
    {
      label: 'Open briefings dir',
      click: () => shell.openPath(path.join(ALVUM_ROOT, 'generated', 'briefings')),
    },
    {
      label: 'Open log',
      click: () => shell.openPath(LOG_OUT),
    },
    { type: 'separator' },
    { label: 'Quit alvum', click: () => app.quit() },
  ]);
  tray.setContextMenu(menu);
  tray.setToolTip(status);
}

app.whenReady().then(async () => {
  if (process.platform === 'darwin' && app.dock) app.dock.hide();

  await requestPermissions();

  tray = new Tray(trayIcon());
  tray.setTitle('alvum');
  rebuildTrayMenu();

  startCapture();
});

app.on('before-quit', () => {
  stopCapture();
});

app.on('window-all-closed', (e) => {
  // Background agent: keep running when no windows exist.
  e.preventDefault?.();
});
