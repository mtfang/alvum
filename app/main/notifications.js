function createNotificationService({ nativeImage, Notification, path, fs, appRoot, NOTIFY_QUEUE, appendShellLog }) {
// Notification icon as ATTACHMENT image. Self-signed LSUIElement
// Electron apps don't get a sender-side icon (left of the toast) —
// that requires a proper Apple Developer ID signature or a compiled
// Assets.car (needs Xcode). Passing this here renders on the right
// side, which still surfaces the alvum brand on every notification.
const APP_ICON = nativeImage.createFromPath(path.join(appRoot, 'assets', 'icon.png'));

function notify(title, body) {
  try {
    new Notification({ title, body, icon: APP_ICON }).show();
  } catch (e) {
    console.error('notify failed', e);
  }
}

// External-notification queue. Out-of-process tools (capture.sh toggle,
// menu-bar.sh, briefing.sh, …) append a JSON line per notification; we
// poll the file and fan each line into the bundle's Electron Notification
// API so the system shows the alvum logo instead of the AppleScript icon
// `osascript display notification` is hard-locked to since Big Sur.
const NOTIFY_TTL_MS = 60 * 1000;          // ignore lines older than this on startup
const NOTIFY_POLL_MS = 500;
let notifyCursor = 0;

function startNotifyQueueWatcher() {
  fs.mkdirSync(path.dirname(NOTIFY_QUEUE), { recursive: true });
  // Seed the cursor at current size so a backlog of stale entries (e.g.
  // a long-running queue from before this app instance launched) doesn't
  // dump every old notification at once. The TTL filter further protects
  // against time-skewed lines that may race in during the first poll.
  if (fs.existsSync(NOTIFY_QUEUE)) {
    notifyCursor = fs.statSync(NOTIFY_QUEUE).size;
  } else {
    fs.writeFileSync(NOTIFY_QUEUE, '');
  }
  setInterval(pollNotifyQueue, NOTIFY_POLL_MS);
}

function pollNotifyQueue() {
  let stat;
  try {
    stat = fs.statSync(NOTIFY_QUEUE);
  } catch {
    return;                                // file vanished; nothing to do
  }
  if (stat.size === notifyCursor) return;
  if (stat.size < notifyCursor) {           // truncated externally; resync
    notifyCursor = 0;
  }

  let chunk;
  try {
    const fd = fs.openSync(NOTIFY_QUEUE, 'r');
    const len = stat.size - notifyCursor;
    const buf = Buffer.alloc(len);
    fs.readSync(fd, buf, 0, len, notifyCursor);
    fs.closeSync(fd);
    chunk = buf.toString('utf8');
    notifyCursor = stat.size;
  } catch (e) {
    appendShellLog(`[notify-queue] read failed: ${e.message}`);
    return;
  }

  const now = Date.now();
  for (const line of chunk.split('\n')) {
    if (!line.trim()) continue;
    let payload;
    try {
      payload = JSON.parse(line);
    } catch (e) {
      appendShellLog(`[notify-queue] bad JSON: ${e.message} line=${line}`);
      continue;
    }
    // Drop ancient lines so a long-stopped Alvum.app doesn't burst
    // weeks of notifications when it relaunches.
    if (payload.ts && now - payload.ts > NOTIFY_TTL_MS) continue;
    notify(payload.title || 'Alvum', payload.body || '');
  }
}

  return { notify, startNotifyQueueWatcher };
}

module.exports = { createNotificationService };
