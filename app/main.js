// Alvum Electron shell: composition root for the menu-bar app.
// Domain behavior lives under app/main/*.js; this file owns startup wiring.

const { app, Tray, Menu, BrowserWindow, ipcMain, shell, screen, systemPreferences, powerMonitor, Notification, nativeImage, dialog } = require('electron');
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const runtime = require('./main/runtime');
const { createShellLogger } = require('./main/shell-log');
const { createCliRunner } = require('./main/cli-runner');
const { createNotificationService } = require('./main/notifications');
const { createTailService } = require('./main/tail');
const { createTrayPopover } = require('./main/tray-popover');
const { createCaptureService } = require('./main/capture-service');
const { createBriefingService } = require('./main/briefing-service');
const { createProviderService } = require('./main/provider-service');
const { createConnectorService } = require('./main/connector-service');
const { createUpdateService } = require('./main/update-service');
const { createSynthesisScheduler } = require('./main/synthesis-scheduler');
const { bindIpc } = require('./main/ipc');

let autoUpdater = null;
let updaterLoadError = null;
try {
  ({ autoUpdater } = require('electron-updater'));
} catch (e) {
  updaterLoadError = e;
}

const { appendShellLog, installConsoleCapture } = createShellLogger({
  fs,
  LOG_DIR: runtime.LOG_DIR,
  SHELL_LOG: runtime.SHELL_LOG,
});
installConsoleCapture();

const cliRunner = createCliRunner({
  spawn,
  resolveBinary: runtime.resolveBinary,
  alvumSpawnEnv: runtime.alvumSpawnEnv,
});
const notifications = createNotificationService({
  nativeImage,
  Notification,
  path,
  fs,
  appRoot: runtime.appRoot,
  NOTIFY_QUEUE: path.join(runtime.ALVUM_ROOT, 'runtime', 'notify.queue'),
  appendShellLog,
});
const tail = createTailService({
  fs,
  SHELL_LOG: runtime.SHELL_LOG,
  BRIEFING_LOG: runtime.BRIEFING_LOG,
  EVENTS_FILE: path.join(runtime.ALVUM_ROOT, 'runtime', 'pipeline.events'),
});

let trayPopover;
let capture;
let briefing;
let provider;
let connector;
let update;
let scheduler;

function sendToPopover(channel, payload) {
  return trayPopover ? trayPopover.send(channel, payload) : false;
}

function broadcastState() {
  if (!trayPopover || !trayPopover.getPopover() || trayPopover.getPopover().isDestroyed()) return;
  const catchup = briefing.pendingBriefingCatchup();
  const latestBriefing = briefing.latestBriefingInfo();
  const captureStats = briefing.captureStats();
  const captureState = capture.getCaptureState();
  sendToPopover('alvum:state', {
    captureRunning: captureState.running,
    captureStartedAt: captureState.startedAtLabel,
    briefingRunning: briefing.isBriefingRunning(),
    briefingRuns: briefing.briefingRunSnapshot(),
    briefingStartedAt: null,
    briefingTargetDate: null,
    briefingCatchupPending: catchup.count,
    briefingCatchupDates: catchup.dates,
    captureStats,
    captureInputs: capture.captureInputsSummary(),
    permissions: capture.capturePermissionStatus(),
    stats: captureStats.summary,
    latestBriefing,
    briefingTargets: briefing.recentBriefingTargets(),
    briefingCalendar: briefing.briefingCalendarMonth(),
    providerSummary: provider.providerProbeSnapshot(),
    providerStats: provider.providerRuntimeStatsSnapshot(),
    providerIssue: provider.currentProviderIssueSnapshot(),
    synthesisSchedule: scheduler ? scheduler.scheduleSnapshot() : null,
    updateState: update.updateSnapshot(),
  });
}

function rebuildTrayMenu() {
  if (trayPopover) trayPopover.rebuildTrayMenu();
}

