function createBriefingCalendar({
  fs,
  path,
  BRIEFINGS_DIR,
  todayStamp,
  artifactSummaryForDate,
  readBriefingFailure,
  latestBriefingRunInfo,
}) {
  function briefingDayInfo(date) {
    const briefingPath = path.join(BRIEFINGS_DIR, date, 'briefing.md');
    const artifacts = artifactSummaryForDate(date);
    const failure = readBriefingFailure(date);
    const hasBriefing = fs.existsSync(briefingPath);
    const hasCapture = artifacts.files > 0;
    const latestRun = latestBriefingRunInfo(date);
    return {
      date,
      hasCapture,
      hasBriefing,
      artifacts: artifacts.summary,
      status: hasBriefing ? 'success' : (failure ? 'failed' : (hasCapture ? 'captured' : 'empty')),
      failure,
      latestRun,
    };
  }

  function briefingCalendarMonth(month) {
    const today = todayStamp();
    const monthStamp = /^\d{4}-\d{2}$/.test(month || '') ? month : today.slice(0, 7);
    const [y, m] = monthStamp.split('-').map(Number);
    const first = new Date(y, m - 1, 1);
    const start = new Date(y, m - 1, 1 - first.getDay());
    const days = [];
    for (let i = 0; i < 42; i += 1) {
      const d = new Date(start.getFullYear(), start.getMonth(), start.getDate() + i);
      const date = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
      days.push({
        ...briefingDayInfo(date),
        inMonth: date.slice(0, 7) === monthStamp,
        isToday: date === today,
      });
    }
    return {
      month: monthStamp,
      label: first.toLocaleString(undefined, { month: 'long', year: 'numeric' }),
      today,
      days,
    };
  }

  return {
    briefingDayInfo,
    briefingCalendarMonth,
  };
}

module.exports = { createBriefingCalendar };
