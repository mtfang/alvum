// Preload bridge for the tray popover. Exposes a tiny `window.alvum`
// API so the renderer never gets direct ipcRenderer / Node access —
// keeps `contextIsolation: true` honest and the surface small enough
// to audit.
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('alvum', {
  // Subscribe to state pushes from main (capture state, briefing flag,
  // capture stats text). Main re-broadcasts on every transition, so the
  // popover always opens with fresh state — no manual polling.
  onState:    (cb) => ipcRenderer.on('alvum:state',    (_e, s) => cb(s)),
  onProgress: (cb) => ipcRenderer.on('alvum:progress', (_e, p) => cb(p)),

  // Pipeline-events stream (richer per-stage / per-LLM-call signal).
  // One callback per event line — the renderer maintains its own
  // recent-events buffer for the live panel.
  onEvent:    (cb) => ipcRenderer.on('alvum:event',    (_e, evt) => cb(evt)),

  // Pull state on initial render (covers the case where the popover
  // opens between two state-change events).
  requestState: () => ipcRenderer.send('alvum:request-state'),
  resizePopover: (height) => ipcRenderer.send('alvum:resize-popover', height),

  // Commands. Fire-and-forget — confirmation flows back via state push.
  toggleCapture:  () => ipcRenderer.send('alvum:toggle-capture'),
  captureInputs:  () => ipcRenderer.invoke('alvum:capture-inputs'),
  toggleCaptureInput: (id) => ipcRenderer.invoke('alvum:toggle-capture-input', id),
  captureInputSetSetting: (id, key, value) => ipcRenderer.invoke('alvum:set-capture-input-setting', id, key, value),
  chooseDirectory: (defaultPath) => ipcRenderer.invoke('alvum:choose-directory', defaultPath),
  startBriefing:  () => ipcRenderer.send('alvum:start-briefing'),
  startBriefingDate: (date) => ipcRenderer.invoke('alvum:start-briefing-date', date),
  briefingCalendarMonth: (month) => ipcRenderer.invoke('alvum:briefing-calendar-month', month),
  openBriefing:   () => ipcRenderer.send('alvum:open-briefing'),
  openBriefingDate: (date) => ipcRenderer.invoke('alvum:open-briefing-date', date),
  readBriefingDate: (date) => ipcRenderer.invoke('alvum:read-briefing-date', date),
  briefingRunLogDate: (date) => ipcRenderer.invoke('alvum:briefing-run-log', date),
  openBriefingRunLogs: (date) => ipcRenderer.invoke('alvum:open-briefing-run-logs', date),
  decisionGraphDate: (date) => ipcRenderer.invoke('alvum:decision-graph-date', date),
  synthesisProfile: () => ipcRenderer.invoke('alvum:synthesis-profile'),
  synthesisProfileSave: (profile) => ipcRenderer.invoke('alvum:synthesis-profile-save', profile),
  synthesisProfileSuggestions: () => ipcRenderer.invoke('alvum:synthesis-profile-suggestions'),
  synthesisProfilePromote: (id) => ipcRenderer.invoke('alvum:synthesis-profile-promote', id),
  synthesisProfileIgnore: (id) => ipcRenderer.invoke('alvum:synthesis-profile-ignore', id),
  openBriefingLog:() => ipcRenderer.send('alvum:open-briefing-log'),
  openCaptureDir: () => ipcRenderer.send('alvum:open-capture-dir'),
  openShellLog:   () => ipcRenderer.send('alvum:open-shell-log'),
  openPermissionSettings: (permission) => ipcRenderer.invoke('alvum:open-permission-settings', permission),
  quit:           () => ipcRenderer.send('alvum:quit'),

  // Provider config + validation. Main owns the provider snapshot and
  // pushes it through alvum:state; these calls only mutate or ping one
  // provider.
  providerList:      ()             => ipcRenderer.invoke('alvum:provider-list'),
  providerTest:      (name)         => ipcRenderer.invoke('alvum:provider-test', name),
  providerSetActive: (name)         => ipcRenderer.invoke('alvum:provider-set-active', name),
  providerSetEnabled: (name, enabled) => ipcRenderer.invoke('alvum:provider-set-enabled', name, enabled),
  providerConfigure: (name, payload) => ipcRenderer.invoke('alvum:provider-configure', name, payload),
  providerModels:    (name)         => ipcRenderer.invoke('alvum:provider-models', name),
  providerInstallModel: (name, model) => ipcRenderer.invoke('alvum:provider-install-model', name, model),
  installWhisperModel: ()            => ipcRenderer.invoke('alvum:install-whisper-model'),
  providerSetup:     (name, action) => ipcRenderer.invoke('alvum:provider-setup', name, action),
  updateCheck:       ()             => ipcRenderer.invoke('alvum:update-check'),
  updateInstall:     ()             => ipcRenderer.invoke('alvum:update-install'),
  logSnapshot:       (kind)         => ipcRenderer.invoke('alvum:log-snapshot', kind),

  // External extension packages. The CLI remains the source of truth;
  // the popover only renders structured status and sends enable/disable
  // package commands back through main.
  extensionList:      ()            => ipcRenderer.invoke('alvum:extension-list'),
  extensionSetEnabled:(id, enabled) => ipcRenderer.invoke('alvum:extension-set-enabled', id, enabled),
  extensionDoctor:    ()            => ipcRenderer.invoke('alvum:extension-doctor'),
  openExtensionsDir:  ()            => ipcRenderer.invoke('alvum:open-extensions-dir'),

  // User-facing connector management. This is the menu-bar contract;
  // extension package APIs stay available for package/admin detail.
  connectorList:      ()            => ipcRenderer.invoke('alvum:connector-list'),
  connectorSetEnabled:(id, enabled) => ipcRenderer.invoke('alvum:connector-set-enabled', id, enabled),
  connectorProcessorSetSetting: (component, key, value) => ipcRenderer.invoke('alvum:set-connector-processor-setting', component, key, value),
  doctor:             ()            => ipcRenderer.invoke('alvum:doctor'),

  // Lifecycle event — main fires this when the popover becomes visible
  // (tray-icon click). The renderer asks for the latest pushed state.
  onPopoverShow: (cb) => ipcRenderer.on('alvum:popover-show', () => cb()),
});