trayPopover = createTrayPopover({
  app,
  Tray,
  Menu,
  BrowserWindow,
  screen,
  shell,
  nativeImage,
  fs,
  path,
  appRoot: runtime.appRoot,
  SHELL_LOG: runtime.SHELL_LOG,
  BRIEFING_LOG: runtime.BRIEFING_LOG,
  getCaptureState: () => capture.getCaptureState(),
  broadcastState,
});

capture = createCaptureService({
  app,
  fs,
  path,
  spawn,
  shell,
  systemPreferences,
  powerMonitor,
  dialog,
  ALVUM_ROOT: runtime.ALVUM_ROOT,
  LOG_DIR: runtime.LOG_DIR,
  LOG_OUT: runtime.LOG_OUT,
  LOG_ERR: runtime.LOG_ERR,
  CONFIG_FILE: runtime.CONFIG_FILE,
  PERMISSION_SETTINGS_URLS: runtime.PERMISSION_SETTINGS_URLS,
  PERMISSION_WATCH_MS: runtime.PERMISSION_WATCH_MS,
  WAKE_CAPTURE_RESTART_DELAY_MS: runtime.WAKE_CAPTURE_RESTART_DELAY_MS,
  SOURCE_PERMISSION_REQUIREMENTS: runtime.SOURCE_PERMISSION_REQUIREMENTS,
  appendShellLog,
  notify: notifications.notify,
  resolveBinary: runtime.resolveBinary,
  alvumSpawnEnv: runtime.alvumSpawnEnv,
  runAlvumText: cliRunner.runAlvumText,
  getPopover: () => trayPopover.getPopover(),
  connectorList: (...args) => connector.connectorList(...args),
  broadcastState,
  rebuildTrayMenu,
});

provider = createProviderService({
  fs,
  path,
  shell,
  spawn,
  PROVIDER_HEALTH_FILE: runtime.PROVIDER_HEALTH_FILE,
  appendShellLog,
  notify: notifications.notify,
  runAlvumJson: cliRunner.runAlvumJson,
  alvumSpawnEnv: runtime.alvumSpawnEnv,
  connectorList: (...args) => connector.connectorList(...args),
  broadcastState,
});

briefing = createBriefingService({
  fs,
  path,
  crypto,
  shell,
  spawn,
  ALVUM_ROOT: runtime.ALVUM_ROOT,
  BRIEFINGS_DIR: runtime.BRIEFINGS_DIR,
  CAPTURE_DIR: runtime.CAPTURE_DIR,
  BRIEFING_LOG: runtime.BRIEFING_LOG,
  BRIEFING_ERR: runtime.BRIEFING_ERR,
  appendShellLog,
  notify: notifications.notify,
  resolveScript: runtime.resolveScript,
  resolveBinary: runtime.resolveBinary,
  alvumSpawnEnv: runtime.alvumSpawnEnv,
  ensureLogDir: runtime.ensureLogDir,
  readTail: tail.readTail,
  providerDiagnosticSnapshot: (...args) => provider.providerDiagnosticSnapshot(...args),
  providerProbeSummary: (...args) => provider.providerProbeSummary(...args),
  providerSelectableForAuto: (...args) => provider.providerSelectableForAuto(...args),
  refreshProviderWatch: (...args) => provider.refreshProviderWatch(...args),
  recordProviderEvent: (...args) => provider.recordProviderEvent(...args),
  broadcastState,
  rebuildTrayMenu,
  sendToPopover,
  onRunFinished: (...args) => scheduler && scheduler.handleBriefingRunFinished(...args),
});

scheduler = createSynthesisScheduler({
  fs,
  path,
  spawn,
  powerMonitor,
  appBundlePath: () => path.resolve(path.dirname(process.execPath), '..', '..'),
  ALVUM_ROOT: runtime.ALVUM_ROOT,
  CONFIG_FILE: runtime.CONFIG_FILE,
  LAUNCHAGENTS_DIR: path.join(runtime.HOME, 'Library', 'LaunchAgents'),
  LAUNCHD_LABEL: 'com.alvum.briefing',
  LAUNCHD_PLIST: path.join(runtime.HOME, 'Library', 'LaunchAgents', 'com.alvum.briefing.plist'),
  appendShellLog,
  notify: notifications.notify,
  runAlvumText: cliRunner.runAlvumText,
  alvumSpawnEnv: runtime.alvumSpawnEnv,
  briefing,
  broadcastState,
});

