const path = require('path');
const fs = require('fs');
const os = require('os');

const appRoot = path.resolve(__dirname, '..');
const HOME = os.homedir();
const ALVUM_ROOT = path.join(HOME, '.alvum');
const LOG_DIR = path.join(ALVUM_ROOT, 'runtime', 'logs');
const LOG_OUT = path.join(LOG_DIR, 'capture.out');
const LOG_ERR = path.join(LOG_DIR, 'capture.err');
const SHELL_LOG = path.join(LOG_DIR, 'shell.log');
const BRIEFINGS_DIR = path.join(ALVUM_ROOT, 'generated', 'briefings');
const CAPTURE_DIR = path.join(ALVUM_ROOT, 'capture');
const BRIEFING_LOG = path.join(LOG_DIR, 'briefing.out');
const BRIEFING_ERR = path.join(LOG_DIR, 'briefing.err');
const CONFIG_FILE = path.join(ALVUM_ROOT, 'runtime', 'config.toml');
const EXTENSIONS_DIR = path.join(ALVUM_ROOT, 'runtime', 'extensions');
const LAUNCH_INTENT_FILE = path.join(ALVUM_ROOT, 'runtime', 'launch-intent.json');
const LAUNCH_INTENT_TTL_MS = 5 * 60 * 1000;
const UPDATE_STATE_FILE = path.join(ALVUM_ROOT, 'runtime', 'update-check.json');
const PROVIDER_HEALTH_FILE = path.join(ALVUM_ROOT, 'runtime', 'provider-health.json');

const PERMISSION_SETTINGS_URLS = {
  microphone: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone',
  screen: 'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture',
};
const PERMISSION_WATCH_MS = 3000;
const WAKE_CAPTURE_RESTART_DELAY_MS = 5000;
const UPDATE_FEED = { provider: 'github', owner: 'mtfang', repo: 'alvum' };
const UPDATE_STARTUP_DELAY_MS = 30 * 1000;
const UPDATE_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
const SOURCE_PERMISSION_REQUIREMENTS = {
  'audio-mic': [{ permission: 'microphone', label: 'Microphone' }],
  'audio-system': [{ permission: 'screen', label: 'Screen & System Audio Recording' }],
  screen: [{ permission: 'screen', label: 'Screen Recording' }],
};

function ensureExtensionsDir() {
  fs.mkdirSync(EXTENSIONS_DIR, { recursive: true });
}

function ensureLogDir() {
  fs.mkdirSync(LOG_DIR, { recursive: true });
}

function consumeLaunchIntent(now = Date.now()) {
  if (!fs.existsSync(LAUNCH_INTENT_FILE)) return {};
  let raw = '';
  try {
    raw = fs.readFileSync(LAUNCH_INTENT_FILE, 'utf8');
  } catch {
    return {};
  } finally {
    try { fs.unlinkSync(LAUNCH_INTENT_FILE); } catch {}
  }

  let intent;
  try {
    intent = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!intent || typeof intent !== 'object' || Array.isArray(intent)) return {};

  const createdAt = Date.parse(intent.created_at || intent.createdAt || '');
  if (Number.isFinite(createdAt) && now - createdAt > LAUNCH_INTENT_TTL_MS) return {};
  return intent;
}

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
  const dev = path.resolve(appRoot, '..', 'target', 'release', 'alvum');
  const legacy = path.join(ALVUM_ROOT, 'runtime', 'Alvum.app', 'Contents', 'MacOS', 'alvum');
  for (const candidate of [packagedHelper, packaged, dev, legacy]) {
    if (candidate && fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function resolveScript(name) {
  const packaged = path.join(process.resourcesPath || '', 'scripts', name);
  const dev = path.resolve(appRoot, '..', 'scripts', name);
  for (const candidate of [packaged, dev]) {
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

module.exports = {
  appRoot,
  HOME,
  ALVUM_ROOT,
  LOG_DIR,
  LOG_OUT,
  LOG_ERR,
  SHELL_LOG,
  BRIEFINGS_DIR,
  CAPTURE_DIR,
  BRIEFING_LOG,
  BRIEFING_ERR,
  CONFIG_FILE,
  EXTENSIONS_DIR,
  LAUNCH_INTENT_FILE,
  UPDATE_STATE_FILE,
  PROVIDER_HEALTH_FILE,
  PERMISSION_SETTINGS_URLS,
  PERMISSION_WATCH_MS,
  WAKE_CAPTURE_RESTART_DELAY_MS,
  UPDATE_FEED,
  UPDATE_STARTUP_DELAY_MS,
  UPDATE_CHECK_INTERVAL_MS,
  SOURCE_PERMISSION_REQUIREMENTS,
  ensureExtensionsDir,
  ensureLogDir,
  consumeLaunchIntent,
  resolveBinary,
  resolveScript,
  alvumSpawnEnv,
};
