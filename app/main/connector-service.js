function createConnectorService({
  shell,
  EXTENSIONS_DIR,
  BRIEFING_LOG,
  SHELL_LOG,
  ensureExtensionsDir,
  runAlvumJson,
  runAlvumText,
  readTail,
  capturePermissionStatus,
  sourcePermissionRequirements,
  blockedPermissionsForSource,
  enabledPermissionIssues,
  promptForSourcePermissions,
  notifyPermissionIssues,
  captureInputsSummary,
  restartCapture,
  providerProbeSummary,
  providerIssues,
  broadcastState,
}) {
async function openExtensionsDir() {
  try {
    ensureExtensionsDir();
    const error = await shell.openPath(EXTENSIONS_DIR);
    if (error) return { ok: false, path: EXTENSIONS_DIR, error };
    return { ok: true, path: EXTENSIONS_DIR };
  } catch (e) {
    return { ok: false, path: EXTENSIONS_DIR, error: e.message };
  }
}

async function extensionList() {
  const data = await runAlvumJson(['extensions', 'list', '--json'], 5000);
  if (!data || data.error) return { extensions: [], error: data && data.error ? data.error : 'extension list failed' };
  return {
    extensions: Array.isArray(data.extensions) ? data.extensions : [],
    core: Array.isArray(data.core) ? data.core : [],
  };
}

async function extensionSetEnabled(id, enabled) {
  const result = await runAlvumText(['extensions', enabled ? 'enable' : 'disable', id], 30000);
  const list = await extensionList();
  if (result.ok) restartCapture();
  broadcastState();
  return { ok: result.ok, id, enabled, error: result.error, stdout: result.stdout, extensions: list.extensions, core: list.core };
}

async function extensionDoctor() {
  const data = await runAlvumJson(['extensions', 'doctor', '--json'], 30000);
  if (!data || data.error) return { extensions: [], error: data && data.error ? data.error : 'extension doctor failed' };
  return {
    extensions: Array.isArray(data.extensions) ? data.extensions : [],
  };
}

async function connectorList() {
  const data = await runAlvumJson(['connectors', 'list', '--json'], 5000);
  if (!data || data.error) return { connectors: [], error: data && data.error ? data.error : 'connector list failed' };
  return {
    connectors: annotateConnectorPermissions(Array.isArray(data.connectors) ? data.connectors : []),
  };
}

function annotateConnectorPermissions(connectors, permissions = capturePermissionStatus()) {
  return connectors.map((connector) => {
    const source_controls = Array.isArray(connector.source_controls)
      ? connector.source_controls.map((control) => {
          const blocked_permissions = control.enabled
            ? blockedPermissionsForSource(control.id, permissions)
            : [];
          return {
            ...control,
            required_permissions: sourcePermissionRequirements(control.id),
            blocked_permissions,
            blocked: blocked_permissions.length > 0,
          };
        })
      : [];
    const permission_issues = source_controls.flatMap((control) =>
      (control.blocked_permissions || []).map((issue) => ({
        ...issue,
        connector_id: connector.id,
        connector_label: connector.display_name || connector.id,
        source_id: control.id,
        source_label: control.label || control.id,
      })));
    return {
      ...connector,
      source_controls,
      permission_issues,
      blocked: permission_issues.length > 0,
    };
  });
}

async function connectorSetEnabled(id, enabled) {
  const result = await runAlvumText(['connectors', enabled ? 'enable' : 'disable', id], 30000);
  let list = await connectorList();
  let permission_issues = [];
  if (result.ok && enabled) {
    const record = list.connectors.find((connector) => connector.id === id || connector.component_id === id);
    const sourceIds = record && Array.isArray(record.source_controls)
      ? record.source_controls.filter((control) => control.enabled).map((control) => control.id)
      : [];
    if (sourceIds.length) await promptForSourcePermissions(sourceIds, true);
    list = await connectorList();
    const updated = list.connectors.find((connector) => connector.id === id || connector.component_id === id);
    permission_issues = updated && Array.isArray(updated.permission_issues)
      ? updated.permission_issues
      : [];
    notifyPermissionIssues(permission_issues);
  }
  if (result.ok) restartCapture();
  const captureInputs = captureInputsSummary();
  broadcastState();
  return { ok: result.ok, id, enabled, error: result.error, stdout: result.stdout, connectors: list.connectors, captureInputs, permission_issues };
}

async function globalDoctor() {
  const data = await runAlvumJson(['doctor', '--json'], 30000);
  const permissionIssues = enabledPermissionIssues();
  const permissionChecks = permissionIssues.map((issue) => ({
    id: `permission.${issue.permission}.${issue.source_id || 'source'}`,
    label: issue.label || issue.permission,
    level: 'warning',
    message: `${issue.source_label || 'Enabled connector'} is enabled but ${issue.label || issue.permission} is ${issue.status}.`,
  }));
  const providerSummary = await providerProbeSummary(true, true);
  const providerChecks = providerIssues(providerSummary).map((issue, index) => ({
    id: `providers.runtime.${index}`,
    label: 'Providers',
    level: issue.level || 'warning',
    message: issue.message,
  }));
  if (!data || data.error) {
    return {
      ok: false,
      error_count: 1,
      warning_count: permissionChecks.length + providerChecks.length,
      checks: permissionChecks.concat(providerChecks),
      error: data && data.error ? data.error : 'doctor failed',
    };
  }
  const checks = Array.isArray(data.checks) ? data.checks : [];
  return {
    ok: !!data.ok && permissionChecks.length === 0 && providerChecks.length === 0,
    error_count: Number(data.error_count) || 0,
    warning_count: (Number(data.warning_count) || 0) + permissionChecks.length + providerChecks.length,
    checks: checks.concat(permissionChecks, providerChecks),
  };
}

async function synthesisProfile() {
  const data = await runAlvumJson(['profile', 'show', '--json'], 5000);
  if (!data || data.error) return { ok: false, error: data && data.error ? data.error : 'profile load failed' };
  return { ok: true, profile: data };
}

async function synthesisProfileSave(profile) {
  const result = await runAlvumText(['profile', 'save', '--json', JSON.stringify(profile)], 10000);
  const updated = await synthesisProfile();
  const suggestions = await synthesisProfileSuggestions();
  return {
    ok: result.ok,
    error: result.error,
    stdout: result.stdout,
    profile: updated.profile,
    suggestions: suggestions.suggestions || [],
  };
}

async function synthesisProfileSuggestions() {
  const data = await runAlvumJson(['profile', 'suggestions', '--json'], 10000);
  if (!data || data.error) return { ok: false, suggestions: [], error: data && data.error ? data.error : 'profile suggestions failed' };
  return { ok: true, suggestions: Array.isArray(data.suggestions) ? data.suggestions : [], knowledge_dir: data.knowledge_dir };
}

async function synthesisProfilePromote(id) {
  const result = await runAlvumText(['profile', 'promote', id], 10000);
  const profile = await synthesisProfile();
  const suggestions = await synthesisProfileSuggestions();
  return {
    ok: result.ok,
    error: result.error,
    stdout: result.stdout,
    profile: profile.profile,
    suggestions: suggestions.suggestions || [],
  };
}

async function synthesisProfileIgnore(id) {
  const result = await runAlvumText(['profile', 'ignore', id], 10000);
  const suggestions = await synthesisProfileSuggestions();
  return {
    ok: result.ok,
    error: result.error,
    stdout: result.stdout,
    suggestions: suggestions.suggestions || [],
  };
}

  return {
    openExtensionsDir,
    extensionList,
    extensionSetEnabled,
    extensionDoctor,
    connectorList,
    annotateConnectorPermissions,
    connectorSetEnabled,
    globalDoctor,
    synthesisProfile,
    synthesisProfileSave,
    synthesisProfileSuggestions,
    synthesisProfilePromote,
    synthesisProfileIgnore,
  };
}

module.exports = { createConnectorService };
