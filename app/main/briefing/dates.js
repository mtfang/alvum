function todayStamp() {
  // Local-day YYYY-MM-DD so it matches briefing.sh's `date +%Y-%m-%d`.
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const dd = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${dd}`;
}

function dateAddDays(stamp, days) {
  const [y, m, d] = stamp.split('-').map(Number);
  const date = new Date(y, m - 1, d + days);
  const yy = date.getFullYear();
  const mm = String(date.getMonth() + 1).padStart(2, '0');
  const dd = String(date.getDate()).padStart(2, '0');
  return `${yy}-${mm}-${dd}`;
}

function localMidnightUtc(stamp) {
  const [y, m, d] = stamp.split('-').map(Number);
  return new Date(y, m - 1, d).toISOString().replace(/\.\d{3}Z$/, 'Z');
}

function validDateStamp(date) {
  return /^\d{4}-\d{2}-\d{2}$/.test(date || '');
}

module.exports = {
  todayStamp,
  dateAddDays,
  localMidnightUtc,
  validDateStamp,
};
