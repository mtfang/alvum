const os = require('node:os');

function createProviderService({
  fs,
  path,
  shell,
  spawn,
  PROVIDER_HEALTH_FILE,
  appendShellLog,
  notify,
  runAlvumJson,
  alvumSpawnEnv,
  connectorList,
  broadcastState,
}) {
  function readJsonFileIfPresent(file) {
    if (!fs.existsSync(file)) return null;
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  }

function providerDiagnosticSnapshot(summary = providerProbeCache) {
  if (!summary || summary.error || !Array.isArray(summary.providers)) {
    return summary ? { error: summary.error || null } : null;
  }
  return {
    configured: summary.configured || 'auto',
    auto_resolved: summary.auto_resolved || null,
    connected: summary.connected || 0,
    checked_at: summary.checked_at || null,
    providers: summary.providers.map((provider) => ({
      name: provider.name,
      enabled: provider.enabled !== false,
      active: !!provider.active,
      available: !!provider.available,
      status: provider.ui ? provider.ui.status : null,
      reason: provider.ui ? provider.ui.reason : null,
      test_status: provider.test ? provider.test.status : null,
      test_ok: provider.test ? !!provider.test.ok : false,
      test_error: provider.test ? provider.test.error || null : null,
      resolved_model: provider.test ? provider.test.resolved_model || null : null,
      model_source: provider.test ? provider.test.model_source || null : null,
      timeout_secs: provider.test ? provider.test.timeout_secs || null : null,
      backend_hint: provider.test ? provider.test.backend_hint || null : null,
      recommended_setup_actions: provider.test && Array.isArray(provider.test.recommended_setup_actions)
        ? provider.test.recommended_setup_actions
        : [],
    })),
  };
}

let providerProbeCache = null;
let providerProbeCacheAt = 0;
let providerProbeCacheLive = false;
let providerWatchTimer = null;
let lastProviderIssueKey = '';
let currentProviderIssue = null;
let providerRuntimeStats = {};
const PROVIDER_PROBE_TTL_MS = 2 * 60 * 1000;
const PROVIDER_WATCH_MS = 3 * 60 * 1000;
const PROVIDER_BACKGROUND_TEST_TIMEOUT_MS = 30000;
const PROVIDER_MANUAL_TEST_TIMEOUT_MS = 120000;
const PROVIDER_BACKGROUND_TEST_TIMEOUT_SECS = '25';
const PROVIDER_MANUAL_TEST_TIMEOUT_SECS = '90';

function numeric(value, fallback = 0) {
  const n = Number(value);
  return Number.isFinite(n) ? n : fallback;
}

function providerRuntimeRecord(name) {
  const provider = String(name || '').trim();
  if (!provider) return null;
  providerRuntimeStats[provider] = providerRuntimeStats[provider] || {
    provider,
    active_calls: 0,
    calls_started: 0,
    calls_finished: 0,
    calls_failed: 0,
    prompt_chars: 0,
    response_chars: 0,
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 0,
    input_tokens_estimate: 0,
    output_tokens_estimate: 0,
    total_tokens_estimate: 0,
    latency_ms: 0,
    last_call_site: null,
    last_status: null,
    last_started_at: null,
    last_completed_at: null,
    last_latency_ms: null,
    last_tokens_per_sec: null,
    last_token_source: null,
    updated_at: null,
  };
  return providerRuntimeStats[provider];
}

function recordProviderEvent(evt) {
  if (!evt || (evt.kind !== 'llm_call_start' && evt.kind !== 'llm_call_end')) return;
  const stats = providerRuntimeRecord(evt.provider);
  if (!stats) return;
  const now = new Date().toISOString();
  stats.updated_at = now;
  stats.last_call_site = evt.call_site || stats.last_call_site;
  if (evt.kind === 'llm_call_start') {
    stats.calls_started += 1;
    stats.active_calls += 1;
    stats.last_started_at = now;
    stats.last_status = 'running';
    return;
  }

  stats.calls_finished += 1;
  stats.active_calls = Math.max(0, stats.active_calls - 1);
  if (evt.ok === false) stats.calls_failed += 1;
  stats.prompt_chars += numeric(evt.prompt_chars);
  stats.response_chars += numeric(evt.response_chars);
  stats.input_tokens += numeric(evt.input_tokens);
  stats.output_tokens += numeric(evt.output_tokens);
  stats.total_tokens += numeric(evt.total_tokens);
  stats.input_tokens_estimate += numeric(evt.prompt_tokens_estimate);
  stats.output_tokens_estimate += numeric(evt.response_tokens_estimate);
  stats.total_tokens_estimate += numeric(evt.total_tokens_estimate);
  stats.latency_ms += numeric(evt.latency_ms);
  stats.last_completed_at = now;
  stats.last_latency_ms = numeric(evt.latency_ms, null);
  stats.last_tokens_per_sec = numeric(evt.tokens_per_sec, numeric(evt.tokens_per_sec_estimate, null));
  stats.last_token_source = evt.token_source || (evt.tokens_per_sec ? 'provider' : 'estimated');
  stats.last_status = evt.ok === false ? 'failed' : 'ok';
}

function providerRuntimeStatsSnapshot() {
  const providers = {};
  for (const [name, stats] of Object.entries(providerRuntimeStats)) {
    providers[name] = { ...stats };
  }
  return {
    providers,
    updated_at: new Date().toISOString(),
  };
}

function readProviderHealth() {
  try {
    return readJsonFileIfPresent(PROVIDER_HEALTH_FILE) || {};
  } catch {
    return {};
  }
}

function writeProviderHealth(summary) {
  if (!summary || summary.error || !Array.isArray(summary.providers)) return;
  try {
    fs.mkdirSync(path.dirname(PROVIDER_HEALTH_FILE), { recursive: true });
    const providers = {};
    for (const provider of summary.providers) {
      providers[provider.name] = {
        test: provider.test || null,
        ui: provider.ui || null,
        available: provider.available,
        enabled: provider.enabled,
        active: provider.active,
        checked_at: summary.checked_at || new Date().toISOString(),
      };
    }
    fs.writeFileSync(PROVIDER_HEALTH_FILE, JSON.stringify({
      checked_at: summary.checked_at || new Date().toISOString(),
      configured: summary.configured || 'auto',
      auto_resolved: summary.auto_resolved || null,
      providers,
    }, null, 2));
  } catch (e) {
    appendShellLog(`[providers] failed to write provider health: ${e.message}`);
  }
}

function providerUiStatus(entry, test) {
  if (entry.enabled === false) {
    return { level: 'yellow', status: 'removed', reason: 'removed from Alvum provider list' };
  }
  if (!entry.available) {
    return { level: 'red', status: 'not_setup', reason: entry.auth_hint || 'not configured' };
  }
  if (!test) {
    return { level: 'yellow', status: 'not_checked', reason: 'not checked yet' };
  }
  if (test.ok) {
    return { level: 'green', status: test.status || 'available', reason: 'authenticated and returning tokens' };
  }
  if (test.status === 'timeout') {
    return { level: 'yellow', status: 'timeout', reason: test.error || 'provider probe timed out' };
  }
  return {
    level: 'yellow',
    status: test.status || 'needs_setup',
    reason: test.error || 'probe failed',
  };
}

function providerSelectableForAuto(provider) {
  if (!provider || provider.enabled === false || !provider.available) return false;
  return !!(provider.test && provider.test.ok);
}

function autoProviderName(providers) {
  const match = (providers || []).find(providerSelectableForAuto);
  return match ? match.name : null;
}

function applyProviderAutoSelection(summary) {
  if (!summary || summary.error || !Array.isArray(summary.providers)) return summary;
  if ((summary.configured || 'auto') !== 'auto') return summary;
  const autoResolved = autoProviderName(summary.providers);
  return {
    ...summary,
    auto_resolved: autoResolved,
    providers: summary.providers.map((provider) => ({
      ...provider,
      active: provider.name === autoResolved,
    })),
  };
}

async function providerProbeSummary(force = false, liveProbe = true) {
  const now = Date.now();
  if (!force
      && providerProbeCache
      && now - providerProbeCacheAt < PROVIDER_PROBE_TTL_MS
      && (!liveProbe || providerProbeCacheLive)) {
    return providerProbeCache;
  }
  const previousSummary = providerProbeCache && !providerProbeCache.error ? providerProbeCache : null;
  const persistedHealth = readProviderHealth();
  const previousByName = new Map(
    previousSummary && Array.isArray(previousSummary.providers)
      ? previousSummary.providers.map((provider) => [provider.name, provider])
      : [],
  );
  const data = await runAlvumJson(['providers', 'list'], 30000);
  if (!data || data.error || !Array.isArray(data.providers)) {
    const errorSummary = {
      error: (data && data.error) || 'provider list failed',
      connected: 0,
      total: 0,
      live_checked: false,
      checked_at: new Date().toISOString(),
      providers: [],
    };
    providerProbeCache = errorSummary;
    providerProbeCacheAt = Date.now();
    providerProbeCacheLive = false;
    return errorSummary;
  }
  const providers = await Promise.all(data.providers.map(async (entry) => {
    const previous = previousByName.get(entry.name);
    const persisted = persistedHealth.providers && persistedHealth.providers[entry.name];
    let test = null;
    if (liveProbe && entry.enabled !== false && entry.available) {
      test = await runAlvumJson(['providers', 'test', '--provider', entry.name, '--timeout-secs', PROVIDER_BACKGROUND_TEST_TIMEOUT_SECS], PROVIDER_BACKGROUND_TEST_TIMEOUT_MS);
    } else if (previous && previous.test && entry.enabled !== false && entry.available) {
      test = previous.test;
    } else if (persisted && persisted.test && entry.enabled !== false && entry.available) {
      test = persisted.test;
    }
    return {
      ...entry,
      test,
      usage: previous ? previous.usage : null,
      ui: providerUiStatus(entry, test),
    };
  }));
  const result = applyProviderAutoSelection({
    configured: data.configured,
    auto_resolved: data.auto_resolved,
    connected: providers.filter((p) => p.ui.level === 'green').length,
    total: providers.length,
    live_checked: !!liveProbe || !!(previousSummary && previousSummary.live_checked),
    checked_at: new Date().toISOString(),
    providers,
  });
  providerProbeCache = result;
  providerProbeCacheAt = Date.now();
  providerProbeCacheLive = !!liveProbe;
  writeProviderHealth(result);
  return result;
}

function providerIssues(summary) {
  if (!summary || summary.error) {
    return [{ level: 'warning', message: summary && summary.error ? summary.error : 'Provider check failed.' }];
  }
  const providers = Array.isArray(summary.providers) ? summary.providers : [];
  const enabled = providers.filter((provider) => provider.enabled !== false);
  const configured = summary.configured || 'auto';
  const usable = enabled.filter(providerSelectableForAuto);

  if (enabled.length === 0) {
    return [{ level: 'warning', message: 'No providers are enabled in Alvum.' }];
  }
  if (usable.length === 0) {
    return [{ level: 'warning', message: 'No enabled provider is currently usable.' }];
  }
  if (configured === 'auto') return [];

  const active = providers.find((provider) => provider.name === configured);
  if (!active) {
    return [{ level: 'warning', message: `Configured provider ${configured} is not recognized.` }];
  }
  if (active.enabled === false) {
    return [{ level: 'warning', message: `Configured provider ${configured} is removed from Alvum's provider list.` }];
  }
  if (!active.available) {
    return [{ level: 'warning', message: `Configured provider ${configured} is not detected; ${active.auth_hint || 'check setup'}.` }];
  }
  if (active.test && !active.test.ok) {
    return [{ level: 'warning', message: `Configured provider ${configured} failed its live check: ${active.test.status || 'unavailable'}.` }];
  }
  return [];
}

function notifyProviderIssues(issues) {
  const activeIssues = (issues || []).filter(Boolean);
  const nextIssue = activeIssues[0] || null;
  currentProviderIssue = nextIssue;
  const key = nextIssue ? `${nextIssue.level}:${nextIssue.message}` : '';
  if (!nextIssue) {
    lastProviderIssueKey = '';
    broadcastState();
    return;
  }
  if (key !== lastProviderIssueKey) {
    lastProviderIssueKey = key;
    appendShellLog(`[providers] ${nextIssue.message}`);
    notify('Alvum provider issue', nextIssue.message);
  }
  broadcastState();
}

async function refreshProviderWatch(liveProbe = false) {
  const summary = liveProbe
    ? await providerProbeSummary(true, true)
    : await providerProbeSummary(true, false);
  notifyProviderIssues(providerIssues(summary));
  return summary;
}

async function bootstrapProvidersIfNeeded() {
  const result = await runAlvumJson(['providers', 'bootstrap'], 120000);
  if (!result || result.error) {
    appendShellLog(`[providers] bootstrap failed: ${result && result.error ? result.error : 'unknown error'}`);
    return result;
  }
  if (result.skipped) {
    appendShellLog(`[providers] bootstrap skipped: ${result.reason || 'already initialized'}`);
  } else {
    appendShellLog(`[providers] bootstrap enabled: ${(result.enabled || []).join(', ') || 'none'}`);
  }
  providerProbeCache = null;
  providerProbeCacheAt = 0;
  providerProbeCacheLive = false;
  return result;
}

function startProviderWatcher() {
  if (providerWatchTimer) return;
  (async () => {
    await bootstrapProvidersIfNeeded();
    await refreshProviderWatch(true);
  })();
  providerWatchTimer = setInterval(() => refreshProviderWatch(!!currentProviderIssue), PROVIDER_WATCH_MS);
}

async function providerTest(name) {
  const result = await runAlvumJson(['providers', 'test', '--provider', name, '--timeout-secs', PROVIDER_MANUAL_TEST_TIMEOUT_SECS], PROVIDER_MANUAL_TEST_TIMEOUT_MS);
  const summary = await providerProbeSummary(false, false);
  if (!summary || summary.error || !Array.isArray(summary.providers)) return result;
  const providers = summary.providers.map((provider) => {
    if (provider.name !== name) return provider;
    return {
      ...provider,
      test: result,
      ui: providerUiStatus(provider, result),
    };
  });
  const nextSummary = applyProviderAutoSelection({
    ...summary,
    connected: providers.filter((p) => p.ui.level === 'green').length,
    providers,
  });
  providerProbeCache = nextSummary;
  providerProbeCacheAt = Date.now();
  providerProbeCacheLive = true;
  writeProviderHealth(nextSummary);
  notifyProviderIssues(providerIssues(nextSummary));
  return { ...result, summary: nextSummary };
}

async function providerSetActive(name) {
  const result = await runAlvumJson(['providers', 'set-active', name], 5000);
  const summary = await refreshProviderWatch(false);
  setTimeout(() => refreshProviderWatch(true), 0);
  return { ...result, summary };
}

async function providerSetEnabled(name, enabled) {
  const result = await runAlvumJson(['providers', enabled ? 'enable' : 'disable', name], 5000);
  const summary = await refreshProviderWatch(false);
  setTimeout(() => refreshProviderWatch(true), 0);
  return { ...result, summary };
}

async function providerConfigure(name, payload) {
  const result = await runAlvumJson(
    ['providers', 'configure', name],
    10000,
    JSON.stringify(payload || {}),
  );
  const summary = await refreshProviderWatch(false);
  setTimeout(() => refreshProviderWatch(true), 0);
  return { ...result, summary };
}

async function providerModels(name) {
  return runAlvumJson(['providers', 'models', '--provider', name], 15000);
}

async function providerInstallModel(name, model) {
  const result = await runAlvumJson(
    ['providers', 'install-model', '--provider', name, '--model', model],
    60 * 60 * 1000,
  );
  const models = await providerModels(name);
  const summary = await refreshProviderWatch(false);
  return { ...result, models, summary };
}

async function installWhisperModel() {
  const result = await runAlvumJson(['models', 'install', 'whisper'], 60 * 60 * 1000);
  const connectors = await connectorList();
  broadcastState();
  return { ...result, connectors: connectors.connectors };
}

function providerByNameFromSummary(name) {
  const providers = providerProbeCache && Array.isArray(providerProbeCache.providers)
    ? providerProbeCache.providers
    : [];
  return providers.find((provider) => provider.name === name) || null;
}

function escapeAppleScriptString(value) {
  return String(value || '').replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

function openTerminalCommand(command) {
  return new Promise((resolve) => {
    const env = alvumSpawnEnv();
    const pathPrefix = env.PATH ? `export PATH=${shellArg(env.PATH)}:"$PATH"; ` : '';
    const script = [
      'tell application "Terminal"',
      'activate',
      `do script "${escapeAppleScriptString(`${pathPrefix}${command}`)}"`,
      'end tell',
    ].join('\n');
    const child = spawn('/usr/bin/osascript', ['-e', script], {
      stdio: ['ignore', 'pipe', 'pipe'],
      env,
    });
    let stderr = '';
    child.stderr.on('data', (d) => { stderr += d.toString(); });
    child.on('close', (code) => {
      resolve({
        ok: code === 0,
        action: 'terminal',
        error: code === 0 ? null : (stderr || `osascript exited ${code}`),
      });
    });
    child.on('error', (e) => resolve({ ok: false, action: 'terminal', error: e.message }));
  });
}

function providerSetupActions(provider) {
  return provider && Array.isArray(provider.setup_actions) ? provider.setup_actions : [];
}

function providerConfigFieldValue(provider, key, { includePlaceholder = false } = {}) {
  if (!provider || !Array.isArray(provider.config_fields)) return '';
  const field = provider.config_fields.find((item) => item && item.key === key);
  if (!field) return '';
  const value = field.value == null ? '' : String(field.value).trim();
  if (value || !includePlaceholder) return value;
  return field.placeholder == null ? '' : String(field.placeholder).trim();
}

function providerModelValue(provider, key) {
  const value = providerConfigFieldValue(provider, key);
  if (value) return value;
  const selected = provider && provider.selected_models && typeof provider.selected_models === 'object'
    ? provider.selected_models
    : {};
  const modality = key === 'image_model' ? 'image' : (key === 'audio_model' ? 'audio' : 'text');
  const selectedValue = selected[modality];
  if (!selectedValue || selectedValue === 'CLI default' || selectedValue === 'No model selected') return '';
  return String(selectedValue).trim();
}

function shellArg(value) {
  return `'${String(value || '').replace(/'/g, "'\\''")}'`;
}

function commandWithAwsConfig(base, provider) {
  const parts = [base];
  const profile = providerConfigFieldValue(provider, 'aws_profile');
  const region = providerConfigFieldValue(provider, 'aws_region', { includePlaceholder: true });
  if (profile) parts.push('--profile', shellArg(profile));
  if (region) parts.push('--region', shellArg(region));
  return parts.join(' ');
}

function homePath(...parts) {
  return path.join(os.homedir(), ...parts);
}

function providerSetupActionById(provider, actionId) {
  const id = String(actionId || '').trim();
  if (!provider || !id) return null;
  const declared = providerSetupActions(provider).some((action) => action && action.id === id);
  if (!declared) return null;
  switch (id) {
    case 'claude_doctor':
      return { kind: 'terminal', command: 'claude doctor' };
    case 'open_claude_config':
      return { kind: 'folder', path: homePath('.claude') };
    case 'edit_extra_path':
      return { kind: 'inline', focusKey: 'extra_path' };
    case 'codex_login':
      return { kind: 'terminal', command: 'codex login' };
    case 'codex_models':
      return { kind: 'terminal', command: 'codex debug models --bundled' };
    case 'open_codex_config': {
      const file = homePath('.codex', 'config.toml');
      return fs.existsSync(file)
        ? { kind: 'file', path: file }
        : { kind: 'folder', path: homePath('.codex') };
    }
    case 'anthropic_keys':
      return { kind: 'url', url: 'https://console.anthropic.com/settings/keys' };
    case 'anthropic_models':
      return { kind: 'url', url: 'https://docs.anthropic.com/en/docs/about-claude/models' };
    case 'edit_anthropic_key':
      return { kind: 'inline' };
    case 'open_aws_config':
      return { kind: 'folder', path: homePath('.aws') };
    case 'bedrock_refresh_catalog':
      return { kind: 'inline', refreshModels: true };
    case 'aws_sts':
      return { kind: 'providerCommand', args: ['providers', 'identity', '--provider', 'bedrock'] };
    case 'bedrock_list_models':
      return { kind: 'terminal', command: commandWithAwsConfig('aws bedrock list-foundation-models', provider) };
    case 'ollama_download':
      return { kind: 'url', url: 'https://ollama.com/download' };
    case 'ollama_serve':
      return { kind: 'terminal', command: 'ollama serve' };
    case 'ollama_list':
      return { kind: 'terminal', command: 'ollama list' };
    case 'ollama_show_text': {
      const model = providerModelValue(provider, 'text_model') || providerModelValue(provider, 'model');
      return model ? { kind: 'terminal', command: `ollama show ${shellArg(model)}` } : { kind: 'inline' };
    }
    case 'ollama_show_image': {
      const model = providerModelValue(provider, 'image_model');
      return model ? { kind: 'terminal', command: `ollama show ${shellArg(model)}` } : { kind: 'inline' };
    }
    default:
      return null;
  }
}

async function runProviderSetupAction(provider, actionId, resolved) {
  const descriptor = resolved || providerSetupActionById(provider, actionId);
  if (!descriptor) {
    return { ok: false, provider: provider.name, action: actionId || null, error: 'unknown setup action' };
  }
  if (descriptor.kind === 'terminal') {
    return { provider: provider.name, ...(await openTerminalCommand(descriptor.command)) };
  }
  if (descriptor.kind === 'providerCommand') {
    try {
      const result = await runAlvumJson(descriptor.args);
      return {
        ok: !!(result && result.ok),
        provider: provider.name,
        action: 'provider_command',
        result,
        error: result && result.error ? result.error : null,
      };
    } catch (e) {
      return { ok: false, provider: provider.name, action: 'provider_command', error: e.message };
    }
  }
  if (descriptor.kind === 'url') {
    try {
      await shell.openExternal(descriptor.url);
      return { ok: true, provider: provider.name, action: 'url', url: descriptor.url };
    } catch (e) {
      return { ok: false, provider: provider.name, action: 'url', url: descriptor.url, error: e.message };
    }
  }
  if (descriptor.kind === 'file' || descriptor.kind === 'folder') {
    try {
      const error = await shell.openPath(descriptor.path);
      return {
        ok: !error,
        provider: provider.name,
        action: descriptor.kind,
        path: descriptor.path,
        error: error || null,
      };
    } catch (e) {
      return { ok: false, provider: provider.name, action: descriptor.kind, path: descriptor.path, error: e.message };
    }
  }
  if (descriptor.kind === 'inline') {
    return {
      ok: true,
      provider: provider.name,
      action: 'inline',
      focus_key: descriptor.focusKey || null,
      refresh_models: !!descriptor.refreshModels,
    };
  }
  return { ok: false, provider: provider.name, action: actionId || null, error: 'unsupported setup action' };
}

async function providerSetup(name, action = null) {
  let provider = providerByNameFromSummary(name);
  if (!provider) {
    const summary = await providerProbeSummary(true, false);
    provider = Array.isArray(summary.providers)
      ? summary.providers.find((entry) => entry.name === name)
      : null;
  }
  if (!provider) return { ok: false, provider: name, error: 'unknown provider' };
  if (action && action !== 'terminal' && action !== 'url') {
    return runProviderSetupAction(provider, action);
  }
  if (action === 'terminal' && provider.setup_command) {
    return { provider: name, ...(await openTerminalCommand(provider.setup_command)) };
  }
  if (action === 'url' && provider.setup_url) {
    try {
      await shell.openExternal(provider.setup_url);
      return { ok: true, provider: name, action: 'url', url: provider.setup_url };
    } catch (e) {
      return { ok: false, provider: name, action: 'url', url: provider.setup_url, error: e.message };
    }
  }
  if (provider.setup_kind === 'inline') {
    return { ok: true, provider: name, action: 'inline' };
  }
  if (provider.setup_kind === 'terminal' && provider.setup_command) {
    return { provider: name, ...(await openTerminalCommand(provider.setup_command)) };
  }
  if (provider.setup_kind === 'url' && provider.setup_url) {
    try {
      await shell.openExternal(provider.setup_url);
      return { ok: true, provider: name, action: 'url', url: provider.setup_url };
    } catch (e) {
      return { ok: false, provider: name, action: 'url', url: provider.setup_url, error: e.message };
    }
  }
  if (provider.setup_kind === 'instructions') {
    return {
      ok: true,
      provider: name,
      action: 'instructions',
      message: provider.setup_hint || provider.auth_hint || 'Configure this provider in its native tool, then Ping it.',
    };
  }
  return {
    ok: false,
    provider: name,
    action: 'instructions',
    error: provider.setup_hint || provider.auth_hint || 'No setup action is available.',
  };
}

  function providerProbeSnapshot() {
    return providerProbeCache;
  }

  function currentProviderIssueSnapshot() {
    return currentProviderIssue;
  }

  return {
    providerDiagnosticSnapshot,
    numeric,
    providerRuntimeRecord,
    recordProviderEvent,
    providerRuntimeStatsSnapshot,
    readProviderHealth,
    writeProviderHealth,
    providerUiStatus,
    providerSelectableForAuto,
    autoProviderName,
    applyProviderAutoSelection,
    providerProbeSummary,
    providerIssues,
    notifyProviderIssues,
    refreshProviderWatch,
    bootstrapProvidersIfNeeded,
    startProviderWatcher,
    providerTest,
    providerSetActive,
    providerSetEnabled,
    providerConfigure,
    providerModels,
    providerInstallModel,
    installWhisperModel,
    providerByNameFromSummary,
    escapeAppleScriptString,
    openTerminalCommand,
    providerSetup,
    providerProbeSnapshot,
    currentProviderIssueSnapshot,
  };
}

module.exports = { createProviderService };
