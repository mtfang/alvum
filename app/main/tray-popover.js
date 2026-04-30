function createTrayPopover({
  app,
  Tray,
  Menu,
  BrowserWindow,
  screen,
  shell,
  nativeImage,
  fs,
  path,
  appRoot,
  SHELL_LOG,
  BRIEFING_LOG,
  getCaptureState,
  broadcastState,
}) {
  let tray = null;

function trayIcon() {
  // Idle icon: rendered as a template image so macOS strips the source
  // colour and tints to black (light menu bar) or white (dark menu bar)
  // to match the rest of the bar. Resized to 22×22 logical.
  const diskIcon = path.join(appRoot, 'assets', 'tray-icon.png');
  if (fs.existsSync(diskIcon)) {
    const img = nativeImage.createFromPath(diskIcon).resize({ width: 22, height: 22 });
    if (!img.isEmpty()) {
      img.setTemplateImage(true);
      return img;
    }
  }
  // Last-resort placeholder so startup never fails on a missing asset.
  const placeholder = Buffer.from(
    'iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAQAAAC1+jfqAAAAL0lEQVR42mNkIAAYiVLCwMDw'
      + 'BzOBEdcwDO4ECmAEd4LFYCrATGsQgzAOXg0AAFc8Aew8p+a7AAAAAElFTkSuQmCC',
    'base64'
  );
  const img = nativeImage.createFromBuffer(placeholder);
  img.setTemplateImage(true);
  return img;
}

// Active icon: white-logo variant with a green recording dot composited
// in. Template mode strips colour, so we ship a non-template asset and
// the menu bar's natural dark substrate keeps the white legible. Falls
// back to the idle template icon if the active asset is missing on disk.
function trayIconActive() {
  const diskIcon = path.join(appRoot, 'assets', 'tray-icon-active.png');
  if (fs.existsSync(diskIcon)) {
    const img = nativeImage.createFromPath(diskIcon).resize({ width: 22, height: 22 });
    if (!img.isEmpty()) {
      // Explicitly NOT a template image — the green must survive untinted.
      img.setTemplateImage(false);
      return img;
    }
  }
  return trayIcon();
}

// Apply the right icon for the current capture state. Called on every
// state transition (start/stop/restart) and on system theme changes.
function applyTrayIcon() {
  if (!tray) return;
  tray.setImage(getCaptureState().running ? trayIconActive() : trayIcon());
}

// Right-click fallback context menu — minimal "nuclear" options so the
// user is never trapped if the popover renderer breaks. Left-click goes
// to the popover; right-click gets just Quit + log access.
function rightClickMenu() {
  return Menu.buildFromTemplate([
    { label: 'Alvum', enabled: false },
    {
      label: getCaptureState().running
        ? `● running since ${getCaptureState().startedAt.toLocaleTimeString()}`
        : '○ stopped',
      enabled: false,
    },
    { type: 'separator' },
    { label: 'Open shell log', click: () => shell.openPath(SHELL_LOG) },
    { label: 'Open briefing log', click: () => shell.openPath(BRIEFING_LOG) },
    { type: 'separator' },
    { label: 'Quit alvum', click: () => app.quit() },
  ]);
}

// Refresh the tooltip + icon. The full UI lives in the popover; the
// tray itself only carries glanceable state via icon + tooltip.
function rebuildTrayMenu() {
  const state = getCaptureState();
  const status = state.running
    ? `● Capture running (started ${state.startedAt.toLocaleTimeString()})`
    : '○ Capture stopped';
  tray.setToolTip(status);
  applyTrayIcon();
  broadcastState();
}

// === Popover BrowserWindow ============================================
//
// Standard menu-bar-app pattern: a frameless transparent BrowserWindow
// positioned next to the tray icon, shown on click and dismissed on
// blur. Replaces the previous ContextMenu so we can render real UI
// (progress bar, stage list, vibrancy) instead of NSMenuItem-only text.

let popover = null;
const POPOVER_W = 320;
const POPOVER_MIN_H = 300;
const POPOVER_MAX_H = 640;
let popoverHeight = POPOVER_MIN_H;
let popoverResizeTarget = POPOVER_MIN_H;
let popoverResizeTimer = null;

function createPopover() {
  popover = new BrowserWindow({
    width: POPOVER_W,
    height: popoverHeight,
    show: false,
    frame: false,
    transparent: true,
    resizable: false,
    movable: false,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    skipTaskbar: true,
    alwaysOnTop: true,
    hasShadow: true,
    vibrancy: 'menu',                 // native macOS popover translucency
    visualEffectState: 'active',
    webPreferences: {
      preload: path.join(appRoot, 'popover-preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  });
  popover.loadFile(path.join(appRoot, 'popover.html'));
  popover.on('blur', () => {
    if (popover && !popover.isDestroyed() && !popover.webContents.isDevToolsOpened()) popover.hide();
  });
  // Hide instead of close on the window's own X equivalent so we keep
  // a single instance for the lifetime of the app.
  popover.on('close', (e) => {
    if (!app.isQuitting) {
      e.preventDefault();
      popover.hide();
    }
  });
}

function popoverWorkArea() {
  const trayBounds = tray.getBounds();
  const display = screen.getDisplayNearestPoint({
    x: Math.round(trayBounds.x + trayBounds.width / 2),
    y: trayBounds.y,
  });
  return { trayBounds, work: display.workArea };
}

function clampPopoverHeight(height) {
  const { work } = popoverWorkArea();
  const screenMax = Math.max(POPOVER_MIN_H, work.height - 12);
  const requested = Number.isFinite(height) ? Math.ceil(height) : POPOVER_MIN_H;
  return Math.max(POPOVER_MIN_H, Math.min(requested, POPOVER_MAX_H, screenMax));
}

function positionPopover() {
  if (!popover || popover.isDestroyed() || !tray) return;
  const { trayBounds, work } = popoverWorkArea();
  let x = Math.round(trayBounds.x + trayBounds.width / 2 - POPOVER_W / 2);
  let y = Math.round(trayBounds.y + trayBounds.height + 6);
  x = Math.max(work.x + 6, Math.min(x, work.x + work.width - POPOVER_W - 6));
  y = Math.max(work.y + 6, Math.min(y, work.y + work.height - popoverHeight - 6));
  popover.setPosition(x, y, false);
}

function stopPopoverResizeAnimation() {
  if (!popoverResizeTimer) return;
  clearInterval(popoverResizeTimer);
  popoverResizeTimer = null;
}

function applyPopoverHeight(height) {
  if (!popover || popover.isDestroyed()) return;
  popoverHeight = height;
  popover.setSize(POPOVER_W, popoverHeight, false);
  if (popover.isVisible()) positionPopover();
}

function resizePopover(height) {
  if (!popover || popover.isDestroyed() || !tray) return;
  const nextHeight = clampPopoverHeight(height);
  if (popoverResizeTimer && nextHeight === popoverResizeTarget) return;
  popoverResizeTarget = nextHeight;
  if (nextHeight === popoverHeight) {
    stopPopoverResizeAnimation();
    if (popover.isVisible()) positionPopover();
    return;
  }
  if (!popover.isVisible()) {
    stopPopoverResizeAnimation();
    applyPopoverHeight(nextHeight);
    return;
  }
  stopPopoverResizeAnimation();
  const startHeight = popoverHeight;
  const delta = nextHeight - startHeight;
  const startedAt = Date.now();
  const durationMs = 180;
  popoverResizeTimer = setInterval(() => {
    if (!popover || popover.isDestroyed()) {
      stopPopoverResizeAnimation();
      return;
    }
    const t = Math.min(1, (Date.now() - startedAt) / durationMs);
    const eased = 1 - Math.pow(1 - t, 3);
    applyPopoverHeight(Math.round(startHeight + delta * eased));
    if (t >= 1) stopPopoverResizeAnimation();
  }, 16);
}

function togglePopover() {
  if (!popover || popover.isDestroyed()) return;
  if (popover.isVisible()) {
    popover.hide();
    return;
  }
  positionPopover();
  popover.show();
  popover.focus();
  // Tell the renderer to refresh anything that depends on the
  // outside-world state (provider availability, capture stats, etc.).
  popover.webContents.send('alvum:popover-show');
}

  function createTray() {
    tray = new Tray(trayIcon());
    return tray;
  }

  function bindTrayEvents() {
    if (!tray) return;
    tray.on('click', () => togglePopover());
    tray.on('right-click', () => tray.popUpContextMenu(rightClickMenu()));
  }

  function send(channel, payload) {
    if (!popover || popover.isDestroyed()) return false;
    popover.webContents.send(channel, payload);
    return true;
  }

  function getPopover() {
    return popover;
  }

  return {
    createTray,
    bindTrayEvents,
    trayIcon,
    trayIconActive,
    applyTrayIcon,
    rightClickMenu,
    rebuildTrayMenu,
    createPopover,
    popoverWorkArea,
    clampPopoverHeight,
    positionPopover,
    stopPopoverResizeAnimation,
    applyPopoverHeight,
    resizePopover,
    togglePopover,
    send,
    getPopover,
  };
}

module.exports = { createTrayPopover };
