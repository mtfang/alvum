function bindIpc({ ipcMain, shell, app, runtime, trayPopover, capture, briefing, provider, connector, update, tail }) {
  ipcMain.on('alvum:request-state',  () => runtime.broadcastState());
  ipcMain.on('alvum:resize-popover', (_e, height) => trayPopover.resizePopover(height));
  ipcMain.on('alvum:toggle-capture', () => (
    capture.getCaptureState().running
      ? capture.stopCapture()
      : capture.reconcileCaptureProcess({ userInitiated: true })
  ));
  ipcMain.handle('alvum:capture-inputs', () => capture.captureInputsSummary());
  ipcMain.handle('alvum:toggle-capture-input', (_e, id) => capture.toggleCaptureInput(id));
  ipcMain.handle('alvum:set-capture-input-setting', (_e, id, key, value) =>
    capture.setCaptureInputSetting(id, key, value));
  ipcMain.handle('alvum:choose-directory', (_e, defaultPath) =>
    capture.chooseDirectory(defaultPath));
  ipcMain.on('alvum:start-briefing', () => briefing.generateBriefing());
  ipcMain.handle('alvum:start-briefing-date', (_e, date) => briefing.generateBriefingForDate(date));
  ipcMain.handle('alvum:briefing-calendar-month', (_e, month) => briefing.briefingCalendarMonth(month));
  ipcMain.on('alvum:open-briefing',  () => briefing.openTodayBriefing());
  ipcMain.handle('alvum:open-briefing-date', (_e, date) => briefing.openBriefingForDate(date));
  ipcMain.handle('alvum:read-briefing-date', (_e, date) => briefing.readBriefingForDate(date));
  ipcMain.handle('alvum:briefing-run-log', (_e, date) => briefing.briefingRunLog(date));
  ipcMain.handle('alvum:open-briefing-run-logs', (_e, date) => briefing.openBriefingRunLogs(date));
  ipcMain.handle('alvum:decision-graph-date', (_e, date) => briefing.readDecisionGraphForDate(date));
  ipcMain.handle('alvum:synthesis-profile', () =>
    connector.synthesisProfile());
  ipcMain.handle('alvum:synthesis-profile-save', (_e, profile) =>
    connector.synthesisProfileSave(profile));
  ipcMain.handle('alvum:synthesis-profile-suggestions', () =>
    connector.synthesisProfileSuggestions());
  ipcMain.handle('alvum:synthesis-profile-promote', (_e, id) =>
    connector.synthesisProfilePromote(id));
  ipcMain.handle('alvum:synthesis-profile-ignore', (_e, id) =>
    connector.synthesisProfileIgnore(id));
  ipcMain.on('alvum:open-briefing-log',  () => shell.openPath(runtime.BRIEFING_LOG));
  ipcMain.on('alvum:open-capture-dir',   () => shell.openPath(runtime.CAPTURE_DIR));
  ipcMain.handle('alvum:open-extensions-dir', () => connector.openExtensionsDir());
  ipcMain.on('alvum:open-shell-log',     () => shell.openPath(runtime.SHELL_LOG));
  ipcMain.handle('alvum:open-permission-settings', (_e, permission) =>
    capture.openPermissionSettings(permission));
  ipcMain.on('alvum:quit',           () => app.quit());

  // Provider status / test / set-active. The renderer drives these via
  // ipcRenderer.invoke (request/response), not .send, so we use handle()
  // and return the parsed CLI JSON synchronously to the renderer.
  ipcMain.handle('alvum:provider-list', () =>
    runtime.runAlvumJson(['providers', 'list'], 30000));
  ipcMain.handle('alvum:provider-test', (_e, name) =>
    provider.providerTest(name));
  ipcMain.handle('alvum:provider-set-active', (_e, name) =>
    provider.providerSetActive(name));
  ipcMain.handle('alvum:provider-set-enabled', (_e, name, enabled) =>
    provider.providerSetEnabled(name, !!enabled));
  ipcMain.handle('alvum:provider-configure', (_e, name, payload) =>
    provider.providerConfigure(name, payload));
  ipcMain.handle('alvum:provider-models', (_e, name) =>
    provider.providerModels(name));
  ipcMain.handle('alvum:provider-install-model', (_e, name, model) =>
    provider.providerInstallModel(name, model));
  ipcMain.handle('alvum:install-whisper-model', () =>
    provider.installWhisperModel());
  ipcMain.handle('alvum:provider-setup', (_e, name, action) =>
    provider.providerSetup(name, action));
  ipcMain.handle('alvum:update-check', () =>
    update.checkForUpdates(true));
  ipcMain.handle('alvum:update-install', () =>
    update.installDownloadedUpdate());
  ipcMain.handle('alvum:log-snapshot', (_e, kind) =>
    tail.logSnapshot(kind));
  ipcMain.handle('alvum:extension-list', () =>
    connector.extensionList());
  ipcMain.handle('alvum:extension-set-enabled', (_e, id, enabled) =>
    connector.extensionSetEnabled(id, !!enabled));
  ipcMain.handle('alvum:extension-doctor', () =>
    connector.extensionDoctor());
  ipcMain.handle('alvum:connector-list', () =>
    connector.connectorList());
  ipcMain.handle('alvum:connector-set-enabled', (_e, id, enabled) =>
    connector.connectorSetEnabled(id, !!enabled));
  ipcMain.handle('alvum:set-connector-processor-setting', (_e, component, key, value) =>
    capture.setConnectorProcessorSetting(component, key, value));
  ipcMain.handle('alvum:doctor', () =>
    connector.globalDoctor());
}

module.exports = { bindIpc };
