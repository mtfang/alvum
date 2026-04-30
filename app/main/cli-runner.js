function createCliRunner({ spawn, resolveBinary, alvumSpawnEnv }) {
function runAlvumText(args, timeoutMs = 5000) {
  return new Promise((resolve) => {
    const bin = resolveBinary();
    if (!bin) return resolve({ ok: false, error: 'alvum binary not found' });
    const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'], env: alvumSpawnEnv() });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => { stdout += d.toString(); });
    child.stderr.on('data', (d) => { stderr += d.toString(); });
    const timer = setTimeout(() => child.kill('SIGTERM'), timeoutMs);
    child.on('close', (code, signal) => {
      clearTimeout(timer);
      resolve({ ok: code === 0, code, signal, stdout, stderr, error: code === 0 ? null : (stderr || stdout || `code ${code}`) });
    });
    child.on('error', (e) => {
      clearTimeout(timer);
      resolve({ ok: false, error: e.message });
    });
  });
}

function runAlvumJson(args, timeoutMs, stdinText = null) {
  return new Promise((resolve) => {
    const bin = resolveBinary();
    if (!bin) return resolve({ error: 'alvum binary not found' });
    const child = spawn(bin, args, {
      stdio: [stdinText == null ? 'ignore' : 'pipe', 'pipe', 'pipe'],
      env: alvumSpawnEnv(),
    });
    let stdout = '';
    let stderr = '';
    let timedOut = false;
    if (stdinText != null) {
      child.stdin.end(stdinText);
    }
    child.stdout.on('data', (d) => { stdout += d.toString(); });
    child.stderr.on('data', (d) => { stderr += d.toString(); });
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill('SIGTERM');
    }, timeoutMs);
    child.on('close', (code, signal) => {
      clearTimeout(timer);
      if (timedOut) {
        return resolve({
          error: `command timed out after ${Math.round(timeoutMs / 1000)}s`,
          status: 'timeout',
          code,
          signal,
          stdout,
          stderr,
        });
      }
      if (!stdout.trim()) {
        return resolve({
          error: code === 0 ? 'command produced no JSON output' : `command exited ${code || signal || 'unknown'} with no JSON output`,
          status: 'empty_output',
          code,
          signal,
          stdout,
          stderr,
        });
      }
      try {
        const parsed = JSON.parse(stdout);
        if (code && !parsed.error) {
          parsed.error = `command exited ${code}`;
          parsed.code = code;
          parsed.stderr = stderr;
        }
        resolve(parsed);
      } catch (e) {
        resolve({
          error: `malformed JSON output: ${e.message}`,
          status: 'malformed_json',
          code,
          signal,
          stdout,
          stderr,
        });
      }
    });
    child.on('error', (e) => {
      clearTimeout(timer);
      resolve({ error: e.message, status: 'spawn_error', stdout, stderr });
    });
  });
}

  return { runAlvumText, runAlvumJson };
}

module.exports = { createCliRunner };
