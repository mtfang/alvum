function createTailService({ fs, SHELL_LOG, BRIEFING_LOG, EVENTS_FILE }) {
function readTail(file, maxBytes = 80 * 1024) {
  try {
    if (!fs.existsSync(file)) return '';
    const stat = fs.statSync(file);
    const start = Math.max(0, stat.size - maxBytes);
    const fd = fs.openSync(file, 'r');
    const buf = Buffer.alloc(stat.size - start);
    fs.readSync(fd, buf, 0, buf.length, start);
    fs.closeSync(fd);
    return buf.toString('utf8');
  } catch (e) {
    return `Could not read log: ${e.message}`;
  }
}

function logSnapshot(kind) {
  const files = {
    shell: SHELL_LOG,
    briefing: BRIEFING_LOG,
    pipeline: EVENTS_FILE,
  };
  const file = files[kind] || SHELL_LOG;
  return { kind, file, text: readTail(file) };
}

  return { readTail, logSnapshot };
}

module.exports = { createTailService };
