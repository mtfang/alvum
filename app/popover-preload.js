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

  // Pull state on initial render (covers the case where the popover
  // opens between two state-change events).
  requestState: () => ipcRenderer.send('alvum:request-state'),

  // Commands. Fire-and-forget — confirmation flows back via state push.
  toggleCapture:  () => ipcRenderer.send('alvum:toggle-capture'),
  startBriefing:  () => ipcRenderer.send('alvum:start-briefing'),
  openBriefing:   () => ipcRenderer.send('alvum:open-briefing'),
  openBriefingLog:() => ipcRenderer.send('alvum:open-briefing-log'),
  openCaptureDir: () => ipcRenderer.send('alvum:open-capture-dir'),
  openShellLog:   () => ipcRenderer.send('alvum:open-shell-log'),
  quit:           () => ipcRenderer.send('alvum:quit'),

  // Provider config + validation. Request/response (invoke) so the
  // renderer can await the parsed JSON directly without juggling
  // pending callbacks.
  providerList:      ()             => ipcRenderer.invoke('alvum:provider-list'),
  providerSetActive: (name)         => ipcRenderer.invoke('alvum:provider-set-active', name),

  // Lifecycle event — main fires this when the popover becomes visible
  // (tray-icon click). Lets the renderer refresh ambient state like
  // provider availability without polling.
  onPopoverShow: (cb) => ipcRenderer.on('alvum:popover-show', () => cb()),
});