connector = createConnectorService({
  shell,
  EXTENSIONS_DIR: runtime.EXTENSIONS_DIR,
  BRIEFING_LOG: runtime.BRIEFING_LOG,
  SHELL_LOG: runtime.SHELL_LOG,
  ensureExtensionsDir: runtime.ensureExtensionsDir,
  runAlvumJson: cliRunner.runAlvumJson,
  runAlvumText: cliRunner.runAlvumText,
  readTail: tail.readTail,
  capturePermissionStatus: (...args) => capture.capturePermissionStatus(...args),
  sourcePermissionRequirements: (...args) => capture.sourcePermissionRequirements(...args),
  blockedPermissionsForSource: (...args) => capture.blockedPermissionsForSource(...args),
  enabledPermissionIssues: (...args) => capture.enabledPermissionIssues(...args),
  promptForSourcePermissions: (...args) => capture.promptForSourcePermissions(...args),
  notifyPermissionIssues: (...args) => capture.notifyPermissionIssues(...args),
  captureInputsSummary: (...args) => capture.captureInputsSummary(...args),
  restartCapture: (...args) => capture.restartCapture(...args),
  providerProbeSummary: (...args) => provider.providerProbeSummary(...args),
  providerIssues: (...args) => provider.providerIssues(...args),
  broadcastState,
});

update = createUpdateService({
  app,
  autoUpdater,
  updaterLoadError,
  fs,
  path,
  UPDATE_FEED: runtime.UPDATE_FEED,
  UPDATE_STATE_FILE: runtime.UPDATE_STATE_FILE,
  UPDATE_STARTUP_DELAY_MS: runtime.UPDATE_STARTUP_DELAY_MS,
  UPDATE_CHECK_INTERVAL_MS: runtime.UPDATE_CHECK_INTERVAL_MS,
  appendShellLog,
  notify: notifications.notify,
  broadcastState,
});

app.whenReady().then(() => {
  if (process.platform === 'darwin' && app.dock) app.dock.hide();

  runtime.ensureExtensionsDir();

  trayPopover.createTray();

  // Tray button bindings:
  //   left-click -> popover (rich UI)
  //   right-click -> minimal context menu (Quit, log access)
  // popUpContextMenu handles its own dismissal; popover dismisses on blur.
  bindIpc({
    ipcMain,
    shell,
    app,
    runtime: {
      ...runtime,
      broadcastState,
      runAlvumJson: cliRunner.runAlvumJson,
    },
    trayPopover,
    capture,
    briefing,
    provider,
    connector,
    update,
    scheduler,
    tail,
  });
  trayPopover.createPopover();
  trayPopover.bindTrayEvents();

  rebuildTrayMenu();
  notifications.startNotifyQueueWatcher();
  capture.startPowerWatcher();
  capture.startPermissionWatcher();
  provider.startProviderWatcher();
  update.startUpdateWatcher();
  briefing.startProgressWatcher();
  briefing.startEventsWatcher();

  const launchIntent = runtime.consumeLaunchIntent();
  scheduler.start(launchIntent);
  if (launchIntent.skip_capture_autostart || launchIntent.skipCaptureAutostart) {
    appendShellLog(`[capture] startup auto-start skipped by launch intent (${launchIntent.source || 'unknown'})`);
  } else {
    capture.reconcileCaptureProcess({ userInitiated: false });
  }
});

app.on('before-quit', () => { app.isQuitting = true; });

app.on('before-quit', () => {
  capture.shutdown();
  if (scheduler) scheduler.shutdown();
});

app.on('window-all-closed', (e) => {
  // Background agent: keep running when no windows exist.
  e.preventDefault?.();
});
