function createArtifactStore({
  fs,
  path,
  CAPTURE_DIR,
  BRIEFINGS_DIR,
  todayStamp,
  dateAddDays,
}) {
  function formatBytes(bytes) {
    if (!bytes) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
      value /= 1024;
      unit += 1;
    }
    const digits = value >= 10 || unit === 0 ? 0 : 1;
    return `${value.toFixed(digits)} ${units[unit]}`;
  }

  function scanFileStats(root) {
    const totals = { files: 0, bytes: 0, byExt: new Map() };
    function walk(dir) {
      let entries;
      try {
        entries = fs.readdirSync(dir, { withFileTypes: true });
      } catch {
        return;
      }
      for (const entry of entries) {
        if (entry.name === '.DS_Store') continue;
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
          walk(full);
          continue;
        }
        if (!entry.isFile()) continue;
        let size = 0;
        try {
          size = fs.statSync(full).size;
        } catch {
          size = 0;
        }
        const ext = (path.extname(entry.name).slice(1) || 'file').toLowerCase();
        const current = totals.byExt.get(ext) || { files: 0, bytes: 0 };
        current.files += 1;
        current.bytes += size;
        totals.byExt.set(ext, current);
        totals.files += 1;
        totals.bytes += size;
      }
    }
    walk(root);
    return totals;
  }

  function artifactSummaryForDate(stamp) {
    const dir = path.join(CAPTURE_DIR, stamp);
    const stats = scanFileStats(dir);
    const detail = [...stats.byExt.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([ext, v]) => `${ext}: ${v.files} files / ${formatBytes(v.bytes)}`)
      .join('\n');
    return {
      date: stamp,
      files: stats.files,
      bytes: stats.bytes,
      summary: `${stats.files} files · ${formatBytes(stats.bytes)}`,
      detail: detail || 'No capture artifacts for today',
    };
  }

  function pendingBriefingCatchup() {
    const today = todayStamp();
    try {
      if (!fs.existsSync(CAPTURE_DIR)) return { count: 0, dates: [] };
      const dates = fs.readdirSync(CAPTURE_DIR)
        .filter((name) => /^\d{4}-\d{2}-\d{2}$/.test(name))
        .filter((name) => name < today)
        .filter((name) => {
          const briefingPath = path.join(BRIEFINGS_DIR, name, 'briefing.md');
          if (fs.existsSync(briefingPath)) return false;
          return artifactSummaryForDate(name).files > 0;
        })
        .sort();
      return { count: dates.length, dates };
    } catch {
      return { count: 0, dates: [] };
    }
  }

  function latestBriefingInfo() {
    try {
      if (!fs.existsSync(BRIEFINGS_DIR)) return null;
      const entries = fs.readdirSync(BRIEFINGS_DIR)
        .filter((name) => /^\d{4}-\d{2}-\d{2}$/.test(name))
        .map((date) => {
          const file = path.join(BRIEFINGS_DIR, date, 'briefing.md');
          if (!fs.existsSync(file)) return null;
          const stat = fs.statSync(file);
          return { date, path: file, mtimeMs: stat.mtimeMs, mtime: new Date(stat.mtimeMs).toLocaleString() };
        })
        .filter(Boolean)
        .sort((a, b) => b.date.localeCompare(a.date));
      return entries[0] || null;
    } catch {
      return null;
    }
  }

  function recentBriefingTargets() {
    const today = todayStamp();
    const wanted = new Set([today, dateAddDays(today, -1)]);
    try {
      if (fs.existsSync(CAPTURE_DIR)) {
        for (const name of fs.readdirSync(CAPTURE_DIR)) {
          if (/^\d{4}-\d{2}-\d{2}$/.test(name)) wanted.add(name);
        }
      }
    } catch {
      // Keep the Today/Yesterday fallback list.
    }
    return [...wanted]
      .sort((a, b) => b.localeCompare(a))
      .slice(0, 10)
      .map((date) => {
        const captureDir = path.join(CAPTURE_DIR, date);
        const briefingPath = path.join(BRIEFINGS_DIR, date, 'briefing.md');
        const artifacts = artifactSummaryForDate(date);
        return {
          date,
          label: date === today ? 'Today' : (date === dateAddDays(today, -1) ? 'Yesterday' : date),
          hasCapture: artifacts.files > 0,
          hasBriefing: fs.existsSync(briefingPath),
          artifacts: artifacts.summary,
          captureDir,
        };
      });
  }

  function captureStats() {
    // Cheap on-demand counts so the popover always shows fresh numbers
    // without a long-running watcher. Failures degrade to empty stats.
    try {
      return artifactSummaryForDate(todayStamp());
    } catch {
      return { date: todayStamp(), files: 0, bytes: 0, summary: '0 files · 0 B', detail: 'Capture stats unavailable' };
    }
  }

  return {
    formatBytes,
    scanFileStats,
    artifactSummaryForDate,
    pendingBriefingCatchup,
    latestBriefingInfo,
    recentBriefingTargets,
    captureStats,
  };
}

module.exports = { createArtifactStore };
