// @ts-nocheck

import './styles/popover.css';
import { getAlvumApi } from './api/alvum-api';
import { createCaptureFeature } from './features/capture';
import { createConnectorsFeature } from './features/connectors';
import { createLogsFeature } from './features/logs';
import { createProfileFeature } from './features/profile';
import { createProvidersFeature } from './features/providers';
import { createSynthesisFeature } from './features/synthesis';
import { installMockAlvum } from './mock/alvum';

  const $ = (id) => document.getElementById(id);
  const DEFAULT_DAILY_BRIEFING_OUTLINE = [
    'Alignment narrative: measure the day against active intentions.',
    'Key decisions: cite the most important choices, deferrals, and revealed commitments.',
    'Causal chains and patterns: show what connected across domains.',
    'Open threads and nudges: end with the next actions that get the user back on track.',
  ].join('\n');

  installMockAlvum(DEFAULT_DAILY_BRIEFING_OUTLINE);
  const api = getAlvumApi();


  const STAGES = ['gather', 'process', 'thread', 'cluster', 'cluster-correlate', 'domain', 'domain-correlate', 'day', 'knowledge'];
  const STAGE_WEIGHTS = { gather: 2, process: 23, thread: 20, cluster: 12, 'cluster-correlate': 6, domain: 14, 'domain-correlate': 8, day: 12, knowledge: 3 };
  const STAGE_LABELS = {
    'gather': 'Gather refs',
    'process': 'Process media',
    'thread': 'Thread episodes',
    'cluster': 'Cluster threads',
    'cluster-correlate': 'Map cluster links',
    'domain': 'Resolve domains',
    'domain-correlate': 'Link decisions',
    'day': 'Compose synthesis',
    'knowledge': 'Update knowledge',
    'distill': 'Distill decisions',
    'causal': 'Link causally',
    'brief': 'Compose synthesis',
  };
  const STAGE_STARTS = {};
  {
    let acc = 0;
    for (const s of STAGES) {
      STAGE_STARTS[s] = acc;
      acc += STAGE_WEIGHTS[s] || 0;
    }
  }

  let activeView = 'main';
  let currentState = {};
  let runStartedAt = null;
  let lastPct = 0;
  let prevBriefingRunning = false;
  let eventRows = [];
  let providerProbe = null;
  let providerProbeLoading = false;
  let providerProbeError = null;
  let providerTelemetry = {};
  let updateState = null;
  let logKind = 'shell';
  let resizePending = false;
  let currentCalendar = null;
  let selectedBriefingDate = null;
  let activeProgress = null;
  let progressByDate = {};
  let eventRowsByDate = {};
  let persistedRunLogsByDate = {};
  let readerMarkdown = '';
  let readerDate = null;
  let logDate = null;
  let decisionGraphDate = null;
  let decisionGraphData = null;
  let decisionGraphError = null;
  let decisionGraphLoading = false;
  let selectedDecisionGraphNode = null;
  let captureInputs = null;
  let selectedCaptureInput = null;
  let captureInputParent = 'extensions';
  let selectedProvider = null;
  let providerDetailParent = 'providers';
  const providerModelLoadState = new Map();
  const providerModelInstallState = new Map();
  let whisperInstallLoading = false;
  let pyannoteInstallLoading = false;
  let pyannoteSetupIssue = null;
  let lastRenderedProviderIssueKey = '';
  let extensionSummary = null;
  let speakerSummary = null;
  let speakerLoading = false;
  let selectedProfileVoiceId = null;
  let selectedProfileVoiceSampleId = null;
  let activeSpeakerAudio = null;
  let menuNotificationDismissTimer = null;
  let menuNotificationHideTimer = null;
  let selectedExtension = null;
  let briefingReaderParent = 'briefing';
  let synthesisProfile = null;
  let synthesisSchedule = null;
	  let synthesisProfileSuggestions = [];
	  let synthesisProfileLoading = false;
	  let synthesisProfileSaving = false;
  let synthesisScheduleSaving = false;
	  let synthesisProfileError = null;
	  let selectedProfileIntentionId = null;
	  let selectedProfileDomainId = null;
	  let selectedProfileInterestId = null;
	  let viewTransitioning = false;
	  let pendingResizeHeight = null;
	  const POPOVER_MAX_HEIGHT = 640;
	  const POPOVER_MIN_HEIGHT = 300;
  const viewHandlers = new Map();
  const featureModules = [];
  const appContext = {
    api,
    state: {},
    router: {
      activeView: () => activeView,
      setView: (view, direction) => setView(view, direction),
      parentViewFor: (view) => parentViewFor(view),
      registerViewHandler: (view, handler) => viewHandlers.set(view, handler),
    },
    notify: {
      show: (text, level = 'info', heading = null) => showMenuNotification(text, level, heading),
    },
    layout: {
      requestResize: (height) => requestPopoverResize(height),
    },
    dom: { $ },
  };
  let rendererFeaturesRegistered = false;

  function registerFeatureModule(module) {
    featureModules.push(module);
    module.init(appContext);
  }

  function requestPopoverResize(height) {
    if (viewTransitioning && !Number.isFinite(height)) return;
    if (Number.isFinite(height)) pendingResizeHeight = height;
    if (resizePending) return;
    resizePending = true;
    requestAnimationFrame(() => {
      resizePending = false;
      if (!window.alvum.resizePopover) return;
      const requested = Number.isFinite(pendingResizeHeight) ? pendingResizeHeight : popoverContentHeight();
      pendingResizeHeight = null;
      window.alvum.resizePopover(requested);
    });
  }

  function bodyBlockPadding() {
    const style = getComputedStyle(document.body);
    return (parseFloat(style.paddingTop) || 0) + (parseFloat(style.paddingBottom) || 0);
  }

  function popoverMaxHeight() {
    const available = window.screen && Number.isFinite(window.screen.availHeight)
      ? Math.max(POPOVER_MIN_HEIGHT, window.screen.availHeight - 12)
      : POPOVER_MAX_HEIGHT;
    return Math.max(POPOVER_MIN_HEIGHT, Math.min(POPOVER_MAX_HEIGHT, available));
  }

  function viewsTopOffset() {
    const shell = document.querySelector('.popover-shell');
    const viewsEl = document.querySelector('.views');
    if (!shell || !viewsEl) return 0;
    const shellRect = shell.getBoundingClientRect();
    const viewsRect = viewsEl.getBoundingClientRect();
    return Math.max(0, viewsRect.top - shellRect.top);
  }

  function applyViewScrollLimit() {
    const maxHeight = popoverMaxHeight();
    const maxViewsHeight = Math.max(120, maxHeight - bodyBlockPadding() - viewsTopOffset());
    document.documentElement.style.setProperty('--popover-max-height', `${maxHeight}px`);
    document.documentElement.style.setProperty('--popover-view-max-height', `${maxViewsHeight}px`);
    return maxViewsHeight;
  }

  function cappedViewHeight(height) {
    const maxViewsHeight = applyViewScrollLimit();
    const requested = Number.isFinite(height) ? Math.ceil(height) : maxViewsHeight;
    return Math.max(0, Math.min(requested, maxViewsHeight));
  }

  function popoverContentHeight(viewHeight) {
    const shell = document.querySelector('.popover-shell');
    const viewsEl = document.querySelector('.views');
    if (!shell || !viewsEl) return Math.ceil(bodyBlockPadding());
    const shellRect = shell.getBoundingClientRect();
    const viewsRect = viewsEl.getBoundingClientRect();
    const activeViewEl = document.querySelector(`.view[data-view="${activeView}"]`);
    const viewsTop = Math.max(0, viewsRect.top - shellRect.top);
    applyViewScrollLimit();
    const targetViewsHeight = Number.isFinite(viewHeight)
      ? viewHeight
      : ((activeViewEl && !activeViewEl.hidden) ? activeViewEl.scrollHeight : viewsRect.height);
    const fullHeight = Math.ceil(viewsTop + targetViewsHeight + bodyBlockPadding());
    return Math.min(fullHeight, popoverMaxHeight());
  }

  function monthFromDate(date) {
    return (date || new Date().toISOString()).slice(0, 7);
  }

  function addMonths(month, delta) {
    const [year, monthIndex] = month.split('-').map(Number);
    const d = new Date(year, monthIndex - 1 + delta, 1);
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
  }

  function calendarDay(date) {
    if (!currentCalendar || !Array.isArray(currentCalendar.days)) return null;
    return currentCalendar.days.find((day) => day.date === date) || null;
  }

  function briefingRun(date) {
    return (currentState.briefingRuns && currentState.briefingRuns[date]) || null;
  }

  function progressPct(p, previous = 0) {
    if (!p) return previous || 0;
    const stageStart = STAGE_STARTS[p.stage] ?? 0;
    const stageWeight = STAGE_WEIGHTS[p.stage] ?? 0;
    const inner = p.total > 0 ? Math.min(1, p.current / p.total) : 0;
    return Math.max(previous || 0, Math.min(100, Math.round(stageStart + stageWeight * inner)));
  }

  function stageLabel(stage) {
    return STAGE_LABELS[stage] || stage || 'Synthesis';
  }

  function displayDate(date) {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(date || '')) return date || '';
    const [year, month, day] = date.split('-').map(Number);
    return new Date(year, month - 1, day).toLocaleDateString(undefined, {
      weekday: 'short',
      month: 'short',
      day: 'numeric',
    });
  }

  function dateFromStamp(stamp) {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(stamp || '')) return null;
    const [year, month, day] = stamp.split('-').map(Number);
    return new Date(year, month - 1, day);
  }

  function dayDistance(fromStamp, toStamp) {
    const from = dateFromStamp(fromStamp);
    const to = dateFromStamp(toStamp);
    if (!from || !to) return null;
    return Math.round((to.getTime() - from.getTime()) / 86400000);
  }

  function latestBriefingButtonText(s) {
    const date = s.latestBriefing && s.latestBriefing.date;
    if (!date) return 'None';
    const today = (s.briefingCalendar && s.briefingCalendar.today) || new Date().toISOString().slice(0, 10);
    const ageDays = dayDistance(date, today);
    const d = dateFromStamp(date);
    if (ageDays === 0) return 'Today';
    if (ageDays === 1) return 'Yesterday';
    if (ageDays != null && ageDays > 1 && ageDays < 7) return d.toLocaleDateString(undefined, { weekday: 'short' });
    if (ageDays != null && ageDays >= 0 && ageDays < 365) return d.toLocaleDateString(undefined, { month: 'numeric', day: 'numeric' });
    return d ? d.toLocaleDateString(undefined, { month: 'numeric', day: 'numeric', year: '2-digit' }) : date;
  }

  function viewDepth(view) {
    let depth = 0;
    let cursor = view;
    while (cursor && cursor !== 'main') {
      depth += 1;
      cursor = parentViewFor(cursor);
    }
    return depth;
  }

  function transitionDirection(from, to, requested) {
    if (requested) return requested;
    if (from === to) return 'replace';
    return viewDepth(to) > viewDepth(from) ? 'forward' : 'back';
  }

  function prepareView(view) {
    $('back-button').hidden = view === 'main';
    $('view-title').textContent = {
      main: 'Status',
      capture: 'Capture',
      'capture-input': 'Input',
      briefing: 'Synthesis',
      providers: 'Providers',
      'provider-add': 'Add Provider',
      extensions: 'Connectors',
      'connector-add': 'Add Connector',
      'extension-detail': 'Connector',
      'provider-detail': 'Provider',
	      'briefing-reader': 'Synthesis',
	      'decision-graph': 'Decision Graph',
	      'briefing-log': 'Progress Log',
	      'synthesis-profile': 'Customize',
	      'profile-intentions-list': 'Intentions',
	      'profile-intention-detail': 'Intention',
	      'profile-domains-list': 'Domains',
	      'profile-domain-detail': 'Domain',
	      'profile-interests-list': 'Tracked',
	      'profile-interest-detail': 'Tracked item',
	      'profile-voices-list': 'Voices',
	      'profile-voice-detail': 'Link Voice',
	      'profile-writing-detail': 'Writing',
      'profile-schedule-detail': 'Schedule',
	      'profile-advanced-detail': 'Advanced',
	      logs: 'Logs',
	    }[view] || 'Status';
    const handler = viewHandlers.get(view);
    if (handler) handler();
    renderUpdateChip();
    requestPopoverResize();
  }

  function setView(view, requestedDirection) {
    const previousView = activeView;
    const direction = transitionDirection(previousView, view, requestedDirection);
    if (previousView === view && direction !== 'replace') return;

    const previousEl = document.querySelector(`.view[data-view="${previousView}"]`);
    const nextEl = document.querySelector(`.view[data-view="${view}"]`);
    const viewsEl = document.querySelector('.views');
    const previousHeight = previousEl ? cappedViewHeight(previousEl.scrollHeight || previousEl.offsetHeight) : 0;
    activeView = view;
    viewTransitioning = direction !== 'replace';
    prepareView(view);

    if (!previousEl || !nextEl || previousEl === nextEl || direction === 'replace') {
      document.querySelectorAll('.view').forEach((el) => {
        const active = el.dataset.view === view;
        el.hidden = !active;
        el.setAttribute('aria-hidden', String(!active));
        el.classList.remove('transitioning', 'enter-forward', 'exit-forward', 'enter-back', 'exit-back');
      });
      viewTransitioning = false;
      requestPopoverResize();
      return;
    }

    document.querySelectorAll('.view').forEach((el) => {
      if (el !== previousEl && el !== nextEl) {
        el.hidden = true;
        el.setAttribute('aria-hidden', 'true');
        el.classList.remove('transitioning', 'enter-forward', 'exit-forward', 'enter-back', 'exit-back');
      }
    });

    previousEl.hidden = false;
    nextEl.hidden = false;
    const nextContentHeight = nextEl.scrollHeight || nextEl.offsetHeight;
    const nextHeight = cappedViewHeight(nextContentHeight);
    viewsEl.style.height = `${previousHeight}px`;
    void viewsEl.offsetHeight;
    previousEl.setAttribute('aria-hidden', 'true');
    nextEl.setAttribute('aria-hidden', 'false');
    previousEl.classList.add('transitioning', direction === 'forward' ? 'exit-forward' : 'exit-back');
    nextEl.classList.add('transitioning', direction === 'forward' ? 'enter-forward' : 'enter-back');
    requestAnimationFrame(() => {
      viewsEl.style.height = `${nextHeight}px`;
      requestPopoverResize(popoverContentHeight(nextContentHeight));
    });

    const finish = () => {
      previousEl.hidden = true;
      previousEl.classList.remove('transitioning', 'exit-forward', 'exit-back');
      nextEl.classList.remove('transitioning', 'enter-forward', 'enter-back');
      viewTransitioning = false;
      viewsEl.style.height = '';
      requestPopoverResize();
    };
    nextEl.addEventListener('animationend', finish, { once: true });
  }

  function elapsed(ts) {
    const s = Math.max(0, Math.floor((Date.now() - ts) / 1000));
    if (s < 60) return `${s}s`;
    return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`;
  }

  function briefStatusText(s) {
    const runningCount = s.briefingRuns ? Object.keys(s.briefingRuns).length : 0;
    if (runningCount > 1) return `Generating ${runningCount} days`;
    if (runningCount === 1) {
      const run = Object.values(s.briefingRuns)[0];
      return `Generating ${run.lastPct || progressPct(run.progress)}%`;
    }
    if (s.latestBriefing) return 'Ready';
    if (s.briefingCatchupPending > 0) return `${s.briefingCatchupPending} pending`;
    return 'Needs briefing';
  }

  function enabledConnectorCount() {
    const connectors = extensionSummary && Array.isArray(extensionSummary.connectors)
      ? extensionSummary.connectors
      : [];
    return connectors.filter((connector) => connector.enabled).length;
  }

  function enabledProviderCount() {
    return configuredProviders().length;
  }

  function audioProcessorReadiness() {
    const connectors = extensionSummary && Array.isArray(extensionSummary.connectors)
      ? extensionSummary.connectors
      : [];
    const audio = connectors.find((connector) => connector.component_id === 'alvum.audio/audio');
    const processors = audio && Array.isArray(audio.processor_controls) ? audio.processor_controls : [];
    const whisper = processors.find((processor) => processor.component === 'alvum.audio/whisper');
    return whisper ? whisper.readiness : null;
  }

  function audioProcessingRelevant() {
    const inputs = (captureInputs && captureInputs.inputs)
      || (currentState.captureInputs && currentState.captureInputs.inputs)
      || [];
    if (inputs.some((input) => input.enabled && (input.id === 'audio-mic' || input.id === 'audio-system'))) return true;
    if (selectedCaptureInput === 'audio-mic' || selectedCaptureInput === 'audio-system') return true;
    const detail = String((currentState.captureStats && currentState.captureStats.detail) || '').toLowerCase();
    return /\b(wav|opus|mp3|m4a|audio)\b/.test(detail);
  }

  function firstSynthesisTarget() {
    const today = (currentState.briefingCalendar && currentState.briefingCalendar.today)
      || new Date().toISOString().slice(0, 10);
    const catchupDates = new Set(Array.isArray(currentState.briefingCatchupDates)
      ? currentState.briefingCatchupDates
      : []);
    const targets = Array.isArray(currentState.briefingTargets) ? currentState.briefingTargets : [];
    const eligible = targets
      .filter((target) =>
        target
        && target.hasCapture
        && !target.hasBriefing
        && /^\d{4}-\d{2}-\d{2}$/.test(target.date || '')
        && target.date < today
        && (!catchupDates.size || catchupDates.has(target.date)))
      .sort((a, b) => String(b.date).localeCompare(String(a.date)));
    if (eligible.length) return eligible[0];
    const fallbackDate = [...catchupDates]
      .filter((date) => /^\d{4}-\d{2}-\d{2}$/.test(date) && date < today)
      .sort((a, b) => b.localeCompare(a))[0];
    return fallbackDate
      ? { date: fallbackDate, label: displayDate(fallbackDate), hasCapture: true, hasBriefing: false }
      : null;
  }

  async function openSynthesisForDate(date) {
    if (date) selectedBriefingDate = date;
    setView('briefing');
    if (!date) return;
    if (currentCalendar && calendarDay(date)) {
      renderBriefingCalendar(currentCalendar);
      return;
    }
    if (!window.alvum.briefingCalendarMonth) return;
    try {
      renderBriefingCalendar(await window.alvum.briefingCalendarMonth(monthFromDate(date)));
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Synthesis');
    }
  }

  async function installWhisperModelFromUi() {
    if (whisperInstallLoading || !window.alvum.installWhisperModel) return;
    const variant = whisperVariantFromSelectedModel();
    whisperInstallLoading = true;
    renderSetupChecklist();
    renderExtensionDetail();
    try {
      const result = await window.alvum.installWhisperModel(variant);
      if (result && Array.isArray(result.connectors)) extensionSummary = { connectors: result.connectors };
      else await refreshExtensions(true);
      showMenuNotification(
        result && result.ok === false ? (result.error || 'Whisper install failed') : 'Whisper model installed.',
        result && result.ok === false ? 'warning' : 'success',
        'Whisper',
      );
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Whisper');
    } finally {
      whisperInstallLoading = false;
      renderSetupChecklist();
      renderExtensionDetail();
    }
  }

  function isPyannoteAccessResult(result) {
    return !!result && result.ok === false && result.status === 'requires_huggingface_access';
  }

  async function installPyannoteFromUi() {
    if (pyannoteInstallLoading || !window.alvum.installPyannote) return;
    pyannoteInstallLoading = true;
    renderSetupChecklist();
    renderExtensionDetail();
    try {
      const result = await window.alvum.installPyannote();
      if (result && Array.isArray(result.connectors)) extensionSummary = { connectors: result.connectors };
      else await refreshExtensions(true);
      if (isPyannoteAccessResult(result)) {
        pyannoteSetupIssue = result;
      } else {
        pyannoteSetupIssue = null;
        showMenuNotification(
          result && result.ok === false ? (result.detail || result.error || 'Pyannote install failed') : 'Pyannote installed.',
          result && result.ok === false ? 'warning' : 'success',
          'Pyannote',
        );
      }
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Pyannote');
    } finally {
      pyannoteInstallLoading = false;
      renderSetupChecklist();
      renderExtensionDetail();
    }
  }

  function setupChecklistItems() {
    const items = [];
    const issues = permissionIssuesFrom(captureInputs || currentState.captureInputs || {});
    if (issues.length) {
      const names = [...new Set(issues.map((issue) => issue.source_label || issue.label || issue.permission || 'source'))];
      items.push({
        key: 'permissions',
        title: 'Grant capture permissions',
        meta: `${names.slice(0, 2).join(', ')}${names.length > 2 ? ` and ${names.length - 2} more` : ''} cannot capture until macOS access is granted.`,
        action: 'Grant permission',
        onAction: () => setView('capture'),
      });
    }

    if (providerProbe && Array.isArray(providerProbe.providers)) {
      const usableProvider = providerProbe.providers.some(providerIsWorking);
      if (!usableProvider) {
        items.push({
          key: 'providers',
          title: 'Set up a provider',
          meta: 'Synthesis needs one enabled provider that can answer a live ping.',
          action: 'Set up provider',
          onAction: () => setView('providers'),
        });
      }
    }

    const whisperReadiness = audioProcessorReadiness();
    if (
      audioProcessingRelevant()
      && whisperReadiness
      && whisperReadiness.status === 'waiting_on_install'
    ) {
      items.push({
        key: 'whisper',
        title: 'Install Whisper',
        meta: whisperReadiness.detail || 'Local audio transcription needs the selected Whisper model.',
        action: whisperInstallLoading ? 'Installing...' : (whisperReadiness.action && whisperReadiness.action.label) || 'Install',
        onAction: () => installWhisperModelFromUi(),
      });
    }
    if (
      audioProcessingRelevant()
      && whisperReadiness
      && whisperReadiness.status === 'waiting_on_diarization_install'
    ) {
      items.push({
        key: 'pyannote',
        title: 'Install speaker IDs',
        meta: whisperReadiness.detail || 'Local audio transcription needs Pyannote for speaker turns.',
        action: pyannoteInstallLoading ? 'Installing...' : (whisperReadiness.action && whisperReadiness.action.label) || 'Install',
        onAction: () => installPyannoteFromUi(),
      });
    }

    const schedule = synthesisScheduleValue();
    const hasSuccessfulSynthesis = !!(currentState.latestBriefing && currentState.latestBriefing.date);
    const needsFirstSynthesis = !schedule.setup_completed && !hasSuccessfulSynthesis;
    const synthesisTarget = firstSynthesisTarget()
      || (needsFirstSynthesis && currentState.latestBriefing && currentState.latestBriefing.date
        ? {
          date: currentState.latestBriefing.date,
          label: displayDate(currentState.latestBriefing.date),
          hasCapture: true,
          hasBriefing: true,
        }
        : null);
    const hasAnyCaptureData = !!synthesisTarget || Number(currentState.captureStats && currentState.captureStats.files) > 0;
    if (!hasAnyCaptureData && !currentState.latestBriefing) {
      items.push({
        key: 'data',
        title: 'Capture or import data',
        meta: 'Session connectors are available by default; audio and screen start only when enabled.',
        action: 'Choose source',
        onAction: () => setView('capture'),
      });
    }

    if (synthesisTarget && needsFirstSynthesis) {
      items.push({
        key: 'synthesis',
        title: 'Run first synthesis',
        meta: `Ready target: ${synthesisTarget.label || displayDate(synthesisTarget.date)}.`,
        action: 'Synthesize',
        onAction: () => openSynthesisForDate(synthesisTarget.date),
      });
    }
    return items;
  }

  function renderSetupChecklist() {
    const card = $('setup-checklist');
    if (!card) return;
    const items = setupChecklistItems();
    card.hidden = items.length === 0;
    card.replaceChildren();
    if (!items.length) {
      requestPopoverResize();
      return;
    }
    const title = document.createElement('div');
    title.className = 'setup-checklist-title';
    const copy = document.createElement('div');
    const heading = document.createElement('div');
    heading.className = 'value';
    heading.textContent = 'Setup needed';
    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = `${items.length} item${items.length === 1 ? '' : 's'} to check`;
    copy.append(heading, meta);
    title.appendChild(copy);
    card.appendChild(title);
    for (const item of items) {
      const row = document.createElement('div');
      row.className = 'settings-row setup-checklist-row';
      const text = document.createElement('div');
      text.className = 'setup-checklist-copy';
      const name = document.createElement('div');
      name.className = 'value';
      name.textContent = item.title;
      const detail = document.createElement('div');
      detail.className = 'meta';
      detail.textContent = item.meta;
      text.append(name, detail);
      const button = document.createElement('button');
      button.type = 'button';
      button.textContent = item.action;
      button.disabled = (item.key === 'whisper' && whisperInstallLoading) || (item.key === 'pyannote' && pyannoteInstallLoading);
      button.onclick = item.onAction;
      button.className = 'setup-checklist-action';
      row.append(text, button);
      card.appendChild(row);
    }
    requestPopoverResize();
  }

  function renderMainBadges() {
    const connectorBadge = $('extension-enabled-badge');
    const providerBadge = $('provider-enabled-badge');
    if (connectorBadge) {
      connectorBadge.textContent = String(enabledConnectorCount());
      connectorBadge.hidden = !extensionSummary || !!extensionSummary.error;
    }
    if (providerBadge) {
      providerBadge.textContent = String(enabledProviderCount());
      providerBadge.hidden = !providerProbe || !!providerProbe.error;
    }
    renderSetupChecklist();
  }

  function renderUpdateChip() {
    renderVersionLabel();
    const state = updateState || {};
    const chip = $('update-chip');
    if (!chip) return;
    const ready = state.supported !== false && state.status === 'downloaded' && activeView === 'main';
    chip.hidden = !ready;
    $('update-chip-text').textContent = ready
      ? `Update ${state.latestVersion || 'ready'}`
      : 'Update ready';
    requestPopoverResize();
  }

  function renderVersionLabel() {
    const label = $('version-label');
    if (!label) return;
    const version = updateState && updateState.currentVersion
      ? String(updateState.currentVersion)
      : '';
    label.textContent = version ? `v${version}` : '';
    label.title = version ? `Alvum ${version}` : '';
    label.hidden = !version;
  }

  function updatePanelTitle(state) {
    if (!state || state.status === 'current') return `Alvum ${state && state.currentVersion ? state.currentVersion : ''} is current`.trim();
    if (state.status === 'downloaded') return `Alvum ${state.latestVersion || 'update'} is ready`;
    if (state.status === 'downloading') return `Downloading ${state.latestVersion || 'update'}`;
    if (state.status === 'checking') return 'Checking for updates';
    if (state.status === 'installing') return 'Installing update';
    if (state.status === 'error') return 'Update unavailable';
    return 'Updates';
  }

  function updatePanelMeta(state) {
    const checked = state && state.checkedAt
      ? `Last checked ${new Date(state.checkedAt).toLocaleString()}. `
      : '';
    const cadence = 'Auto-checks once per day; Check now bypasses the daily throttle.';
    if (!state) return cadence;
    if (state.error) return state.error;
    if (state.status === 'downloaded') return 'The update downloaded in the background. Restart Alvum to install it.';
    if (state.status === 'downloading') return 'The update is downloading in the background.';
    if (state.status === 'checking') return 'Alvum is checking the latest GitHub release.';
    return `${checked}${cadence}`;
  }

  function updatePanelDot(state) {
    if (!state || state.status === 'error') return 'red';
    if (state.status === 'downloaded' || state.status === 'downloading') return 'yellow';
    return 'green';
  }

  function renderUpdatePanel() {
    const state = updateState || {};
    $('update-panel-title').textContent = updatePanelTitle(state);
    $('update-panel-meta').textContent = updatePanelMeta(state);
    $('update-panel-dot').className = `dot ${updatePanelDot(state)}`;
    const ready = state.supported !== false && state.status === 'downloaded';
    const busy = state.status === 'checking' || state.status === 'downloading' || state.status === 'installing';
    $('update-panel-actions').className = `footer-buttons ${ready ? 'two' : 'single'}`;
    $('update-panel-actions').hidden = false;
    $('update-check-now').disabled = busy || state.supported === false;
    $('update-confirm-restart').hidden = !ready;
    $('update-confirm-restart').disabled = !ready || state.status === 'installing';
    requestPopoverResize();
  }

  function captureInputById(id) {
    return captureInputs && Array.isArray(captureInputs.inputs)
      ? captureInputs.inputs.find((input) => input.id === id)
      : null;
  }

  function renderCaptureInputList(list, titleEl) {
    if (!list) return;
    list.replaceChildren();
    const inputs = captureInputs && Array.isArray(captureInputs.inputs) ? captureInputs.inputs : [];
    if (titleEl) {
      titleEl.textContent = 'Sources';
    }
    for (const input of inputs) {
      const row = document.createElement('div');
      row.className = 'input-row status-only-row';
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 'value status-line';
      const dot = document.createElement('span');
      dot.className = `status-dot ${input.enabled ? 'live' : ''}`;
      const name = document.createElement('span');
      name.textContent = input.label;
      title.append(dot, name);
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = input.blocked_permissions && input.blocked_permissions.length
        ? permissionIssueText(input)
        : (input.detail || input.kind || 'input');
      text.append(title, meta);
      const state = document.createElement('span');
      state.className = `state-badge ${input.enabled ? 'on' : ''}`;
      state.textContent = input.blocked_permissions && input.blocked_permissions.length
        ? 'Blocked'
        : (input.enabled ? 'Enabled' : 'Disabled');
      row.append(text, state);
      list.appendChild(row);
    }
  }

  function renderCaptureInputs() {
    renderCaptureInputList($('capture-inputs-list'), $('capture-inputs-title'));
    requestPopoverResize();
  }

  async function refreshCaptureInputs(force) {
    if (!force && currentState.captureInputs) captureInputs = currentState.captureInputs;
    if (!captureInputs || force) captureInputs = await window.alvum.captureInputs();
    renderCaptureInputs();
  }

  async function saveCaptureInputSetting(input, key, nextValue, control) {
    if (!input || !key) return;
    if (control) control.disabled = true;
    let result;
    try {
      result = await window.alvum.captureInputSetSetting(input.id, key, nextValue);
      if (result && result.captureInputs) captureInputs = result.captureInputs;
      else captureInputs = await window.alvum.captureInputs();
      if (captureInputParent === 'extension-detail') await refreshExtensions(true);
      if (result && result.ok === false) console.error('[capture-input] setting update failed', result.error || 'setting update failed');
    } catch (err) {
      console.error('[capture-input] setting update failed', extensionErrorMessage(err));
      captureInputs = await window.alvum.captureInputs();
    }
    renderCaptureInputSettings();
  }

  const SETTING_OPTION_SETS = {
    mode: {
      options: [
        { value: 'local', label: 'Local' },
        { value: 'provider', label: 'Provider' },
        { value: 'ocr', label: 'OCR' },
        { value: 'off', label: 'Off' },
      ],
    },
    vision: {
      options: [
        { value: 'ocr', label: 'OCR' },
        { value: 'provider', label: 'Provider' },
        { value: 'off', label: 'Off' },
      ],
    },
    whisper_language: {
      options: [
        { value: 'en', label: 'English' },
        { value: 'auto', label: 'Auto detect' },
      ],
    },
  };

  const LOCAL_AUDIO_PROCESSOR_SETTING_KEYS = new Set([
    'whisper_model',
    'whisper_language',
    'diarization_enabled',
    'diarization_model',
    'pyannote_command',
    'pyannote_hf_token',
    'speaker_registry',
  ]);

  function settingOptions(setting, key) {
    if (setting && Array.isArray(setting.options) && setting.options.length) return setting.options;
    const fallback = SETTING_OPTION_SETS[String(key || '')];
    return fallback && Array.isArray(fallback.options) ? fallback.options : [];
  }

  function settingControlKind(key, value, options = []) {
    key = String(key || '');
    if (options.length) return 'enum';
    if (key.endsWith('_token')) return 'secret';
    if (key === 'since') return 'datetime';
    if (key === 'session_dir' || key.endsWith('_dir')) return 'directory';
    if (typeof value === 'boolean') return 'boolean';
    if (typeof value === 'number') return 'number';
    return 'text';
  }

  function settingValueLabel(value, options = []) {
    const match = options.find((option) => String(option.value) === String(value));
    if (match) return match.label || String(match.value);
    if (typeof value === 'boolean') return value ? 'Enabled' : 'Disabled';
    if (value == null || value === '') return 'Not configured';
    return String(value);
  }

  function toDateTimeLocalValue(value) {
    if (!value) return '';
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return '';
    const pad = (n) => String(n).padStart(2, '0');
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
  }

  function fromDateTimeLocalValue(value) {
    if (!value) return '';
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return value;
    return date.toISOString().replace('.000Z', 'Z');
  }

  function whisperVariantFromPath(path) {
    const match = String(path || '').match(/(?:^|\/)ggml-([A-Za-z0-9._-]+)\.bin$/);
    return match ? match[1] : 'base.en';
  }

  function selectedWhisperModelPath() {
    const connectors = extensionSummary && Array.isArray(extensionSummary.connectors)
      ? extensionSummary.connectors
      : [];
    const audio = connectors.find((connector) => connector && (
      connector.component_id === 'alvum.audio/audio'
      || (connector.package_id === 'alvum.audio' && connector.connector_id === 'audio')
      || connector.id === 'alvum.audio/audio'
    ));
    const controls = audio && Array.isArray(audio.processor_controls) ? audio.processor_controls : [];
    const processor = controls.find((control) => control && control.component === 'alvum.audio/whisper');
    const settings = processor && Array.isArray(processor.settings) ? processor.settings : [];
    const model = settings.find((setting) => setting && setting.key === 'whisper_model');
    return model && model.value ? String(model.value) : '';
  }

  function whisperVariantFromSelectedModel() {
    return whisperVariantFromPath(selectedWhisperModelPath());
  }

  function renderSettingEditor(settings, setting, value, saveFn) {
    const row = document.createElement('div');
    row.className = 'settings-row editable-setting-row';
    const key = setting && setting.key ? setting.key : '';
    const options = settingOptions(setting, key);
    const kind = settingControlKind(key, value, options);
    const text = document.createElement('div');
    const label = document.createElement('div');
    label.className = 'value';
    label.textContent = (setting && setting.label) || key;
    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = (setting && setting.detail) || settingValueLabel(value, options);
    text.append(label, meta);

    const controls = document.createElement('div');
    controls.className = 'setting-control-row';

    if (kind === 'boolean') {
      controls.classList.add('single');
      const toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = `switch ${value ? 'on' : ''}`;
      toggle.textContent = value ? 'On' : 'Off';
      toggle.onclick = () => saveFn(!value, toggle);
      controls.appendChild(toggle);
      row.append(text, controls);
      settings.appendChild(row);
      return;
    }

    if (kind === 'directory') {
      const current = document.createElement('span');
      current.className = 'setting-path-value';
      current.textContent = value ? String(value) : 'Not configured';
      const browse = document.createElement('button');
      browse.type = 'button';
      browse.textContent = 'Browse';
      browse.onclick = async () => {
        browse.disabled = true;
        try {
          const result = await window.alvum.chooseDirectory(String(value || ''));
          if (result && result.ok && result.path) await saveFn(result.path, browse);
          else browse.disabled = false;
        } catch (err) {
          console.error('[settings] directory chooser failed', extensionErrorMessage(err));
          browse.disabled = false;
        }
      };
      controls.append(current, browse);
      row.append(text, controls);
      settings.appendChild(row);
      return;
    }

    const editor = kind === 'enum'
      ? document.createElement('select')
      : document.createElement('input');
    editor.className = 'setting-editor';
    editor.setAttribute('aria-label', key);
    if (kind === 'enum') {
      for (const option of options) {
        const item = document.createElement('option');
        item.value = String(option.value);
        item.textContent = option.label || String(option.value);
        editor.appendChild(item);
      }
      editor.value = value == null ? '' : String(value);
    } else if (kind === 'datetime') {
      editor.type = 'datetime-local';
      editor.value = toDateTimeLocalValue(value);
    } else {
      editor.type = kind === 'number' ? 'number' : (kind === 'secret' ? 'password' : 'text');
      editor.placeholder = kind === 'secret' && setting && setting.configured
        ? 'Configured'
        : ((setting && setting.placeholder) || '');
      editor.value = kind === 'secret' ? '' : (value == null ? '' : String(value));
    }

    const save = document.createElement('button');
    save.type = 'button';
    save.textContent = 'Save';
    save.disabled = true;
    const original = editor.value;
    editor.oninput = () => {
      save.disabled = kind === 'secret'
        ? editor.value.trim() === ''
        : editor.value === original;
    };
    editor.onchange = editor.oninput;
    editor.onkeydown = (e) => {
      if (e.key !== 'Enter' || save.disabled) return;
      e.preventDefault();
      save.click();
    };
    save.onclick = () => {
      const nextValue = kind === 'number'
        ? Number(editor.value)
        : (kind === 'datetime' ? fromDateTimeLocalValue(editor.value) : editor.value);
      saveFn(nextValue, save);
    };
    controls.append(editor, save);
    row.append(text, controls);
    settings.appendChild(row);
  }

  function renderEditableSettingRow(settings, input, key, value) {
    renderSettingEditor(settings, { key, label: key }, value, (nextValue, control) =>
      saveCaptureInputSetting(input, key, nextValue, control));
  }

  function renderCaptureInputSettings() {
    const input = captureInputById(selectedCaptureInput);
    const connectorScoped = captureInputParent === 'extension-detail';
    const summary = $('capture-input-summary');
    summary.hidden = connectorScoped;
    if (input && connectorScoped) $('view-title').textContent = input.label || 'Input';
    $('capture-input-title').textContent = input ? input.label : 'No input selected';
    $('capture-input-meta').textContent = input ? `${input.enabled ? 'Enabled' : 'Disabled'} · ${input.detail || input.kind}` : '';
    $('capture-input-toggle').textContent = input && input.enabled ? 'Disable' : 'Enable';
    $('capture-input-toggle').disabled = !input;
    const settings = $('capture-input-settings');
    settings.replaceChildren();
    if (!input) return;
    appendPermissionIssueRows(settings, input);
    const entries = Object.entries(input.settings || {});
    if (entries.length === 0 && !permissionIssuesFrom(input).length) {
      const empty = document.createElement('div');
      empty.className = 'settings-row';
      empty.innerHTML = '<div><div class="value">No configurable settings</div><div class="meta">This input only has an enabled flag.</div></div>';
      settings.appendChild(empty);
      return;
    }
    for (const [key, value] of entries) {
      renderEditableSettingRow(settings, input, key, value);
    }
    requestPopoverResize();
  }

  function renderProgressElements() {
    const bar = $('progress-bar');
    if (!bar) return;
    const run = briefingRun(selectedBriefingDate);
    const progress = progressByDate[selectedBriefingDate] || (run && run.progress) || null;
    const pct = run ? progressPct(progress, run.lastPct) : progressPct(progress, 0);
    bar.value = pct;
    const label = $('progress-label');
    if (label) {
      label.textContent = progress
        ? (progress.total > 1 ? `${stageLabel(progress.stage)} ${progress.current}/${progress.total}` : `${stageLabel(progress.stage)}...`)
        : 'starting...';
    }
    const elapsedLabel = $('progress-elapsed');
    const startedAt = run && run.startedAtMs ? run.startedAtMs : runStartedAt;
    if (elapsedLabel) elapsedLabel.textContent = startedAt ? elapsed(startedAt) : '0s';
    const stageIdx = progress ? STAGES.indexOf(progress.stage) : -1;
    document.querySelectorAll('#stages-list li').forEach((li, i) => {
      li.classList.toggle('active', i === stageIdx && progress.current < progress.total);
      li.classList.toggle('done', i < stageIdx || (i === stageIdx && progress.current >= progress.total));
    });
  }

  function appendProgressBlock(parent) {
    const block = document.createElement('div');
    block.id = 'progress-block';
    const bar = document.createElement('progress');
    bar.id = 'progress-bar';
    bar.value = progressPct(progressByDate[selectedBriefingDate], briefingRun(selectedBriefingDate)?.lastPct);
    bar.max = 100;
    const meta = document.createElement('div');
    meta.className = 'meta';
    const label = document.createElement('span');
    label.id = 'progress-label';
    const elapsedLabel = document.createElement('span');
    elapsedLabel.id = 'progress-elapsed';
    elapsedLabel.style.float = 'right';
    meta.append(label, elapsedLabel);
    const stages = document.createElement('ul');
    stages.className = 'stages';
    stages.id = 'stages-list';
    STAGES.forEach((stage) => {
      const li = document.createElement('li');
      li.dataset.stage = stage;
      li.textContent = stageLabel(stage);
      stages.appendChild(li);
    });
    block.append(bar, meta, stages);
    parent.appendChild(block);
    renderProgressElements();
  }

  function renderState(s) {
    currentState = s;
    const running = !!s.captureRunning;
    $('capture-pill').textContent = running ? 'Running' : 'Stopped';
    $('capture-pill').classList.toggle('live', running);
    $('capture-label').textContent = running ? 'Running' : 'Stopped';
    $('capture-state-dot').classList.toggle('live', running);
    const capture = s.captureStats || { summary: s.stats || '0 files · 0 B', detail: '' };
    captureInputs = s.captureInputs || captureInputs;
    if (s.providerSummary) applyProviderSummary(s.providerSummary);
    if (s.providerStats) mergeProviderTelemetrySnapshot(s.providerStats);
    if (s.updateState) updateState = s.updateState;
    if (s.synthesisSchedule) synthesisSchedule = s.synthesisSchedule;
    if (activeView === 'profile-schedule-detail') renderProfileSchedule();
    renderVersionLabel();
    renderMainBadges();
    renderUpdateChip();
    $('capture-summary').title = capture.detail || capture.summary || '';
    if (s.providerIssue && s.providerIssue.message) {
      const key = `${s.providerIssue.level || 'warning'}:${s.providerIssue.message}`;
      if (key !== lastRenderedProviderIssueKey) {
        lastRenderedProviderIssueKey = key;
        showMenuNotification(s.providerIssue.message, s.providerIssue.level || 'warning');
      }
    } else {
      lastRenderedProviderIssueKey = '';
    }
    if (activeView === 'capture') renderCaptureInputs();
    if (activeView === 'extension-detail') renderExtensionDetail();
    if (activeView === 'providers') renderProviderProbe();
    if (activeView === 'provider-add') renderProviderAdd();
    if (activeView === 'provider-detail') renderProviderDetail();
    if (activeView === 'logs' && logKind === 'updates') renderUpdatePanel();

    const previousProgressByDate = progressByDate;
    progressByDate = {};
    for (const [date, run] of Object.entries(s.briefingRuns || {})) {
      progressByDate[date] = run.progress || previousProgressByDate[date] || null;
    }
    const briefRunning = Object.keys(s.briefingRuns || {}).length > 0;
    if (briefRunning && !prevBriefingRunning) {
      lastPct = 0;
      runStartedAt = Date.now();
      activeProgress = null;
      eventRowsByDate = {};
      renderEventLog();
    }
    prevBriefingRunning = briefRunning;
    if (s.briefingCalendar) renderBriefingCalendar(s.briefingCalendar);
    requestPopoverResize();
  }

  function renderProgress(p) {
    if (!runStartedAt) runStartedAt = Date.now();
    const date = p.briefingDate || selectedBriefingDate;
    activeProgress = p;
    progressByDate[date] = p;
    if (currentState.briefingRuns && currentState.briefingRuns[date]) {
      currentState.briefingRuns[date].progress = p;
      currentState.briefingRuns[date].lastPct = progressPct(p, currentState.briefingRuns[date].lastPct);
    }
    lastPct = progressPct(p, lastPct);
    appendProgressLog(p);
    renderProgressElements();
    if (date === selectedBriefingDate) renderSelectedDateActions();
    requestPopoverResize();
  }

  function fmtEvent(evt) {
    const ts = evt.ts ? new Date(evt.ts).toLocaleTimeString() : '';
    if (evt.kind === 'input_inventory') return `${ts} ${evt.connector}/${evt.source} = ${evt.ref_count}`;
    if (evt.kind === 'llm_call_start') return `${ts} ${evt.provider || 'provider'} ${evt.call_site} (~${evt.prompt_tokens_estimate || 0} input tok)`;
    if (evt.kind === 'llm_call_end') {
      const out = evt.output_tokens || evt.response_tokens_estimate || 0;
      const rate = evt.tokens_per_sec || evt.tokens_per_sec_estimate;
      const stopReason = evt.stop_reason ? ` · stop=${evt.stop_reason}` : '';
      const contentBlocks = Array.isArray(evt.content_block_kinds) && evt.content_block_kinds.length
        ? ` · blocks=${evt.content_block_kinds.join('+')}`
        : '';
      return `${ts} ${evt.provider || 'provider'} ${evt.call_site} ${evt.ok ? 'ok' : 'failed'} (${out} output tok${rate ? ` · ${formatRate(rate)}` : ''}${stopReason}${contentBlocks})`;
    }
    if (evt.kind === 'warning' || evt.kind === 'error') return `${ts} ${evt.kind}: ${evt.source}: ${evt.message}`;
    if (evt.kind === 'stage_exit') return `${ts} ${evt.stage} ${evt.ok ? 'ok' : 'failed'} ${evt.elapsed_ms}ms`;
    return `${ts} ${evt.kind} ${evt.stage || ''}`.trim();
  }

  function renderEventLog() {
    if (activeView === 'briefing-log') renderBriefingLogView();
    requestPopoverResize();
  }

  async function loadPersistedBriefingLog(date, force = false) {
    if (!date || !window.alvum.briefingRunLogDate) return null;
    if (!force && persistedRunLogsByDate[date]) return persistedRunLogsByDate[date];
    try {
      const result = await window.alvum.briefingRunLogDate(date);
      persistedRunLogsByDate[date] = result && result.ok !== false
        ? result
        : { ok: false, text: result && result.error ? result.error : 'Could not load run log' };
      if (activeView === 'briefing-log' && (logDate || selectedBriefingDate) === date) {
        renderBriefingLogView(false);
      }
      return persistedRunLogsByDate[date];
    } catch (err) {
      persistedRunLogsByDate[date] = { ok: false, text: extensionErrorMessage(err) };
      if (activeView === 'briefing-log' && (logDate || selectedBriefingDate) === date) {
        renderBriefingLogView(false);
      }
      return persistedRunLogsByDate[date];
    }
  }

  function briefingLogText(date) {
    const liveRows = eventRowsByDate[date] || [];
    const persisted = persistedRunLogsByDate[date];
    const parts = [];
    if (liveRows.length) parts.push(`Live events:\n${liveRows.join('\n')}`);
    if (persisted && persisted.text) parts.push(`${liveRows.length ? 'Persisted run log:\n' : ''}${persisted.text}`);
    return parts.join('\n\n');
  }

  function renderBriefingLogView(load = true) {
    const date = logDate || selectedBriefingDate;
    const rows = eventRowsByDate[date] || [];
    const persisted = persistedRunLogsByDate[date];
    const persistedText = persisted && persisted.text ? persisted.text : '';
    const run = persisted && persisted.run ? persisted.run : null;
    $('briefing-log-title').textContent = date ? displayDate(date) : 'No day selected';
    $('briefing-log-meta').textContent = rows.length
      ? `${rows.length} live event${rows.length === 1 ? '' : 's'}${run ? ` · ${run.status || 'run'}` : ''}`
      : (run ? `${run.status || 'run'} · ${run.run_id || 'latest run'}` : 'No progress events yet');
    $('briefing-event-log-view').textContent = briefingLogText(date) || persistedText || '(empty)';
    if (load && date && !persistedRunLogsByDate[date]) {
      loadPersistedBriefingLog(date);
    }
    requestPopoverResize();
  }

  function openBriefingLogView(date) {
    logDate = date;
    setView('briefing-log');
    loadPersistedBriefingLog(date, true);
  }

  function appendLogRow(date, row) {
    const rows = eventRowsByDate[date] || [];
    rows.push(row);
    eventRowsByDate[date] = rows.length > 80 ? rows.slice(-80) : rows;
  }

  function appendProgressLog(progress) {
    const date = progress.briefingDate || selectedBriefingDate || 'global';
    const ts = progress.ts ? new Date(progress.ts).toLocaleTimeString() : new Date().toLocaleTimeString();
    appendLogRow(date, `${ts} progress: ${progress.stage} ${progress.current}/${progress.total}`);
    if (date === selectedBriefingDate || (activeView === 'briefing-log' && date === logDate)) renderEventLog();
  }

  function appendEvent(evt) {
    recordProviderTelemetry(evt);
    const date = evt.briefingDate || selectedBriefingDate || 'global';
    appendLogRow(date, fmtEvent(evt));
    if (date === selectedBriefingDate || (activeView === 'briefing-log' && date === logDate)) renderEventLog();
    if (activeView === 'provider-detail') renderProviderDetail();
    if (activeView === 'logs' && logKind === 'pipeline') {
      $('log-content').textContent += `${JSON.stringify(evt)}\n`;
    }
  }

  function briefingStatusText(day) {
    if (!day) return 'Select a day';
    const schedule = synthesisScheduleValue();
    if (schedule.running_date === day.date || briefingRun(day.date)) return 'Synthesis running';
    if (schedule.queued_dates.includes(day.date)) return 'Queued for synthesis';
    if (day.status === 'success') return 'Synthesis generated';
    if (day.status === 'failed') return 'Generation failed';
    if (day.status === 'canceled') return 'Synthesis canceled';
    if (day.status === 'captured') return 'Data captured; no synthesis yet';
    return 'No synthesis generated';
  }

  async function cancelBriefingDateFromUi(date) {
    const run = briefingRun(date);
    if (run) run.canceling = true;
    renderSelectedDateActions();
    const result = await window.alvum.cancelBriefingDate(date);
    if (result && result.ok === false) {
      const latestRun = briefingRun(date);
      if (latestRun) latestRun.canceling = false;
      renderSelectedDateActions();
      showMenuNotification(result.error || 'Synthesis could not be canceled.', 'warning', 'Cancel failed');
    }
  }

  function renderBriefingCalendar(calendar) {
    currentCalendar = calendar;
    $('calendar-month-label').textContent = calendar.label || calendar.month;
    if (!selectedBriefingDate || !calendarDay(selectedBriefingDate)) {
      const today = calendar.days.find((day) => day.isToday);
      const firstInMonth = calendar.days.find((day) => day.inMonth);
      selectedBriefingDate = (today || firstInMonth || calendar.days[0]).date;
    }

    const grid = $('calendar-grid');
    grid.replaceChildren();
    for (const weekday of ['S', 'M', 'T', 'W', 'T', 'F', 'S']) {
      const label = document.createElement('div');
      label.className = 'calendar-weekday';
      label.textContent = weekday;
      grid.appendChild(label);
    }
    for (const day of calendar.days) {
      const schedule = synthesisScheduleValue();
      const queuedForDay = schedule.queued_dates.includes(day.date);
      const button = document.createElement('button');
      button.type = 'button';
      button.className = [
        'calendar-day',
        day.inMonth ? '' : 'outside-month',
        day.isToday ? 'today' : '',
        day.date === selectedBriefingDate ? 'selected' : '',
        briefingRun(day.date) ? 'generating' : '',
        queuedForDay ? 'queued' : '',
      ].filter(Boolean).join(' ');
      button.title = `${day.date}\n${briefingStatusText(day)}\n${day.artifacts || '0 files · 0 B'}`;
      const number = document.createElement('span');
      number.className = 'day-number';
      number.textContent = String(Number(day.date.slice(8, 10)));
      const dot = document.createElement('span');
      dot.className = `calendar-dot ${queuedForDay ? 'queued' : (day.status || 'empty')}`;
      button.append(number, dot);
      button.onclick = () => {
        selectedBriefingDate = day.date;
        renderBriefingCalendar(currentCalendar);
        renderEventLog();
      };
      grid.appendChild(button);
    }
    renderSelectedDateActions();
    requestPopoverResize();
  }

  function renderSelectedDateActions() {
    const day = calendarDay(selectedBriefingDate);
    const wrap = $('selected-date-actions');
    wrap.replaceChildren();
    const selectedRun = briefingRun(selectedBriefingDate);
    const runningDates = Object.keys(currentState.briefingRuns || {});
    const runningForDay = !!selectedRun;
    const cancelingForDay = !!(selectedRun && selectedRun.canceling);
    const schedule = synthesisScheduleValue();
    const queuedForDay = schedule.queued_dates.includes(selectedBriefingDate);
    wrap.classList.toggle('generating', runningForDay);
    wrap.classList.toggle('queued', queuedForDay);
    const title = document.createElement('div');
    title.className = 'value';
    title.textContent = runningForDay
      ? `${cancelingForDay ? 'Canceling' : 'Synthesizing'} ${displayDate(selectedBriefingDate)}`
      : (queuedForDay ? `Queued ${displayDate(selectedBriefingDate)}`
      : (selectedBriefingDate ? displayDate(selectedBriefingDate) : 'Select a day'));
    const meta = document.createElement('div');
    meta.className = 'meta';
    const reason = day && day.failure && day.failure.reason ? ` · ${day.failure.reason}` : '';
    if (runningForDay) {
      const pct = progressPct(progressByDate[selectedBriefingDate] || selectedRun.progress, selectedRun.lastPct);
      meta.textContent = `${cancelingForDay ? 'Canceling' : 'Synthesizing'} ${pct}% · ${day ? (day.artifacts || '0 files · 0 B') : 'capture artifacts'}`;
    } else if (queuedForDay) {
      meta.textContent = day
        ? `Waiting for earlier days · ${day.artifacts || '0 files · 0 B'}`
        : 'Waiting in synthesis queue';
    } else if (runningDates.length > 0) {
      meta.textContent = day
        ? `${briefingStatusText(day)} · ${day.artifacts || '0 files · 0 B'} · ${runningDates.length} day${runningDates.length === 1 ? '' : 's'} generating`
        : `${runningDates.length} day${runningDates.length === 1 ? '' : 's'} generating`;
    } else {
      meta.textContent = day
        ? `${briefingStatusText(day)} · ${day.artifacts || '0 files · 0 B'}${reason}`
        : 'Pick a date from the calendar';
    }
    wrap.append(title, meta);
    if (!day) return;

    const actions = document.createElement('div');
    actions.className = 'date-action-buttons';
    const generate = document.createElement('button');
    generate.type = 'button';
    generate.className = day.hasBriefing ? '' : 'primary full-row';
    generate.textContent = queuedForDay
      ? 'Queued'
      : (day.hasBriefing ? 'Resynthesize' : (day.status === 'failed' ? 'Retry' : 'Synthesize'));
    generate.disabled = queuedForDay || !day.hasCapture;
    generate.onclick = async () => {
      runStartedAt = Date.now();
      lastPct = 0;
      activeProgress = null;
      currentState.briefingRunning = true;
      currentState.briefingRuns = currentState.briefingRuns || {};
      currentState.briefingRuns[day.date] = {
        date: day.date,
        startedAt: new Date().toLocaleTimeString(),
        startedAtMs: Date.now(),
        lastPct: 0,
        progress: null,
      };
      renderSelectedDateActions();
      const result = await window.alvum.startBriefingDate(day.date);
      if (result && result.ok === false) {
        delete currentState.briefingRuns[day.date];
        currentState.briefingRunning = Object.keys(currentState.briefingRuns).length > 0;
        renderSelectedDateActions();
        const message = result.error || 'Synthesis could not start.';
        showMenuNotification(message, 'warning', 'Synthesis blocked');
        if (result.setupTarget === 'providers') setView('providers');
        else if (result.setupTarget === 'capture') setView('capture');
      }
    };
    if (runningForDay) {
      const progressLog = document.createElement('button');
      progressLog.type = 'button';
      progressLog.textContent = 'Progress log';
      progressLog.onclick = () => openBriefingLogView(day.date);
      const cancel = document.createElement('button');
      cancel.type = 'button';
      cancel.textContent = cancelingForDay ? 'Canceling...' : 'Cancel';
      cancel.disabled = cancelingForDay;
      cancel.onclick = () => cancelBriefingDateFromUi(day.date);
      actions.append(progressLog, cancel);
    } else if (queuedForDay) {
      const progressLog = document.createElement('button');
      progressLog.type = 'button';
      progressLog.textContent = 'Progress log';
      progressLog.onclick = () => openBriefingLogView(day.date);
      actions.append(progressLog, generate);
    } else if (day.status === 'failed') {
      const details = document.createElement('button');
      details.type = 'button';
      details.textContent = 'View details';
      details.onclick = () => openBriefingLogView(day.date);
      generate.classList.remove('full-row');
      actions.append(generate, details);
    } else if (day.status === 'canceled') {
      const details = document.createElement('button');
      details.type = 'button';
      details.textContent = 'Progress log';
      details.onclick = () => openBriefingLogView(day.date);
      generate.classList.remove('full-row');
      actions.append(generate, details);
    } else if (day.hasBriefing) {
      const view = document.createElement('button');
      view.type = 'button';
      view.className = 'primary full-row';
      view.textContent = 'View Synthesis';
      view.onclick = () => openBriefingReader(day.date);
      const graph = document.createElement('button');
      graph.type = 'button';
      graph.textContent = 'Decision graph';
      graph.onclick = () => openDecisionGraphView(day.date);
      const progressLog = document.createElement('button');
      progressLog.type = 'button';
      progressLog.textContent = 'Progress log';
      progressLog.onclick = () => openBriefingLogView(day.date);
      generate.classList.add('full-row');
      actions.append(view, graph, progressLog, generate);
    } else {
      actions.appendChild(generate);
    }
    wrap.appendChild(actions);
    if (runningForDay) appendProgressBlock(wrap);
  }

	  function emptyProfile() {
	    return {
	      intentions: [],
	      domains: [],
	      interests: [],
	      writing: {
	        detail_level: 'detailed',
	        tone: 'direct',
	        outline: DEFAULT_DAILY_BRIEFING_OUTLINE,
	      },
	      advanced_instructions: '',
	      ignored_suggestions: [],
	    };
	  }

	  function ensureSynthesisProfileShape() {
	    synthesisProfile = synthesisProfile || emptyProfile();
	    synthesisProfile.intentions = Array.isArray(synthesisProfile.intentions) ? synthesisProfile.intentions : [];
	    synthesisProfile.domains = Array.isArray(synthesisProfile.domains) ? synthesisProfile.domains : [];
	    synthesisProfile.interests = Array.isArray(synthesisProfile.interests) ? synthesisProfile.interests : [];
	    const writing = synthesisProfile.writing || {};
	    synthesisProfile.writing = {
	      detail_level: writing.detail_level || 'detailed',
	      tone: writing.tone || 'direct',
	      outline: writing.outline || DEFAULT_DAILY_BRIEFING_OUTLINE,
	    };
	    return synthesisProfile;
	  }

	  function renderActiveSynthesisProfileView() {
	    if (activeView === 'synthesis-profile') renderSynthesisProfile();
	    if (activeView === 'profile-intentions-list') renderProfileIntentions();
	    if (activeView === 'profile-intention-detail') renderProfileIntentionDetail();
	    if (activeView === 'profile-domains-list') renderProfileDomains();
	    if (activeView === 'profile-domain-detail') renderProfileDomainDetail();
	    if (activeView === 'profile-interests-list') renderProfileInterests();
	    if (activeView === 'profile-interest-detail') renderProfileInterestDetail();
	    if (activeView === 'profile-voices-list') renderProfileVoices();
	    if (activeView === 'profile-voice-detail') renderProfileVoiceDetail();
	    if (activeView === 'profile-writing-detail') renderProfileWriting();
    if (activeView === 'profile-schedule-detail') renderProfileSchedule();
	    if (activeView === 'profile-advanced-detail') renderProfileAdvanced();
	  }

	  async function refreshSynthesisProfile(force = true) {
	    if (synthesisProfileLoading) return;
	    if (!force && synthesisProfile) return;
	    synthesisProfileLoading = true;
	    synthesisProfileError = null;
	    renderActiveSynthesisProfileView();
	    try {
	      const [profileResult, suggestionsResult] = await Promise.all([
	        window.alvum.synthesisProfile(),
	        window.alvum.synthesisProfileSuggestions(),
      ]);
      if (!profileResult || !profileResult.ok) {
        synthesisProfileError = (profileResult && profileResult.error) || 'Could not load synthesis profile';
      } else {
        synthesisProfile = profileResult.profile || emptyProfile();
      }
      synthesisProfileSuggestions = suggestionsResult && Array.isArray(suggestionsResult.suggestions)
        ? suggestionsResult.suggestions
        : [];
	    } catch (err) {
	      synthesisProfileError = extensionErrorMessage(err);
	    } finally {
	      synthesisProfileLoading = false;
	      renderActiveSynthesisProfileView();
	    }
	  }

  function renderSynthesisProfile() {
    if (activeView !== 'synthesis-profile') return;
    if (synthesisProfileLoading && !synthesisProfile) {
      renderSimpleCard($('profile-menu'), 'Loading profile', 'Reading synthesis customization.');
      requestPopoverResize();
      return;
    }
    if (synthesisProfileError && !synthesisProfile) {
      renderSimpleCard($('profile-menu'), 'Could not load profile', synthesisProfileError);
      requestPopoverResize();
      return;
    }
	    ensureSynthesisProfileShape();
	    const menu = $('profile-menu');
	    menu.replaceChildren();
	    menu.append(
	      profileMenuRow(
	        'Intentions',
	        profileEnabledMeta(synthesisProfile.intentions, 'active'),
	        () => setView('profile-intentions-list'),
	      ),
	      profileMenuRow(
	        'Domains',
	        profileEnabledMeta(synthesisProfile.domains, 'enabled'),
	        () => setView('profile-domains-list'),
	      ),
	      profileMenuRow(
	        'Tracked',
	        profileTrackedSummary(),
	        () => setView('profile-interests-list'),
	      ),
	      profileMenuRow(
	        'Writing',
	        profileWritingSummary(),
	        () => setView('profile-writing-detail'),
	      ),
      profileMenuRow(
        'Schedule',
        synthesisScheduleSummary(),
        () => setView('profile-schedule-detail'),
      ),
	      profileMenuRow(
	        'Advanced',
	        (synthesisProfile.advanced_instructions || '').trim()
	          ? 'Custom instructions set'
	          : 'No advanced instructions',
	        () => setView('profile-advanced-detail'),
	      ),
	    );
	    updateProfileSaveButtons();
	    requestPopoverResize();
	  }

	  function profileEnabledMeta(items, label) {
	    const total = Array.isArray(items) ? items.length : 0;
	    const enabled = (items || []).filter((item) => item.enabled !== false).length;
	    if (!total) return `No ${label} items`;
	    return `${enabled}/${total} ${label}`;
	  }

	  function profileTrackedSummary() {
	    const base = profileEnabledMeta(synthesisProfile.interests, 'tracked');
	    const suggestions = synthesisProfileSuggestions.length;
	    const voices = speakerItems().length;
	    const withSuggestions = suggestions ? `${base} · ${suggestions} suggested` : base;
	    return voices ? `${withSuggestions} · ${voices} voice clusters` : withSuggestions;
	  }

	  function profileWritingSummary() {
	    const writing = (synthesisProfile && synthesisProfile.writing) || {};
	    const detail = profileOptionLabel(writing.detail_level || 'detailed');
	    const tone = profileOptionLabel(writing.tone || 'direct');
	    return (writing.outline || '').trim() ? `${detail} · ${tone} · outline set` : `${detail} · ${tone}`;
	  }

  function synthesisScheduleValue() {
    return {
      enabled: !!(synthesisSchedule && synthesisSchedule.enabled),
      time: synthesisSchedule && synthesisSchedule.time ? synthesisSchedule.time : '07:00',
      policy: synthesisSchedule && synthesisSchedule.policy ? synthesisSchedule.policy : 'completed_days',
      setup_completed: !!(synthesisSchedule && synthesisSchedule.setup_completed),
      setup_pending: !!(synthesisSchedule && synthesisSchedule.setup_pending),
      last_auto_run_date: synthesisSchedule && synthesisSchedule.last_auto_run_date ? synthesisSchedule.last_auto_run_date : '',
      due_dates: synthesisSchedule && Array.isArray(synthesisSchedule.due_dates) ? synthesisSchedule.due_dates : [],
      queued_dates: synthesisSchedule && Array.isArray(synthesisSchedule.queued_dates) ? synthesisSchedule.queued_dates : [],
      running_date: synthesisSchedule ? synthesisSchedule.running_date : null,
      last_error: synthesisSchedule ? synthesisSchedule.last_error : null,
    };
  }

  function synthesisScheduleSummary() {
    const schedule = synthesisScheduleValue();
    if (schedule.setup_pending) return 'Enables after first synthesis';
    if (schedule.enabled) return `Daily at ${schedule.time} · completed days`;
    return 'Off';
  }

	  function profileMenuRow(title, meta, onOpen) {
	    const row = profileRow(title, meta);
	    appendProfileDrill(row, onOpen);
	    return row;
	  }

  function renderSimpleCard(parent, title, meta) {
    parent.replaceChildren();
    appendSimpleCard(parent, title, meta);
  }

  function appendSimpleCard(parent, title, meta) {
    const row = document.createElement('div');
    row.className = 'summary-row';
    const text = document.createElement('div');
    const value = document.createElement('div');
    value.className = 'value';
    value.textContent = title;
    const detail = document.createElement('div');
    detail.className = 'meta';
    detail.textContent = meta || '';
    text.append(value, detail);
    row.appendChild(text);
    parent.appendChild(row);
  }

	  function sortedProfileItems(items) {
	    return (items || [])
	      .slice()
	      .sort((a, b) => Number(a.priority || 0) - Number(b.priority || 0));
	  }

	  function profilePriorityLevel(priority) {
	    const value = Number(priority) || 0;
	    if (value < 0) return 'high';
	    if (value >= 10) return 'low';
	    return 'normal';
	  }

	  function profilePriorityValue(level) {
	    if (level === 'high') return -10;
	    if (level === 'low') return 10;
	    return 0;
	  }

	  function profilePriorityLabel(priority) {
	    const level = profilePriorityLevel(priority);
	    return level.charAt(0).toUpperCase() + level.slice(1);
	  }

	  function profileOptionLabel(value) {
	    return String(value || '')
	      .replace(/[-_]+/g, ' ')
	      .replace(/\b\w/g, (ch) => ch.toUpperCase());
	  }

	  function profileIntentionById(id) {
	    const profile = ensureSynthesisProfileShape();
	    return profile.intentions.find((intention) => intention.id === id) || null;
	  }

	  function profileDomainById(id) {
	    const profile = ensureSynthesisProfileShape();
	    return profile.domains.find((domain) => domain.id === id) || null;
	  }

	  function profileInterestById(id) {
	    const profile = ensureSynthesisProfileShape();
	    return profile.interests.find((interest) => interest.id === id) || null;
	  }

	  function profileInterestType(interest) {
	    return String((interest && (interest.type || interest.interest_type)) || 'topic');
	  }

	  function profilePersonInterests() {
	    return sortedProfileItems(ensureSynthesisProfileShape().interests)
	      .filter((interest) => profileInterestType(interest).toLowerCase() === 'person');
	  }

	  function speakerItems() {
	    return speakerSummary && Array.isArray(speakerSummary.speakers)
	      ? speakerSummary.speakers
	      : [];
	  }

	  function profileVoiceById(id) {
	    return speakerItems().find((speaker) => speaker && speaker.speaker_id === id) || null;
	  }

	  function profileDomainDisplay(id) {
	    if (!id) return 'Unassigned';
	    const domain = profileDomainById(id);
	    return (domain && (domain.name || domain.id)) || id;
	  }

	  function enabledProfileDomainCount() {
	    return ensureSynthesisProfileShape().domains
	      .filter((domain) => domain.enabled !== false)
	      .length;
	  }

	  function canDisableProfileDomain(domain) {
	    return domain.enabled === false || enabledProfileDomainCount() > 1;
	  }

	  function appendProfileDrill(row, onOpen) {
	    row.classList.add('drill-row');
	    row.role = 'button';
	    row.tabIndex = 0;
	    row.onclick = (e) => {
	      if (e.target && e.target.closest('button, input, select, textarea')) return;
	      onOpen();
	    };
	    row.onkeydown = (e) => {
	      if (e.key !== 'Enter' && e.key !== ' ') return;
	      e.preventDefault();
	      onOpen();
	    };
	    const hint = document.createElement('span');
	    hint.className = 'action-hint';
	    hint.setAttribute('aria-hidden', 'true');
	    hint.textContent = '›';
	    row.querySelector('.profile-row-header').appendChild(hint);
	  }

	  function renderProfileIntentions() {
	    const list = $('profile-intentions');
	    list.replaceChildren();
	    if (synthesisProfileLoading && !synthesisProfile) {
	      renderSimpleCard(list, 'Loading intentions', 'Reading synthesis customization.');
	      requestPopoverResize();
	      return;
	    }
	    if (synthesisProfileError && !synthesisProfile) {
	      renderSimpleCard(list, 'Could not load profile', synthesisProfileError);
	      requestPopoverResize();
	      return;
	    }
	    ensureSynthesisProfileShape();
	    if (!synthesisProfile.intentions.length) {
	      renderSimpleCard(list, 'No intentions', 'Add goals, habits, commitments, missions, or ambitions to measure the day against.');
	      updateProfileSaveButtons();
	      requestPopoverResize();
	      return;
	    }
	    sortedProfileItems(synthesisProfile.intentions).forEach((intention) => {
	      const kind = intention.kind || 'Goal';
	      const domain = profileDomainDisplay(intention.domain);
	      const state = intention.enabled === false ? 'Off' : 'On';
	      const row = profileRow(intention.description || 'Untitled intention', `${kind} · ${domain} · ${profilePriorityLabel(intention.priority)} · ${state}`);
	      const toggle = document.createElement('button');
	      toggle.type = 'button';
	      toggle.className = `switch ${intention.enabled !== false ? 'on' : ''}`;
	      toggle.textContent = intention.enabled !== false ? 'On' : 'Off';
	      toggle.onclick = (e) => {
	        e.stopPropagation();
	        intention.enabled = !(intention.enabled !== false);
	        renderProfileIntentions();
	      };
	      row.querySelector('.profile-row-header').appendChild(toggle);
	      appendProfileDrill(row, () => {
	        selectedProfileIntentionId = intention.id;
	        setView('profile-intention-detail');
	      });
	      list.appendChild(row);
	    });
	    updateProfileSaveButtons();
	    requestPopoverResize();
	  }

	  function renderProfileDomains() {
	    const list = $('profile-domains');
	    list.replaceChildren();
	    if (synthesisProfileLoading && !synthesisProfile) {
	      renderSimpleCard(list, 'Loading domains', 'Reading synthesis customization.');
	      requestPopoverResize();
	      return;
	    }
	    if (synthesisProfileError && !synthesisProfile) {
	      renderSimpleCard(list, 'Could not load profile', synthesisProfileError);
	      requestPopoverResize();
	      return;
	    }
	    ensureSynthesisProfileShape();
	    if (!synthesisProfile.domains.length) {
	      renderSimpleCard(list, 'No domains', 'Add the synthesis lanes you want decisions grouped into.');
	      updateProfileSaveButtons();
	      requestPopoverResize();
	      return;
	    }
	    sortedProfileItems(synthesisProfile.domains).forEach((domain) => {
	      const state = domain.enabled === false ? 'Off' : 'On';
	      const row = profileRow(domain.name || domain.id || 'Untitled domain', `${profilePriorityLabel(domain.priority)} · ${state}`);
	      const toggle = document.createElement('button');
	      toggle.type = 'button';
	      toggle.className = `switch ${domain.enabled !== false ? 'on' : ''}`;
	      toggle.textContent = domain.enabled !== false ? 'On' : 'Off';
	      toggle.onclick = (e) => {
	        e.stopPropagation();
	        if (!canDisableProfileDomain(domain)) {
	          showMenuNotification('Keep at least one synthesis domain enabled.', 'warning');
	          return;
	        }
	        domain.enabled = !(domain.enabled !== false);
	        renderProfileDomains();
	      };
	      row.querySelector('.profile-row-header').appendChild(toggle);
	      appendProfileDrill(row, () => {
	        selectedProfileDomainId = domain.id;
	        setView('profile-domain-detail');
	      });
	      list.appendChild(row);
	    });
	    updateProfileSaveButtons();
	    requestPopoverResize();
	  }

	  function renderProfileIntentionDetail() {
	    const summary = $('profile-intention-detail-summary');
	    const fields = $('profile-intention-detail-fields');
	    summary.replaceChildren();
	    fields.replaceChildren();
	    if (synthesisProfileLoading && !synthesisProfile) {
	      renderSimpleCard(summary, 'Loading intention', 'Reading synthesis customization.');
	      requestPopoverResize();
	      return;
	    }
	    if (synthesisProfileError && !synthesisProfile) {
	      renderSimpleCard(summary, 'Could not load profile', synthesisProfileError);
	      requestPopoverResize();
	      return;
	    }
	    ensureSynthesisProfileShape();
	    const intention = profileIntentionById(selectedProfileIntentionId);
	    if (!intention) {
	      renderSimpleCard(summary, 'No intention selected', 'Go back and choose an intention.');
	      requestPopoverResize();
	      return;
	    }

	    const state = intention.enabled === false ? 'Off' : 'On';
	    const row = profileRow(intention.description || 'Untitled intention', `${intention.kind || 'Goal'} · ${profileDomainDisplay(intention.domain)} · ${profilePriorityLabel(intention.priority)} · ${state}`);
	    const toggle = document.createElement('button');
	    toggle.type = 'button';
	    toggle.className = `switch ${intention.enabled !== false ? 'on' : ''}`;
	    toggle.textContent = intention.enabled !== false ? 'On' : 'Off';
	    toggle.onclick = () => {
	      intention.enabled = !(intention.enabled !== false);
	      renderProfileIntentionDetail();
	    };
	    row.querySelector('.profile-row-header').appendChild(toggle);
	    summary.appendChild(row);

	    fields.append(
	      profileFieldGrid([
	        profileSelect('Kind', intention.kind || 'Goal', ['Mission', 'Ambition', 'Goal', 'Habit', 'Commitment'], (value) => { intention.kind = value; renderProfileIntentionDetail(); }),
	        profileDomainSelect('Domain', intention.domain || '', (value) => { intention.domain = value; renderProfileIntentionDetail(); }),
	        profilePrioritySelect('Priority', intention.priority, (value) => { intention.priority = value; renderProfileIntentionDetail(); }),
	        profileInput('Target date', intention.target_date || '', (value) => { intention.target_date = value || null; }, 'date'),
	      ]),
	      profileFieldGrid([
	        profileTextareaField('Description', intention.description || '', (value) => { intention.description = value; }),
	        profileTextareaField('Success', intention.success_criteria || '', (value) => { intention.success_criteria = value; }),
	      ], true),
	    );
	    updateProfileSaveButtons();
	    requestPopoverResize();
	  }

	  function renderProfileDomainDetail() {
	    const summary = $('profile-domain-detail-summary');
	    const fields = $('profile-domain-detail-fields');
	    summary.replaceChildren();
	    fields.replaceChildren();
	    if (synthesisProfileLoading && !synthesisProfile) {
	      renderSimpleCard(summary, 'Loading domain', 'Reading synthesis customization.');
	      requestPopoverResize();
	      return;
	    }
	    if (synthesisProfileError && !synthesisProfile) {
	      renderSimpleCard(summary, 'Could not load profile', synthesisProfileError);
	      requestPopoverResize();
	      return;
	    }
	    ensureSynthesisProfileShape();
	    const domain = profileDomainById(selectedProfileDomainId);
	    if (!domain) {
	      renderSimpleCard(summary, 'No domain selected', 'Go back and choose a domain.');
	      requestPopoverResize();
	      return;
	    }

	    const state = domain.enabled === false ? 'Off' : 'On';
	    const row = profileRow(domain.name || domain.id || 'Untitled domain', `${profilePriorityLabel(domain.priority)} · ${state}`);
	    const toggle = document.createElement('button');
	    toggle.type = 'button';
	    toggle.className = `switch ${domain.enabled !== false ? 'on' : ''}`;
	    toggle.textContent = domain.enabled !== false ? 'On' : 'Off';
	    toggle.onclick = () => {
	      if (!canDisableProfileDomain(domain)) {
	        showMenuNotification('Keep at least one synthesis domain enabled.', 'warning');
	        return;
	      }
	      domain.enabled = !(domain.enabled !== false);
	      renderProfileDomainDetail();
	    };
	    row.querySelector('.profile-row-header').appendChild(toggle);
	    summary.appendChild(row);

	    fields.append(
	      profileFieldGrid([
	        profileInput('Name', domain.name || domain.id || '', (value) => {
	          renameProfileDomain(domain, value);
	        }),
	        profilePrioritySelect('Priority', domain.priority, (value) => { domain.priority = value; renderProfileDomainDetail(); }),
	      ]),
	      profileFieldGrid([
	        profileTextareaField('Description', domain.description || '', (value) => { domain.description = value; }),
	      ], true),
	    );
	    updateProfileSaveButtons();
	    requestPopoverResize();
	  }

	  function appendTrackedTabs(list, active) {
	    const row = profileRow('Tracked', 'Items you want synthesis to recognize, plus voice evidence linked to people.');
	    const actions = document.createElement('div');
	    actions.className = 'profile-row-actions';
	    const items = document.createElement('button');
	    items.type = 'button';
	    items.textContent = 'Items';
	    items.disabled = active === 'items';
	    items.onclick = () => setView('profile-interests-list');
	    const voices = document.createElement('button');
	    voices.type = 'button';
	    voices.textContent = 'Voices';
	    voices.disabled = active === 'voices';
	    voices.onclick = () => setView('profile-voices-list');
	    actions.append(items, voices);
	    row.appendChild(actions);
	    list.appendChild(row);
	  }

	  function renderProfileInterests() {
    const list = $('profile-interests');
    list.replaceChildren();
    appendTrackedTabs(list, 'items');
    if (synthesisProfileLoading && !synthesisProfile) {
      appendSimpleCard(list, 'Loading tracked items', 'Reading synthesis customization.');
      requestPopoverResize();
      return;
    }
    if (synthesisProfileError && !synthesisProfile) {
      appendSimpleCard(list, 'Could not load profile', synthesisProfileError);
      requestPopoverResize();
      return;
    }
    ensureSynthesisProfileShape();
    if (!synthesisProfile.interests.length && !synthesisProfileSuggestions.length) {
      appendSimpleCard(list, 'No tracked items', 'Add people, projects, places, tools, organizations, or topics. Recurring suggestions will appear here.');
      updateProfileSaveButtons();
      requestPopoverResize();
      return;
    }
    synthesisProfile.interests
      .slice()
      .sort((a, b) => Number(a.priority || 0) - Number(b.priority || 0))
      .forEach((interest) => {
        const type = interest.type || interest.interest_type || 'topic';
        const state = interest.enabled === false ? 'Off' : 'On';
        const row = profileRow(interest.name || interest.id || 'Untitled tracked item', `${type} · ${profilePriorityLabel(interest.priority)} · ${state}`);
        const toggle = document.createElement('button');
        toggle.type = 'button';
        toggle.className = `switch ${interest.enabled !== false ? 'on' : ''}`;
        toggle.textContent = interest.enabled !== false ? 'On' : 'Off';
        toggle.onclick = (e) => {
          e.stopPropagation();
          interest.enabled = !(interest.enabled !== false);
          renderProfileInterests();
        };
        row.querySelector('.profile-row-header').appendChild(toggle);
        appendProfileDrill(row, () => {
          selectedProfileInterestId = interest.id;
          setView('profile-interest-detail');
        });
        list.appendChild(row);
      });
    if (synthesisProfileSuggestions.length) {
      const suggestionsTitle = document.createElement('div');
      suggestionsTitle.className = 'meta';
      suggestionsTitle.textContent = 'Suggested from recurring items Alvum noticed.';
      list.appendChild(suggestionsTitle);
      for (const suggestion of synthesisProfileSuggestions) {
        const row = profileRow(suggestion.name || suggestion.id, `${suggestion.type || suggestion.suggestion_type || 'topic'} · suggested`);
        const actions = document.createElement('div');
        actions.className = 'profile-row-actions';
        const track = document.createElement('button');
        track.type = 'button';
        track.className = 'primary';
        track.textContent = 'Track';
        track.onclick = async () => {
          const result = await window.alvum.synthesisProfilePromote(suggestion.id);
          if (result && result.profile) synthesisProfile = result.profile;
          if (result && Array.isArray(result.suggestions)) synthesisProfileSuggestions = result.suggestions;
          renderActiveSynthesisProfileView();
        };
        const ignore = document.createElement('button');
        ignore.type = 'button';
        ignore.textContent = 'Ignore';
        ignore.onclick = async () => {
          const result = await window.alvum.synthesisProfileIgnore(suggestion.id);
          if (result && Array.isArray(result.suggestions)) synthesisProfileSuggestions = result.suggestions;
          renderActiveSynthesisProfileView();
        };
        actions.append(track, ignore);
        row.append(profileMeta(suggestion.description || 'Recurring item from generated knowledge.'), actions);
        list.appendChild(row);
      }
    }
    updateProfileSaveButtons();
    requestPopoverResize();
  }

	  function renderProfileInterestDetail() {
	    const summary = $('profile-interest-detail-summary');
	    const fields = $('profile-interest-detail-fields');
	    summary.replaceChildren();
	    fields.replaceChildren();
	    if (synthesisProfileLoading && !synthesisProfile) {
	      renderSimpleCard(summary, 'Loading tracked item', 'Reading synthesis customization.');
	      requestPopoverResize();
	      return;
	    }
	    if (synthesisProfileError && !synthesisProfile) {
	      renderSimpleCard(summary, 'Could not load profile', synthesisProfileError);
	      requestPopoverResize();
	      return;
	    }
	    ensureSynthesisProfileShape();
	    const interest = profileInterestById(selectedProfileInterestId);
	    if (!interest) {
	      renderSimpleCard(summary, 'No tracked item selected', 'Go back and choose a tracked item.');
	      requestPopoverResize();
	      return;
	    }

	    const type = interest.type || interest.interest_type || 'topic';
	    const state = interest.enabled === false ? 'Off' : 'On';
	    const row = profileRow(interest.name || interest.id || 'Untitled tracked item', `${type} · ${profilePriorityLabel(interest.priority)} · ${state}`);
	    const toggle = document.createElement('button');
	    toggle.type = 'button';
	    toggle.className = `switch ${interest.enabled !== false ? 'on' : ''}`;
	    toggle.textContent = interest.enabled !== false ? 'On' : 'Off';
	    toggle.onclick = () => {
	      interest.enabled = !(interest.enabled !== false);
	      renderProfileInterestDetail();
	    };
	    row.querySelector('.profile-row-header').appendChild(toggle);
	    summary.appendChild(row);

	    fields.append(
	      profileFieldGrid([
	        profileInput('Name', interest.name || '', (value) => { interest.name = value; }),
	        profileSelect('Type', type, ['person', 'place', 'project', 'organization', 'tool', 'topic'], (value) => {
	          interest.type = value;
	          interest.interest_type = value;
	          renderProfileInterestDetail();
	        }),
	        profilePrioritySelect('Priority', interest.priority, (value) => { interest.priority = value; renderProfileInterestDetail(); }),
	      ]),
	      profileFieldGrid([
	        profileTextareaField('Description', interest.notes || '', (value) => { interest.notes = value; }),
	      ], true),
	    );
	    updateProfileSaveButtons();
	    requestPopoverResize();
	  }

  function voiceDisplayName(speaker) {
    if (!speaker) return 'Voice cluster';
    if (speaker.linked_interest && speaker.linked_interest.name) return speaker.linked_interest.name;
    if (speaker.label) return speaker.label;
    return speaker.speaker_id || 'Unlinked voice';
  }

  function voiceMeta(speaker) {
    const count = Number(speaker && speaker.fingerprint_count || 0);
    const samples = Array.isArray(speaker && speaker.samples) ? speaker.samples.length : 0;
    const state = speaker && speaker.linked_interest_id ? 'linked person' : 'unlinked voice cluster';
    return `${state} · ${count} fingerprint${count === 1 ? '' : 's'} · ${samples} sample${samples === 1 ? '' : 's'}`;
  }

  function voiceSampleItems() {
    if (speakerSummary && Array.isArray(speakerSummary.samples)) return speakerSummary.samples;
    return speakerItems().flatMap((speaker) => (Array.isArray(speaker.samples) ? speaker.samples : []).map((sample, index) => ({
      ...sample,
      sample_id: `${speaker.speaker_id}:${index}`,
      cluster_id: speaker.speaker_id,
      linked_interest_id: speaker.linked_interest_id || null,
      linked_interest: speaker.linked_interest || null,
      person_candidates: speaker.person_candidates || [],
      context_interests: speaker.context_interests || [],
    })));
  }

  function voiceSampleById(sampleId) {
    return voiceSampleItems().find((sample) => sample && sample.sample_id === sampleId) || null;
  }

  function voiceClusterById(clusterId) {
    return speakerItems().find((speaker) => speaker && speaker.speaker_id === clusterId) || null;
  }

  function voiceSampleDisplayName(sample) {
    if (!sample) return 'Voice sample';
    if (isIgnoredVoiceSample(sample)) return 'Ignored voice sample';
    if (sample.linked_interest && sample.linked_interest.name) return sample.linked_interest.name;
    const candidate = Array.isArray(sample.person_candidates) ? sample.person_candidates[0] : null;
    if (candidate && candidate.name) return `Possible ${candidate.name}`;
    return 'Unlinked voice sample';
  }

  function isIgnoredVoiceSample(sample) {
    return !!(sample && Array.isArray(sample.quality_flags) && sample.quality_flags.includes('ignored_by_user'));
  }

  function voiceSampleMeta(sample) {
    if (!sample) return '';
    const pieces = [];
    if (isIgnoredVoiceSample(sample)) pieces.push('ignored');
    if (sample.source) pieces.push(sample.source);
    if (sample.ts) pieces.push(sample.ts);
    if (sample.cluster_id) pieces.push(sample.cluster_id);
    return pieces.join(' · ');
  }

  function candidateScore(candidate) {
    const score = Number(candidate && candidate.score);
    if (!Number.isFinite(score)) return '';
    return `${Math.round(score * 100)}%`;
  }

  function appendVoiceSample(parent, speaker, sample, sampleIndex) {
    const row = profileRow('Sample turn', sample && sample.text ? sample.text : 'No transcript text recorded.');
    const actions = document.createElement('div');
    actions.className = 'profile-row-actions';
    if (sample && (sample.media_path || (sample.source && sample.ts))) {
      const play = document.createElement('button');
      play.type = 'button';
      play.textContent = 'Play';
      play.onclick = () => playSpeakerSample(speaker, sampleIndex, play);
      actions.appendChild(play);
    }
    if (sample && (sample.source || sample.ts)) {
      row.appendChild(profileMeta([sample.source, sample.ts].filter(Boolean).join(' · ')));
    }
    if (actions.childNodes.length) row.appendChild(actions);
    parent.appendChild(row);
  }

  function appendVoiceEvidenceSample(parent, sample, { drill = true } = {}) {
    const row = profileRow(voiceSampleDisplayName(sample), sample && sample.text ? sample.text : 'No transcript text recorded.');
    if (sample && (sample.source || sample.ts || sample.cluster_id)) {
      row.appendChild(profileMeta(voiceSampleMeta(sample)));
    }
    const candidate = Array.isArray(sample && sample.person_candidates) ? sample.person_candidates[0] : null;
    if (candidate && !sample.linked_interest_id) {
      row.appendChild(profileMeta(`Suggested person: ${candidate.name} · ${candidateScore(candidate)} · ${candidate.reason || 'candidate match'}`));
    }
    const context = Array.isArray(sample && sample.context_interests) ? sample.context_interests[0] : null;
    if (context) {
      row.appendChild(profileMeta(`Context nearby: ${context.name}`));
    }
    const actions = document.createElement('div');
    actions.className = 'profile-row-actions';
    if (sample && (sample.media_path || (sample.source && sample.ts))) {
      const play = document.createElement('button');
      play.type = 'button';
      play.textContent = 'Play';
      play.onclick = (event) => {
        event.stopPropagation();
        playVoiceSample(sample, play);
      };
      actions.appendChild(play);
    }
    if (actions.childNodes.length) row.appendChild(actions);
    if (drill && sample && sample.sample_id) {
      appendProfileDrill(row, () => {
        selectedProfileVoiceSampleId = sample.sample_id;
        selectedProfileVoiceId = sample.cluster_id || null;
        setView('profile-voice-detail');
      });
    }
    parent.appendChild(row);
  }

  function appendVoiceCandidate(parent, speaker, candidate) {
    const row = profileRow(candidate.name || candidate.id || 'Tracked person', `${candidateScore(candidate)} · ${candidate.reason || 'suggested person match'}`);
    const actions = document.createElement('div');
    actions.className = 'profile-row-actions';
    const link = document.createElement('button');
    link.type = 'button';
    link.className = 'primary';
    link.textContent = 'Link voice';
    link.onclick = () => linkSpeakerToInterest(speaker, candidate.id, link);
    actions.appendChild(link);
    row.appendChild(actions);
    parent.appendChild(row);
  }

  function renderProfileVoices() {
    const list = $('profile-voices');
    list.replaceChildren();
    appendTrackedTabs(list, 'voices');
    if (!synthesisProfile && !synthesisProfileLoading) setTimeout(() => refreshSynthesisProfile(false), 0);
    if (!speakerSummary && !speakerLoading) setTimeout(() => refreshSpeakers(false), 0);
    if (synthesisProfileLoading && !synthesisProfile) {
      appendSimpleCard(list, 'Loading tracked voices', 'Reading synthesis customization.');
      requestPopoverResize();
      return;
    }
    if (speakerLoading && !speakerSummary) {
      appendSimpleCard(list, 'Loading voice clusters', 'Reading local speaker evidence.');
      requestPopoverResize();
      return;
    }
    if (speakerSummary && speakerSummary.error) {
      appendSimpleCard(list, 'Voice registry unavailable', speakerSummary.error);
      requestPopoverResize();
      return;
    }
    ensureSynthesisProfileShape();
    const samples = voiceSampleItems()
      .filter((sample) => !isIgnoredVoiceSample(sample))
      .slice()
      .sort((a, b) => Number(!!a.linked_interest_id) - Number(!!b.linked_interest_id) || String(b.ts || '').localeCompare(String(a.ts || '')));
    const speakers = speakerItems()
      .slice()
      .sort((a, b) => Number(!!a.linked_interest_id) - Number(!!b.linked_interest_id));
    if (!samples.length && !speakers.length) {
      appendSimpleCard(list, 'No voice evidence yet', 'Voice evidence appears after audio processing emits diarized speaker turns.');
      requestPopoverResize();
      return;
    }
    if (samples.length) {
      appendSimpleCard(list, 'Review queue', 'Playable voice clips sorted by likely action. Confirm identities clip by clip, then clusters update around that evidence.');
      for (const sample of samples) {
        appendVoiceEvidenceSample(list, sample);
      }
    }
    if (speakers.length) {
      list.appendChild(profileMeta('Voice clusters'));
    }
    for (const speaker of speakers) {
      const row = profileRow(voiceDisplayName(speaker), voiceMeta(speaker));
      const samples = Array.isArray(speaker.samples) ? speaker.samples : [];
      const sample = samples[samples.length - 1];
      if (sample && sample.text) row.appendChild(profileMeta(`Latest: ${sample.text}`));
      const candidate = Array.isArray(speaker.person_candidates) ? speaker.person_candidates[0] : null;
      if (candidate && !speaker.linked_interest_id) {
        row.appendChild(profileMeta(`Suggested person: ${candidate.name} · ${candidateScore(candidate)}`));
      }
      const context = Array.isArray(speaker.context_interests) ? speaker.context_interests[0] : null;
      if (context) {
        row.appendChild(profileMeta(`Context mentioned nearby: ${context.name}`));
      }
      appendProfileDrill(row, () => {
        selectedProfileVoiceId = speaker.speaker_id;
        selectedProfileVoiceSampleId = null;
        setView('profile-voice-detail');
      });
      list.appendChild(row);
    }
    requestPopoverResize();
  }

  function renderProfileVoiceDetail() {
    const summary = $('profile-voice-detail-summary');
    const actions = $('profile-voice-detail-actions');
    summary.replaceChildren();
    actions.replaceChildren();
    if (!synthesisProfile && !synthesisProfileLoading) setTimeout(() => refreshSynthesisProfile(false), 0);
    if (!speakerSummary && !speakerLoading) setTimeout(() => refreshSpeakers(false), 0);
    if ((synthesisProfileLoading && !synthesisProfile) || (speakerLoading && !speakerSummary)) {
      renderSimpleCard(summary, 'Loading voice evidence', 'Reading tracked people and local voice clusters.');
      requestPopoverResize();
      return;
    }
    ensureSynthesisProfileShape();
    const sample = voiceSampleById(selectedProfileVoiceSampleId);
    const speaker = sample ? voiceClusterById(sample.cluster_id) : profileVoiceById(selectedProfileVoiceId);
    if (!speaker && !sample) {
      renderSimpleCard(summary, 'No voice selected', 'Go back and choose a voice cluster.');
      requestPopoverResize();
      return;
    }

    if (sample) {
      summary.appendChild(profileRow(voiceSampleDisplayName(sample), sample.text || 'No transcript text recorded.'));
      appendVoiceEvidenceSample(summary, sample, { drill: false });
    }
    if (speaker) {
      summary.appendChild(profileRow(voiceDisplayName(speaker), voiceMeta(speaker)));
      const samples = Array.isArray(speaker.samples) ? speaker.samples : [];
      samples.slice(-3).forEach((clusterSample, offset) => {
        const sampleIndex = samples.length - Math.min(samples.length, 3) + offset;
        appendVoiceSample(summary, speaker, clusterSample, sampleIndex);
      });
    }

    const candidates = Array.isArray(sample && sample.person_candidates)
      ? sample.person_candidates
      : (Array.isArray(speaker && speaker.person_candidates) ? speaker.person_candidates : []);
    if (candidates.length) {
      actions.appendChild(profileMeta('Suggested tracked people'));
      candidates.forEach((candidate) => {
        if (sample) {
          const row = profileRow(candidate.name || candidate.id || 'Tracked person', `${candidateScore(candidate)} · ${candidate.reason || 'suggested person match'}`);
          const link = document.createElement('button');
          link.type = 'button';
          link.className = 'primary';
          link.textContent = 'Link clip';
          link.onclick = () => linkVoiceSampleToInterest(sample, candidate.id, link);
          row.appendChild(link);
          actions.appendChild(row);
        } else {
          appendVoiceCandidate(actions, speaker, candidate);
        }
      });
    }

    const people = profilePersonInterests();
    if (people.length) {
      const selected = (sample && sample.linked_interest_id) || (speaker && speaker.linked_interest_id) || (candidates[0] && candidates[0].id) || people[0].id;
      const target = profileSelect(
        sample ? 'Link clip to tracked person' : 'Link cluster to tracked person',
        selected,
        people.map((interest) => ({ value: interest.id, label: interest.name || interest.id })),
        () => {},
      );
      const select = target.querySelector('select') as HTMLSelectElement;
      const row = document.createElement('div');
      row.className = 'profile-row';
      const action = document.createElement('button');
      action.type = 'button';
      action.className = 'primary';
      action.textContent = sample ? 'Link clip' : 'Link voice';
      action.onclick = () => sample ? linkVoiceSampleToInterest(sample, select.value, action) : linkSpeakerToInterest(speaker, select.value, action);
      row.append(target, action);
      actions.appendChild(row);
    }

    const createInput = document.createElement('input');
    createInput.placeholder = 'Tracked person name';
    createInput.value = (speaker && speaker.label) || (sample && sample.linked_interest && sample.linked_interest.name) || '';
    const create = document.createElement('button');
    create.type = 'button';
    create.textContent = 'Create tracked person';
    create.onclick = () => sample ? createTrackedPersonForVoiceSample(sample, createInput.value, create) : createTrackedPersonForSpeaker(speaker, createInput.value, create);
    const createRow = document.createElement('div');
    createRow.className = 'profile-row';
    createRow.append(profileMeta('Create tracked person from this voice evidence.'), createInput, create);
    actions.appendChild(createRow);

    if (sample) {
      const clusters = speakerItems().filter((item) => item.speaker_id !== sample.cluster_id);
      if (clusters.length) {
        const selectedCluster = clusters[0].speaker_id;
        const target = profileSelect(
          'Move clip to cluster',
          selectedCluster,
          clusters.map((item) => ({ value: item.speaker_id, label: voiceDisplayName(item) })),
          () => {},
        );
        const select = target.querySelector('select') as HTMLSelectElement;
        const row = document.createElement('div');
        row.className = 'profile-row';
        const move = document.createElement('button');
        move.type = 'button';
        move.textContent = 'Move clip';
        move.onclick = () => moveVoiceSample(sample, select.value, move);
        row.append(target, move);
        actions.appendChild(row);
      }
      const newClusterRow = document.createElement('div');
      newClusterRow.className = 'profile-row profile-row-actions';
      const newCluster = document.createElement('button');
      newCluster.type = 'button';
      newCluster.textContent = 'New cluster from clip';
      newCluster.onclick = () => moveVoiceSample(sample, 'new', newCluster);
      const ignore = document.createElement('button');
      ignore.type = 'button';
      ignore.textContent = 'Ignore clip';
      ignore.onclick = () => ignoreVoiceSample(sample, ignore);
      newClusterRow.append(newCluster, ignore);
      actions.appendChild(newClusterRow);
    }

    const duplicates = Array.isArray(speaker && speaker.duplicate_candidates) ? speaker.duplicate_candidates : [];
    if (duplicates.length) {
      actions.appendChild(profileMeta('Possible duplicate voice clusters'));
      duplicates.forEach((candidate) => {
        const row = profileRow(candidate.label || candidate.speaker_id, `${candidateScore(candidate)} voice similarity`);
        const merge = document.createElement('button');
        merge.type = 'button';
        merge.textContent = 'Merge cluster';
        merge.onclick = () => mergeSpeaker(speaker.speaker_id, candidate.speaker_id, merge);
        row.appendChild(merge);
        actions.appendChild(row);
      });
    }

    const contexts = Array.isArray(sample && sample.context_interests)
      ? sample.context_interests
      : (Array.isArray(speaker && speaker.context_interests) ? speaker.context_interests : []);
    if (contexts.length) {
      actions.appendChild(profileMeta('Context mentioned nearby'));
      contexts.forEach((context) => {
        actions.appendChild(profileRow(context.name || context.id, `${profileOptionLabel(context.type || 'topic')} · ${context.reason || 'context mentioned nearby'}`));
      });
    }

    const finalRow = document.createElement('div');
    finalRow.className = 'profile-row profile-row-actions';
    if (speaker && speaker.linked_interest_id) {
      const unlink = document.createElement('button');
      unlink.type = 'button';
      unlink.textContent = 'Unlink voice';
      unlink.onclick = () => unlinkSpeakerFromInterest(speaker, unlink);
      finalRow.appendChild(unlink);
    }
    const forget = document.createElement('button');
    forget.type = 'button';
    forget.className = 'danger';
    forget.textContent = 'Forget cluster';
    if (speaker) {
      forget.onclick = () => forgetSpeaker(speaker.speaker_id, forget);
      finalRow.appendChild(forget);
    }
    if (finalRow.childNodes.length) actions.appendChild(finalRow);
    requestPopoverResize();
  }

  function renderProfileWriting() {
    const list = $('profile-writing');
    list.replaceChildren();
    if (synthesisProfileLoading && !synthesisProfile) {
      renderSimpleCard(list, 'Loading writing preferences', 'Reading synthesis customization.');
      requestPopoverResize();
      return;
    }
    if (synthesisProfileError && !synthesisProfile) {
      renderSimpleCard(list, 'Could not load profile', synthesisProfileError);
      requestPopoverResize();
      return;
    }
    ensureSynthesisProfileShape();
    const writing = synthesisProfile.writing || {};
    list.appendChild(profileFieldGrid([
      profileSelect('Detail', writing.detail_level || 'detailed', [
        { value: 'concise', label: 'Concise' },
        { value: 'balanced', label: 'Balanced' },
        { value: 'detailed', label: 'Detailed' },
        { value: 'exhaustive', label: 'Exhaustive' },
      ], (value) => { writing.detail_level = value; synthesisProfile.writing = writing; }),
      profileSelect('Tone', writing.tone || 'direct', [
        { value: 'direct', label: 'Direct' },
        { value: 'analytical', label: 'Analytical' },
        { value: 'coaching', label: 'Coaching' },
        { value: 'reflective', label: 'Reflective' },
      ], (value) => { writing.tone = value; synthesisProfile.writing = writing; }),
    ]));
    list.appendChild(profileFieldGrid([
      profileTextareaField('Daily Briefing Outline', writing.outline || DEFAULT_DAILY_BRIEFING_OUTLINE, (value) => { writing.outline = value; synthesisProfile.writing = writing; }, 7),
    ], true));
    updateProfileSaveButtons();
    requestPopoverResize();
  }

  function renderProfileSchedule() {
    const list = $('profile-schedule');
    if (!list) return;
    list.replaceChildren();
    const schedule = synthesisScheduleValue();
    const statusMeta = schedule.running_date
      ? `Synthesizing ${displayDate(schedule.running_date)}`
      : (schedule.queued_dates.length
        ? `${schedule.queued_dates.length} queued`
        : (schedule.due_dates.length ? `${schedule.due_dates.length} due` : 'No completed days due'));
    const status = profileRow(
      schedule.setup_pending ? 'Enabled after first synthesis' : (schedule.enabled ? 'Automatic synthesis on' : 'Automatic synthesis off'),
      schedule.last_error ? `${statusMeta} · ${schedule.last_error}` : statusMeta,
    );
    list.appendChild(status);
    list.appendChild(profileFieldGrid([
      profileSelect('Automatic synthesis', schedule.enabled ? 'on' : 'off', [
        { value: 'on', label: 'On' },
        { value: 'off', label: 'Off' },
      ], (value) => {
        synthesisSchedule = { ...synthesisScheduleValue(), enabled: value === 'on', setup_pending: false };
        renderProfileSchedule();
      }),
      profileInput('Run time', schedule.time || '07:00', (value) => {
        synthesisSchedule = { ...synthesisScheduleValue(), time: value || '07:00' };
      }, 'time'),
    ]));
    list.appendChild(profileFieldGrid([
      profileSelect('Policy', schedule.policy || 'completed_days', [
        { value: 'completed_days', label: 'Completed days only' },
      ], (value) => {
        synthesisSchedule = { ...synthesisScheduleValue(), policy: value || 'completed_days' };
      }),
    ], true));
    updateProfileSaveButtons();
    requestPopoverResize();
  }

	  function renderProfileAdvanced() {
	    const textarea = $('profile-advanced');
	    if (!textarea) return;
	    if (synthesisProfileLoading && !synthesisProfile) {
	      textarea.value = '';
	      textarea.placeholder = 'Loading profile...';
	      requestPopoverResize();
	      return;
	    }
	    if (synthesisProfileError && !synthesisProfile) {
	      textarea.value = '';
	      textarea.placeholder = synthesisProfileError;
	      requestPopoverResize();
	      return;
	    }
	    ensureSynthesisProfileShape();
	    textarea.placeholder = '';
	    textarea.value = synthesisProfile.advanced_instructions || '';
	    updateProfileSaveButtons();
	    requestPopoverResize();
	  }

  function profileRow(title, meta) {
    const row = document.createElement('div');
    row.className = 'profile-row';
    const header = document.createElement('div');
    header.className = 'profile-row-header';
    const text = document.createElement('div');
    const name = document.createElement('div');
    name.className = 'value';
    name.textContent = title;
    text.appendChild(name);
    if (meta) text.appendChild(profileMeta(meta));
    header.appendChild(text);
    row.appendChild(header);
    return row;
  }

  function profileMeta(text) {
    const el = document.createElement('div');
    el.className = 'meta';
    el.textContent = text;
    return el;
  }

  function profileFieldGrid(fields, single = false) {
    const grid = document.createElement('div');
    grid.className = `profile-field-grid${single ? ' single' : ''}`;
    fields.forEach((field) => grid.appendChild(field));
    return grid;
  }

  function profileInput(label, value, onChange, type = 'text') {
    const wrap = profileField(label);
    const input = document.createElement('input');
    input.className = 'profile-input';
    input.type = type;
    input.value = value == null ? '' : String(value);
    input.oninput = () => onChange(input.value);
    wrap.appendChild(input);
    return wrap;
  }

	  function profileSelect(label, value, options, onChange) {
	    const wrap = profileField(label);
	    const select = document.createElement('select');
	    select.className = 'profile-select';
	    for (const option of options) {
	      const optionValue = typeof option === 'object' ? option.value : option;
	      const optionLabel = typeof option === 'object' ? option.label : optionValue;
	      const el = document.createElement('option');
	      el.value = optionValue;
	      el.textContent = optionLabel;
	      select.appendChild(el);
	    }
	    select.value = value;
	    select.onchange = () => onChange(select.value);
	    wrap.appendChild(select);
	    return wrap;
	  }

	  function profilePrioritySelect(label, priority, onChange) {
	    return profileSelect(label, profilePriorityLevel(priority), [
	      { value: 'low', label: 'Low' },
	      { value: 'normal', label: 'Normal' },
	      { value: 'high', label: 'High' },
	    ], (value) => onChange(profilePriorityValue(value)));
	  }

	  function profileDomainSelect(label, value, onChange) {
	    const domains = sortedProfileItems(ensureSynthesisProfileShape().domains)
	      .filter((domain) => domain.enabled !== false || domain.id === value);
	    const options = [{ value: '', label: 'Unassigned' }];
	    for (const domain of domains) {
	      options.push({ value: domain.id, label: domain.name || domain.id });
	    }
	    if (value && !domains.some((domain) => domain.id === value)) {
	      options.push({ value, label: value });
	    }
	    return profileSelect(label, value || '', options, onChange);
	  }

  function profileTextareaField(label, value, onChange, rows = 3) {
    const wrap = profileField(label);
    const textarea = document.createElement('textarea');
    textarea.className = 'profile-textarea';
    textarea.rows = rows;
    textarea.value = value || '';
    textarea.oninput = () => onChange(textarea.value);
    wrap.appendChild(textarea);
    return wrap;
  }

  function profileField(label) {
    const wrap = document.createElement('label');
    wrap.className = 'profile-field';
    const text = document.createElement('span');
    text.className = 'label';
    text.textContent = label;
    wrap.appendChild(text);
    return wrap;
  }

	  function csv(value) {
	    return String(value || '').split(',').map((item) => item.trim()).filter(Boolean);
	  }

	  function makeProfileId(prefix, items) {
	    const existing = new Set((items || []).map((item) => item.id).filter(Boolean));
	    const seed = Date.now().toString(36);
	    let candidate = `${prefix}_${seed}`;
	    let suffix = 2;
	    while (existing.has(candidate)) {
	      candidate = `${prefix}_${seed}_${suffix}`;
	      suffix += 1;
	    }
	    return candidate;
	  }

	  function uniqueProfileDomainId(name, currentDomain) {
	    const base = String(name || '').trim();
	    if (!base) return currentDomain.id || 'Custom';
	    const existing = new Set(
	      ensureSynthesisProfileShape().domains
	        .filter((domain) => domain !== currentDomain)
	        .map((domain) => domain.id)
	        .filter(Boolean),
	    );
	    let candidate = base;
	    let suffix = 2;
	    while (existing.has(candidate)) {
	      candidate = `${base} ${suffix}`;
	      suffix += 1;
	    }
	    return candidate;
	  }

	  function renameProfileDomain(domain, nextName) {
	    const previousId = domain.id;
	    domain.name = nextName;
	    const nextId = uniqueProfileDomainId(nextName, domain);
	    if (nextId && nextId !== previousId) {
	      domain.id = nextId;
	      selectedProfileDomainId = nextId;
	      for (const intention of ensureSynthesisProfileShape().intentions) {
	        if (intention.domain === previousId) intention.domain = nextId;
	      }
	    }
	  }

	  function updateProfileSaveButtons() {
	    for (const id of [
	      'profile-intentions-save',
	      'profile-intention-detail-save',
	      'profile-domains-save',
	      'profile-domain-detail-save',
	      'profile-interests-save',
	      'profile-interest-detail-save',
	      'profile-writing-save',
      'profile-schedule-save',
      'profile-schedule-run-due',
	      'profile-advanced-save',
	    ]) {
	      const button = $(id);
	      if (!button) continue;
      const saving = id.startsWith('profile-schedule') ? synthesisScheduleSaving : synthesisProfileSaving;
	      button.disabled = saving;
      if (id === 'profile-schedule-run-due') button.textContent = saving ? 'Running...' : 'Run due';
	      else button.textContent = saving ? 'Saving...' : 'Save';
	    }
	  }

  async function saveSynthesisSchedule() {
    if (!window.alvum.synthesisScheduleSave) return;
    synthesisScheduleSaving = true;
    updateProfileSaveButtons();
    try {
      const result = await window.alvum.synthesisScheduleSave(synthesisScheduleValue());
      if (!result || result.ok === false) {
        showMenuNotification((result && result.error) || 'Could not save synthesis schedule', 'warning');
      } else if (result.schedule) {
        synthesisSchedule = result.schedule;
        showMenuNotification('Synthesis schedule saved.', 'success', 'Schedule');
      }
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Schedule');
    } finally {
      synthesisScheduleSaving = false;
      renderProfileSchedule();
    }
  }

  async function runDueSynthesisFromSchedule() {
    if (!window.alvum.synthesisScheduleRunDue) return;
    synthesisScheduleSaving = true;
    updateProfileSaveButtons();
    try {
      const result = await window.alvum.synthesisScheduleRunDue();
      if (result && result.schedule) synthesisSchedule = result.schedule;
      if (!result || result.ok === false) showMenuNotification((result && result.error) || 'No due synthesis started.', 'warning', 'Schedule');
      else showMenuNotification('Due synthesis queued.', 'success', 'Schedule');
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Schedule');
    } finally {
      synthesisScheduleSaving = false;
      renderProfileSchedule();
    }
  }

	  async function saveSynthesisProfile() {
	    if (!synthesisProfile) return;
	    if (activeView === 'profile-advanced-detail') {
	      synthesisProfile.advanced_instructions = $('profile-advanced').value;
	    }
	    synthesisProfileSaving = true;
	    renderActiveSynthesisProfileView();
	    const result = await window.alvum.synthesisProfileSave(synthesisProfile);
	    synthesisProfileSaving = false;
	    if (!result || !result.ok) {
	      showMenuNotification((result && result.error) || 'Could not save synthesis profile', 'warning');
	    } else {
	      if (result.profile) synthesisProfile = result.profile;
	      if (Array.isArray(result.suggestions)) synthesisProfileSuggestions = result.suggestions;
	    }
	    renderActiveSynthesisProfileView();
	  }

	  async function removeProfileIntention() {
	    if (!synthesisProfile) return;
	    const intention = profileIntentionById(selectedProfileIntentionId);
	    if (!intention) return;
	    if (!window.confirm(`Remove "${intention.description || 'this intention'}" from the synthesis profile?`)) return;
	    synthesisProfile.intentions = synthesisProfile.intentions.filter((item) => item !== intention);
	    selectedProfileIntentionId = null;
	    setView('profile-intentions-list', 'back');
	    await saveSynthesisProfile();
	  }

	  async function removeProfileDomain() {
	    if (!synthesisProfile) return;
	    const domain = profileDomainById(selectedProfileDomainId);
	    if (!domain) return;
	    if (synthesisProfile.domains.length <= 1 || !canDisableProfileDomain(domain)) {
	      showMenuNotification('Keep at least one synthesis domain enabled.', 'warning');
	      return;
	    }
	    if (!window.confirm(`Remove "${domain.name || domain.id || 'this domain'}" from the synthesis profile?`)) return;
	    synthesisProfile.domains = synthesisProfile.domains.filter((item) => item !== domain);
	    for (const intention of ensureSynthesisProfileShape().intentions) {
	      if (intention.domain === domain.id) intention.domain = '';
	    }
	    selectedProfileDomainId = null;
	    setView('profile-domains-list', 'back');
	    await saveSynthesisProfile();
	  }

	  async function removeProfileInterest() {
	    if (!synthesisProfile) return;
	    const interest = profileInterestById(selectedProfileInterestId);
	    if (!interest) return;
	    if (!window.confirm(`Remove "${interest.name || 'this tracked item'}" from the synthesis profile?`)) return;
	    synthesisProfile.interests = synthesisProfile.interests.filter((item) => item !== interest);
	    selectedProfileInterestId = null;
	    setView('profile-interests-list', 'back');
	    await saveSynthesisProfile();
	  }

	  async function openBriefingReader(date, parentView = 'briefing') {
    briefingReaderParent = parentView;
    const result = await window.alvum.readBriefingDate(date);
    readerDate = date;
    if (!result || !result.ok) {
      readerMarkdown = '';
      $('reader-title').textContent = date;
      $('reader-meta').textContent = (result && result.error) || 'Synthesis unavailable';
      $('briefing-markdown').innerHTML = '<p>Synthesis unavailable.</p>';
    } else {
      readerMarkdown = result.markdown || '';
      $('reader-title').textContent = result.date;
      $('reader-meta').textContent = result.mtime || result.path || '';
      $('briefing-markdown').innerHTML = result.html || '<p>No synthesis content.</p>';
    }
    setView('briefing-reader');
    requestPopoverResize();
  }

  function escapeHtml(value) {
    return String(value == null ? '' : value)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function decisionTimeMs(decision, index = 0) {
    const timestamp = decision && decision.timestamp ? Date.parse(decision.timestamp) : NaN;
    if (Number.isFinite(timestamp)) return timestamp;
    const date = decision && decision.date ? decision.date : decisionGraphDate;
    const time = decision && decision.time ? decision.time : '12:00';
    const parsed = Date.parse(`${date}T${time}:00`);
    return Number.isFinite(parsed) ? parsed : index;
  }

  function decisionTimeLabel(decision) {
    if (decision && decision.time) return decision.time;
    if (decision && decision.timestamp) {
      const parsed = new Date(decision.timestamp);
      if (!Number.isNaN(parsed.getTime())) {
        return parsed.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
      }
    }
    return '';
  }

  function graphDecisionDomain(decision) {
    return String((decision && decision.domain) || 'Other');
  }

  function graphEdges(data) {
    return (data && data.edges || [])
      .filter((edge) => edge && edge.from_id && edge.to_id);
  }

  function graphSourceKey(source) {
    const value = String(source || '').toLowerCase();
    if (value.includes('spoken')) return 'spoken';
    if (value.includes('revealed')) return 'revealed';
    if (value.includes('explained')) return 'explained';
    return 'other';
  }

  function graphSourceColorFromKey(key) {
    if (key === 'spoken') return 'var(--accent)';
    if (key === 'revealed') return 'var(--warn)';
    if (key === 'explained') return 'var(--dusk)';
    return 'var(--fg-faint)';
  }

  function graphSourceColor(source) {
    return graphSourceColorFromKey(graphSourceKey(source));
  }

  function graphSourceLabel(source) {
    const key = graphSourceKey(source);
    if (key === 'spoken') return 'Spoken';
    if (key === 'revealed') return 'Revealed';
    if (key === 'explained') return 'Explained';
    return 'Other';
  }

  function graphStrengthWidth(strength) {
    const value = String(strength || '').toLowerCase();
    if (value === 'primary') return 2.2;
    if (value === 'background') return 1;
    return 1.5;
  }

  function graphDecisionRadius(decision) {
    const magnitude = Math.max(0.1, Math.min(1, Number(decision && decision.magnitude || 0.35)));
    return 6 + magnitude * 8;
  }

  function graphClamp(value, min, max) {
    return Math.max(min, Math.min(max, value));
  }

  function decisionGraphComponentEdges(component, edges) {
    const ids = new Set((component.decisions || []).map((decision) => decision.id));
    return edges.filter((edge) => ids.has(edge.from_id) && ids.has(edge.to_id));
  }

  function decisionGraphLaneCount(component, componentEdges) {
    const size = (component.decisions || []).length;
    if (size <= 2) return 1;
    if (!componentEdges.length) return Math.min(2, Math.ceil(size / 6));
    const degree = new Map((component.decisions || []).map((decision) => [decision.id, { incoming: 0, outgoing: 0 }]));
    componentEdges.forEach((edge) => {
      const from = degree.get(edge.from_id);
      const to = degree.get(edge.to_id);
      if (from) from.outgoing += 1;
      if (to) to.incoming += 1;
    });
    const branching = Array.from(degree.values()).some((value) => value.incoming > 1 || value.outgoing > 2);
    return Math.min(4, Math.max(2, Math.ceil(size / 5) + (branching ? 1 : 0)));
  }

  function decisionGraphLaneOrder(laneCount) {
    const center = (laneCount - 1) / 2;
    return Array.from({ length: laneCount }, (_, index) => index)
      .sort((left, right) => Math.abs(left - center) - Math.abs(right - center) || left - right);
  }

  function decisionGraphLaneY(top, bottom, lane, laneCount) {
    if (laneCount <= 1) return (top + bottom) / 2;
    const padding = Math.min(42, Math.max(24, (bottom - top) * 0.18));
    const laneTop = top + padding;
    const laneBottom = bottom - padding;
    return laneTop + (laneBottom - laneTop) * (lane / (laneCount - 1));
  }

  function graphComponents(decisions, edges) {
    const byId = new Map(decisions.map((decision) => [decision.id, decision]));
    const adjacency = new Map(decisions.map((decision) => [decision.id, new Set()]));
    edges.forEach((edge) => {
      if (!byId.has(edge.from_id) || !byId.has(edge.to_id)) return;
      adjacency.get(edge.from_id).add(edge.to_id);
      adjacency.get(edge.to_id).add(edge.from_id);
    });
    const visited = new Set();
    const components = [];
    decisions.forEach((decision) => {
      if (visited.has(decision.id)) return;
      const stack = [decision.id];
      const ids = [];
      visited.add(decision.id);
      while (stack.length) {
        const id = stack.pop();
        ids.push(id);
        (adjacency.get(id) || []).forEach((next) => {
          if (visited.has(next)) return;
          visited.add(next);
          stack.push(next);
        });
      }
      const items = ids
        .map((id) => byId.get(id))
        .filter(Boolean)
        .sort((left, right) => decisionTimeMs(left) - decisionTimeMs(right));
      components.push({
        id: `component_${components.length + 1}`,
        decisions: items,
        firstMs: items.length ? decisionTimeMs(items[0]) : 0,
      });
    });
    const sorted = components.sort((left, right) => left.firstMs - right.firstMs || right.decisions.length - left.decisions.length);
    const packed = [];
    let isolated = [];
    const flushIsolated = () => {
      if (!isolated.length) return;
      packed.push({
        id: `component_${packed.length + 1}`,
        decisions: isolated,
        firstMs: decisionTimeMs(isolated[0]),
      });
      isolated = [];
    };
    sorted.forEach((component) => {
      const only = component.decisions[0];
      const isIsolated = component.decisions.length === 1 && only && !(adjacency.get(only.id) || new Set()).size;
      if (!isIsolated) {
        flushIsolated();
        packed.push(component);
        return;
      }
      isolated.push(only);
      if (isolated.length >= 10) flushIsolated();
    });
    flushIsolated();
    return packed;
  }

  function relaxDecisionGraphNodes(nodes, bounds) {
    for (let iteration = 0; iteration < 84; iteration += 1) {
      for (let i = 0; i < nodes.length; i += 1) {
        const node = nodes[i];
        node.x += (node.targetX - node.x) * 0.055;
        node.y += (node.targetY - node.y) * 0.04;
      }
      for (let i = 0; i < nodes.length; i += 1) {
        for (let j = i + 1; j < nodes.length; j += 1) {
          const left = nodes[i];
          const right = nodes[j];
          let dx = right.x - left.x;
          let dy = right.y - left.y;
          let distance = Math.sqrt(dx * dx + dy * dy);
          if (distance < 0.001) {
            const angle = ((i * 37 + j * 19) % 360) * Math.PI / 180;
            dx = Math.cos(angle);
            dy = Math.sin(angle);
            distance = 1;
          }
          const minimum = left.r + right.r + 16;
          if (distance >= minimum) continue;
          const push = (minimum - distance) * 0.5;
          const nx = dx / distance;
          const ny = dy / distance;
          left.x -= nx * push;
          right.x += nx * push;
          left.y -= ny * push;
          right.y += ny * push;
        }
      }
      nodes.forEach((node) => {
        node.x = graphClamp(node.x, bounds.left + node.r, bounds.right - node.r);
        node.y = graphClamp(node.y, bounds.top + node.r, bounds.bottom - node.r);
      });
    }
  }

  function layoutDecisionGraph(data) {
    const decisions = (data.decisions || [])
      .slice()
      .sort((left, right) => decisionTimeMs(left) - decisionTimeMs(right));
    const edges = graphEdges(data);
    const components = graphComponents(decisions, edges);
    const width = 720;
    const padL = 34;
    const padR = 36;
    const padT = 28;
    const padB = 30;
    const gap = 18;
    const componentHeights = components.map((component) => {
      const size = component.decisions.length;
      const componentEdges = decisionGraphComponentEdges(component, edges);
      const laneCount = decisionGraphLaneCount(component, componentEdges);
      if (!componentEdges.length) {
        return Math.max(86, Math.min(150, 70 + laneCount * 30 + Math.ceil(size / Math.max(1, laneCount)) * 4));
      }
      return Math.max(116, Math.min(260, 70 + laneCount * 44 + Math.ceil(size / laneCount) * 8));
    });
    const height = Math.max(220, padT + padB + componentHeights.reduce((sum, value) => sum + value, 0) + Math.max(0, components.length - 1) * gap);
    const innerW = width - padL - padR;
    const positions = [];
    let offsetY = padT;
    components.forEach((component, componentIndex) => {
      const bandHeight = componentHeights[componentIndex];
      const top = offsetY;
      const bottom = top + bandHeight;
      const componentEdges = decisionGraphComponentEdges(component, edges);
      const laneCount = decisionGraphLaneCount(component, componentEdges);
      const laneOrder = decisionGraphLaneOrder(laneCount);
      const nodes = component.decisions.map((decision, index) => {
        const xFactor = component.decisions.length <= 1 ? 0.5 : index / (component.decisions.length - 1);
        const lane = laneOrder[index % laneOrder.length];
        const laneTargetY = decisionGraphLaneY(top, bottom, lane, laneCount);
        const jitter = ((index % 3) - 1) * 3 + (componentIndex % 2 ? 2 : -2);
        const r = graphDecisionRadius(decision);
        return {
          component: component.id,
          decision,
          r,
          targetX: padL + innerW * xFactor,
          targetY: laneTargetY,
          x: padL + innerW * xFactor,
          y: laneTargetY + jitter,
        };
      });
      relaxDecisionGraphNodes(nodes, {
        left: padL,
        right: width - padR,
        top: top + 10,
        bottom: bottom - 10,
      });
      positions.push(...nodes);
      component.top = top;
      component.bottom = bottom;
      offsetY = bottom + gap;
    });
    return { width, height, components, positions };
  }

  function decisionGraphEdgeBend(edge, index, from, to) {
    if (from.component !== to.component) return -22;
    const dx = Math.abs(to.x - from.x);
    if (dx < 1) return 0;
    const base = graphClamp(dx * 0.06, 10, 28);
    const sameLane = Math.abs(to.y - from.y) < 8;
    const direction = index % 2 === 0 ? -1 : 1;
    return direction * (sameLane ? base : base * 0.45);
  }

  function decisionGraphSvg(data, selectedId) {
    const { width, height, components, positions } = layoutDecisionGraph(data);
    const byId = Object.fromEntries(positions.map((position) => [position.decision.id, position]));
    const componentBands = components.map((component, index) => {
      const y = component.bottom + 9;
      if (index === components.length - 1) return '';
      return `<line class="decision-graph-component-band" x1="26" x2="${width - 26}" y1="${y}" y2="${y}" stroke="var(--hairline)" stroke-dasharray="2 6" stroke-width="1" opacity="0.72"/>`;
    }).join('');
    const edgePaths = graphEdges(data).map((edge, index) => {
      const from = byId[edge.from_id];
      const to = byId[edge.to_id];
      if (!from || !to) return '';
      const selected = from.decision.id === selectedId || to.decision.id === selectedId;
      const dx = to.x - from.x;
      const c1x = from.x + dx * 0.38;
      const c2x = to.x - dx * 0.38;
      const bend = decisionGraphEdgeBend(edge, index, from, to);
      const width = selected ? graphStrengthWidth(edge.strength) + 0.7 : graphStrengthWidth(edge.strength);
      return `<path d="M ${from.x} ${from.y} C ${c1x} ${from.y + bend}, ${c2x} ${to.y + bend}, ${to.x} ${to.y}" fill="none" stroke="${selected ? 'var(--fg)' : 'var(--fg-faint)'}" stroke-width="${width}" stroke-linecap="round" marker-end="url(#decisionGraphArrow)" opacity="${selected ? 0.82 : 0.34}"/>`;
    }).join('');
    const nodes = positions.map((position) => {
      const decision = position.decision;
      const selected = decision.id === selectedId;
      const r = position.r;
      const title = `${decision.id} · ${graphDecisionDomain(decision)} · ${decision.summary || ''}`;
      return `<g class="decision-graph-node" data-decision-id="${escapeHtml(decision.id)}" transform="translate(${position.x} ${position.y})" role="button" tabindex="0" aria-label="${escapeHtml(title)}"><circle class="decision-graph-node-ring" r="${r + 10}" fill="none" stroke="var(--fg)" stroke-width="1.5" opacity="${selected ? 0.5 : 0}"/><circle r="${r + (decision.open ? 4 : 0)}" fill="none" stroke="${graphSourceColor(decision.source)}" stroke-width="1.2" stroke-dasharray="${decision.open ? '3 3' : '0 30'}" opacity="${decision.open ? 0.75 : 0}"/><circle r="${r}" fill="${selected ? 'var(--fg)' : graphSourceColor(decision.source)}" opacity="${selected ? 1 : 0.92}"/><circle r="${Math.max(2, r * 0.34)}" fill="${selected ? 'var(--bg-raised)' : 'color-mix(in srgb, var(--bg-raised) 42%, transparent)'}"/></g>`;
    }).join('');
    return `<svg viewBox="0 0 ${width} ${height}" role="img" aria-label="Decision graph for ${escapeHtml(data.date || '')}" preserveAspectRatio="xMidYMid meet"><defs><marker id="decisionGraphArrow" markerHeight="7" markerWidth="7" orient="auto" refX="9" refY="5" viewBox="0 0 10 10"><path d="M 0 0 L 10 5 L 0 10 z" fill="var(--fg-faint)"/></marker></defs>${componentBands}${edgePaths}${nodes}</svg>`;
  }

  function renderDecisionGraphLegend(data) {
    const legend = $('decision-graph-legend');
    legend.replaceChildren();
    const order = ['revealed', 'spoken', 'explained', 'other'];
    const labels = {
      revealed: 'Revealed',
      spoken: 'Spoken',
      explained: 'Explained',
      other: 'Other',
    };
    const present = new Set((data && data.decisions || []).map((decision) => graphSourceKey(decision.source)));
    const keys = order.filter((key) => present.has(key));
    if (!keys.length) keys.push('other');
    keys.forEach((key) => {
      const item = document.createElement('div');
      item.className = 'decision-graph-legend-item';
      const swatch = document.createElement('span');
      swatch.className = 'decision-graph-swatch';
      swatch.style.background = graphSourceColorFromKey(key);
      const label = document.createElement('span');
      label.textContent = labels[key] || 'Other';
      item.append(swatch, label);
      legend.appendChild(item);
    });
  }

  function hideDecisionGraphHover() {
    const hover = $('decision-graph-hover');
    hover.hidden = true;
    hover.replaceChildren();
  }

  function showDecisionGraphHover(decision, event = null) {
    if (!decision) return;
    const hover = $('decision-graph-hover');
    hover.replaceChildren();
    const value = document.createElement('div');
    value.className = 'value';
    value.textContent = decision.summary || decision.id;
    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = [
      decision.id,
      graphSourceLabel(decision.source),
      graphDecisionDomain(decision),
      decisionTimeLabel(decision),
    ].filter(Boolean).join(' · ');
    hover.append(value, meta);
    hover.hidden = false;
    const cardRect = hover.parentElement.getBoundingClientRect();
    const targetRect = event && event.currentTarget ? event.currentTarget.getBoundingClientRect() : cardRect;
    const x = event ? event.clientX - cardRect.left + 12 : targetRect.left - cardRect.left + targetRect.width + 8;
    const y = event ? event.clientY - cardRect.top + 12 : targetRect.top - cardRect.top + 8;
    const maxX = Math.max(8, cardRect.width - hover.offsetWidth - 8);
    const maxY = Math.max(8, cardRect.height - hover.offsetHeight - 8);
    hover.style.left = `${graphClamp(x, 8, maxX)}px`;
    hover.style.top = `${graphClamp(y, 8, maxY)}px`;
  }

  function selectDecisionGraphNode(id) {
    if (!id) return;
    selectedDecisionGraphNode = id;
    renderDecisionGraphView();
  }

  function appendDecisionGraphEdgeGroup(parent, title, rows) {
    const group = document.createElement('div');
    group.className = 'decision-graph-link-group';
    const groupTitle = document.createElement('div');
    groupTitle.className = 'decision-graph-link-title';
    groupTitle.textContent = title;
    group.appendChild(groupTitle);
    const row = document.createElement('div');
    row.className = 'decision-graph-link-row';
    rows.slice(0, 6).forEach((decision) => {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'decision-graph-link-chip';
      button.textContent = decision.id;
      button.setAttribute('aria-label', `Select ${decision.id}`);
      button.onclick = () => selectDecisionGraphNode(decision.id);
      row.appendChild(button);
    });
    group.appendChild(row);
    parent.appendChild(group);
  }

  function linkedGraphDecisions(edges, byId, idKey) {
    const seen = new Set();
    return edges
      .map((edge) => byId[edge[idKey]])
      .filter((decision) => {
        if (!decision || seen.has(decision.id)) return false;
        seen.add(decision.id);
        return true;
      });
  }

  function renderDecisionGraphDetail() {
    const wrap = $('decision-graph-detail');
    wrap.replaceChildren();
    if (!decisionGraphData || !Array.isArray(decisionGraphData.decisions)) return;
    const decisions = decisionGraphData.decisions;
    const byId = Object.fromEntries(decisions.map((decision) => [decision.id, decision]));
    const selected = decisions.find((decision) => decision.id === selectedDecisionGraphNode) || decisions[0];
    if (!selected) return;
    const incoming = graphEdges(decisionGraphData).filter((edge) => edge.to_id === selected.id);
    const outgoing = graphEdges(decisionGraphData).filter((edge) => edge.from_id === selected.id);

    const row = document.createElement('div');
    row.className = 'decision-graph-selected';
    const label = document.createElement('div');
    label.className = 'label';
    label.textContent = 'Selected decision';
    const value = document.createElement('div');
    value.className = 'value';
    value.textContent = selected.summary || selected.id;
    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = [
      selected.id,
      graphSourceLabel(selected.source),
      graphDecisionDomain(selected),
      decisionTimeLabel(selected),
      selected.open ? 'Open' : 'Closed',
      selected.magnitude != null ? `Magnitude ${Number(selected.magnitude).toFixed(2)}` : null,
    ].filter(Boolean).join(' · ');
    row.append(label, value, meta);
    wrap.appendChild(row);

    const edgeList = document.createElement('div');
    edgeList.className = 'summary-row';
    const edgeTitle = document.createElement('div');
    edgeTitle.className = 'value';
    edgeTitle.textContent = 'Graph links';
    const edgeMeta = document.createElement('div');
    edgeMeta.className = 'meta';
    edgeMeta.textContent = `${incoming.length} previous · ${outgoing.length} next`;
    edgeList.append(edgeTitle, edgeMeta);
    const groups = document.createElement('div');
    groups.className = 'decision-graph-link-groups';
    appendDecisionGraphEdgeGroup(
      groups,
      'Previous',
      linkedGraphDecisions(incoming, byId, 'from_id'));
    appendDecisionGraphEdgeGroup(
      groups,
      'Next',
      linkedGraphDecisions(outgoing, byId, 'to_id'));
    edgeList.appendChild(groups);
    wrap.appendChild(edgeList);
  }

  function renderDecisionGraphView() {
    const canvas = $('decision-graph-canvas');
    const detail = $('decision-graph-detail');
    detail.replaceChildren();
    hideDecisionGraphHover();
    if (decisionGraphLoading) {
      canvas.innerHTML = '<div class="decision-graph-empty"><div class="value">Loading graph</div><div class="meta">Reading decision artifacts for this day.</div></div>';
      $('decision-graph-legend').replaceChildren();
      $('decision-graph-title').textContent = decisionGraphDate ? displayDate(decisionGraphDate) : 'Decision graph';
      $('decision-graph-meta').textContent = 'Loading';
      requestPopoverResize();
      return;
    }
    if (decisionGraphError || !decisionGraphData) {
      canvas.innerHTML = `<div class="decision-graph-empty"><div class="value">Graph unavailable</div><div class="meta">${escapeHtml(decisionGraphError || 'No graph loaded.')}</div></div>`;
      $('decision-graph-legend').replaceChildren();
      $('decision-graph-title').textContent = decisionGraphDate ? displayDate(decisionGraphDate) : 'Decision graph';
      $('decision-graph-meta').textContent = decisionGraphError || 'No graph loaded';
      requestPopoverResize();
      return;
    }
    const decisions = Array.isArray(decisionGraphData.decisions) ? decisionGraphData.decisions : [];
    const edges = graphEdges(decisionGraphData);
    const decisionById = Object.fromEntries(decisions.map((decision) => [decision.id, decision]));
    if (!selectedDecisionGraphNode && decisions.length) selectedDecisionGraphNode = decisions[0].id;
    canvas.innerHTML = decisionGraphSvg(decisionGraphData, selectedDecisionGraphNode);
    canvas.querySelectorAll('[data-decision-id]').forEach((node) => {
      const decision = decisionById[node.getAttribute('data-decision-id')];
      node.addEventListener('mouseenter', (event) => showDecisionGraphHover(decision, event));
      node.addEventListener('mousemove', (event) => showDecisionGraphHover(decision, event));
      node.addEventListener('mouseleave', hideDecisionGraphHover);
      node.addEventListener('focus', (event) => showDecisionGraphHover(decision, event));
      node.addEventListener('blur', hideDecisionGraphHover);
      node.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        selectedDecisionGraphNode = node.getAttribute('data-decision-id');
        renderDecisionGraphView();
      });
      node.addEventListener('click', () => {
        selectedDecisionGraphNode = node.getAttribute('data-decision-id');
        renderDecisionGraphView();
      });
    });
    renderDecisionGraphLegend(decisionGraphData);
    $('decision-graph-title').textContent = displayDate(decisionGraphData.date || decisionGraphDate);
    $('decision-graph-meta').textContent = [
      `${decisions.length} decision${decisions.length === 1 ? '' : 's'}`,
      `${edges.length} link${edges.length === 1 ? '' : 's'}`,
      decisionGraphData.derived_edges ? `${decisionGraphData.derived_edges} derived` : null,
    ].filter(Boolean).join(' · ');
    renderDecisionGraphDetail();
    requestPopoverResize();
  }

  async function openDecisionGraphView(date) {
    decisionGraphDate = date;
    decisionGraphData = null;
    decisionGraphError = null;
    decisionGraphLoading = true;
    selectedDecisionGraphNode = null;
    setView('decision-graph');
    renderDecisionGraphView();
    try {
      const result = await window.alvum.decisionGraphDate(date);
      if (!result || !result.ok) {
        decisionGraphError = (result && result.error) || 'Decision graph unavailable';
      } else {
        decisionGraphData = result;
        selectedDecisionGraphNode = result.decisions && result.decisions[0] ? result.decisions[0].id : null;
      }
    } catch (err) {
      decisionGraphError = extensionErrorMessage(err);
    } finally {
      decisionGraphLoading = false;
      renderDecisionGraphView();
    }
  }

  function extensionStatusLabel(ext) {
    if (!ext) return 'unknown';
    if (permissionIssuesFrom(ext).length) return 'Blocked by permissions';
    const controls = connectorSourceControls(ext);
    if (controls.length) return connectorSourceStatusLabel(ext);
    return ext.enabled ? 'Enabled' : 'Disabled';
  }

  function extensionDotLevel(ext) {
    if (!ext) return 'red';
    if (permissionIssuesFrom(ext).length) return 'yellow';
    const aggregate = connectorAggregateState(ext);
    if (aggregate === 'partial') return 'yellow';
    if (aggregate === 'all_off') return 'red';
    return ext.enabled ? 'green' : 'red';
  }

  function extensionById(id) {
    if (!extensionSummary) return null;
    const connectors = Array.isArray(extensionSummary.connectors) ? extensionSummary.connectors : [];
    return connectors.find((connector) => connector.id === id || connector.component_id === id) || null;
  }

  function connectorSourceControls(ext) {
    return ext && Array.isArray(ext.source_controls) ? ext.source_controls : [];
  }

  function connectorProcessorControls(ext) {
    if (!ext) return [];
    if (Array.isArray(ext.processor_controls)) return ext.processor_controls;
    if (!Array.isArray(ext.processors)) return [];
    return ext.processors.map((processor) => ({
      id: processor.component,
      component: processor.component,
      label: processor.display_name || processor.component,
      kind: processor.kind || 'processor',
      detail: processor.exists === false ? 'Processor component is not installed.' : '',
      settings: [],
    }));
  }

  function isAudioProcessorControl(control) {
    return control && control.component === 'alvum.audio/whisper';
  }

  function audioProcessorMode(settings) {
    const mode = settings.find((setting) => setting && setting.key === 'mode');
    const value = mode && mode.value != null ? String(mode.value) : 'local';
    return value || 'local';
  }

  function audioProcessorSettingCopy(setting) {
    if (!setting || !setting.key) return setting;
    if (setting.key === 'mode') {
      return {
        ...setting,
        label: 'Audio processing',
        detail: 'Choose Local Whisper + speaker IDs, provider diarized transcription, or off.',
        options: [
          { value: 'local', label: 'Local Whisper + speaker IDs' },
          { value: 'provider', label: 'Provider diarized transcription' },
          { value: 'off', label: 'Off' },
        ],
      };
    }
    if (setting.key === 'whisper_model') {
      return {
        ...setting,
        label: 'Local transcription model',
        detail: 'Whisper model file used when audio processing is Local.',
      };
    }
    if (setting.key === 'whisper_language') {
      return {
        ...setting,
        label: 'Local transcription language',
        detail: 'Language hint used by local Whisper transcription.',
      };
    }
    if (setting.key === 'diarization_enabled') {
      return {
        ...setting,
        label: 'Local speaker IDs',
        detail: 'Stores anonymous local speaker IDs across runs when local processing is enabled.',
      };
    }
    if (setting.key === 'diarization_model') {
      return {
        ...setting,
        label: 'Local diarization model',
        detail: 'Local diarization and embedding backend used for anonymous voice evidence.',
      };
    }
    if (setting.key === 'pyannote_command') {
      return {
        ...setting,
        label: 'Pyannote command',
        detail: 'Optional local command that emits pyannote-compatible diarization JSON for an audio file.',
      };
    }
    if (setting.key === 'pyannote_hf_token') {
      return {
        ...setting,
        label: 'Hugging Face token',
        detail: 'Used only to download and load gated Pyannote diarization models.',
        placeholder: setting.configured ? 'Configured' : 'hf_...',
        secret: true,
      };
    }
    if (setting.key === 'speaker_registry') {
      return {
        ...setting,
        label: 'Local speaker registry',
        detail: 'Local file storing anonymous speaker IDs and confirmed labels.',
      };
    }
    if (setting.key === 'provider') {
      return {
        ...setting,
        label: 'Provider diarized transcription',
        detail: 'Used only when audio processing mode is Provider. Local mode uses Whisper and local speaker IDs.',
      };
    }
    return setting;
  }

  function processorSettingsForMode(control, settings) {
    if (!isAudioProcessorControl(control)) return settings;
    const mode = audioProcessorMode(settings);
    const visible = settings.filter((setting) => {
      if (!setting || !setting.key) return false;
      if (setting.key === 'mode') return true;
      if (mode === 'provider') {
        return setting.key === 'provider' || setting.key === 'speaker_registry';
      }
      if (mode === 'local') {
        return LOCAL_AUDIO_PROCESSOR_SETTING_KEYS.has(String(setting.key || ''));
      }
      return false;
    });
    return visible.map(audioProcessorSettingCopy);
  }

  function connectorEnabledSourceCount(ext) {
    const controls = connectorSourceControls(ext);
    if (!controls.length) return ext && ext.enabled ? 1 : 0;
    return controls.filter((control) => control.enabled).length;
  }

  function connectorAggregateState(ext) {
    if (!ext) return 'all_off';
    if (ext.aggregate_state) return ext.aggregate_state;
    const controls = connectorSourceControls(ext);
    if (!controls.length) return ext.enabled ? 'all_on' : 'all_off';
    const enabled = controls.filter((control) => control.enabled).length;
    if (enabled === 0) return 'all_off';
    if (enabled === controls.length) return 'all_on';
    return 'partial';
  }

  function connectorSourceStatusLabel(ext) {
    if (permissionIssuesFrom(ext).length) return 'Blocked by permissions';
    const controls = connectorSourceControls(ext);
    if (!controls.length) return ext && ext.enabled ? 'Enabled' : 'Disabled';
    const enabled = connectorEnabledSourceCount(ext);
    if (enabled === 0) return 'Off';
    return `${enabled} of ${controls.length} source${controls.length === 1 ? '' : 's'} on`;
  }

  function connectorListStatusLabel(ext) {
    if (permissionIssuesFrom(ext).length) return 'Blocked by permissions';
    const aggregate = connectorAggregateState(ext);
    if (aggregate === 'partial') return 'Partially enabled';
    if (aggregate === 'all_off') return 'Disabled';
    return ext && ext.enabled ? 'Enabled' : 'Disabled';
  }

  function extensionErrorMessage(error) {
    if (!error) return 'unknown error';
    return error.message || String(error);
  }

  function permissionIssuesFrom(value) {
    if (!value) return [];
    if (Array.isArray(value)) return value;
    if (Array.isArray(value.permission_issues)) return value.permission_issues;
    if (Array.isArray(value.blocked_permissions)) {
      return value.blocked_permissions.map((issue) => ({
        ...issue,
        source_label: issue.source_label || value.label || value.id,
      }));
    }
    return [];
  }

  function permissionIssueText(value) {
    const issues = permissionIssuesFrom(value);
    if (!issues.length) return '';
    const permissionsById = new Map(issues.map((issue) => [
      issue.permission || issue.label || 'permission',
      issue.permission === 'screen'
        ? 'Screen & System Audio Recording'
        : (issue.label || issue.permission || 'permission'),
    ]));
    const permissions = [...permissionsById.values()];
    const sources = [...new Set(issues.map((issue) => issue.source_label).filter(Boolean))];
    const target = sources.length === 1 ? sources[0] : 'Enabled connectors';
    const suffix = permissions.length === 1 ? 'permission' : 'permissions';
    return `${target} blocked by ${permissions.join(' and ')} ${suffix}.`;
  }

  function handlePermissionIssues(value) {
    const text = permissionIssueText(value);
    if (text) showMenuNotification(text, 'warning', 'Permission needed');
  }

  function appendPermissionIssueRows(list, value) {
    const issues = permissionIssuesFrom(value);
    if (!issues.length) return;
    for (const issue of issues) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 'value';
      title.textContent = `${issue.label || issue.permission || 'Permission'} blocked`;
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = `${issue.source_label || 'This source'} is enabled but macOS reports ${issue.status || 'not granted'}.`;
      text.append(title, meta);
      const open = document.createElement('button');
      open.type = 'button';
      open.textContent = 'Open Settings';
      open.onclick = async () => {
        open.disabled = true;
        try {
          await window.alvum.openPermissionSettings(issue.permission);
          showMenuNotification('Open System Settings and allow Alvum. Capture will restart after the grant is visible.', 'warning', 'Permission needed');
        } catch (err) {
          console.error('[permissions] open settings failed', extensionErrorMessage(err));
          open.disabled = false;
        }
      };
      row.append(text, open);
      list.appendChild(row);
    }
  }

  function doctorSummaryText(result) {
    if (!result) return 'Diagnostics did not return a result.';
    if (result.error) return result.error;
    const checks = Array.isArray(result.checks) ? result.checks : [];
    const errors = Number.isFinite(Number(result.error_count))
      ? Number(result.error_count)
      : checks.filter((check) => check.level === 'error').length;
    const warnings = Number.isFinite(Number(result.warning_count))
      ? Number(result.warning_count)
      : checks.filter((check) => check.level === 'warning').length;
    if (errors > 0) {
      const warningText = warnings > 0 ? ` and ${warnings} warning${warnings === 1 ? '' : 's'}` : '';
      return `Diagnostics found ${errors} error${errors === 1 ? '' : 's'}${warningText}.`;
    }
    if (warnings > 0) return `Diagnostics found ${warnings} warning${warnings === 1 ? '' : 's'}.`;
    return `Diagnostics passed ${checks.length} check${checks.length === 1 ? '' : 's'}.`;
  }

  function doctorNotificationLevel(result) {
    if (!result || result.error || Number(result.error_count) > 0) return 'error';
    if (Number(result.warning_count) > 0) return 'warning';
    return 'info';
  }

  function showMenuNotification(text, level = 'info', heading = null) {
    const notification = $('menu-notification');
    const title = $('menu-notification-title');
    const meta = $('menu-notification-meta');
    if (!notification || !title || !meta) return;
    if (menuNotificationDismissTimer) {
      clearTimeout(menuNotificationDismissTimer);
      menuNotificationDismissTimer = null;
    }
    if (menuNotificationHideTimer) {
      clearTimeout(menuNotificationHideTimer);
      menuNotificationHideTimer = null;
    }
    if (!text) {
      notification.classList.remove('presenting');
      notification.classList.add('dismissing');
      menuNotificationHideTimer = setTimeout(() => {
        menuNotificationHideTimer = null;
        notification.hidden = true;
        notification.className = 'menu-notification';
        title.textContent = '';
        meta.textContent = '';
      }, 180);
      requestPopoverResize();
      return;
    }
    notification.hidden = false;
    const normalizedLevel = level === 'error' || level === 'warning' ? level : 'info';
    notification.className = `menu-notification ${normalizedLevel}`;
    title.textContent = heading || (normalizedLevel === 'error'
      ? 'Diagnostics failed'
      : (normalizedLevel === 'warning' ? 'Diagnostics warning' : 'Diagnostics complete'));
    meta.textContent = text;
    void notification.offsetHeight;
    notification.classList.add('presenting');
    requestPopoverResize();
    menuNotificationDismissTimer = setTimeout(() => {
      menuNotificationDismissTimer = null;
      showMenuNotification(null);
    }, 2000);
  }

  async function saveConnectorProcessorSetting(control, setting, nextValue, controlEl) {
    if (!control || !setting || !setting.key) return;
    if (controlEl) controlEl.disabled = true;
    let result;
    try {
      result = await window.alvum.connectorProcessorSetSetting(control.component, setting.key, nextValue);
      if (result && Array.isArray(result.connectors)) extensionSummary = { connectors: result.connectors };
      else await refreshExtensions(true);
      if (result && result.ok === false) console.error('[connector] processor setting update failed', result.error || 'processor setting update failed');
    } catch (err) {
      console.error('[connector] processor setting update failed', extensionErrorMessage(err));
      await refreshExtensions(true);
    }
    renderExtensionDetail();
  }

  function isAudioConnector(ext) {
    return !!ext && (
      ext.component_id === 'alvum.audio/audio'
      || (ext.package_id === 'alvum.audio' && ext.connector_id === 'audio')
      || ext.id === 'alvum.audio/audio'
    );
  }

  async function refreshSpeakers(force = false) {
    if (speakerLoading) return;
    if (speakerSummary && !force) return;
    speakerLoading = true;
    renderSpeakerManagement(extensionById(selectedExtension));
    renderActiveSynthesisProfileView();
    try {
      speakerSummary = await window.alvum.speakerList();
    } catch (err) {
      speakerSummary = { ok: false, speakers: [], error: extensionErrorMessage(err) };
    } finally {
      speakerLoading = false;
      renderSpeakerManagement(extensionById(selectedExtension));
      renderActiveSynthesisProfileView();
    }
  }

  function applySpeakerResult(result) {
    if (result && Array.isArray(result.speakers)) {
      speakerSummary = result;
    }
    if (result && result.ok === false) {
      showMenuNotification(result.error || 'Speaker registry update failed.', 'warning', 'Speakers');
    }
    renderActiveSynthesisProfileView();
  }

  async function linkSpeakerToInterest(speaker, interestId, controlEl) {
    if (!speaker || !speaker.speaker_id || !interestId) return;
    if (controlEl) controlEl.disabled = true;
    try {
      applySpeakerResult(await window.alvum.speakerLink(speaker.speaker_id, interestId));
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Voices');
    }
    renderSpeakerManagement(extensionById(selectedExtension));
  }

  async function linkVoiceSampleToInterest(sample, interestId, controlEl) {
    if (!sample || !sample.sample_id || !interestId) return;
    if (controlEl) controlEl.disabled = true;
    try {
      applySpeakerResult(await window.alvum.speakerLinkSample(sample.sample_id, interestId));
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Voices');
    } finally {
      if (controlEl) controlEl.disabled = false;
    }
    renderSpeakerManagement(extensionById(selectedExtension));
  }

  async function moveVoiceSample(sample, clusterId, controlEl) {
    if (!sample || !sample.sample_id || !clusterId) return;
    if (controlEl) controlEl.disabled = true;
    try {
      applySpeakerResult(await window.alvum.speakerMoveSample(sample.sample_id, clusterId));
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Voices');
    } finally {
      if (controlEl) controlEl.disabled = false;
    }
    renderSpeakerManagement(extensionById(selectedExtension));
  }

  async function ignoreVoiceSample(sample, controlEl) {
    if (!sample || !sample.sample_id) return;
    if (controlEl) controlEl.disabled = true;
    try {
      applySpeakerResult(await window.alvum.speakerIgnoreSample(sample.sample_id));
      selectedProfileVoiceSampleId = null;
      setView('profile-voices-list');
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Voices');
      if (controlEl) controlEl.disabled = false;
    }
    renderSpeakerManagement(extensionById(selectedExtension));
  }

  async function unlinkSpeakerFromInterest(speaker, controlEl) {
    if (!speaker || !speaker.speaker_id) return;
    if (controlEl) controlEl.disabled = true;
    try {
      applySpeakerResult(await window.alvum.speakerUnlink(speaker.speaker_id));
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Voices');
    }
    renderSpeakerManagement(extensionById(selectedExtension));
  }

  async function createTrackedPersonForSpeaker(speaker, name, controlEl) {
    if (!speaker || !speaker.speaker_id) return;
    ensureSynthesisProfileShape();
    const sample = Array.isArray(speaker.samples) && speaker.samples.length
      ? speaker.samples[speaker.samples.length - 1]
      : null;
    const trimmed = String(name || '').trim();
    if (!trimmed) return;
    if (controlEl) controlEl.disabled = true;
    const interest = {
      id: makeProfileId('person', synthesisProfile.interests),
      type: 'person',
      interest_type: 'person',
      name: trimmed,
      aliases: [],
      notes: sample && sample.text ? `Created from voice evidence: ${sample.text}` : 'Created from voice evidence.',
      priority: (synthesisProfile.interests || []).length,
      enabled: true,
      linked_knowledge_ids: [],
    };
    synthesisProfile.interests.push(interest);
    try {
      await saveSynthesisProfile();
      await linkSpeakerToInterest(speaker, interest.id, controlEl);
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Voices');
    }
  }

  async function createTrackedPersonForVoiceSample(sample, name, controlEl) {
    if (!sample || !sample.sample_id) return;
    ensureSynthesisProfileShape();
    const trimmed = String(name || '').trim();
    if (!trimmed) return;
    if (controlEl) controlEl.disabled = true;
    const interest = {
      id: makeProfileId('person', synthesisProfile.interests),
      type: 'person',
      interest_type: 'person',
      name: trimmed,
      aliases: [],
      notes: sample && sample.text ? `Created from voice evidence: ${sample.text}` : 'Created from voice evidence.',
      priority: (synthesisProfile.interests || []).length,
      enabled: true,
      linked_knowledge_ids: [],
    };
    synthesisProfile.interests.push(interest);
    try {
      await saveSynthesisProfile();
      await linkVoiceSampleToInterest(sample, interest.id, controlEl);
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Voices');
    } finally {
      if (controlEl) controlEl.disabled = false;
    }
  }

  async function playSpeakerSample(speaker, sampleIndex, controlEl) {
    if (!speaker || !speaker.speaker_id) return;
    if (activeSpeakerAudio) {
      activeSpeakerAudio.pause();
      activeSpeakerAudio = null;
    }
    if (controlEl) controlEl.disabled = true;
    try {
      const result = await window.alvum.speakerSampleAudio(speaker.speaker_id, sampleIndex);
      if (!result || result.ok === false || !result.url) {
        showMenuNotification((result && result.error) || 'Sample audio unavailable.', 'warning', 'Voices');
        return;
      }
      const audio = new Audio(result.url);
      activeSpeakerAudio = audio;
      const start = Number(result.start_secs || 0);
      const end = Number(result.end_secs || 0);
      audio.currentTime = Math.max(0, start);
      if (end > start) {
        audio.ontimeupdate = () => {
          if (audio.currentTime >= end) {
            audio.pause();
            activeSpeakerAudio = null;
          }
        };
      }
      await audio.play();
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Voices');
    } finally {
      if (controlEl) controlEl.disabled = false;
    }
  }

  async function playVoiceSample(sample, controlEl) {
    if (!sample || !sample.sample_id) return;
    if (activeSpeakerAudio) {
      activeSpeakerAudio.pause();
      activeSpeakerAudio = null;
    }
    if (controlEl) controlEl.disabled = true;
    try {
      const result = await window.alvum.voiceSampleAudio(sample.sample_id);
      if (!result || result.ok === false || !result.url) {
        showMenuNotification((result && result.error) || 'Sample audio unavailable.', 'warning', 'Voices');
        return;
      }
      const audio = new Audio(result.url);
      activeSpeakerAudio = audio;
      const start = Number(result.start_secs || 0);
      const end = Number(result.end_secs || 0);
      audio.currentTime = Math.max(0, start);
      if (end > start) {
        audio.ontimeupdate = () => {
          if (audio.currentTime >= end) {
            audio.pause();
            activeSpeakerAudio = null;
          }
        };
      }
      await audio.play();
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Voices');
    } finally {
      if (controlEl) controlEl.disabled = false;
    }
  }

  async function renameSpeaker(speaker, label, controlEl) {
    if (!speaker || !speaker.speaker_id) return;
    if (controlEl) controlEl.disabled = true;
    try {
      applySpeakerResult(await window.alvum.speakerRename(speaker.speaker_id, label));
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Speakers');
    }
    renderSpeakerManagement(extensionById(selectedExtension));
  }

  async function mergeSpeaker(sourceId, targetId, controlEl) {
    if (!sourceId || !targetId || sourceId === targetId) return;
    if (controlEl) controlEl.disabled = true;
    try {
      applySpeakerResult(await window.alvum.speakerMerge(sourceId, targetId));
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Speakers');
    }
    renderSpeakerManagement(extensionById(selectedExtension));
  }

  async function forgetSpeaker(speakerId, controlEl) {
    if (!speakerId) return;
    if (controlEl) controlEl.disabled = true;
    try {
      applySpeakerResult(await window.alvum.speakerForget(speakerId));
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Speakers');
    }
    renderSpeakerManagement(extensionById(selectedExtension));
  }

  async function resetSpeakers(controlEl) {
    if (controlEl) controlEl.disabled = true;
    try {
      applySpeakerResult(await window.alvum.speakerReset());
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Speakers');
    }
    renderSpeakerManagement(extensionById(selectedExtension));
  }

  function renderConnectorCaptureControls(ext) {
    const title = $('extension-detail-capture-title');
    const list = $('extension-detail-capture-controls');
    const controls = connectorSourceControls(ext);
    title.textContent = 'Capture';
    list.replaceChildren();
    if (!ext) return;
    appendPermissionIssueRows(list, ext);
    if (!controls.length) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const label = document.createElement('div');
      label.className = 'value';
      label.textContent = 'No separate capture controls';
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = 'Use the connector action above.';
      text.append(label, meta);
      row.appendChild(text);
      list.appendChild(row);
      return;
    }
    for (const control of controls) {
      const row = document.createElement('div');
      row.className = 'input-row';
      row.role = control.toggleable ? 'button' : 'listitem';
      if (control.toggleable) row.tabIndex = 0;
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 'value status-line';
      const dot = document.createElement('span');
      dot.className = `status-dot ${control.enabled ? 'live' : ''}`;
      const name = document.createElement('span');
      name.textContent = control.label || control.id;
      title.append(dot, name);
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = control.blocked_permissions && control.blocked_permissions.length
        ? `Blocked · ${permissionIssueText(control)}`
        : (control.detail || control.component || control.kind || 'source');
      text.append(title, meta);

      const openSettings = async () => {
        if (!control.toggleable) return;
        if (!captureInputById(control.id)) await refreshCaptureInputs(true);
        selectedCaptureInput = control.id;
        captureInputParent = 'extension-detail';
        setView('capture-input');
      };
      row.onclick = () => openSettings();
      row.onkeydown = (e) => {
        if (e.key !== 'Enter' && e.key !== ' ') return;
        e.preventDefault();
        openSettings();
      };

      const toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = `switch ${control.enabled ? 'on' : ''}`;
      toggle.textContent = control.toggleable ? (control.enabled ? 'On' : 'Off') : 'Managed';
      toggle.disabled = !control.toggleable;
      toggle.onclick = async (e) => {
        e.stopPropagation();
        if (!control.toggleable) return;
        toggle.disabled = true;
        const nextEnabled = !control.enabled;
        let result;
        try {
          result = await window.alvum.toggleCaptureInput(control.id);
          if (result && result.captureInputs) captureInputs = result.captureInputs;
          else captureInputs = await window.alvum.captureInputs();
          await refreshExtensions(true);
          handlePermissionIssues(result);
          if (result && result.ok === false) {
            console.error('[connector] source update failed', result.error || 'source update failed');
          } else {
            toggle.textContent = nextEnabled ? 'On' : 'Off';
          }
        } catch (err) {
          console.error('[connector] source toggle failed', extensionErrorMessage(err));
          await refreshExtensions(true);
        }
        renderExtensionDetail();
      };

      const hint = document.createElement('span');
      hint.className = 'action-hint';
      hint.setAttribute('aria-hidden', 'true');
      hint.textContent = control.toggleable ? '›' : '';
      row.append(text, toggle, hint);
      list.appendChild(row);
    }
  }

  function renderProcessorSettingRow(list, control, setting) {
    renderSettingEditor(list, setting, setting.value, (nextValue, controlEl) =>
      saveConnectorProcessorSetting(control, setting, nextValue, controlEl));
  }

  function shouldShowPyannoteAccessCard(control) {
    const readiness = control && control.readiness;
    const tokenSetting = control && Array.isArray(control.settings)
      ? control.settings.find((setting) => setting && setting.key === 'pyannote_hf_token')
      : null;
    const tokenConfigured = !!(tokenSetting && tokenSetting.configured);
    const pyannoteAction = !!(readiness && readiness.action && readiness.action.kind === 'install_pyannote');
    const accessRequired = !!(
      pyannoteSetupIssue
      || (readiness && readiness.status === 'requires_huggingface_access')
      || (pyannoteAction && !tokenConfigured)
    );
    return isAudioProcessorControl(control) && accessRequired;
  }

  async function savePyannoteTokenAndRetry(control, input, button) {
    if (!control || !input || !button) return;
    const token = String(input.value || '').trim();
    if (!token) return;
    button.disabled = true;
    try {
      const result = await window.alvum.connectorProcessorSetSetting(control.component, 'pyannote_hf_token', token);
      if (result && Array.isArray(result.connectors)) extensionSummary = { connectors: result.connectors };
      else await refreshExtensions(true);
      input.value = '';
      pyannoteSetupIssue = null;
      await installPyannoteFromUi();
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Pyannote');
      button.disabled = false;
    }
  }

  function renderPyannoteAccessCard(list, control) {
    if (!shouldShowPyannoteAccessCard(control)) return;
    const row = document.createElement('div');
    row.className = 'settings-row editable-setting-row pyannote-access-card';

    const text = document.createElement('div');
    const label = document.createElement('div');
    label.className = 'value';
    label.textContent = 'Pyannote needs Hugging Face access';
    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = pyannoteSetupIssue && pyannoteSetupIssue.detail
      ? pyannoteSetupIssue.detail
      : 'Accept the gated Pyannote model terms, then paste a Hugging Face token and retry the install.';
    text.append(label, meta);

    const controls = document.createElement('div');
    controls.className = 'setting-control-row pyannote-access-actions';
    const open = document.createElement('button');
    open.type = 'button';
    open.className = 'link-button';
    open.textContent = 'Open model terms';
    open.onclick = async () => {
      open.disabled = true;
      try {
        const result = await window.alvum.openPyannoteTerms();
        if (result && result.ok === false) showMenuNotification(result.error || 'Could not open Hugging Face.', 'warning', 'Pyannote');
      } catch (err) {
        showMenuNotification(extensionErrorMessage(err), 'warning', 'Pyannote');
      } finally {
        open.disabled = false;
      }
    };
    const retry = document.createElement('button');
    retry.type = 'button';
    retry.textContent = pyannoteInstallLoading ? 'Installing...' : 'Retry install';
    retry.disabled = pyannoteInstallLoading;
    retry.onclick = () => installPyannoteFromUi();
    controls.append(open, retry);

    const tokenControls = document.createElement('div');
    tokenControls.className = 'setting-control-row';
    const token = document.createElement('input');
    token.className = 'setting-editor';
    token.type = 'password';
    token.placeholder = 'HF token';
    token.setAttribute('aria-label', 'Hugging Face token');
    const save = document.createElement('button');
    save.type = 'button';
    save.textContent = 'Save token and retry';
    save.disabled = true;
    token.oninput = () => {
      save.disabled = token.value.trim() === '' || pyannoteInstallLoading;
    };
    token.onkeydown = (e) => {
      if (e.key !== 'Enter' || save.disabled) return;
      e.preventDefault();
      save.click();
    };
    save.onclick = () => savePyannoteTokenAndRetry(control, token, save);
    tokenControls.append(token, save);

    row.append(text, controls, tokenControls);
    list.appendChild(row);
  }

  function renderProcessorReadinessRow(list, control) {
    const readiness = control && control.readiness;
    if (!readiness) return;
    const row = document.createElement('div');
    row.className = 'settings-row';
    const text = document.createElement('div');
    const label = document.createElement('div');
    label.className = 'value status-line';
    const dot = document.createElement('span');
    dot.className = `status-dot ${readiness.level === 'ok' ? 'live' : ''}`;
    const name = document.createElement('span');
    name.textContent = control.label || control.component || 'Processor';
    label.append(dot, name);
    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = readiness.detail || readiness.status || '';
    text.append(label, meta);
    row.appendChild(text);
    if (readiness.action && (readiness.action.kind === 'install_whisper' || readiness.action.kind === 'install_pyannote')) {
      const button = document.createElement('button');
      button.type = 'button';
      const isPyannote = readiness.action.kind === 'install_pyannote';
      const loading = isPyannote ? pyannoteInstallLoading : whisperInstallLoading;
      button.textContent = loading ? 'Installing...' : (readiness.action.label || 'Install');
      button.disabled = loading;
      button.onclick = () => {
        if (isPyannote) installPyannoteFromUi();
        else installWhisperModelFromUi();
      };
      row.appendChild(button);
    }
    list.appendChild(row);
  }

  function renderConnectorProcessorControls(ext) {
    const title = $('extension-detail-processor-title');
    const list = $('extension-detail-processor-controls');
    const controls = connectorProcessorControls(ext);
    title.textContent = 'Processor';
    list.replaceChildren();
    if (!ext) return;
    if (!controls.length) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const label = document.createElement('div');
      label.className = 'value';
      label.textContent = 'No processor controls';
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = 'This connector only captures context.';
      text.append(label, meta);
      row.appendChild(text);
      list.appendChild(row);
      return;
    }
    for (const control of controls) {
      renderPyannoteAccessCard(list, control);
      renderProcessorReadinessRow(list, control);
      const settings = processorSettingsForMode(control, Array.isArray(control.settings) ? control.settings : []);
      if (!settings.length) {
        const row = document.createElement('div');
        row.className = 'settings-row';
        const text = document.createElement('div');
        const label = document.createElement('div');
        label.className = 'value';
        label.textContent = control.label || control.component || control.id || 'Processor';
        const meta = document.createElement('div');
        meta.className = 'meta';
        meta.textContent = control.detail || control.component || control.kind || 'processor';
        text.append(label, meta);
        row.appendChild(text);
        list.appendChild(row);
        continue;
      }
      for (const setting of settings) {
        renderProcessorSettingRow(list, control, setting);
      }
    }
  }

  function renderSpeakerManagement(ext) {
    const section = $('extension-detail-speakers-section');
    const list = $('extension-detail-speakers');
    if (!section || !list) return;
    const audio = isAudioConnector(ext);
    section.hidden = !audio;
    list.replaceChildren();
    if (!audio) return;
    if (!speakerSummary && !speakerLoading) {
      setTimeout(() => refreshSpeakers(), 0);
    }
    if (speakerLoading && !speakerSummary) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const label = document.createElement('div');
      label.className = 'value';
      label.textContent = 'Loading speakers';
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = 'Reading the local speaker registry.';
      text.append(label, meta);
      row.appendChild(text);
      list.appendChild(row);
      return;
    }
    if (speakerSummary && speakerSummary.error) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const label = document.createElement('div');
      label.className = 'value';
      label.textContent = 'Speaker registry unavailable';
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = speakerSummary.error;
      text.append(label, meta);
      const retry = document.createElement('button');
      retry.type = 'button';
      retry.textContent = 'Retry';
      retry.onclick = () => refreshSpeakers(true);
      row.append(text, retry);
      list.appendChild(row);
      return;
    }
    const speakers = speakerSummary && Array.isArray(speakerSummary.speakers)
      ? speakerSummary.speakers
      : [];
    if (!speakers.length) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const label = document.createElement('div');
      label.className = 'value';
      label.textContent = 'No speakers yet';
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = 'Speaker IDs appear after audio processing emits voice turns.';
      text.append(label, meta);
      row.appendChild(text);
      list.appendChild(row);
      return;
    }
    const row = document.createElement('div');
    row.className = 'settings-row';
    const text = document.createElement('div');
    const label = document.createElement('div');
    label.className = 'value';
    label.textContent = 'Tracked voices';
    const meta = document.createElement('div');
    meta.className = 'meta';
    const linked = speakers.filter((speaker) => speaker.linked_interest_id).length;
    meta.textContent = `Tracked voice identities are managed under Synthesis customization. ${linked}/${speakers.length} linked.`;
    text.append(label, meta);
    const review = document.createElement('button');
    review.type = 'button';
    review.textContent = 'Review voices';
    review.onclick = () => {
      selectedExtension = null;
      setView('profile-voices-list');
    };
    row.append(text, review);
    list.appendChild(row);
  }

  function renderExtensionDetail() {
    const ext = extensionById(selectedExtension);
    $('view-title').textContent = ext ? (ext.display_name || ext.id) : 'Connector';
    renderConnectorCaptureControls(ext);
    renderConnectorProcessorControls(ext);
    renderSpeakerManagement(ext);
    requestPopoverResize();
  }

  function renderAddConnector() {
    const list = $('connector-add-core-list');
    list.replaceChildren();
    if (!extensionSummary) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 'value';
      title.textContent = 'Loading connectors';
      text.appendChild(title);
      row.appendChild(text);
      list.appendChild(row);
      return;
    }
    if (extensionSummary.error) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 'value';
      title.textContent = 'Core connectors unavailable';
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = extensionSummary.error;
      text.append(title, meta);
      row.appendChild(text);
      list.appendChild(row);
      return;
    }

    const connectors = Array.isArray(extensionSummary.connectors) ? extensionSummary.connectors : [];
    const core = connectors.filter((connector) => connector.kind === 'core');
    if (!core.length) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 'value';
      title.textContent = 'No core connectors';
      text.appendChild(title);
      row.appendChild(text);
      list.appendChild(row);
      return;
    }

    for (const connector of core) {
      const row = document.createElement('div');
      row.className = 'input-row';
      row.role = 'listitem';
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 'value status-line';
      const dot = document.createElement('span');
      dot.className = `status-dot ${connector.enabled ? 'live' : ''}`;
      const name = document.createElement('span');
      name.textContent = connector.display_name || connector.id;
      title.append(dot, name);
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = connector.enabled ? 'Added' : 'Available';
      text.append(title, meta);

      const add = document.createElement('button');
      add.type = 'button';
      add.textContent = connector.enabled ? 'Added' : 'Add';
      add.disabled = !!connector.enabled;
      add.onclick = async () => {
        add.disabled = true;
        add.textContent = 'Adding...';
        let result;
        try {
          result = await window.alvum.connectorSetEnabled(connector.id, true);
        } catch (e) {
          result = { ok: false, error: extensionErrorMessage(e) };
        }
        if (result && Array.isArray(result.connectors)) extensionSummary = { connectors: result.connectors };
        else await refreshExtensions(true);
        if (result && result.captureInputs) {
          captureInputs = result.captureInputs;
          renderCaptureInputs();
        }
        handlePermissionIssues(result);
        if (!(result && result.ok)) console.error('[connector] add failed', (result && result.error) || 'connector add failed');
        renderExtensions();
        renderAddConnector();
      };
      row.append(text, add);
      list.appendChild(row);
    }
  }

  function renderExtensions() {
    const connectors = extensionSummary && Array.isArray(extensionSummary.connectors)
      ? extensionSummary.connectors
      : [];
    const list = $('extensions-list');
    list.replaceChildren();
    if (!extensionSummary) return;
    if (extensionSummary.error) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 'value';
      title.textContent = 'Connectors unavailable';
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = extensionSummary.error;
      text.append(title, meta);
      row.appendChild(text);
      list.appendChild(row);
      return;
    }
    if (!connectors.length) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 'value';
      title.textContent = 'No connectors installed';
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = 'Use the CLI to install or scaffold a connector package.';
      text.append(title, meta);
      row.appendChild(text);
      list.appendChild(row);
      requestPopoverResize();
      return;
    }
    for (const ext of connectors) {
      const row = document.createElement('div');
      row.className = 'extension-row';
      row.role = 'button';
      row.tabIndex = 0;
      const dot = document.createElement('span');
      dot.className = `dot ${extensionDotLevel(ext)}`;
      const text = document.createElement('div');
      const name = document.createElement('div');
      name.className = 'value';
      name.textContent = ext.display_name || ext.id;
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = connectorListStatusLabel(ext);
      text.append(name, meta);
      const openDetails = () => {
        selectedExtension = ext.id;
        setView('extension-detail');
      };
      row.onclick = openDetails;
      row.onkeydown = (e) => {
        if (e.key !== 'Enter' && e.key !== ' ') return;
        e.preventDefault();
        openDetails();
      };
      const hint = document.createElement('span');
      hint.className = 'action-hint';
      hint.setAttribute('aria-hidden', 'true');
      hint.textContent = '›';
      row.append(dot, text, hint);
      list.appendChild(row);
    }
    if (activeView === 'extension-detail') renderExtensionDetail();
    requestPopoverResize();
  }

  async function refreshExtensions(force) {
    try {
      if (!extensionSummary || force) extensionSummary = await window.alvum.connectorList();
    } catch (e) {
      extensionSummary = { connectors: [], error: extensionErrorMessage(e) };
    }
    renderMainBadges();
    renderExtensions();
    if (activeView === 'connector-add') renderAddConnector();
    return extensionSummary;
  }

  async function runGlobalDoctor() {
    let result;
    try {
      result = await window.alvum.doctor();
    } catch (e) {
      result = {
        ok: false,
        error_count: 1,
        warning_count: 0,
        checks: [],
        error: extensionErrorMessage(e),
      };
    }
    showMenuNotification(doctorSummaryText(result), doctorNotificationLevel(result));
    return result;
  }

  function providerStatusLabel(p) {
    if (!p || !p.ui) return 'unknown';
    if (p.usage != null) return `${p.ui.status} · ${p.usage}% usage`;
    return p.ui.reason || p.ui.status || 'unknown';
  }

  function providerDisplayName(provider) {
    return (provider && (provider.display_name || provider.name)) || 'Provider';
  }

  function configuredProviders() {
    return providerProbe && Array.isArray(providerProbe.providers)
      ? providerProbe.providers.filter((provider) => provider.enabled !== false)
      : [];
  }

  function providerCatalogEntries() {
    return providerProbe && Array.isArray(providerProbe.providers)
      ? providerProbe.providers.filter((provider) => provider.enabled === false)
      : [];
  }

  function providerCatalogActionLabel(_provider) {
    return 'Add';
  }

  function providerPrimaryAction(provider) {
    if (!provider) return { kind: 'none', label: 'Provider action', disabled: true, danger: false };
    if (provider.enabled === false) {
      return { kind: 'add', label: 'Add provider', disabled: false, danger: false };
    }
    if (provider.active) {
      if (providerProbe && providerProbe.configured === 'auto') {
        return { kind: 'none', label: 'Auto selected', disabled: true, danger: false };
      }
      return { kind: 'auto', label: 'Use auto', disabled: false, danger: false };
    }
    if (!providerIsWorking(provider)) {
      return { kind: 'use', label: 'Use', disabled: true, danger: false };
    }
    return { kind: 'use', label: 'Use', disabled: false, danger: false };
  }

  function providerSetupAction(provider) {
    if (!provider) return { label: 'Setup', disabled: true, hidden: false, tone: 'danger' };
    if (providerIsWorking(provider)) {
      return { label: 'Setup', disabled: true, hidden: true, tone: 'none' };
    }
    const actions = providerSetupActions(provider);
    if (provider.setup_kind === 'instructions' && !provider.setup_command && !provider.setup_url && !actions.length) {
      return { label: provider.setup_label || 'Setup', disabled: true, hidden: true, tone: 'none' };
    }
    const level = provider.ui && provider.ui.level ? provider.ui.level : (provider.available ? 'yellow' : 'red');
    const needsRepair = level === 'yellow' || (provider.available && provider.test && provider.test.ok === false);
    return {
      label: needsRepair ? 'Fix setup' : 'Setup',
      disabled: provider.setup_kind !== 'inline'
        && !provider.setup_command
        && !provider.setup_url
        && !actions.length
        && provider.setup_kind !== 'instructions',
      hidden: false,
      tone: needsRepair ? 'warning' : 'danger',
    };
  }

  function providerAddMeta(provider) {
    const state = provider.enabled === false
      ? 'Removed'
      : (providerIsWorking(provider) ? 'Ready' : 'Needs setup');
    return `${state} · ${provider.name} · ${providerStatusLabel(provider)}`;
  }

  function providerByName(name) {
    return providerProbe && Array.isArray(providerProbe.providers)
      ? providerProbe.providers.find((provider) => provider.name === name)
      : null;
  }

  function providerIsWorking(provider) {
    if (!provider || provider.enabled === false || !provider.available) return false;
    return !!(provider.test && provider.test.ok);
  }

  function providerCanRemove(provider) {
    return !!provider && provider.enabled !== false;
  }

  function providerConfigFields(provider) {
    return provider && Array.isArray(provider.config_fields) ? provider.config_fields : [];
  }

  function providerSetupActions(provider) {
    return provider && Array.isArray(provider.setup_actions) ? provider.setup_actions : [];
  }

  function providerConfigFieldGroup(field) {
    if (field && field.group) return String(field.group);
    if (field && (field.key === 'model' || field.key === 'text_model' || field.key === 'image_model' || field.key === 'audio_model')) {
      return 'models';
    }
    return 'connection';
  }

  function providerTextModelField(provider) {
    return providerConfigFields(provider).find((field) => field.key === 'text_model')
      || providerConfigFields(provider).find((field) => field.key === 'model')
      || { key: 'text_model' };
  }

  function fieldOptions(field) {
    return field && Array.isArray(field.options) ? field.options : [];
  }

  function modelLoadState(name) {
    return providerModelLoadState.get(name) || null;
  }

  function invalidateProviderModelLoad(name) {
    if (name) providerModelLoadState.delete(name);
  }

  function modelInstallState(name, model) {
    return providerModelInstallState.get(`${name}:${model}`) || null;
  }

  function telemetryNumber(value, fallback = 0) {
    const n = Number(value);
    return Number.isFinite(n) ? n : fallback;
  }

  function mergeProviderTelemetrySnapshot(snapshot) {
    const providers = snapshot && snapshot.providers && typeof snapshot.providers === 'object'
      ? snapshot.providers
      : {};
    for (const [name, stats] of Object.entries(providers)) {
      const current = providerTelemetry[name];
      const currentCount = telemetryNumber(current && current.calls_started) + telemetryNumber(current && current.calls_finished);
      const nextCount = telemetryNumber(stats && stats.calls_started) + telemetryNumber(stats && stats.calls_finished);
      if (!current || nextCount >= currentCount) {
        providerTelemetry[name] = { ...stats };
      }
    }
  }

  function providerTelemetryRecord(name) {
    const provider = String(name || '').trim();
    if (!provider) return null;
    providerTelemetry[provider] = providerTelemetry[provider] || {
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
      last_latency_ms: null,
      last_tokens_per_sec: null,
      last_token_source: null,
      last_stop_reason: null,
      last_content_block_kinds: null,
      updated_at: null,
    };
    return providerTelemetry[provider];
  }

  function recordProviderTelemetry(evt) {
    if (!evt || (evt.kind !== 'llm_call_start' && evt.kind !== 'llm_call_end')) return;
    const stats = providerTelemetryRecord(evt.provider);
    if (!stats) return;
    stats.updated_at = new Date().toISOString();
    stats.last_call_site = evt.call_site || stats.last_call_site;
    if (evt.kind === 'llm_call_start') {
      stats.calls_started += 1;
      stats.active_calls += 1;
      stats.last_status = 'running';
      return;
    }
    stats.calls_finished += 1;
    stats.active_calls = Math.max(0, telemetryNumber(stats.active_calls) - 1);
    if (evt.ok === false) stats.calls_failed += 1;
    stats.prompt_chars += telemetryNumber(evt.prompt_chars);
    stats.response_chars += telemetryNumber(evt.response_chars);
    stats.input_tokens += telemetryNumber(evt.input_tokens);
    stats.output_tokens += telemetryNumber(evt.output_tokens);
    stats.total_tokens += telemetryNumber(evt.total_tokens);
    stats.input_tokens_estimate += telemetryNumber(evt.prompt_tokens_estimate);
    stats.output_tokens_estimate += telemetryNumber(evt.response_tokens_estimate);
    stats.total_tokens_estimate += telemetryNumber(evt.total_tokens_estimate);
    stats.latency_ms += telemetryNumber(evt.latency_ms);
    stats.last_status = evt.ok === false ? 'failed' : 'ok';
    stats.last_latency_ms = telemetryNumber(evt.latency_ms, null);
    stats.last_tokens_per_sec = telemetryNumber(evt.tokens_per_sec, telemetryNumber(evt.tokens_per_sec_estimate, null));
    stats.last_token_source = evt.token_source || (evt.tokens_per_sec ? 'provider' : 'estimated');
    stats.last_stop_reason = evt.stop_reason || null;
    stats.last_content_block_kinds = Array.isArray(evt.content_block_kinds) ? evt.content_block_kinds : null;
  }

  function providerTelemetryFor(provider) {
    if (!provider || !provider.name) return null;
    return providerTelemetry[provider.name] || null;
  }

  function compactMetric(value, decimals = 0) {
    const n = telemetryNumber(value, null);
    if (n == null) return '0';
    const abs = Math.abs(n);
    if (abs >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (abs >= 1_000) return `${(n / 1_000).toFixed(decimals ? decimals : 1)}k`;
    return decimals ? n.toFixed(decimals) : String(Math.round(n));
  }

  function tokenMetric(actual, estimated) {
    const exact = telemetryNumber(actual);
    if (exact > 0) return compactMetric(exact);
    const estimate = telemetryNumber(estimated);
    return estimate > 0 ? `~${compactMetric(estimate)}` : '0';
  }

  function formatLatency(ms) {
    const n = telemetryNumber(ms, null);
    if (n == null || n <= 0) return 'n/a';
    if (n < 1000) return `${Math.round(n)}ms`;
    return `${(n / 1000).toFixed(1)}s`;
  }

  function formatRate(value) {
    const n = telemetryNumber(value, null);
    if (n == null || n <= 0) return 'n/a';
    return `${compactMetric(n, 1)} tok/s`;
  }

  function mergeProviderModelOptions(name, result) {
    if (!providerProbe || !Array.isArray(providerProbe.providers) || !result) return;
    const options = Array.isArray(result.options) ? result.options : [];
    const optionsByModality = result.options_by_modality && typeof result.options_by_modality === 'object'
      ? result.options_by_modality
      : {};
    const installable = Array.isArray(result.installable_options) ? result.installable_options : [];
    const installableError = result.installable_error || null;
    if (!options.length && !installable.length && !installableError) return;
    providerProbe.providers = providerProbe.providers.map((provider) => {
      if (provider.name !== name || !Array.isArray(provider.config_fields)) return provider;
      return {
        ...provider,
        installable_models: installable,
        installable_model_error: installableError,
        model_catalog_ok: !!result.ok,
        model_source: result.source || null,
        options_by_modality: optionsByModality,
        config_fields: provider.config_fields.map((field) => {
          if (field.key !== 'model' && field.key !== 'text_model' && field.key !== 'image_model' && field.key !== 'audio_model') return field;
          const fieldOptionsForModality = field.key === 'image_model'
            ? optionsByModality.image
            : (field.key === 'audio_model' ? optionsByModality.audio : optionsByModality.text);
          const nextOptions = Array.isArray(fieldOptionsForModality) ? fieldOptionsForModality : options;
          if (!nextOptions.length) return { ...field, options: [] };
          return { ...field, options: nextOptions };
        }),
      };
    });
  }

  function mergeProviderSummaryModelState(summary) {
    if (!summary || !providerProbe || !Array.isArray(summary.providers) || !Array.isArray(providerProbe.providers)) {
      return summary;
    }
    const previousByName = new Map(providerProbe.providers.map((provider) => [provider.name, provider]));
    return {
      ...summary,
      providers: summary.providers.map((provider) => {
        const previous = previousByName.get(provider.name);
        if (!previous) return provider;
        const previousModelField = providerTextModelField(previous);
        const previousOptions = fieldOptions(previousModelField);
        if (!previousOptions.length && !Array.isArray(previous.installable_models) && !previous.installable_model_error) return provider;
        return {
          ...provider,
          installable_models: previous.installable_models,
          installable_model_error: previous.installable_model_error,
          model_catalog_ok: previous.model_catalog_ok,
          model_source: previous.model_source,
          options_by_modality: previous.options_by_modality,
          config_fields: providerConfigFields(provider).map((field) => {
            if ((field.key !== 'model' && field.key !== 'text_model' && field.key !== 'image_model' && field.key !== 'audio_model') || !previousOptions.length) return field;
            const previousField = providerConfigFields(previous).find((item) => item.key === field.key);
            return { ...field, options: previousField ? fieldOptions(previousField) : previousOptions };
          }),
        };
      }),
    };
  }

  function providerInstalledModelValues(provider) {
    if (!provider || provider.name !== 'ollama' || !String(provider.model_source || '').startsWith('ollama') || !provider.model_catalog_ok) {
      return new Set();
    }
    return new Set(providerConfigFields(provider)
      .filter((field) => field.key === 'model' || field.key === 'text_model' || field.key === 'image_model' || field.key === 'audio_model')
      .flatMap((field) => fieldOptions(field).map((option) => String(option.value))));
  }

  function optionListHasValue(options, value) {
    return Array.isArray(options) && options.some((option) => String(option.value) === String(value));
  }

  function providerModelInputSupport(provider, value) {
    const support = { text: false, image: false, audio: false };
    if (!provider || !provider.options_by_modality) return support;
    const byModality = provider.options_by_modality;
    support.text = optionListHasValue(byModality.text, value);
    support.image = optionListHasValue(byModality.image, value);
    support.audio = optionListHasValue(byModality.audio, value);
    return support;
  }

  function modelInputSupport(model) {
    if (!model) return null;
    return model.input_support || model.modalities || null;
  }

  function inputSupportLabels(support) {
    if (!support) return [];
    const labels = [];
    if (support.text) labels.push('Text');
    if (support.image) labels.push('Image');
    if (support.audio) labels.push('Audio');
    return labels;
  }

  function inputSupportCovers(installedSupport, requestedSupport) {
    const requested = requestedSupport || {};
    if (!inputSupportLabels(requested).length) return false;
    const installed = installedSupport || {};
    return (!requested.text || !!installed.text)
      && (!requested.image || !!installed.image)
      && (!requested.audio || !!installed.audio);
  }

  function modelMetaWithInputSupport(model, detail) {
    const labels = inputSupportLabels(modelInputSupport(model));
    const prefix = labels.length ? `${labels.join(', ')} input` : '';
    const body = String(detail || '').trim();
    const maxOutput = Number(model && model.max_output_tokens);
    const output = Number.isFinite(maxOutput) && maxOutput > 0 && !/max output/i.test(body)
      ? `Max output ${maxOutput.toLocaleString()} tokens`
      : '';
    return [prefix, output, body].filter(Boolean).join(' · ');
  }

  function providerInstallableModels(provider) {
    if (!provider || provider.name !== 'ollama' || !Array.isArray(provider.installable_models)) return [];
    const installed = providerInstalledModelValues(provider);
    return provider.installable_models.filter((model) => {
      const value = String(model.value);
      if (installed.has(value)) return false;
      if (value.includes(':')) return true;
      return !Array.from(installed).some((installedValue) => {
        if (String(installedValue).split(':')[0] !== value) return false;
        return inputSupportCovers(providerModelInputSupport(provider, installedValue), modelInputSupport(model));
      });
    });
  }

  function providerInstalledModelOptions(provider) {
    if (!provider || provider.name !== 'ollama' || !String(provider.model_source || '').startsWith('ollama') || !provider.model_catalog_ok) return [];
    const modelField = providerTextModelField(provider);
    return fieldOptions(modelField).filter((option) => String(option.value || '').trim() !== '');
  }

  async function loadProviderModels(provider) {
    if (!provider || !provider.name || !window.alvum.providerModels) return;
    const existing = modelLoadState(provider.name);
    if (existing && (existing.loading || existing.loaded)) return;
    providerModelLoadState.set(provider.name, { loading: true, loaded: false, error: null });
    renderProviderDetail();
    try {
      const result = await window.alvum.providerModels(provider.name);
      mergeProviderModelOptions(provider.name, result);
      providerModelLoadState.set(provider.name, {
        loading: false,
        loaded: true,
        error: result && result.error ? result.error : null,
      });
    } catch (err) {
      providerModelLoadState.set(provider.name, {
        loading: false,
        loaded: true,
        error: extensionErrorMessage(err),
      });
    }
    if (activeView === 'provider-detail' && selectedProvider === provider.name) renderProviderDetail();
  }

  function providerFieldValueLabel(field) {
    if (!field) return 'Not configured';
    if (field.secret) return field.configured ? 'Configured' : 'Not configured';
    const options = fieldOptions(field);
    const currentValue = field.value == null || field.value === ''
      ? String(field.placeholder || '')
      : String(field.value);
    const option = options.find((item) => String(item.value) === currentValue);
    if (option) return option.label || String(option.value || 'Default');
    if (field.value == null || field.value === '') return field.placeholder || 'Not configured';
    return String(field.value);
  }

  async function saveProviderConfigField(provider, field, rawValue, control) {
    if (!provider || !field || !field.key) return;
    if (control) control.disabled = true;
    const payload = field.secret
      ? { secrets: { [field.key]: rawValue } }
      : { settings: { [field.key]: rawValue } };
    if (provider.enabled === false) payload.enabled = true;
    try {
      const result = await window.alvum.providerConfigure(provider.name, payload);
      updateProviderFromActionResult(result);
      if (result && result.ok === false) console.error('[provider] config update failed', result.error || 'config update failed');
    } catch (err) {
      console.error('[provider] config update failed', extensionErrorMessage(err));
      window.alvum.requestState();
    } finally {
      renderProviderDetail();
    }
  }

  async function installProviderModel(provider, model, control) {
    if (!provider || !model || !window.alvum.providerInstallModel) return;
    const key = `${provider.name}:${model}`;
    providerModelInstallState.set(key, { loading: true, error: null });
    if (control) control.disabled = true;
    renderProviderDetail();
    try {
      const result = await window.alvum.providerInstallModel(provider.name, model);
      if (result && result.summary) mergeProviderSummary(result.summary);
      if (result && result.models) mergeProviderModelOptions(provider.name, result.models);
      providerModelInstallState.set(key, {
        loading: false,
        error: result && result.ok === false ? (result.error || 'Download failed') : null,
      });
      if (result && result.ok === false) {
        showMenuNotification(result.error || 'Model download failed', 'warning', 'Ollama download');
      } else {
        showMenuNotification(`${model} downloaded`, 'info', 'Ollama download');
      }
    } catch (err) {
      providerModelInstallState.set(key, { loading: false, error: extensionErrorMessage(err) });
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Ollama download');
    } finally {
      renderSetupChecklist();
      renderProviderDetail();
    }
  }

  function providerFieldKeySelector(key) {
    return String(key || '').replace(/["\\]/g, '\\$&');
  }

  function focusProviderConfigField(key) {
    const selector = key
      ? `#provider-detail-settings [data-field-key="${providerFieldKeySelector(key)}"]`
      : '#provider-detail-settings input, #provider-detail-settings select';
    const field = document.querySelector(selector);
    if (!field) return false;
    field.focus();
    if (field.select) field.select();
    return true;
  }

  function autoResolvedProviderName(providers) {
    const match = (providers || []).find((provider) => provider.enabled !== false && providerIsWorking(provider));
    return match ? match.name : null;
  }

  function setProviderEnabledLocal(name, enabled) {
    if (!providerProbe || !Array.isArray(providerProbe.providers)) return;
    providerProbe.providers = providerProbe.providers.map((provider) => {
      if (provider.name !== name) return provider;
      return {
        ...provider,
        enabled: !!enabled,
        active: enabled ? provider.active : false,
      };
    });
    if (!enabled && providerProbe.configured === name) {
      providerProbe.configured = 'auto';
    }
    if (providerProbe.configured === 'auto') {
      const autoName = autoResolvedProviderName(providerProbe.providers);
      providerProbe.auto_resolved = autoName;
      providerProbe.providers = providerProbe.providers.map((provider) => ({
        ...provider,
        active: provider.name === autoName,
      }));
    }
  }

  function setProviderActiveLocal(name) {
    if (!providerProbe || !Array.isArray(providerProbe.providers)) return;
    providerProbe.configured = name;
    providerProbe.providers = providerProbe.providers.map((provider) => ({
      ...provider,
      enabled: provider.name === name ? true : provider.enabled,
      active: provider.name === name,
    }));
  }

  function setProviderAutoLocal() {
    if (!providerProbe || !Array.isArray(providerProbe.providers)) return;
    providerProbe.configured = 'auto';
    const autoName = autoResolvedProviderName(providerProbe.providers);
    providerProbe.auto_resolved = autoName;
    providerProbe.providers = providerProbe.providers.map((provider) => ({
      ...provider,
      active: provider.name === autoName,
    }));
  }

  function createProviderSection(title, meta, className = '') {
    const section = document.createElement('div');
    section.className = `provider-section${className ? ` ${className}` : ''}`;
    const header = document.createElement('div');
    header.className = 'provider-section-header';
    const text = document.createElement('div');
    const name = document.createElement('div');
    name.className = 'value';
    name.textContent = title;
    const detail = document.createElement('div');
    detail.className = 'meta';
    text.append(name, detail);
    if (meta) {
      detail.textContent = meta;
    } else {
      detail.hidden = true;
    }
    header.appendChild(text);
    const body = document.createElement('div');
    body.className = 'provider-section-body';
    section.append(header, body);
    return { section, body };
  }

  function appendProviderDetailRow(list, title, meta, actionLabel, onAction) {
    const row = document.createElement('div');
    row.className = 'provider-detail-row';
    const text = document.createElement('div');
    const name = document.createElement('div');
    name.className = 'value';
    name.textContent = title;
    const detail = document.createElement('div');
    detail.className = 'meta';
    detail.textContent = meta;
    text.append(name, detail);
    row.appendChild(text);
    if (actionLabel && onAction) {
      const action = document.createElement('button');
      action.type = 'button';
      action.textContent = actionLabel;
      action.onclick = onAction;
      row.appendChild(action);
    }
    list.appendChild(row);
    return row;
  }

  function appendProviderSubhead(list, title) {
    const subhead = document.createElement('div');
    subhead.className = 'provider-section-subhead';
    subhead.textContent = title;
    list.appendChild(subhead);
  }

  function renderProviderConfigField(settings, provider, field) {
    const row = document.createElement('div');
    row.className = 'provider-detail-row provider-config-row';
    const text = document.createElement('div');
    const label = document.createElement('div');
    label.className = 'value';
    label.textContent = field.label || field.key;
    const meta = document.createElement('div');
    meta.className = 'meta';
    const isModelField = field.key === 'model' || field.key === 'text_model' || field.key === 'image_model' || field.key === 'audio_model';
    const isOllamaModelField = provider && provider.name === 'ollama' && isModelField;
    const loadState = isModelField
      ? modelLoadState(provider.name)
      : null;
    const loadSuffix = loadState && loadState.loading
      ? ' · Loading models...'
      : (loadState && loadState.error ? ` · ${loadState.error}` : '');
    meta.textContent = `Current: ${providerFieldValueLabel(field)}`;
    meta.textContent += loadSuffix;
    text.append(label, meta);

    const controls = document.createElement('div');
    controls.className = 'setting-control-row';
    const options = fieldOptions(field);
    const useSelect = !field.secret && isModelField && (options.length || isOllamaModelField);
    const editor = useSelect ? document.createElement('select') : document.createElement('input');
    editor.className = 'setting-editor provider-config-editor';
    editor.dataset.fieldKey = field.key;
    editor.setAttribute('aria-label', field.label || field.key);
    if (useSelect) {
      const currentValue = field.value == null || field.value === ''
        ? (field.placeholder || '')
        : String(field.value);
      const selectOptions = options.slice();
      if (currentValue && !selectOptions.some((option) => String(option.value) === currentValue)) {
        const missingLabel = provider && provider.name === 'ollama'
          ? `${currentValue} (not installed)`
          : currentValue;
        selectOptions.unshift({ value: currentValue, label: missingLabel, disabled: provider && provider.name === 'ollama' });
      }
      if (!selectOptions.length) {
        const emptyLabel = field.key === 'image_model'
          ? 'No image models'
          : (field.key === 'audio_model' ? 'No audio models' : 'No installed models');
        selectOptions.push({ value: '', label: emptyLabel, disabled: true });
      } else if (isOllamaModelField && currentValue && !selectOptions.some((option) => String(option.value) === '')) {
        selectOptions.push({ value: '', label: 'No model selected' });
      }
      const hasEnabledOptions = selectOptions.some((option) => !option.disabled);
      for (const option of selectOptions) {
        const item = document.createElement('option');
        item.value = String(option.value);
        item.textContent = option.label || String(option.value || 'Default');
        if (option.disabled) item.disabled = true;
        editor.appendChild(item);
      }
      editor.value = currentValue;
      if (!hasEnabledOptions) editor.disabled = true;
    } else {
      editor.type = field.secret ? 'password' : (field.kind === 'url' ? 'url' : 'text');
      editor.placeholder = field.secret && field.configured
        ? 'Configured'
        : (field.placeholder || '');
      editor.value = field.secret ? '' : (field.value == null ? '' : String(field.value));
    }

    const save = document.createElement('button');
    save.type = 'button';
    save.textContent = 'Save';
    save.disabled = true;
    const original = editor.value;
    editor.oninput = () => {
      save.disabled = field.secret
        ? editor.value.trim() === ''
        : editor.value === original;
    };
    editor.onchange = editor.oninput;
    editor.onkeydown = (e) => {
      if (e.key !== 'Enter' || save.disabled) return;
      e.preventDefault();
      save.click();
    };
    save.onclick = () => saveProviderConfigField(provider, field, editor.value, save);
    controls.append(editor, save);
    row.append(text, controls);
    settings.appendChild(row);
  }

  function renderProviderConfigGroups(settings, provider, fields) {
    const { section, body } = createProviderSection(
      'Configuration',
      '',
    );
    settings.appendChild(section);
    if (!fields.length) {
      appendProviderDetailRow(body, 'No configurable values', 'This provider is controlled by its own local setup.');
      return;
    }
    const groups = [
      ['connection', 'Connection'],
      ['models', 'Models'],
      ['advanced', 'Advanced'],
    ];
    const rendered = new Set();
    for (const [group, title] of groups) {
      const groupFields = fields.filter((field) => providerConfigFieldGroup(field) === group);
      if (!groupFields.length) continue;
      appendProviderSubhead(body, title);
      for (const field of groupFields) {
        renderProviderConfigField(body, provider, field);
        rendered.add(field);
      }
    }
    const remaining = fields.filter((field) => !rendered.has(field));
    if (remaining.length) {
      appendProviderSubhead(body, 'Other');
      for (const field of remaining) {
        renderProviderConfigField(body, provider, field);
      }
    }
  }

  function providerSettingRows(provider) {
    if (!provider) return [];
    const rows = [
      ['Provider ID', provider.name],
      ['Status', provider.ui ? provider.ui.status : 'unknown'],
      ['Health', providerIsWorking(provider) ? 'Working' : providerStatusLabel(provider)],
      ['Installed', provider.available ? 'Yes' : 'No'],
    ];
    if (provider.resolved_model) rows.push(['Resolved invoke target', provider.resolved_model]);
    if (provider.resolved_model_kind) rows.push(['Resolved target type', provider.resolved_model_kind]);
    if (provider.resolved_model_source) rows.push(['Resolved source', provider.resolved_model_source]);
    const maxOutput = Number(provider.resolved_model_max_output_tokens);
    if (Number.isFinite(maxOutput) && maxOutput > 0) rows.push(['Max output tokens', maxOutput.toLocaleString()]);
    if (provider.test) {
      rows.push(['Last check', provider.test.ok ? 'OK' : 'Failed']);
      if (provider.test.status) rows.push(['Probe status', provider.test.status]);
      if (provider.test.elapsed_ms != null) rows.push(['Probe latency', `${provider.test.elapsed_ms}ms`]);
      if (provider.test.timeout_secs != null) rows.push(['Probe timeout', `${provider.test.timeout_secs}s`]);
      if (provider.test.resolved_model) rows.push(['Resolved model', provider.test.resolved_model]);
      if (provider.test.model_source) rows.push(['Model source', provider.test.model_source]);
      if (provider.test.backend_hint) rows.push(['Backend', provider.test.backend_hint]);
      if (Array.isArray(provider.test.recommended_setup_actions) && provider.test.recommended_setup_actions.length) {
        rows.push(['Recommended actions', provider.test.recommended_setup_actions.join(', ')]);
      }
      if (provider.test.response_preview) rows.push(['Response', provider.test.response_preview]);
      if (provider.test.error) rows.push(['Error', provider.test.error]);
    }
    if (provider.usage != null) rows.push(['Usage', `${provider.usage}%`]);
    return rows;
  }

  function providerCapabilities(provider) {
    return provider && provider.capabilities && typeof provider.capabilities === 'object'
      ? provider.capabilities
      : null;
  }

  function capabilityStatusLabel(capability) {
    if (!capability) return 'Unknown';
    if (capability.status === 'ready') return 'Ready';
    if (capability.status === 'transport_limited') return 'Transport limited';
    return 'Unsupported';
  }

  function renderProviderCapabilities(settings, provider) {
    const capabilities = providerCapabilities(provider);
    if (!provider || !capabilities) return;
    const { section, body } = createProviderSection('Data types', '');
    settings.appendChild(section);
    const models = provider.selected_models || {};
    for (const [key, label] of [['text', 'Text'], ['image', 'Image'], ['audio', 'Audio']]) {
      const capability = capabilities[key] || {};
      const model = models[key] || 'No model selected';
      const provenance = capability.provenance ? ` · ${capability.provenance}` : '';
      appendProviderDetailRow(
        body,
        `${label}: ${capabilityStatusLabel(capability)}`,
        `${model}${provenance}`,
      );
    }
  }

  function appendProviderStateRow(list, title, meta, actionLabel, onAction) {
    const row = document.createElement('div');
    row.className = 'settings-row';
    const text = document.createElement('div');
    const name = document.createElement('div');
    name.className = 'value';
    name.textContent = title;
    const detail = document.createElement('div');
    detail.className = 'meta';
    detail.textContent = meta;
    text.append(name, detail);
    row.appendChild(text);
    if (actionLabel && onAction) {
      const action = document.createElement('button');
      action.type = 'button';
      action.textContent = actionLabel;
      action.onclick = onAction;
      row.appendChild(action);
    }
    list.appendChild(row);
  }

  function renderProviderObservability(settings, provider) {
    if (!provider) return;
    const stats = providerTelemetryFor(provider);
    const { section, body } = createProviderSection(
      'Observability',
      '',
    );
    settings.appendChild(section);

    if (!stats || telemetryNumber(stats.calls_started) === 0) {
      appendProviderDetailRow(body, 'No synthesis calls yet', 'Stats appear during synthesis.');
      return;
    }

    const finished = telemetryNumber(stats.calls_finished);
    const failed = telemetryNumber(stats.calls_failed);
    const ok = Math.max(0, finished - failed);
    const active = telemetryNumber(stats.active_calls);
    const grid = document.createElement('div');
    grid.className = 'provider-compact-grid';
    appendProviderDetailRow(
      grid,
      'Active',
      active > 0
        ? `${active} call${active === 1 ? '' : 's'} running · ${stats.last_call_site || 'provider call'}`
        : 'Idle',
    );
    appendProviderDetailRow(
      grid,
      'Calls',
      `${ok} ok · ${failed} failed`,
    );
    appendProviderDetailRow(
      grid,
      'Tokens',
      `${tokenMetric(stats.total_tokens, stats.total_tokens_estimate)} total`,
    );
    const avgLatency = finished > 0 ? telemetryNumber(stats.latency_ms) / finished : 0;
    const avgOutputTokens = telemetryNumber(stats.output_tokens) || telemetryNumber(stats.output_tokens_estimate);
    const avgThroughput = telemetryNumber(stats.latency_ms) > 0
      ? avgOutputTokens / (telemetryNumber(stats.latency_ms) / 1000)
      : 0;
    appendProviderDetailRow(
      grid,
      'Throughput',
      `${formatRate(stats.last_tokens_per_sec || avgThroughput)} · ${stats.last_token_source === 'provider' || stats.last_token_source === 'ollama' || stats.last_token_source === 'anthropic-api' || stats.last_token_source === 'bedrock' ? 'provider reported' : 'estimated'}`,
    );
    appendProviderDetailRow(
      grid,
      'Latency',
      `${formatLatency(avgLatency)} avg · ${formatLatency(stats.last_latency_ms)} last`,
    );
    if (stats.last_stop_reason) {
      appendProviderDetailRow(grid, 'Stop reason', String(stats.last_stop_reason));
    }
    if (Array.isArray(stats.last_content_block_kinds) && stats.last_content_block_kinds.length) {
      appendProviderDetailRow(grid, 'Content blocks', stats.last_content_block_kinds.join(' + '));
    }
    appendProviderDetailRow(
      grid,
      'Last call',
      `${stats.last_call_site || 'provider call'} · ${stats.last_status || 'unknown'}`,
    );
    body.appendChild(grid);
  }

  async function runProviderSetupAction(provider, action, control) {
    if (!provider || !action) return;
    if (control) control.disabled = true;
    try {
      const result = await window.alvum.providerSetup(provider.name, action);
      updateProviderFromActionResult(result);
      if (result && result.action === 'inline') {
        setTimeout(() => focusProviderConfigField(result.focus_key), 0);
      }
      if (result && result.refresh_models) {
        invalidateProviderModelLoad(provider.name);
        await loadProviderModels(provider);
      }
      if (result && result.error) {
        showMenuNotification(result.error, 'warning', 'Provider setup');
      }
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Provider setup');
      window.alvum.requestState();
    } finally {
      renderProviderDetail();
    }
  }

  function renderProviderSetupGuide(settings, provider) {
    if (!provider) return;
    const actions = providerSetupActions(provider);
    if (!actions.length && providerIsWorking(provider)) return;
    const { section, body } = createProviderSection(
      'Setup actions',
      provider.setup_hint || provider.auth_hint || 'Configure this provider, then Ping it.',
    );
    settings.appendChild(section);

    if (!actions.length) {
      appendProviderDetailRow(body, 'Setup', provider.setup_hint || provider.auth_hint || 'Configure this provider, then Ping it.');
      return;
    }
    for (const action of actions) {
      if (!action || !action.id) continue;
      const kind = String(action.kind || '');
      const id = String(action.id || '');
      const label = kind === 'terminal' || id.includes('refresh') || id.includes('check') || id === 'aws_sts'
        ? 'Run'
        : (kind === 'inline' ? 'Edit' : 'Open');
      appendProviderDetailRow(
        body,
        action.label || action.id,
        action.detail || kind || 'Provider setup action.',
        label,
        (event) => runProviderSetupAction(provider, action.id, event.currentTarget),
      );
    }
  }

  function renderProviderInstalledModels(settings, provider) {
    if (!provider || provider.name !== 'ollama' || !provider.available) return false;
    const loadState = modelLoadState(provider.name);
    appendProviderSubhead(settings, 'Installed');

    if (!loadState || loadState.loading) {
      appendProviderDetailRow(settings, 'Loading installed models', 'Checking the local Ollama model list.');
      return true;
    }
    if (loadState.error && !provider.model_catalog_ok) {
      appendProviderDetailRow(settings, 'Could not load installed models', loadState.error);
      return true;
    }

    const models = providerInstalledModelOptions(provider);
    if (!models.length) {
      appendProviderDetailRow(settings, 'No installed models found', 'Download a model below, then select it here.');
      return true;
    }

    const modelField = providerTextModelField(provider);
    const current = String(modelField.value || '');
    for (const model of models) {
      const value = String(model.value || '');
      const meta = modelMetaWithInputSupport(
        { input_support: providerModelInputSupport(provider, value) },
        value === current ? 'Current model' : 'Installed locally',
      );
      appendProviderDetailRow(
        settings,
        model.label || value,
        meta,
        value === current ? 'Current' : 'Use',
        value === current ? () => {} : (event) => saveProviderConfigField(provider, modelField, value, event.currentTarget),
      );
      const button = settings.lastElementChild.querySelector('button');
      if (button && value === current) button.disabled = true;
    }
    return true;
  }

  function renderProviderInstallableModels(settings, provider) {
    if (!provider || provider.name !== 'ollama' || !provider.available) return false;
    const loadState = modelLoadState(provider.name);
    appendProviderSubhead(settings, 'Available to download');

    if (!loadState || loadState.loading) {
      appendProviderDetailRow(settings, 'Loading Ollama models', 'Checking installed and suggested local models.');
      return true;
    }

    const models = providerInstallableModels(provider);
    if (!models.length) {
      appendProviderDetailRow(
        settings,
        provider.installable_model_error ? 'Could not load download suggestions' : 'No suggested downloads',
        provider.installable_model_error || 'Installed Ollama models already cover the Ollama library suggestions.',
      );
      return true;
    }

    for (const model of models) {
      const state = modelInstallState(provider.name, model.value);
      const meta = state && state.error
        ? state.error
        : modelMetaWithInputSupport(model, model.detail || '');
      appendProviderDetailRow(
        settings,
        model.label || model.value,
        meta,
        state && state.loading ? 'Downloading...' : 'Download',
        (event) => installProviderModel(provider, model.value, event.currentTarget),
      );
      if (state && state.loading) {
        settings.lastElementChild.querySelector('button').disabled = true;
      }
    }
    return true;
  }

  function renderProviderModels(settings, provider) {
    if (!provider || provider.name !== 'ollama' || !provider.available) return;
    const { section, body } = createProviderSection(
      'Models',
      '',
    );
    settings.appendChild(section);
    renderProviderInstalledModels(body, provider);
    renderProviderInstallableModels(body, provider);
  }

  function applyProviderSummary(summary) {
    providerProbe = mergeProviderSummaryModelState(summary);
    providerProbeError = summary && summary.error ? summary.error : null;
    renderMainBadges();
  }

  function mergeProviderSummary(summary) {
    if (!summary || summary.error) {
      if (summary && summary.error) providerProbeError = summary.error;
      return;
    }
    applyProviderSummary(summary);
  }

  function updateProviderFromActionResult(result) {
    if (result && result.summary) mergeProviderSummary(result.summary);
    if (result && result.provider) invalidateProviderModelLoad(result.provider);
    if (result && result.error) showMenuNotification(result.error, 'warning');
    if (activeView === 'provider-add') renderProviderAdd();
    if (activeView === 'providers') renderProviderProbe();
    renderSetupChecklist();
  }

  function renderProviderDetail() {
    const provider = providerByName(selectedProvider);
    const primary = providerPrimaryAction(provider);
    const setup = providerSetupAction(provider);
    $('provider-detail-title').textContent = provider ? providerDisplayName(provider) : 'No provider selected';
    $('provider-detail-meta').textContent = provider
      ? `${provider.active ? 'Active' : 'Enabled'} · ${provider.name}`
      : 'Pick a provider from the list';
    $('provider-detail-dot').className = `dot ${provider && provider.ui ? provider.ui.level : 'red'}`;
    $('provider-detail-actions').hidden = !provider;
    $('provider-detail-primary').textContent = primary.label;
    $('provider-detail-primary').disabled = primary.disabled;
    $('provider-detail-primary').hidden = primary.disabled && (primary.kind === 'use' || primary.kind === 'none');
    $('provider-detail-primary').className = primary.danger ? 'danger' : 'primary';
    $('provider-detail-setup').textContent = setup.label;
    $('provider-detail-setup').disabled = setup.disabled;
    $('provider-detail-setup').hidden = setup.hidden;
    $('provider-detail-setup').className = setup.tone === 'warning'
      ? 'warning'
      : (setup.tone === 'danger' ? 'danger' : '');
    $('provider-detail-check').disabled = !provider || provider.enabled === false || !provider.available;
    $('provider-detail-remove').hidden = !providerCanRemove(provider);
    $('provider-detail-remove').disabled = !providerCanRemove(provider);
    const actionExtra = $('provider-detail-action-extra');
    actionExtra.replaceChildren();
    actionExtra.hidden = true;
    const settings = $('provider-detail-settings');
    settings.replaceChildren();
    renderProviderCapabilities(settings, provider);
    renderProviderSetupGuide(settings, provider);
    const fields = providerConfigFields(provider);
    if (provider) {
      renderProviderConfigGroups(settings, provider, fields);
    }
    renderProviderModels(settings, provider);
    renderProviderObservability(settings, provider);
    if (provider) {
      const { section, body } = createProviderSection(
        'Reported values',
        '',
      );
      settings.appendChild(section);
      const grid = document.createElement('div');
      grid.className = 'provider-compact-grid';
      for (const [key, value] of providerSettingRows(provider)) {
        appendProviderDetailRow(grid, key, String(value));
      }
      body.appendChild(grid);
    }
    requestPopoverResize();
    if (provider) setTimeout(() => loadProviderModels(provider), 0);
  }

  function renderProviderProbe() {
    const list = $('providers-list');
    list.replaceChildren();
    const probeError = providerProbeError || (!providerProbeLoading && providerProbe && providerProbe.error);
    if (probeError) {
      appendProviderStateRow(
        list,
        'Could not load providers',
        probeError,
      );
      requestPopoverResize();
      return;
    }
    if (providerProbeLoading && (!providerProbe || providerProbe.error || !Array.isArray(providerProbe.providers))) {
      appendProviderStateRow(
        list,
        'Loading providers',
        'Checking installed provider catalog and availability.',
      );
      requestPopoverResize();
      return;
    }
    if (!providerProbe || !Array.isArray(providerProbe.providers)) {
      appendProviderStateRow(
        list,
        'Loading providers',
        'Checking installed provider catalog and availability.',
      );
      requestPopoverResize();
      return;
    }
    const enabledProviders = configuredProviders();
    if (enabledProviders.length === 0) {
      appendProviderStateRow(
        list,
        'No configured providers',
        'Add one from the built-in provider catalog.',
        'Add provider',
        () => setView('provider-add'),
      );
    }
    for (const p of enabledProviders) {
      const row = document.createElement('div');
      row.className = 'provider-row';
      row.role = 'button';
      row.tabIndex = 0;
      const dot = document.createElement('span');
      dot.className = `dot ${p.ui && p.ui.level ? p.ui.level : 'red'}`;
      const text = document.createElement('div');
      const name = document.createElement('div');
      name.className = 'value';
      name.textContent = `${providerDisplayName(p)}${p.active ? ' (active)' : ''}`;
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = `${p.name} · ${providerStatusLabel(p)}`;
      text.append(name, meta);
      const openDetails = () => {
        selectedProvider = p.name;
        providerDetailParent = 'providers';
        setView('provider-detail');
      };
      row.onclick = openDetails;
      row.onkeydown = (e) => {
        if (e.key !== 'Enter' && e.key !== ' ') return;
        e.preventDefault();
        openDetails();
      };
      const hint = document.createElement('span');
      hint.className = 'action-hint';
      hint.setAttribute('aria-hidden', 'true');
      hint.textContent = '›';
      row.append(dot, text, hint);
      list.appendChild(row);
    }
    if (activeView === 'provider-detail') renderProviderDetail();
    requestPopoverResize();
  }

  function renderProviderAdd() {
    const list = $('provider-add-list');
    list.replaceChildren();
    const probeError = providerProbeError || (!providerProbeLoading && providerProbe && providerProbe.error);
    if (probeError) {
      appendProviderStateRow(
        list,
        'Could not load providers',
        probeError,
      );
      requestPopoverResize();
      return;
    }
    if (providerProbeLoading && (!providerProbe || providerProbe.error || !Array.isArray(providerProbe.providers))) {
      appendProviderStateRow(
        list,
        'Loading providers',
        'Checking installed provider catalog and availability.',
      );
      requestPopoverResize();
      return;
    }
    if (!providerProbe || !Array.isArray(providerProbe.providers)) {
      appendProviderStateRow(
        list,
        'Loading providers',
        'Checking installed provider catalog and availability.',
      );
      requestPopoverResize();
      return;
    }
    const catalog = providerCatalogEntries();
    for (const provider of catalog) {
      const row = document.createElement('div');
      row.className = 'settings-row';
      const text = document.createElement('div');
      const name = document.createElement('div');
      name.className = 'value';
      name.textContent = providerDisplayName(provider);
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.textContent = providerAddMeta(provider);
      text.append(name, meta);
      const action = document.createElement('button');
      action.type = 'button';
      action.textContent = providerCatalogActionLabel(provider);
      action.onclick = async () => {
        setProviderEnabledLocal(provider.name, true);
        selectedProvider = provider.name;
        providerDetailParent = 'provider-add';
        renderProviderAdd();
        setView('provider-detail');
        const result = await window.alvum.providerSetEnabled(provider.name, true);
        updateProviderFromActionResult(result);
        renderProviderDetail();
      };
      row.onclick = (e) => {
        if (e.target && e.target.closest('button')) return;
        selectedProvider = provider.name;
        providerDetailParent = 'provider-add';
        setView('provider-detail');
      };
      row.append(text, action);
      list.appendChild(row);
    }
    if (catalog.length === 0) {
      appendProviderStateRow(
        list,
        'All known providers are configured',
        'Remove a provider from its detail page to make it available here again.',
      );
    }
    requestPopoverResize();
  }

  async function runProviderPrimaryAction() {
    const provider = providerByName(selectedProvider);
    const action = providerPrimaryAction(provider);
    if (!provider || action.disabled) return;
    $('provider-detail-primary').disabled = true;
    try {
      let result = null;
      if (action.kind === 'add') {
        setProviderEnabledLocal(provider.name, true);
        renderProviderDetail();
        result = await window.alvum.providerSetEnabled(provider.name, true);
      } else if (action.kind === 'auto') {
        setProviderAutoLocal();
        renderProviderDetail();
        result = await window.alvum.providerSetActive('auto');
      } else if (action.kind === 'use') {
        setProviderActiveLocal(provider.name);
        renderProviderDetail();
        result = await window.alvum.providerSetActive(provider.name);
      }
      updateProviderFromActionResult(result);
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning');
      window.alvum.requestState();
    } finally {
      renderProviderDetail();
    }
  }

  async function refreshLog() {
    document.querySelectorAll('.log-tabs button').forEach((button) => {
      button.classList.toggle('active', button.dataset.log === logKind);
    });
    const updates = logKind === 'updates';
    $('log-toolbar').hidden = updates;
    $('log-content').hidden = updates;
    $('update-panel').hidden = !updates;
    if (updates) {
      renderUpdatePanel();
      return;
    }
    const snapshot = await window.alvum.logSnapshot(logKind);
    $('log-content').textContent = snapshot.text || '(empty)';
    requestPopoverResize();
  }

  function parentViewFor(view) {
	    if (view === 'briefing-reader') return briefingReaderParent;
	    if (view === 'decision-graph') return 'briefing';
	    if (view === 'briefing-log') return 'briefing';
	    if (view === 'synthesis-profile') return 'briefing';
	    if (view === 'profile-intentions-list') return 'synthesis-profile';
	    if (view === 'profile-intention-detail') return 'profile-intentions-list';
	    if (view === 'profile-domains-list') return 'synthesis-profile';
	    if (view === 'profile-domain-detail') return 'profile-domains-list';
	    if (view === 'profile-interests-list') return 'synthesis-profile';
	    if (view === 'profile-interest-detail') return 'profile-interests-list';
	    if (view === 'profile-voices-list') return 'profile-interests-list';
	    if (view === 'profile-voice-detail') return 'profile-voices-list';
	    if (view === 'profile-writing-detail') return 'synthesis-profile';
    if (view === 'profile-schedule-detail') return 'synthesis-profile';
	    if (view === 'profile-advanced-detail') return 'synthesis-profile';
	    if (view === 'capture-input') return captureInputParent || 'extensions';
    if (view === 'connector-add') return 'extensions';
    if (view === 'extension-detail') return 'extensions';
    if (view === 'provider-add') return 'providers';
    if (view === 'provider-detail') return providerDetailParent;
    return 'main';
  }

  function registerRendererFeatures() {
    if (rendererFeaturesRegistered) return;
    rendererFeaturesRegistered = true;
    [
      createCaptureFeature({
        capture: () => refreshCaptureInputs(false),
        'capture-input': () => renderCaptureInputSettings(),
      }),
      createSynthesisFeature({
        briefing: () => {
          if (currentCalendar) renderBriefingCalendar(currentCalendar);
        },
        'briefing-reader': () => {},
        'decision-graph': () => renderDecisionGraphView(),
        'briefing-log': () => renderBriefingLogView(),
      }),
      createProfileFeature({
        'synthesis-profile': () => {
          refreshSynthesisProfile(false);
          renderSynthesisProfile();
        },
        'profile-intentions-list': () => {
          refreshSynthesisProfile(false);
          renderProfileIntentions();
        },
        'profile-intention-detail': () => {
          refreshSynthesisProfile(false);
          renderProfileIntentionDetail();
        },
        'profile-domains-list': () => {
          refreshSynthesisProfile(false);
          renderProfileDomains();
        },
        'profile-domain-detail': () => {
          refreshSynthesisProfile(false);
          renderProfileDomainDetail();
        },
        'profile-interests-list': () => {
          refreshSynthesisProfile(false);
          renderProfileInterests();
        },
        'profile-interest-detail': () => {
          refreshSynthesisProfile(false);
          renderProfileInterestDetail();
        },
        'profile-voices-list': () => {
          refreshSynthesisProfile(false);
          refreshSpeakers(false);
          renderProfileVoices();
        },
        'profile-voice-detail': () => {
          refreshSynthesisProfile(false);
          refreshSpeakers(false);
          renderProfileVoiceDetail();
        },
        'profile-writing-detail': () => {
          refreshSynthesisProfile(false);
          renderProfileWriting();
        },
        'profile-schedule-detail': () => {
          renderProfileSchedule();
        },
        'profile-advanced-detail': () => {
          refreshSynthesisProfile(false);
          renderProfileAdvanced();
        },
      }),
      createProvidersFeature({
        providers: () => renderProviderProbe(),
        'provider-add': () => renderProviderAdd(),
        'provider-detail': () => renderProviderDetail(),
      }),
      createConnectorsFeature({
        extensions: () => refreshExtensions(false),
        'connector-add': () => {
          refreshExtensions(false);
          renderAddConnector();
        },
        'extension-detail': () => renderExtensionDetail(),
      }),
      createLogsFeature({
        logs: () => refreshLog(),
      }),
    ].forEach(registerFeatureModule);
  }

  registerRendererFeatures();

  $('back-button').onclick = () => setView(parentViewFor(activeView), 'back');
  $('capture-summary').onclick = (e) => {
    if (e.target && e.target.closest('button')) return;
    setView('capture');
  };
  $('capture-summary').onkeydown = (e) => {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    e.preventDefault();
    setView('capture');
  };
  $('capture-toggle').onclick = (e) => {
    e.stopPropagation();
    window.alvum.toggleCapture();
  };
  $('capture-input-toggle').onclick = async () => {
    if (!selectedCaptureInput) return;
    const result = await window.alvum.toggleCaptureInput(selectedCaptureInput);
    if (result && result.captureInputs) captureInputs = result.captureInputs;
    else captureInputs = await window.alvum.captureInputs();
    handlePermissionIssues(result);
    renderCaptureInputSettings();
  };
  $('briefing-summary').onclick = (e) => {
    if (e.target && e.target.closest('button')) return;
    setView('briefing');
  };
  $('briefing-summary').onkeydown = (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      setView('briefing');
    }
  };
  $('provider-summary').onclick = () => setView('providers');
  $('extension-summary').onclick = () => setView('extensions');
  $('open-logs-view').onclick = () => { logKind = 'shell'; setView('logs'); };
  $('update-chip').onclick = () => { logKind = 'updates'; setView('logs'); };
  $('open-capture-dir').onclick = () => window.alvum.openCaptureDir();
  $('extension-add').onclick = () => setView('connector-add');
  $('extension-diagnose').onclick = async () => {
    $('extension-diagnose').disabled = true;
    $('extension-diagnose').textContent = 'Checking...';
    const result = await runGlobalDoctor();
    $('extension-diagnose').textContent = doctorNotificationLevel(result) === 'info' ? 'Checked' : 'Issues found';
    setTimeout(() => {
      $('extension-diagnose').textContent = 'Diagnose';
      $('extension-diagnose').disabled = false;
    }, 900);
  };
  $('update-check-now').onclick = async () => {
    $('update-check-now').disabled = true;
    $('update-check-now').textContent = 'Checking...';
    try {
      const result = await window.alvum.updateCheck();
      if (result && result.state) updateState = result.state;
      if (result && result.error) showMenuNotification(result.error, 'warning', 'Update check');
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Update check');
      window.alvum.requestState();
    } finally {
      $('update-check-now').textContent = 'Check now';
      renderUpdatePanel();
      renderUpdateChip();
    }
  };
  $('update-confirm-restart').onclick = async () => {
    $('update-confirm-restart').disabled = true;
    try {
      const result = await window.alvum.updateInstall();
      if (result && result.state) updateState = result.state;
      if (result && result.error) {
        showMenuNotification(result.error, 'warning', 'Update install');
        $('update-confirm-restart').disabled = false;
      }
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning', 'Update install');
      $('update-confirm-restart').disabled = false;
      window.alvum.requestState();
    }
    renderUpdatePanel();
    renderUpdateChip();
  };
  $('quit').onclick = () => window.alvum.quit();
  $('calendar-prev').onclick = async () => {
    const month = addMonths(currentCalendar ? currentCalendar.month : monthFromDate(selectedBriefingDate), -1);
    selectedBriefingDate = null;
    renderBriefingCalendar(await window.alvum.briefingCalendarMonth(month));
  };
  $('calendar-next').onclick = async () => {
    const month = addMonths(currentCalendar ? currentCalendar.month : monthFromDate(selectedBriefingDate), 1);
    selectedBriefingDate = null;
    renderBriefingCalendar(await window.alvum.briefingCalendarMonth(month));
  };
  $('calendar-today').onclick = async () => {
    const month = monthFromDate(currentState.briefingCalendar ? currentState.briefingCalendar.today : null);
    selectedBriefingDate = currentState.briefingCalendar ? currentState.briefingCalendar.today : null;
    renderBriefingCalendar(await window.alvum.briefingCalendarMonth(month));
  };
  $('synthesis-customize').onclick = () => setView('synthesis-profile');
	  $('profile-intention-add').onclick = () => {
	    ensureSynthesisProfileShape();
	    const index = (synthesisProfile.intentions || []).length + 1;
	    const intention = {
	      id: makeProfileId('intention', synthesisProfile.intentions),
	      kind: 'Goal',
	      domain: '',
	      description: `New intention ${index}`,
	      aliases: [],
	      notes: '',
	      success_criteria: '',
	      cadence: '',
	      target_date: null,
	      priority: 0,
	      enabled: true,
	      confirmed: true,
	      source: 'UserDefined',
	      nudge: '',
	    };
	    synthesisProfile.intentions.push(intention);
	    selectedProfileIntentionId = intention.id;
	    renderProfileIntentions();
	    setView('profile-intention-detail');
	  };
	  $('profile-domain-add').onclick = () => {
	    ensureSynthesisProfileShape();
	    const index = (synthesisProfile.domains || []).length + 1;
	    const domainName = `Custom ${index}`;
	    const domain = {
	      id: uniqueProfileDomainId(domainName, {}),
	      name: domainName,
	      description: '',
	      aliases: [],
	      priority: 0,
	      enabled: true,
	    };
	    synthesisProfile.domains.push(domain);
	    selectedProfileDomainId = domain.id;
	    renderProfileDomains();
	    setView('profile-domain-detail');
	  };
  $('profile-interest-add').onclick = () => {
    ensureSynthesisProfileShape();
    const index = (synthesisProfile.interests || []).length + 1;
    const interest = {
      id: makeProfileId('interest', synthesisProfile.interests),
      type: 'topic',
      interest_type: 'topic',
      name: `Tracked item ${index}`,
      aliases: [],
      notes: '',
      priority: 0,
      enabled: true,
      linked_knowledge_ids: [],
    };
    synthesisProfile.interests.push(interest);
    selectedProfileInterestId = interest.id;
    renderProfileInterests();
    setView('profile-interest-detail');
  };
  $('profile-advanced').oninput = () => {
    synthesisProfile = synthesisProfile || emptyProfile();
    synthesisProfile.advanced_instructions = $('profile-advanced').value;
  };
	  $('profile-intentions-save').onclick = () => saveSynthesisProfile();
	  $('profile-intention-detail-save').onclick = () => saveSynthesisProfile();
	  $('profile-intention-detail-remove').onclick = () => removeProfileIntention();
	  $('profile-domains-save').onclick = () => saveSynthesisProfile();
	  $('profile-domain-detail-save').onclick = () => saveSynthesisProfile();
	  $('profile-domain-detail-remove').onclick = () => removeProfileDomain();
	  $('profile-interests-save').onclick = () => saveSynthesisProfile();
	  $('profile-interest-detail-save').onclick = () => saveSynthesisProfile();
	  $('profile-interest-detail-remove').onclick = () => removeProfileInterest();
	  $('profile-writing-save').onclick = () => saveSynthesisProfile();
  $('profile-schedule-save').onclick = () => saveSynthesisSchedule();
  $('profile-schedule-run-due').onclick = () => runDueSynthesisFromSchedule();
	  $('profile-advanced-save').onclick = () => saveSynthesisProfile();
  $('briefing-log-refresh').onclick = async () => {
    const date = logDate || selectedBriefingDate;
    $('briefing-log-refresh').disabled = true;
    try {
      await loadPersistedBriefingLog(date, true);
      renderBriefingLogView(false);
    } finally {
      $('briefing-log-refresh').disabled = false;
    }
  };
  $('briefing-log-copy').onclick = async () => {
    try {
      const date = logDate || selectedBriefingDate;
      if (date && !briefingLogText(date)) await loadPersistedBriefingLog(date, true);
      await navigator.clipboard.writeText(briefingLogText(logDate || selectedBriefingDate));
      $('briefing-log-copy').textContent = 'Copied';
      setTimeout(() => { $('briefing-log-copy').textContent = 'Copy details'; }, 900);
    } catch {
      $('briefing-log-copy').textContent = 'Copy failed';
      setTimeout(() => { $('briefing-log-copy').textContent = 'Copy details'; }, 1200);
    }
  };
  $('reader-copy').onclick = async () => {
    try {
      await navigator.clipboard.writeText(readerMarkdown || '');
      $('reader-copy').textContent = 'Copied';
      setTimeout(() => { $('reader-copy').textContent = 'Copy markdown'; }, 900);
    } catch {
      $('reader-copy').textContent = 'Copy failed';
      setTimeout(() => { $('reader-copy').textContent = 'Copy markdown'; }, 1200);
    }
  };
  $('reader-open-file').onclick = () => {
    if (readerDate) window.alvum.openBriefingDate(readerDate);
  };
  $('provider-add').onclick = () => setView('provider-add');
  $('provider-detail-primary').onclick = () => runProviderPrimaryAction();
  $('provider-detail-setup').onclick = async () => {
    const provider = providerByName(selectedProvider);
    if (!provider) return;
    if (provider.setup_kind === 'inline') {
      focusProviderConfigField();
      return;
    }
    const actions = providerSetupActions(provider);
    if (actions.length) {
      await runProviderSetupAction(provider, actions[0].id, $('provider-detail-setup'));
      return;
    }
    $('provider-detail-setup').disabled = true;
    try {
      const result = await window.alvum.providerSetup(provider.name);
      updateProviderFromActionResult(result);
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning');
      window.alvum.requestState();
    } finally {
      renderProviderDetail();
    }
  };
  $('provider-detail-check').onclick = async () => {
    const provider = providerByName(selectedProvider);
    if (!provider) return;
    $('provider-detail-check').disabled = true;
    $('provider-detail-check').textContent = 'Pinging...';
    try {
      const result = await window.alvum.providerTest(provider.name);
      updateProviderFromActionResult(result);
    } catch (err) {
      showMenuNotification(extensionErrorMessage(err), 'warning');
      window.alvum.requestState();
    } finally {
      $('provider-detail-check').textContent = 'Ping';
      renderProviderDetail();
    }
  };
  $('provider-detail-remove').onclick = async () => {
    const provider = providerByName(selectedProvider);
    if (!provider) return;
    setProviderEnabledLocal(provider.name, false);
    renderProviderDetail();
    const result = await window.alvum.providerSetEnabled(provider.name, false);
    updateProviderFromActionResult(result);
    renderProviderDetail();
  };
  $('extension-refresh').onclick = async () => {
    $('extension-refresh').disabled = true;
    $('extension-refresh').textContent = 'Refreshing...';
    const summary = await refreshExtensions(true);
    if (summary && summary.error) console.error('[connector] refresh failed', summary.error);
    $('extension-refresh').textContent = summary && summary.error ? 'Refresh failed' : 'Refreshed';
    setTimeout(() => { $('extension-refresh').textContent = 'Refresh'; }, 900);
    $('extension-refresh').disabled = false;
  };
  $('log-refresh').onclick = () => refreshLog();
  $('log-copy').onclick = async () => {
    const text = $('log-content').textContent || '';
    try {
      await navigator.clipboard.writeText(text);
      $('log-copy').textContent = 'Copied';
      setTimeout(() => { $('log-copy').textContent = 'Copy'; }, 900);
    } catch {
      $('log-copy').textContent = 'Copy failed';
      setTimeout(() => { $('log-copy').textContent = 'Copy'; }, 1200);
    }
  };
  document.querySelectorAll('.log-tabs button').forEach((button) => {
    button.onclick = () => {
      logKind = button.dataset.log;
      refreshLog();
    };
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && activeView !== 'main') setView(parentViewFor(activeView), 'back');
  });

  window.alvum.onState(renderState);
  window.alvum.onProgress(renderProgress);
  window.alvum.onEvent(appendEvent);
  window.alvum.onPopoverShow(() => {
    window.alvum.requestState();
    refreshExtensions(false);
    if (activeView === 'logs') refreshLog();
  });
  window.alvum.requestState();
  refreshExtensions(false);
  setView(window.__initialMockView || 'main', 'replace');
  setInterval(() => {
    if (runStartedAt && $('progress-elapsed')) renderProgressElements();
  }, 1000);
