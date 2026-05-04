function bindIpc({ ipcMain, shell, app, runtime, trayPopover, capture, briefing, provider, connector, speaker, update, scheduler, tail }) {
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
  ipcMain.handle('alvum:cancel-briefing-date', (_e, date) => briefing.cancelBriefingForDate(date));
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
  ipcMain.handle('alvum:synthesis-schedule', () =>
    scheduler.scheduleSnapshot());
  ipcMain.handle('alvum:synthesis-schedule-save', (_e, patch) =>
    scheduler.saveSchedule(patch || {}));
  ipcMain.handle('alvum:synthesis-schedule-run-due', () =>
    scheduler.runDue({ reason: 'user', ignoreEnabled: true }));
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
  ipcMain.handle('alvum:install-whisper-model', (_e, variant) =>
    provider.installWhisperModel(variant));
  ipcMain.handle('alvum:install-pyannote', () =>
    provider.installPyannote());
  ipcMain.handle('alvum:open-pyannote-terms', () =>
    provider.openPyannoteTerms());
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
  ipcMain.handle('alvum:speaker-list', () =>
    speaker.speakerList());
  ipcMain.handle('alvum:speaker-samples', () =>
    speaker.speakerSamples());
  ipcMain.handle('alvum:speaker-link', (_e, id, interestId) =>
    speaker.speakerLink(id, interestId));
  ipcMain.handle('alvum:speaker-link-sample', (_e, sampleId, interestId) =>
    speaker.speakerLinkSample(sampleId, interestId));
  ipcMain.handle('alvum:speaker-move-sample', (_e, sampleId, clusterId) =>
    speaker.speakerMoveSample(sampleId, clusterId));
  ipcMain.handle('alvum:speaker-ignore-sample', (_e, sampleId) =>
    speaker.speakerIgnoreSample(sampleId));
  ipcMain.handle('alvum:speaker-unlink-sample', (_e, sampleId) =>
    speaker.speakerUnlinkSample(sampleId));
  ipcMain.handle('alvum:speaker-split-sample', (_e, sampleId, payload) =>
    speaker.speakerSplitSample(sampleId, payload));
  ipcMain.handle('alvum:speaker-split', (_e, clusterId, sampleIds) =>
    speaker.speakerSplit(clusterId, sampleIds));
  ipcMain.handle('alvum:speaker-recluster', () =>
    speaker.speakerRecluster());
  ipcMain.handle('alvum:speaker-unlink', (_e, id) =>
    speaker.speakerUnlink(id));
  ipcMain.handle('alvum:speaker-unlink-interest', (_e, interestId) =>
    speaker.speakerUnlinkInterest(interestId));
  ipcMain.handle('alvum:speaker-rename', (_e, id, label) =>
    speaker.speakerRename(id, label));
  ipcMain.handle('alvum:speaker-merge', (_e, sourceId, targetId) =>
    speaker.speakerMerge(sourceId, targetId));
  ipcMain.handle('alvum:speaker-forget', (_e, id) =>
    speaker.speakerForget(id));
  ipcMain.handle('alvum:speaker-reset', () =>
    speaker.speakerReset());
  ipcMain.handle('alvum:speaker-sample-audio', (_e, id, sampleIndex) =>
    speaker.speakerSampleAudio(id, sampleIndex));
  ipcMain.handle('alvum:voice-sample-audio', (_e, sampleId) =>
    speaker.voiceSampleAudio(sampleId));
  ipcMain.handle('alvum:doctor', () =>
    connector.globalDoctor());
}

module.exports = { bindIpc };
