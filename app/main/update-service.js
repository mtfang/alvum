function createUpdateService({
  app,
  autoUpdater,
  updaterLoadError,
  fs,
  path,
  UPDATE_FEED,
  UPDATE_STATE_FILE,
  UPDATE_STARTUP_DELAY_MS,
  UPDATE_CHECK_INTERVAL_MS,
  appendShellLog,
  notify,
  broadcastState,
}) {
  function readJsonFileIfPresent(file) {
    if (!fs.existsSync(file)) return null;
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  }

let updateConfigured = false;
let updateWatchTimer = null;
let lastUpdateNotificationKey = '';
let updateState = {
  status: 'idle',
  currentVersion: null,
  latestVersion: null,
  releaseName: null,
  releaseDate: null,
  releaseNotes: null,
  releaseUrl: null,
  error: null,
  progress: null,
  checkedAt: null,
  supported: false,
  packaged: false,
};

function updateSupported() {
  return !!autoUpdater && process.platform === 'darwin' && app.isPackaged;
}

function updateUnavailableReason() {
  if (!autoUpdater) {
    return updaterLoadError
      ? `Updater unavailable: ${updaterLoadError.message}`
      : 'Updater unavailable.';
  }
  if (process.platform !== 'darwin') return 'Automatic updates are configured for macOS builds only.';
  if (!app.isPackaged) return 'Automatic updates are available in packaged builds only.';
  return null;
}

function updateReleaseUrl(version) {
  const base = `https://github.com/${UPDATE_FEED.owner}/${UPDATE_FEED.repo}/releases`;
  return version ? `${base}/tag/${version}` : `${base}/latest`;
}

function updateSnapshot() {
  const supported = updateSupported();
  const error = supported ? updateState.error : (updateState.error || updateUnavailableReason());
  return {
    ...updateState,
    currentVersion: app.getVersion(),
    supported,
    packaged: app.isPackaged,
    feed: { owner: UPDATE_FEED.owner, repo: UPDATE_FEED.repo },
    error,
  };
}

function updateInfoPatch(info) {
  if (!info) return {};
  return {
    latestVersion: info.version || null,
    releaseName: info.releaseName || null,
    releaseDate: info.releaseDate || null,
    releaseNotes: info.releaseNotes || null,
    releaseUrl: updateReleaseUrl(info.version),
  };
}

function setUpdateState(patch) {
  updateState = {
    ...updateState,
    ...patch,
    currentVersion: app.getVersion(),
    supported: updateSupported(),
    packaged: app.isPackaged,
  };
  broadcastState();
  return updateSnapshot();
}

function notifyUpdateOnce(key, title, body) {
  if (!key || key === lastUpdateNotificationKey) return;
  lastUpdateNotificationKey = key;
  appendShellLog(`[updates] ${title}: ${body}`);
  notify(title, body);
}

function readUpdateCheckMeta() {
  try {
    return readJsonFileIfPresent(UPDATE_STATE_FILE) || {};
  } catch {
    return {};
  }
}

function writeUpdateCheckMeta(meta) {
  try {
    fs.mkdirSync(path.dirname(UPDATE_STATE_FILE), { recursive: true });
    fs.writeFileSync(UPDATE_STATE_FILE, JSON.stringify(meta, null, 2));
  } catch (e) {
    appendShellLog(`[updates] failed to write update state: ${e.message}`);
  }
}

function configureAutoUpdater() {
  if (updateConfigured) return;
  updateConfigured = true;
  if (!autoUpdater) {
    setUpdateState({ status: 'unavailable', error: updateUnavailableReason() });
    return;
  }

  autoUpdater.autoDownload = true;
  autoUpdater.autoInstallOnAppQuit = true;
  autoUpdater.allowPrerelease = false;
  autoUpdater.fullChangelog = false;
  autoUpdater.logger = {
    info: (msg) => appendShellLog(`[updates] ${msg}`),
    warn: (msg) => appendShellLog(`[updates][warn] ${msg}`),
    error: (msg) => appendShellLog(`[updates][error] ${msg}`),
    debug: (msg) => appendShellLog(`[updates][debug] ${msg}`),
  };
  autoUpdater.setFeedURL(UPDATE_FEED);

  autoUpdater.on('checking-for-update', () => {
    setUpdateState({ status: 'checking', error: null, progress: null, checkedAt: new Date().toISOString() });
  });
  autoUpdater.on('update-available', (info) => {
    setUpdateState({
      status: 'downloading',
      error: null,
      progress: { percent: 0, transferred: null, total: null },
      checkedAt: new Date().toISOString(),
      ...updateInfoPatch(info),
    });
  });
  autoUpdater.on('update-not-available', (info) => {
    setUpdateState({
      status: 'current',
      error: null,
      progress: null,
      checkedAt: new Date().toISOString(),
      ...updateInfoPatch(info),
    });
  });
  autoUpdater.on('download-progress', (progress) => {
    setUpdateState({
      status: 'downloading',
      error: null,
      progress: {
        percent: Number.isFinite(progress.percent) ? progress.percent : null,
        transferred: progress.transferred || null,
        total: progress.total || null,
      },
    });
  });
  autoUpdater.on('update-downloaded', (info) => {
    const snapshot = setUpdateState({
      status: 'downloaded',
      error: null,
      progress: null,
      ...updateInfoPatch(info),
    });
    notifyUpdateOnce(
      `downloaded:${snapshot.latestVersion}`,
      'Alvum update ready',
      `Version ${snapshot.latestVersion || 'latest'} is ready to install.`,
    );
  });
  autoUpdater.on('error', (error) => {
    setUpdateState({
      status: 'error',
      error: error && error.message ? error.message : String(error || 'update failed'),
      progress: null,
      checkedAt: new Date().toISOString(),
    });
  });

  setUpdateState({ status: updateSupported() ? 'idle' : 'unavailable', error: updateUnavailableReason() });
}

function shouldRunScheduledUpdateCheck() {
  const meta = readUpdateCheckMeta();
  const last = Date.parse(meta.last_checked_at || '');
  if (!Number.isFinite(last)) return true;
  return Date.now() - last >= UPDATE_CHECK_INTERVAL_MS;
}

async function checkForUpdates(manual = false) {
  configureAutoUpdater();
  if (!updateSupported()) {
    const snapshot = setUpdateState({ status: 'unavailable', error: updateUnavailableReason() });
    return { ok: false, error: snapshot.error, state: snapshot };
  }
  if (!manual && !shouldRunScheduledUpdateCheck()) {
    return { ok: true, skipped: true, state: updateSnapshot() };
  }

  const checkedAt = new Date().toISOString();
  writeUpdateCheckMeta({ last_checked_at: checkedAt });
  setUpdateState({ status: 'checking', error: null, progress: null, checkedAt });
  try {
    const result = await autoUpdater.checkForUpdates();
    if (!result) {
      const snapshot = setUpdateState({ status: 'unavailable', error: updateUnavailableReason() || 'Updater skipped.' });
      return { ok: false, error: snapshot.error, state: snapshot };
    }
    if (result.updateInfo) {
      setUpdateState(updateInfoPatch(result.updateInfo));
    }
    return { ok: true, available: !!result.isUpdateAvailable, state: updateSnapshot() };
  } catch (e) {
    const snapshot = setUpdateState({
      status: 'error',
      error: e && e.message ? e.message : String(e),
      progress: null,
      checkedAt: new Date().toISOString(),
    });
    return { ok: false, error: snapshot.error, state: snapshot };
  }
}

function installDownloadedUpdate() {
  configureAutoUpdater();
  if (!updateSupported()) {
    const snapshot = setUpdateState({ status: 'unavailable', error: updateUnavailableReason() });
    return { ok: false, error: snapshot.error, state: snapshot };
  }
  if (updateState.status !== 'downloaded') {
    return { ok: false, error: 'No downloaded update is ready to install.', state: updateSnapshot() };
  }
  setUpdateState({ status: 'installing', error: null, progress: null });
  autoUpdater.quitAndInstall(false, true);
  return { ok: true, state: updateSnapshot() };
}

function startUpdateWatcher() {
  configureAutoUpdater();
  if (!updateSupported() || updateWatchTimer) return;
  setTimeout(() => checkForUpdates(false), UPDATE_STARTUP_DELAY_MS);
  updateWatchTimer = setInterval(() => checkForUpdates(false), UPDATE_CHECK_INTERVAL_MS);
}

  return {
    updateSupported,
    updateUnavailableReason,
    updateReleaseUrl,
    updateSnapshot,
    updateInfoPatch,
    setUpdateState,
    notifyUpdateOnce,
    readUpdateCheckMeta,
    writeUpdateCheckMeta,
    configureAutoUpdater,
    shouldRunScheduledUpdateCheck,
    checkForUpdates,
    installDownloadedUpdate,
    startUpdateWatcher,
  };
}

module.exports = { createUpdateService };
