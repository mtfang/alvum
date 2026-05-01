const path = require('path');
const fs = require('fs');
const os = require('os');
const { execFileSync } = require('child_process');

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
const LOGIN_SHELL_PATH_MARKER = '__ALVUM_LOGIN_SHELL_PATH__=';
const CONFIG_EXTRA_PATH_KEYS = new Set(['extra_path', 'credential_process_path', 'helper_path']);
let loginShellPathCache = null;

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

function splitPathEntries(value) {
  return String(value || '')
    .split(path.delimiter)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function shellPathWithArgs(shell, args) {
  try {
    const raw = execFileSync(
      shell,
      [...args, `printf '${LOGIN_SHELL_PATH_MARKER}%s\\n' "$PATH"`],
      {
        encoding: 'utf8',
        env: process.env,
        stdio: ['ignore', 'pipe', 'ignore'],
        timeout: 2000,
      },
    );
    const lines = raw.split(/\r?\n/).filter(Boolean);
    const markerLine = lines.reverse().find((line) => line.startsWith(LOGIN_SHELL_PATH_MARKER));
    return markerLine ? markerLine.slice(LOGIN_SHELL_PATH_MARKER.length).trim() : '';
  } catch {
    return '';
  }
}

function loginShellPath() {
  if (loginShellPathCache !== null) return loginShellPathCache;
  loginShellPathCache = '';
  if (process.env.ALVUM_DISABLE_LOGIN_SHELL_PATH === '1') return loginShellPathCache;

  const shell = process.env.SHELL && path.isAbsolute(process.env.SHELL)
    ? process.env.SHELL
    : '/bin/zsh';
  if (!fs.existsSync(shell)) return loginShellPathCache;

  loginShellPathCache = [
    shellPathWithArgs(shell, ['-lc']),
    shellPathWithArgs(shell, ['-lic']),
  ].filter(Boolean).join(path.delimiter);
  return loginShellPathCache;
}

function unquoteTomlString(value) {
  const raw = String(value || '').trim();
  if (!raw) return '';
  if ((raw.startsWith('"') && raw.endsWith('"')) || (raw.startsWith("'") && raw.endsWith("'"))) {
    try {
      return JSON.parse(raw);
    } catch {
      return raw.slice(1, -1);
    }
  }
  return raw;
}

function parseExtraPathValue(value) {
  const raw = String(value || '').trim().replace(/\s+#.*$/, '');
  if (!raw) return [];
  if (raw.startsWith('[')) {
    const entries = [];
    const regex = /"([^"\\]*(?:\\.[^"\\]*)*)"|'([^']*)'/g;
    let match;
    while ((match = regex.exec(raw)) !== null) {
      entries.push(...splitPathEntries(unquoteTomlString(match[0])));
    }
    return entries;
  }
  return splitPathEntries(unquoteTomlString(raw));
}

function configExtraPathEntries(configFile = CONFIG_FILE) {
  let raw = '';
  try {
    raw = fs.readFileSync(configFile, 'utf8');
  } catch {
    return [];
  }
  const entries = [];
  let section = '';
  for (const line of raw.split(/\r?\n/)) {
    const sectionMatch = line.match(/^\s*\[([^\]]+)\]\s*$/);
    if (sectionMatch) {
      section = sectionMatch[1].trim();
      continue;
    }
    const valueMatch = line.match(/^\s*([A-Za-z0-9_-]+)\s*=\s*(.+?)\s*$/);
    if (!valueMatch || !CONFIG_EXTRA_PATH_KEYS.has(valueMatch[1])) continue;
    if (section !== 'runtime' && !section.startsWith('providers.')) continue;
    entries.push(...parseExtraPathValue(valueMatch[2]));
  }
  return entries;
}

function buildAlvumPath(extraPath = '', configFile = CONFIG_FILE) {
  const pathEntries = [
    ...splitPathEntries(extraPath),
    ...splitPathEntries(process.env.ALVUM_EXTRA_PATH),
    ...configExtraPathEntries(configFile),
    ...splitPathEntries(loginShellPath()),
    path.join(HOME, '.local', 'bin'),
    path.join(HOME, 'bin'),
    path.join(HOME, '.cargo', 'bin'),
    path.join(HOME, '.bun', 'bin'),
    path.join(HOME, '.npm-global', 'bin'),
    path.join(HOME, '.volta', 'bin'),
    '/opt/homebrew/bin',
    '/opt/homebrew/sbin',
    '/usr/local/bin',
    '/usr/local/sbin',
    '/opt/amazon/bin',
    '/usr/local/amazon/bin',
    '/usr/bin',
    '/bin',
    '/usr/sbin',
    '/sbin',
    ...splitPathEntries(process.env.PATH),
  ].filter(Boolean);
  return [...new Set(pathEntries)].join(path.delimiter);
}

function alvumSpawnEnv(extraEnv = {}) {
  const PATH = buildAlvumPath(extraEnv.PATH);
  return { ...process.env, ...extraEnv, PATH };
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
  buildAlvumPath,
  configExtraPathEntries,
  alvumSpawnEnv,
};
