function createShellLogger({ fs, LOG_DIR, SHELL_LOG }) {
  let fallbackConsoleError = console.error.bind(console);

  function appendShellLog(line) {
    try {
      fs.mkdirSync(LOG_DIR, { recursive: true });
      fs.appendFileSync(SHELL_LOG, `${new Date().toISOString()} ${line}\n`);
    } catch (e) {
      fallbackConsoleError('appendShellLog failed', e);
    }
  }

  function fmtArgs(args) {
    return args.map((a) => {
      if (typeof a === 'string') return a;
      try { return JSON.stringify(a); } catch { return String(a); }
    }).join(' ');
  }

  function installConsoleCapture() {
    const origConsoleLog = console.log.bind(console);
    const origConsoleError = console.error.bind(console);
    const origConsoleWarn = console.warn.bind(console);
    fallbackConsoleError = origConsoleError;

    console.log = (...args) => { appendShellLog(`[log] ${fmtArgs(args)}`); origConsoleLog(...args); };
    console.error = (...args) => { appendShellLog(`[err] ${fmtArgs(args)}`); origConsoleError(...args); };
    console.warn = (...args) => { appendShellLog(`[warn] ${fmtArgs(args)}`); origConsoleWarn(...args); };

    process.on('uncaughtException', (e) => {
      appendShellLog(`[uncaughtException] ${e && e.stack ? e.stack : e}`);
    });
    process.on('unhandledRejection', (reason) => {
      appendShellLog(`[unhandledRejection] ${reason && reason.stack ? reason.stack : reason}`);
    });
  }

  return { appendShellLog, installConsoleCapture };
}

module.exports = { createShellLogger };
