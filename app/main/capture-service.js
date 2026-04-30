function createCaptureService({
  app,
  fs,
  path,
  spawn,
  shell,
  systemPreferences,
  powerMonitor,
  dialog,
  ALVUM_ROOT,
  LOG_DIR,
  LOG_OUT,
  LOG_ERR,
  CONFIG_FILE,
  PERMISSION_SETTINGS_URLS,
  PERMISSION_WATCH_MS,
  WAKE_CAPTURE_RESTART_DELAY_MS,
  SOURCE_PERMISSION_REQUIREMENTS,
  appendShellLog,
  notify,
  resolveBinary,
  alvumSpawnEnv,
  runAlvumText,
  getPopover,
  connectorList,
  broadcastState,
  rebuildTrayMenu,
}) {
  let captureProc = null;
  let captureStartedAt = null;
  let captureWasRunningBeforeSuspend = false;
  let wakeRestartTimer = null;
  let permissionWatchTimer = null;
  let lastPermissionStatusKey = '';
  let lastPermissionBlockKey = '';

  function getCaptureState() {
    return {
      running: !!captureProc,
      proc: captureProc,
      startedAt: captureStartedAt,
      startedAtLabel: captureStartedAt ? captureStartedAt.toLocaleTimeString() : null,
    };
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

function isCaptureSourceInput(input) {
  return input && input.kind === 'capture';
}

function enabledCaptureInputs() {
  return captureInputsSummary().inputs.filter((input) => isCaptureSourceInput(input) && input.enabled);
}

function enabledExternalCaptureConnectorConfigured() {
  const sections = loadConfigSections();
  return Object.entries(sections).some(([name, section]) =>
    name.startsWith('connectors.')
      && section
      && section.kind === 'external-http'
      && sectionEnabled(sections, name));
}

function hasEnabledCaptureSource() {
  return enabledCaptureInputs().length > 0 || enabledExternalCaptureConnectorConfigured();
}

function enabledCapturePermissionIssues() {
  return enabledCaptureInputs().flatMap((input) =>
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
    if (hadBlocks && !hasBlocks && hasEnabledCaptureSource()) {
      appendShellLog('[permissions] Permissions restored; reconciling capture');
      notify('Alvum permissions restored', 'Capture is ready.');
      reconcileCaptureProcess({ userInitiated: false });
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

function reconcileCaptureProcess(options = {}) {
  const userInitiated = !!options.userInitiated;
  if (!hasEnabledCaptureSource()) {
    if (captureProc) {
      appendShellLog('[capture] stopping because no capture sources are enabled');
      stopCapture();
    }
    return { ok: true, status: 'no_enabled_sources' };
  }

  const permission_issues = enabledCapturePermissionIssues();
  if (permission_issues.length) {
    if (userInitiated) notifyPermissionIssues(permission_issues);
    if (captureProc) {
      appendShellLog('[capture] stopping because enabled capture sources are permission-blocked');
      stopCapture();
    }
    return { ok: false, status: 'blocked_permissions', permission_issues };
  }

  if (captureProc && !captureProc.killed) {
    return { ok: true, status: 'running', pid: captureProc.pid };
  }
  return startCapture({ userInitiated });
}

function startCapture(options = {}) {
  if (captureProc && !captureProc.killed) {
    return { ok: true, status: 'running', pid: captureProc.pid };
  }

  if (!hasEnabledCaptureSource()) {
    appendShellLog('[startCapture] skipped: no capture sources enabled');
    return { ok: true, status: 'no_enabled_sources' };
  }
  const permission_issues = enabledCapturePermissionIssues();
  if (permission_issues.length) {
    appendShellLog(`[startCapture] skipped: ${permissionIssueSummary(permission_issues)}`);
    if (options.userInitiated) notifyPermissionIssues(permission_issues);
    return { ok: false, status: 'blocked_permissions', permission_issues };
  }

  const bin = resolveBinary();
  appendShellLog(`[startCapture] resolveBinary → ${bin}`);
  if (!bin) {
    notify('Alvum', 'Could not locate alvum binary. Build with `cargo build --release -p alvum-cli`.');
    return { ok: false, status: 'missing_binary', error: 'alvum binary not found' };
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
    fs.closeSync(out);
    fs.closeSync(err);
  } catch (e) {
    try { fs.closeSync(out); } catch {}
    try { fs.closeSync(err); } catch {}
    appendShellLog(`[startCapture] spawn threw: ${e.stack || e}`);
    notify('Alvum', `Failed to spawn capture: ${e.message}`);
    return { ok: false, status: 'spawn_failed', error: e.message };
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
  return { ok: true, status: 'started', pid: captureProc.pid };
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
  if (!hasEnabledCaptureSource()) {
    stopCapture();
    return { ok: true, status: 'no_enabled_sources' };
  }
  if (captureProc) {
    captureProc.once('exit', () => reconcileCaptureProcess({ userInitiated: false }));
    stopCapture();
  } else {
    return reconcileCaptureProcess({ userInitiated: false });
  }
  return { ok: true, status: 'restarting' };
}

function scheduleWakeCaptureRestart(reason) {
  if (app.isQuitting || (!captureProc && !captureWasRunningBeforeSuspend)) return;
  if (wakeRestartTimer) clearTimeout(wakeRestartTimer);
  appendShellLog(`[power] ${reason}; scheduling capture restart after wake`);
  wakeRestartTimer = setTimeout(() => {
    wakeRestartTimer = null;
    const shouldRestart = !!captureProc || captureWasRunningBeforeSuspend;
    captureWasRunningBeforeSuspend = false;
    if (!shouldRestart || app.isQuitting) return;
    appendShellLog(`[power] ${reason}; restarting capture after wake`);
    restartCapture();
  }, WAKE_CAPTURE_RESTART_DELAY_MS);
}

function startPowerWatcher() {
  if (!powerMonitor) return;
  powerMonitor.on('suspend', () => {
    captureWasRunningBeforeSuspend = !!captureProc;
    appendShellLog(`[power] suspend; capture_running=${captureWasRunningBeforeSuspend}`);
  });
  powerMonitor.on('resume', () => scheduleWakeCaptureRestart('resume'));
  powerMonitor.on('unlock-screen', () => {
    if (captureWasRunningBeforeSuspend) scheduleWakeCaptureRestart('unlock-screen');
  });
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
      enabled: sectionEnabled(sections, 'capture.audio-mic', false) && sectionEnabled(sections, 'connectors.audio'),
      detail: 'Local audio capture',
      settings: settingsFor(sections, ['capture.audio-mic']),
    },
    {
      id: 'audio-system',
      label: 'System audio',
      kind: 'capture',
      enabled: sectionEnabled(sections, 'capture.audio-system', false) && sectionEnabled(sections, 'connectors.audio'),
      detail: 'App and system output',
      settings: settingsFor(sections, ['capture.audio-system']),
    },
    {
      id: 'screen',
      label: 'Screen',
      kind: 'capture',
      enabled: sectionEnabled(sections, 'capture.screen', false) && sectionEnabled(sections, 'connectors.screen'),
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
  const parent = getPopover();
  const result = parent && !parent.isDestroyed()
    ? await dialog.showOpenDialog(parent, options)
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
  if (['audio-mic', 'audio-system', 'screen'].includes(id)) restartCapture();
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
  if (result.ok && (component === 'alvum.audio/whisper' || component === 'alvum.screen/vision')) {
    restartCapture();
  }
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
  let capture_status = null;
  if (['audio-mic', 'audio-system', 'screen'].includes(id)) {
    capture_status = reconcileCaptureProcess({ userInitiated: true });
  }
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
  return { ok: true, input: id, enabled: next, captureInputs, permission_issues, capture_status };
}

  function shutdown() {
    if (wakeRestartTimer) clearTimeout(wakeRestartTimer);
    stopCapture();
  }

  return {
    getCaptureState,
    capturePermissionStatus,
    sourcePermissionRequirements,
    blockedPermissionsForSource,
    permissionIssueSummary,
    notifyPermissionIssues,
    openPermissionSettings,
    promptForSourcePermissions,
    enabledPermissionIssues,
    enabledCaptureInputs,
    enabledExternalCaptureConnectorConfigured,
    hasEnabledCaptureSource,
    enabledCapturePermissionIssues,
    reportEnabledPermissionBlocks,
    permissionIssuesKey,
    startPermissionWatcher,
    requestPermissions,
    reconcileCaptureProcess,
    startCapture,
    stopCapture,
    restartCapture,
    scheduleWakeCaptureRestart,
    startPowerWatcher,
    parseFlatTomlSections,
    loadConfigSections,
    sectionEnabled,
    settingsFor,
    captureInputsSummary,
    setConfigValue,
    captureInputConfigSection,
    chooseDirectory,
    setCaptureInputSetting,
    processorConfigSection,
    setConnectorProcessorSetting,
    toggleCaptureInput,
    shutdown,
  };
}

module.exports = { createCaptureService };
