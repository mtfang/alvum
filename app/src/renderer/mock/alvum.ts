// @ts-nocheck

export function installMockAlvum(DEFAULT_DAILY_BRIEFING_OUTLINE) {
    const params = new URLSearchParams(window.location.search);
    const forcedMock = params.has('mock');
    if (window.alvum && !forcedMock) return;

    const scenario = (params.get('mock') || 'idle').toLowerCase();
    document.documentElement.classList.add('mock-preview');
    const stateListeners = [];
    const progressListeners = [];
    const eventListeners = [];
    const popoverShowListeners = [];
    const today = '2026-04-26';
    function mockCalendar(month = '2026-04') {
      const [year, monthIndex] = month.split('-').map(Number);
      const first = new Date(year, monthIndex - 1, 1);
      const start = new Date(year, monthIndex - 1, 1 - first.getDay());
      const days = [];
      for (let i = 0; i < 42; i += 1) {
        const d = new Date(start.getFullYear(), start.getMonth(), start.getDate() + i);
        const date = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
        const hasCapture = date >= '2026-04-23' && date <= today;
        const status = date === '2026-04-23'
          ? 'failed'
          : (['2026-04-24', today].includes(date) && scenario !== 'idle' ? 'success' : (hasCapture ? 'captured' : 'empty'));
        days.push({
          date,
          inMonth: date.slice(0, 7) === month,
          isToday: date === today,
          hasCapture,
          hasBriefing: status === 'success',
          status,
          artifacts: hasCapture ? '171 files · 87 MB' : '0 files · 0 B',
          failure: status === 'failed' ? { reason: 'generation exited code 137' } : null,
        });
      }
      return { month, label: first.toLocaleString(undefined, { month: 'long', year: 'numeric' }), today, days };
    }
    const captureSourcesEnabled = scenario === 'capture' || scenario === 'briefing';
    const whisperModelOptions = [
      ['tiny', 'Tiny (75 MiB)'],
      ['tiny.en', 'Tiny English (75 MiB)'],
      ['base', 'Base (142 MiB)'],
      ['base.en', 'Base English (142 MiB)'],
      ['small', 'Small (466 MiB)'],
      ['small.en', 'Small English (466 MiB)'],
      ['small.en-tdrz', 'Small English TDRZ (465 MiB)'],
      ['medium', 'Medium (1.5 GiB)'],
      ['medium.en', 'Medium English (1.5 GiB)'],
      ['large-v1', 'Large v1 (2.9 GiB)'],
      ['large-v2', 'Large v2 (2.9 GiB)'],
      ['large-v2-q5_0', 'Large v2 q5_0 (1.1 GiB)'],
      ['large-v3', 'Large v3 (2.9 GiB)'],
      ['large-v3-q5_0', 'Large v3 q5_0 (1.1 GiB)'],
      ['large-v3-turbo', 'Large v3 Turbo (1.5 GiB)'],
      ['large-v3-turbo-q5_0', 'Large v3 Turbo q5_0 (547 MiB)'],
    ].map(([variant, label]) => ({
      value: `/Users/michael/.alvum/runtime/models/ggml-${variant}.bin`,
      label,
    }));
    const state = {
      captureRunning: captureSourcesEnabled,
      captureStartedAt: '9:41:12 AM',
      briefingRunning: scenario === 'briefing',
      briefingStartedAt: scenario === 'briefing' ? '1:58:03 PM' : null,
      briefingTargetDate: scenario === 'briefing' ? '2026-04-25' : null,
      briefingRuns: scenario === 'briefing' ? {
        '2026-04-25': { date: '2026-04-25', startedAt: '1:58:03 PM', lastPct: 0, progress: null },
      } : {},
      briefingCatchupPending: scenario === 'catchup' ? 2 : 0,
      briefingCatchupDates: scenario === 'catchup' ? ['2026-04-24', '2026-04-25'] : [],
      synthesisSchedule: {
        enabled: scenario !== 'idle',
        time: '07:00',
        policy: 'completed_days',
        setup_completed: scenario !== 'idle',
        setup_pending: scenario === 'idle',
        last_auto_run_date: '',
        due_dates: scenario === 'catchup' ? ['2026-04-24', '2026-04-25'] : [],
        queued_dates: scenario === 'catchup' ? ['2026-04-25'] : [],
        running_date: scenario === 'briefing' ? '2026-04-25' : null,
        last_error: null,
      },
      captureStats: scenario === 'idle'
        ? { files: 0, bytes: 0, summary: '0 files · 0 B', detail: 'No capture artifacts for today' }
        : { files: 171, bytes: 91226112, summary: '171 files · 87 MB', detail: 'wav: 29 files / 40 MB\npng: 142 files / 47 MB' },
      captureInputs: {
        running: captureSourcesEnabled,
        enabled: captureSourcesEnabled ? 5 : 2,
        total: 5,
        inputs: [
          { id: 'audio-mic', label: 'Microphone', kind: 'capture', enabled: captureSourcesEnabled, detail: 'Local audio capture', settings: { device: 'default', chunk_duration_secs: 60 } },
          { id: 'audio-system', label: 'System audio', kind: 'capture', enabled: captureSourcesEnabled, detail: 'App and system output', settings: { device: 'default' } },
          { id: 'screen', label: 'Screen', kind: 'capture', enabled: captureSourcesEnabled, detail: 'Screenshots and OCR', settings: { idle_interval_secs: 30 } },
          { id: 'claude-code', label: 'Claude sessions', kind: 'session', enabled: true, detail: 'Claude Code transcript history', settings: { session_dir: '/Users/michael/.claude/projects', since: '2026-04-20T07:00:00Z', auto_detect_latest: true } },
          { id: 'codex', label: 'Codex sessions', kind: 'session', enabled: true, detail: 'Codex transcript history', settings: { session_dir: '/Users/michael/.codex' } },
        ],
      },
      latestBriefing: scenario === 'idle'
        ? null
        : { date: today, mtime: '4/26/2026, 1:43:22 PM' },
      briefingTargets: [
        { date: today, label: 'Today', hasCapture: true, hasBriefing: scenario !== 'idle', artifacts: '171 files · 87 MB' },
        { date: '2026-04-25', label: 'Yesterday', hasCapture: true, hasBriefing: false, artifacts: '93 files · 44 MB' },
        { date: '2026-04-24', label: '2026-04-24', hasCapture: true, hasBriefing: true, artifacts: '211 files · 101 MB' },
      ],
      briefingCalendar: mockCalendar(),
      updateState: {
        status: scenario === 'update' ? 'downloaded' : 'current',
        currentVersion: scenario === 'update' ? '0.1.13' : '0.1.14',
        latestVersion: '0.1.14',
        releaseName: scenario === 'update' ? 'Alvum 0.1.14' : null,
        releaseDate: '2026-05-01T06:26:41.000Z',
        releaseUrl: 'https://github.com/mtfang/alvum/releases/tag/0.1.14',
        error: null,
        progress: null,
        checkedAt: '2026-05-01T04:00:00.000Z',
        supported: true,
        packaged: true,
      },
    };
    const providerProbe = {
      connected: 2,
      total: 4,
      providers: [
        { name: 'claude-cli', display_name: 'Claude CLI', setup_kind: 'instructions', setup_label: 'Setup', setup_hint: 'Configure Claude CLI directly for subscription, API key, Bedrock, Vertex, or another supported backend, then Ping. Alvum uses the CLI default model unless you set an override.', config_fields: [{ key: 'text_model', label: 'Text model', kind: 'text', secret: false, configured: false, value: '', placeholder: '', detail: 'Optional model override. Leave blank to use the CLI default.', options: [{ value: '', label: 'CLI default' }, { value: 'sonnet', label: 'Sonnet' }, { value: 'opus', label: 'Opus' }] }], selected_models: { text: 'CLI default', image: 'CLI default', audio: 'CLI default' }, enabled: true, active: false, available: true, auth_hint: 'configure Claude CLI auth/backend', usage: null, test: { ok: false, status: 'usage_limited', error: 'usage limit reached' }, ui: { level: 'yellow', status: 'usage_limited', reason: 'usage limit reached' } },
        { name: 'codex-cli', display_name: 'Codex CLI', setup_kind: 'terminal', setup_label: 'Login', setup_command: 'codex login', setup_hint: 'Opens Terminal and runs `codex login`.', config_fields: [{ key: 'model', label: 'Model', kind: 'text', secret: false, configured: false, value: '', placeholder: '', detail: 'Optional model override. Leave blank to use the CLI default.', options: [{ value: '', label: 'CLI default' }, { value: 'gpt-5.4', label: 'gpt-5.4' }, { value: 'gpt-5.4-mini', label: 'GPT-5.4 Mini' }] }], enabled: true, active: true, available: true, auth_hint: 'subscription via `codex login`', usage: null, test: { ok: true, status: 'available', response_preview: 'OK' }, ui: { level: 'green', status: 'available', reason: 'authenticated and returning tokens' } },
        { name: 'ollama', display_name: 'Ollama', setup_kind: 'inline', setup_label: 'Setup', setup_command: 'ollama serve', setup_url: 'https://ollama.com/download', setup_hint: 'Set the local Ollama URL and model. `ollama serve` starts the server; if it says the address is already in use, Ollama is already running.', config_fields: [{ key: 'base_url', label: 'Server URL', kind: 'url', secret: false, configured: true, value: 'http://localhost:11434', placeholder: 'http://localhost:11434', detail: 'Local Ollama API endpoint.', options: [] }, { key: 'model', label: 'Model', kind: 'text', secret: false, configured: true, value: 'deepseek-r1:70b', placeholder: 'llama3.2', detail: 'Local model to use for synthesis.', options: [{ value: 'deepseek-r1:70b', label: 'deepseek-r1:70b' }] }], enabled: true, active: false, available: true, auth_hint: 'install from ollama.com and `ollama run <model>`', usage: null, test: { ok: true, status: 'available', response_preview: 'OK' }, ui: { level: 'green', status: 'available', reason: 'local server responding' } },
        { name: 'anthropic-api', display_name: 'Anthropic API', setup_kind: 'inline', setup_label: 'Setup', setup_url: 'https://console.anthropic.com/settings/keys', setup_hint: 'Enter an Anthropic API key. Alvum stores it in macOS Keychain.', config_fields: [{ key: 'api_key', label: 'API key', kind: 'secret', secret: true, configured: false, value: null, placeholder: 'Stored in Keychain', detail: 'Stored in macOS Keychain.', options: [] }, { key: 'model', label: 'Model', kind: 'text', secret: false, configured: false, value: '', placeholder: 'claude-sonnet-4-6', detail: 'Default model for Anthropic API calls.', options: [{ value: 'claude-sonnet-4-6', label: 'claude-sonnet-4-6' }] }], enabled: false, active: false, available: false, auth_hint: 'add an Anthropic API key', usage: null, test: null, ui: { level: 'red', status: 'not_setup', reason: 'add an Anthropic API key' } },
        { name: 'openai-api', display_name: 'OpenAI API', setup_kind: 'inline', setup_label: 'Setup', setup_url: 'https://platform.openai.com/api-keys', setup_hint: 'Enter an OpenAI API key. Alvum stores it in macOS Keychain and discovers available models from OpenAI.', config_fields: [{ key: 'api_key', label: 'API key', kind: 'secret', secret: true, configured: false, value: null, placeholder: 'Stored in Keychain', detail: 'Stored in macOS Keychain.', options: [] }, { key: 'text_model', label: 'Text model', kind: 'text', secret: false, configured: false, value: 'gpt-5.4-mini', placeholder: 'gpt-5.4-mini', detail: 'Model used for synthesis through OpenAI.', options: [{ value: 'gpt-5.4-mini', label: 'gpt-5.4-mini', input_support: { text: true, image: true, audio: false } }] }], enabled: false, active: false, available: false, auth_hint: 'add an OpenAI API key', usage: null, test: null, ui: { level: 'red', status: 'not_setup', reason: 'add an OpenAI API key' } },
      ],
    };
    function mockCapability(modelSupported, adapterSupported, provenance) {
      return {
        supported: modelSupported && adapterSupported,
        model_supported: modelSupported,
        adapter_supported: adapterSupported,
        provenance,
        status: modelSupported && adapterSupported ? 'ready' : (modelSupported ? 'transport_limited' : 'unsupported'),
        detail: '',
      };
    }
    const mockSetupActions = {
      'claude-cli': [
        { id: 'claude_doctor', label: 'Run Claude doctor', kind: 'terminal', detail: 'Open Terminal and run Claude CLI diagnostics.' },
        { id: 'open_claude_config', label: 'Open Claude config', kind: 'folder', detail: 'Open ~/.claude.' },
      ],
      'codex-cli': [
        { id: 'codex_login', label: 'Log in', kind: 'terminal', detail: 'Open Terminal and run codex login.' },
        { id: 'codex_models', label: 'List models', kind: 'terminal', detail: 'Open Terminal and run codex debug models --bundled.' },
        { id: 'open_codex_config', label: 'Open Codex config', kind: 'file', detail: 'Open ~/.codex/config.toml.' },
      ],
      ollama: [
        { id: 'ollama_download', label: 'Install Ollama', kind: 'url', detail: 'Open the Ollama download page.' },
        { id: 'ollama_serve', label: 'Start server', kind: 'terminal', detail: 'Open Terminal and run ollama serve.' },
        { id: 'ollama_list', label: 'List models', kind: 'terminal', detail: 'Open Terminal and run ollama list.' },
        { id: 'ollama_show_text', label: 'Inspect text model', kind: 'terminal', detail: 'Run ollama show for the selected text model.' },
      ],
      'anthropic-api': [
        { id: 'anthropic_keys', label: 'Open API keys', kind: 'url', detail: 'Open the Anthropic API key page.' },
        { id: 'anthropic_models', label: 'Open model docs', kind: 'url', detail: 'Open Anthropic model docs.' },
        { id: 'edit_anthropic_key', label: 'Edit API key', kind: 'inline', detail: 'Focus the API key field below.' },
      ],
      'openai-api': [
        { id: 'openai_keys', label: 'Open API keys', kind: 'url', detail: 'Open the OpenAI API key page.' },
        { id: 'openai_models', label: 'Open model docs', kind: 'url', detail: 'Open OpenAI model docs.' },
        { id: 'openai_audio_docs', label: 'Open audio docs', kind: 'url', detail: 'Open OpenAI speech-to-text docs.' },
        { id: 'edit_openai_key', label: 'Edit API key', kind: 'inline', detail: 'Focus the API key field below.' },
      ],
    };
    providerProbe.providers.forEach((provider) => {
      provider.setup_actions = mockSetupActions[provider.name] || [];
      (provider.config_fields || []).forEach((field) => {
        field.group = field.key === 'model' || field.key === 'text_model' || field.key === 'image_model' || field.key === 'audio_model'
          ? 'models'
          : 'connection';
      });
      const textField = (provider.config_fields || []).find((field) => field.key === 'text_model' || field.key === 'model');
      if (textField) {
        textField.key = 'text_model';
        textField.label = 'Text model';
      }
      const cliDefault = textField && Array.isArray(textField.options)
        ? textField.options.find((option) => String(option.value || '') === '' && option.label === 'CLI default')
        : null;
      const textModel = textField ? String(textField.value || textField.placeholder || (cliDefault ? 'CLI default' : '')) : '';
      const providerUsesCliDefault = provider.name === 'claude-cli' || provider.name === 'codex-cli';
      const imageModel = provider.name === 'ollama' ? '' : (provider.name === 'openai-api' ? 'gpt-5.4-mini' : (providerUsesCliDefault ? 'CLI default' : 'claude-sonnet-4-6'));
      const imageOptions = providerUsesCliDefault ? [{ value: '', label: 'CLI default' }] : [{ value: imageModel, label: imageModel }];
      provider.config_fields.push({
        key: 'image_model',
        label: 'Image model',
        kind: 'text',
        secret: false,
        configured: false,
        value: '',
        placeholder: providerUsesCliDefault ? '' : imageModel,
        detail: provider.name === 'ollama' ? 'Local model to use for provider-backed screen processing.' : 'Tracked for capability display.',
        group: 'models',
        options: imageOptions,
      });
      provider.config_fields.push({
        key: 'audio_model',
        label: 'Audio model',
        kind: 'text',
        secret: false,
        configured: false,
        value: '',
        placeholder: '',
        detail: provider.name === 'openai-api' ? 'Model used for provider-backed diarized audio transcription.' : 'Reserved for provider audio processing; no Alvum audio adapter exists yet.',
        group: 'models',
        options: provider.name === 'openai-api' ? [{ value: 'gpt-4o-transcribe-diarize', label: 'gpt-4o-transcribe-diarize', input_support: { text: false, image: false, audio: true } }] : (providerUsesCliDefault ? [{ value: '', label: 'CLI default' }] : []),
      });
      provider.selected_models = { text: textModel || null, image: imageModel || null, audio: provider.name === 'openai-api' ? 'gpt-4o-transcribe-diarize' : (providerUsesCliDefault ? 'CLI default' : null) };
      const provenance = provider.name === 'ollama' ? 'native_api' : (provider.name === 'codex-cli' ? 'cli_catalog' : 'static_catalog');
      provider.capabilities = {
        text: mockCapability(true, true, provenance),
        image: mockCapability(provider.name !== 'claude-cli' && provider.name !== 'ollama', provider.name === 'ollama' || provider.name === 'anthropic-api' || provider.name === 'openai-api', provenance),
        audio: mockCapability(provider.name === 'openai-api', provider.name === 'openai-api', provenance),
      };
      provider.readiness = {
        status: provider.available ? 'available' : 'setup_required',
        detail: provider.available ? 'Provider setup is detectable.' : 'Provider setup is not detectable yet.',
      };
    });
    state.providerSummary = providerProbe;
    state.providerStats = {
      providers: {
        'codex-cli': {
          provider: 'codex-cli',
          active_calls: 0,
          calls_started: 1,
          calls_finished: 1,
          calls_failed: 0,
          prompt_chars: 18422,
          response_chars: 7200,
          input_tokens: 0,
          output_tokens: 0,
          total_tokens: 0,
          input_tokens_estimate: 4606,
          output_tokens_estimate: 1800,
          total_tokens_estimate: 6406,
          latency_ms: 4200,
          last_call_site: 'thread/chunk_03',
          last_status: 'ok',
          last_latency_ms: 4200,
          last_tokens_per_sec: 428.6,
          last_token_source: 'estimated',
          updated_at: new Date().toISOString(),
        },
      },
    };
    let mockSynthesisProfile = {
      intentions: [
        { id: 'ship_alignment_engine', kind: 'Goal', domain: 'Career', description: 'Ship v1.5 alignment engine by end of May', aliases: ['alignment engine'], notes: 'Primary product goal.', success_criteria: 'A usable signed app with trustworthy synthesis.', cadence: '', target_date: '2026-05-31', priority: 0, enabled: true, confirmed: true, source: 'UserDefined', nudge: 'Protect focused implementation blocks.' },
        { id: 'half_marathon', kind: 'Goal', domain: 'Health', description: 'Run a half marathon in the fall', aliases: ['fall race'], notes: '', success_criteria: 'Stay on training plan.', cadence: 'weekly training', target_date: '2026-10-12', priority: 1, enabled: true, confirmed: true, source: 'CheckIn', nudge: 'Protect the next weekday run slot.' },
      ],
      domains: [
        { id: 'Career', name: 'Career', description: 'Work, projects, professional commitments, tools, codebases.', aliases: [], priority: 0, enabled: true },
        { id: 'Health', name: 'Health', description: 'Exercise, sleep, eating, medical, mental health.', aliases: [], priority: 1, enabled: true },
        { id: 'Family', name: 'Family', description: 'Partner, kids, household, social plans.', aliases: [], priority: 2, enabled: true },
      ],
      interests: [
        { id: 'person_michael', type: 'person', name: 'Michael', aliases: ['Mike'], notes: 'Primary owner.', priority: 0, enabled: true, linked_knowledge_ids: ['entity_michael'] },
        { id: 'person_lana', type: 'person', name: 'Lana', aliases: [], notes: 'Recurring collaborator mentioned in calls.', priority: 1, enabled: true, linked_knowledge_ids: [] },
        { id: 'project_alvum', type: 'project', name: 'Alvum', aliases: ['tray app'], notes: 'Primary product work.', priority: 0, enabled: true, linked_knowledge_ids: ['entity_alvum'] },
      ],
      writing: {
        detail_level: 'detailed',
        tone: 'direct',
        outline: DEFAULT_DAILY_BRIEFING_OUTLINE,
      },
      advanced_instructions: '',
      ignored_suggestions: [],
    };
    let mockProfileSuggestions = [
      { id: 'entity_openai', type: 'organization', name: 'OpenAI', description: 'Organization mentioned in generated knowledge.', source: 'knowledge.entity', knowledge_id: 'entity_openai' },
      { id: 'pattern_scope_creep', type: 'topic', name: 'scope creep', description: 'Recurring pattern detected across decisions.', source: 'knowledge.pattern', knowledge_id: 'pattern_scope_creep' },
    ];
    const extensionState = {
      connectors: [
        {
          id: 'github/activity',
          component_id: 'github/activity',
          package_id: 'github',
          connector_id: 'activity',
          kind: 'external',
          package_name: 'GitHub Activity',
          display_name: 'GitHub activity',
          description: 'Mock GitHub connector',
          version: '0.1.0',
          enabled: true,
          read_only: false,
          package_dir: '/Users/michael/.alvum/runtime/extensions/github',
          aggregate_state: 'all_on',
          source_count: 1,
          enabled_source_count: 1,
          source_controls: [{ id: 'github-events', label: 'GitHub events', component: 'github/events', kind: 'capture', enabled: true, toggleable: false, detail: 'Mock GitHub connector' }],
          captures: [{ component: 'github/events', display_name: 'GitHub events', kind: 'capture', exists: true }],
          processors: [{ component: 'github/summarize', display_name: 'GitHub summarizer', kind: 'processor', exists: true }],
          processor_controls: [{ id: 'github/summarize', component: 'github/summarize', label: 'GitHub summarizer', kind: 'processor', detail: 'Summarizes GitHub activity.', settings: [] }],
          analyses: [{ id: 'weekly-review', component_id: 'github/weekly-review', display_name: 'Weekly review', output: 'artifact', scopes: ['briefing'], exists: true }],
          routes: [{ from: { component: 'github/events', schema: 'github.event.v1', display_name: 'GitHub events', exists: true }, to: [{ component: 'github/summarize', display_name: 'GitHub summarizer', exists: true }], issues: [] }],
          route_count: 1,
          analysis_count: 1,
          issues: [],
        },
        {
          id: 'calendar/main',
          component_id: 'calendar/main',
          package_id: 'calendar',
          connector_id: 'main',
          kind: 'external',
          package_name: 'Calendar Context',
          display_name: 'Calendar',
          description: 'Mock calendar connector',
          version: '0.1.0',
          enabled: false,
          read_only: false,
          package_dir: '/Users/michael/.alvum/runtime/extensions/calendar',
          aggregate_state: 'all_off',
          source_count: 0,
          enabled_source_count: 0,
          source_controls: [],
          captures: [{ component: 'calendar/events', display_name: 'Calendar events', kind: 'capture', exists: true }],
          processors: [],
          processor_controls: [],
          analyses: [],
          routes: [],
          route_count: 0,
          analysis_count: 0,
          issues: [],
        },
        {
          id: 'alvum.audio/audio',
          component_id: 'alvum.audio/audio',
          package_id: 'alvum.audio',
          connector_id: 'audio',
          kind: 'core',
          package_name: 'Alvum Audio',
          display_name: 'Audio',
          description: 'Built-in audio connector',
          version: '0.1.0',
          enabled: true,
          read_only: false,
          package_dir: 'builtin://alvum.audio',
          aggregate_state: captureSourcesEnabled ? 'all_on' : 'all_off',
          source_count: 2,
          enabled_source_count: captureSourcesEnabled ? 2 : 0,
          source_controls: [
            { id: 'audio-mic', label: 'Microphone', component: 'alvum.audio/audio-mic', kind: 'capture', enabled: captureSourcesEnabled, toggleable: true, detail: 'Built-in microphone capture source.' },
            { id: 'audio-system', label: 'System audio', component: 'alvum.audio/audio-system', kind: 'capture', enabled: captureSourcesEnabled, toggleable: true, detail: 'Built-in system-audio capture source.' },
          ],
          captures: [
            { component: 'alvum.audio/audio-mic', display_name: 'Microphone audio', kind: 'capture', exists: true },
            { component: 'alvum.audio/audio-system', display_name: 'System audio', kind: 'capture', exists: true },
          ],
          processors: [{ component: 'alvum.audio/whisper', display_name: 'Whisper transcription', kind: 'processor', exists: true }],
          processor_controls: [
            {
              id: 'alvum.audio/whisper',
              component: 'alvum.audio/whisper',
              label: 'Whisper transcription',
              kind: 'processor',
              detail: 'Built-in audio transcription processor.',
              readiness: {
                status: scenario === 'idle' ? 'waiting_on_install' : 'ready',
                level: scenario === 'idle' ? 'warning' : 'ok',
                detail: scenario === 'idle'
                  ? 'Local audio processing needs Whisper model /Users/michael/.alvum/runtime/models/ggml-base.en.bin.'
                  : 'Local Whisper model is installed at /Users/michael/.alvum/runtime/models/ggml-base.en.bin.',
                action: scenario === 'idle' ? { kind: 'install_whisper', label: 'Install' } : null,
              },
              settings: [
                { key: 'mode', label: 'Audio processing', value: 'local', value_label: 'Local Whisper + speaker IDs', detail: 'Choose Local Whisper + speaker IDs, provider diarized transcription, or off.', options: [{ value: 'local', label: 'Local Whisper + speaker IDs' }, { value: 'provider', label: 'Provider diarized transcription' }, { value: 'off', label: 'Off' }] },
                { key: 'whisper_model', label: 'Local transcription model', value: '/Users/michael/.alvum/runtime/models/ggml-base.en.bin', value_label: 'Base English (142 MiB)', detail: 'Whisper model file used when audio processing is Local.', options: whisperModelOptions },
                { key: 'whisper_language', label: 'Local transcription language', value: 'en', value_label: 'English', detail: 'Language hint used by local Whisper transcription.', options: [{ value: 'en', label: 'English' }, { value: 'auto', label: 'Auto detect' }] },
                { key: 'diarization_enabled', label: 'Local speaker IDs', value: 'true', value_label: 'On', detail: 'Stores anonymous local speaker IDs across runs when local processing is enabled.', options: [{ value: 'true', label: 'On' }, { value: 'false', label: 'Off' }] },
                { key: 'diarization_model', label: 'Local diarization model', value: 'pyannote-local', value_label: 'pyannote-local', detail: 'Local diarization and embedding backend used for anonymous voice evidence.', options: [] },
                { key: 'pyannote_command', label: 'Pyannote command', value: '', value_label: 'Not installed', detail: 'Optional local command that emits pyannote-compatible diarization JSON for an audio file.', options: [] },
                { key: 'pyannote_hf_token', label: 'Hugging Face token', value: null, value_label: 'Not configured', detail: 'Optional token used only to download/load gated Pyannote models from Hugging Face.', secret: true, configured: false, placeholder: 'hf_...', options: [] },
                { key: 'speaker_registry', label: 'Local speaker registry', value: '/Users/michael/.alvum/runtime/speakers.json', value_label: 'speakers.json', detail: 'Local file storing anonymous speaker IDs and confirmed labels.', options: [] },
                { key: 'provider', label: 'Provider diarized transcription', value: 'openai-api', value_label: 'OpenAI API', detail: 'Used only when audio processing mode is Provider. Local mode uses Whisper and local speaker IDs.', options: [{ value: 'openai-api', label: 'OpenAI API' }] },
              ],
            },
          ],
          analyses: [],
          routes: [
            { from: { component: 'alvum.audio/audio-mic', display_name: 'Microphone audio', exists: true }, to: [{ component: 'alvum.audio/whisper', display_name: 'Whisper transcription', exists: true }], issues: [] },
            { from: { component: 'alvum.audio/audio-system', display_name: 'System audio', exists: true }, to: [{ component: 'alvum.audio/whisper', display_name: 'Whisper transcription', exists: true }], issues: [] },
          ],
          route_count: 2,
          analysis_count: 0,
          issues: [],
        },
        {
          id: 'alvum.screen/screen',
          component_id: 'alvum.screen/screen',
          package_id: 'alvum.screen',
          connector_id: 'screen',
          kind: 'core',
          package_name: 'Alvum Screen',
          display_name: 'Screen',
          description: 'Built-in screen connector',
          version: '0.1.0',
          enabled: true,
          read_only: false,
          package_dir: 'builtin://alvum.screen',
          aggregate_state: captureSourcesEnabled ? 'all_on' : 'all_off',
          source_count: 1,
          enabled_source_count: captureSourcesEnabled ? 1 : 0,
          source_controls: [
            { id: 'screen', label: 'Screen', component: 'alvum.screen/snapshot', kind: 'capture', enabled: captureSourcesEnabled, toggleable: true, detail: 'Built-in periodic screen snapshot capture source.' },
          ],
          captures: [{ component: 'alvum.screen/snapshot', display_name: 'Screen snapshot', kind: 'capture', exists: true }],
          processors: [{ component: 'alvum.screen/vision', display_name: 'Vision/OCR', kind: 'processor', exists: true }],
          processor_controls: [
            {
              id: 'alvum.screen/vision',
              component: 'alvum.screen/vision',
              label: 'Vision/OCR',
              kind: 'processor',
              detail: 'Built-in screen image processor.',
              readiness: {
                status: 'ready',
                level: 'ok',
                detail: 'OCR processing uses the local macOS Vision framework.',
                action: null,
              },
              settings: [
                { key: 'mode', label: 'Recognition method', value: 'ocr', value_label: 'OCR', detail: 'Text and content recognition method for screenshots.', options: [{ value: 'ocr', label: 'OCR' }, { value: 'provider', label: 'Provider' }, { value: 'off', label: 'Off' }] },
              ],
            },
          ],
          analyses: [],
          routes: [{ from: { component: 'alvum.screen/snapshot', display_name: 'Screen snapshot', exists: true }, to: [{ component: 'alvum.screen/vision', display_name: 'Vision/OCR', exists: true }], issues: [] }],
          route_count: 1,
          analysis_count: 0,
          issues: [],
        },
      ],
    };
    const speakerState: any = {
      ok: true,
      path: '/Users/michael/.alvum/runtime/speakers.json',
      speakers: [
        {
          speaker_id: 'spk_local_michael',
          label: 'Michael',
          linked_interest_id: 'person_michael',
          linked_interest: { id: 'person_michael', type: 'person', name: 'Michael' },
          fingerprint_count: 4,
          samples: [{ text: 'We should review the release checklist.', source: 'audio-mic', ts: '2026-04-26T09:42:00Z', start_secs: 0, end_secs: 6.2, media_path: '/Users/michael/.alvum/capture/2026-04-26/audio/mic/09-42-00.wav', mime: 'audio/wav' }],
          person_candidates: [],
          duplicate_candidates: [{ speaker_id: 'spk_local_unknown', label: null, linked_interest_id: null, score: 0.64 }],
          context_interests: [{ id: 'project_alvum', type: 'project', name: 'Alvum', score: 0.7, reason: 'sample mentions release checklist' }],
        },
        {
          speaker_id: 'spk_local_unknown',
          label: null,
          linked_interest_id: null,
          linked_interest: null,
          fingerprint_count: 2,
          samples: [{ text: 'The local model finished processing.', source: 'audio-system', ts: '2026-04-26T11:08:00Z', start_secs: 2.0, end_secs: 8.4, media_path: '/Users/michael/.alvum/capture/2026-04-26/audio/system/11-08-00.wav', mime: 'audio/wav' }],
          person_candidates: [{ id: 'person_lana', type: 'person', name: 'Lana', score: 0.82, reason: 'nearby transcript mentions Lana' }],
          duplicate_candidates: [{ speaker_id: 'spk_local_michael', label: 'Michael', linked_interest_id: 'person_michael', score: 0.64 }],
          context_interests: [{ id: 'project_alvum', type: 'project', name: 'Alvum', score: 0.9, reason: 'sample mentions local model processing' }],
        },
      ],
      samples: [
        {
          sample_id: 'vsm_michael_release',
          cluster_id: 'spk_local_michael',
          text: 'We should review the release checklist.',
          source: 'audio-mic',
          ts: '2026-04-26T09:42:00Z',
          start_secs: 0,
          end_secs: 6.2,
          media_path: '/Users/michael/.alvum/capture/2026-04-26/audio/mic/09-42-00.wav',
          mime: 'audio/wav',
          linked_interest_id: 'person_michael',
          linked_interest: { id: 'person_michael', type: 'person', name: 'Michael' },
          person_candidates: [],
          context_interests: [{ id: 'project_alvum', type: 'project', name: 'Alvum', score: 0.7, reason: 'sample mentions release checklist' }],
        },
        {
          sample_id: 'vsm_unknown_model',
          cluster_id: 'spk_local_unknown',
          text: 'The local model finished processing.',
          source: 'audio-system',
          ts: '2026-04-26T11:08:00Z',
          start_secs: 2.0,
          end_secs: 8.4,
          media_path: '/Users/michael/.alvum/capture/2026-04-26/audio/system/11-08-00.wav',
          mime: 'audio/wav',
          linked_interest_id: null,
          linked_interest: null,
          person_candidates: [{ id: 'person_lana', type: 'person', name: 'Lana', score: 0.82, reason: 'nearby transcript mentions Lana' }],
          context_interests: [{ id: 'project_alvum', type: 'project', name: 'Alvum', score: 0.9, reason: 'sample mentions local model processing' }],
        },
      ],
      error: null,
    };
    speakerState.clusters = speakerState.speakers;
    const globalDoctor = {
      ok: true,
      error_count: 0,
      warning_count: 0,
      checks: [
        { id: 'config', label: 'Config', level: 'ok', message: 'Loaded mock config.' },
        { id: 'connectors', label: 'Connectors', level: 'ok', message: '4/4 connectors enabled; route matrix is valid.' },
        { id: 'extensions', label: 'Extensions', level: 'ok', message: 'No external extensions installed.' },
        { id: 'providers', label: 'Providers', level: 'ok', message: 'Configured provider codex-cli is available.' },
      ],
    };
    const mockProgress = [
      { stage: 'gather', current: 1, total: 1 },
      { stage: 'process', current: 8, total: 18 },
      { stage: 'thread', current: 3, total: 5 },
      { stage: 'cluster', current: 1, total: 1 },
      { stage: 'cluster-correlate', current: 1, total: 1 },
      { stage: 'domain', current: 1, total: 1 },
      { stage: 'domain-correlate', current: 1, total: 1 },
      { stage: 'day', current: 1, total: 1 },
      { stage: 'knowledge', current: 1, total: 1 },
    ];
    const mockEvents = [
      { kind: 'stage_enter', stage: 'gather' },
      { kind: 'input_inventory', connector: 'capture', source: 'mic', ref_count: 18 },
      { kind: 'input_inventory', connector: 'capture', source: 'calendar', ref_count: 0 },
      { kind: 'llm_call_start', provider: 'codex-cli', call_site: 'thread/chunk_03', prompt_chars: 18422, prompt_tokens_estimate: 4606 },
      { kind: 'llm_call_end', provider: 'codex-cli', call_site: 'thread/chunk_03', prompt_chars: 18422, latency_ms: 4200, response_chars: 7200, input_tokens: null, output_tokens: null, total_tokens: null, prompt_tokens_estimate: 4606, response_tokens_estimate: 1800, total_tokens_estimate: 6406, tokens_per_sec: null, tokens_per_sec_estimate: 428.6, ok: true, attempts: 1 },
      { kind: 'warning', source: 'knowledge', message: 'bounded prompt to revealed facts; continuing best effort' },
    ];
    function mockDecisionGraph(date) {
      return {
        ok: true,
        date,
        domains: ['Career', 'Health', 'Family'],
        decisions: [
          { id: 'dec_001', date, time: '08:45', summary: 'Chose to protect synthesis quality as the day’s anchor decision.', domain: 'Career', source: 'Revealed', magnitude: 0.82, effects: ['dec_002', 'dec_003', 'dec_004'], causes: [], evidence: ['synthesis quality'], open: false },
          { id: 'dec_002', date, time: '10:05', summary: 'Split the platform work into capture reliability.', domain: 'Career', source: 'Spoken', magnitude: 0.64, effects: ['dec_005', 'dec_006'], causes: ['dec_001'], evidence: ['capture reliability'], open: false },
          { id: 'dec_003', date, time: '10:22', summary: 'Split the platform work into provider diagnostics.', domain: 'Career', source: 'Spoken', magnitude: 0.68, effects: ['dec_007', 'dec_008'], causes: ['dec_001'], evidence: ['provider diagnostics'], open: false },
          { id: 'dec_004', date, time: '10:40', summary: 'Split the platform work into user-facing synthesis polish.', domain: 'Family', source: 'Explained', magnitude: 0.58, effects: ['dec_009', 'dec_010'], causes: ['dec_001'], evidence: ['synthesis polish'], open: true },
          { id: 'dec_005', date, time: '11:30', summary: 'Kept microphone and system audio controls under connector settings.', domain: 'Career', source: 'Revealed', magnitude: 0.52, effects: [], causes: ['dec_002'], evidence: ['connector settings'], open: false },
          { id: 'dec_006', date, time: '12:10', summary: 'Moved capture status toward read-only visibility in the capture pane.', domain: 'Career', source: 'Explained', magnitude: 0.47, effects: [], causes: ['dec_002'], evidence: ['capture pane'], open: false },
          { id: 'dec_007', date, time: '13:35', summary: 'Kept provider pings as explicit checks rather than refresh state.', domain: 'Career', source: 'Spoken', magnitude: 0.55, effects: [], causes: ['dec_003'], evidence: ['provider pings'], open: false },
          { id: 'dec_008', date, time: '14:15', summary: 'Prevented failed providers from becoming auto-selected defaults.', domain: 'Career', source: 'Revealed', magnitude: 0.72, effects: [], causes: ['dec_003'], evidence: ['auto-selected defaults'], open: false },
          { id: 'dec_009', date, time: '16:05', summary: 'Moved the decision graph title above the visualization.', domain: 'Family', source: 'Explained', magnitude: 0.5, effects: [], causes: ['dec_004'], evidence: ['decision graph title'], open: false },
          { id: 'dec_010', date, time: '17:20', summary: 'Turned graph links into clickable decision chips.', domain: 'Family', source: 'Spoken', magnitude: 0.6, effects: [], causes: ['dec_004'], evidence: ['decision chips'], open: false },
        ],
        edges: [
          { from_id: 'dec_001', to_id: 'dec_002', relation: 'decomposition', mechanism: 'The day anchor split into capture reliability work.', strength: 'primary' },
          { from_id: 'dec_001', to_id: 'dec_003', relation: 'decomposition', mechanism: 'The day anchor split into provider diagnostics work.', strength: 'primary' },
          { from_id: 'dec_001', to_id: 'dec_004', relation: 'decomposition', mechanism: 'The day anchor split into synthesis polish work.', strength: 'primary' },
          { from_id: 'dec_002', to_id: 'dec_005', relation: 'implementation', mechanism: 'Capture reliability led to connector-owned audio controls.', strength: 'contributing' },
          { from_id: 'dec_002', to_id: 'dec_006', relation: 'implementation', mechanism: 'Capture reliability led to a cleaner read-only capture pane.', strength: 'contributing' },
          { from_id: 'dec_003', to_id: 'dec_007', relation: 'implementation', mechanism: 'Provider diagnostics kept manual pings available.', strength: 'contributing' },
          { from_id: 'dec_003', to_id: 'dec_008', relation: 'guardrail', mechanism: 'Provider diagnostics prevented failed auto-selection.', strength: 'primary' },
          { from_id: 'dec_004', to_id: 'dec_009', relation: 'presentation', mechanism: 'Synthesis polish moved the graph title above the visualization.', strength: 'contributing' },
          { from_id: 'dec_004', to_id: 'dec_010', relation: 'navigation', mechanism: 'Synthesis polish made graph links directly clickable.', strength: 'contributing' },
        ],
        derived_edges: 0,
        summary: { decision_count: 10, edge_count: 9, domain_count: 3 },
      };
    }
    function emitState() { stateListeners.forEach((cb) => cb({ ...state })); }
    function emitBriefingSamples(date = '2026-04-25') {
      if (!state.briefingRuns[date]) return;
      mockProgress.forEach((p, i) => setTimeout(() => progressListeners.forEach((cb) => cb({ ...p, briefingDate: date })), i * 120));
      mockEvents.forEach((evt, i) => setTimeout(() => eventListeners.forEach((cb) => cb({ ts: Date.now(), briefingDate: date, ...evt })), i * 100));
    }
    window.alvum = {
      onState: (cb) => stateListeners.push(cb),
      onProgress: (cb) => progressListeners.push(cb),
      onEvent: (cb) => eventListeners.push(cb),
      onPopoverShow: (cb) => popoverShowListeners.push(cb),
      requestState: () => setTimeout(() => { emitState(); Object.keys(state.briefingRuns).forEach((date) => emitBriefingSamples(date)); }, 0),
      toggleCapture: () => { state.captureRunning = !state.captureRunning; emitState(); },
      captureInputs: async () => JSON.parse(JSON.stringify(state.captureInputs)),
      toggleCaptureInput: async (id) => {
        const input = state.captureInputs.inputs.find((item) => item.id === id);
        if (!input) return { ok: false, error: 'unknown input' };
        input.enabled = !input.enabled;
        for (const ext of extensionState.connectors) {
          const control = Array.isArray(ext.source_controls)
            ? ext.source_controls.find((item) => item.id === id)
            : null;
          if (!control) continue;
          control.enabled = input.enabled;
          ext.enabled_source_count = ext.source_controls.filter((item) => item.enabled).length;
          ext.source_count = ext.source_controls.length;
          ext.aggregate_state = ext.enabled_source_count === 0 ? 'all_off' : (ext.enabled_source_count === ext.source_count ? 'all_on' : 'partial');
          ext.enabled = ext.enabled_source_count > 0;
        }
        state.captureInputs.enabled = state.captureInputs.inputs.filter((item) => item.enabled).length;
        emitState();
        return { ok: true, input: id, enabled: input.enabled, captureInputs: JSON.parse(JSON.stringify(state.captureInputs)) };
      },
      captureInputSetSetting: async (id, key, value) => {
        const input = state.captureInputs.inputs.find((item) => item.id === id);
        if (!input || !input.settings || !(key in input.settings)) return { ok: false, error: 'unknown setting' };
        const previous = input.settings[key];
        if (typeof previous === 'boolean') input.settings[key] = value === true || value === 'true';
        else if (typeof previous === 'number') input.settings[key] = Number(value);
        else input.settings[key] = String(value);
        emitState();
        return { ok: true, input: id, key, value: input.settings[key], captureInputs: JSON.parse(JSON.stringify(state.captureInputs)) };
      },
      chooseDirectory: async (defaultPath) => ({ ok: true, path: defaultPath || '/Users/michael' }),
      startBriefing: () => { const date = state.briefingCalendar.today; state.briefingRuns[date] = { date, startedAt: new Date().toLocaleTimeString(), lastPct: 0, progress: null }; state.briefingRunning = true; emitState(); emitBriefingSamples(date); },
      startBriefingDate: async (date) => { state.briefingRuns[date] = { date, startedAt: new Date().toLocaleTimeString(), lastPct: 0, progress: null }; state.briefingRunning = true; emitState(); emitBriefingSamples(date); return { ok: true, date }; },
      cancelBriefingDate: async (date) => {
        if (!state.briefingRuns[date]) return { ok: false, error: 'no running synthesis for date' };
        state.briefingRuns[date].canceling = true;
        emitState();
        setTimeout(() => {
          delete state.briefingRuns[date];
          state.briefingRunning = Object.keys(state.briefingRuns).length > 0;
          emitState();
        }, 300);
        return { ok: true, date, status: 'canceling' };
      },
      briefingCalendarMonth: async (month) => mockCalendar(month),
      openBriefing: () => console.log('[mock] open briefing'),
      openBriefingDate: async (date) => { console.log('[mock] open briefing', date); return { ok: true }; },
      readBriefingDate: async (date) => ({
        ok: true,
        date,
        path: `/mock/${date}/briefing.md`,
        mtime: '4/26/2026, 1:43:22 PM',
        markdown: `# Briefing for ${date}\n\n## What actually moved\n\n- Finalized the tray popover calendar and provider status flow.\n- Kept capture running while validating the UI in Electron.\n\n## Decisions\n\n> Keep the tray concise; move operational detail into drilldown views.\n\n## Next\n\n1. Review yesterday's generated briefing.\n2. Investigate any failed generation markers.\n\n\`capture\` remains healthy.`,
        html: `<h1>Briefing for ${date}</h1><h2>What actually moved</h2><ul><li>Finalized the tray popover calendar and provider status flow.</li><li>Kept capture running while validating the UI in Electron.</li></ul><h2>Metrics</h2><table><thead><tr><th>Item</th><th>Value</th></tr></thead><tbody><tr><td>Artifacts</td><td>171</td></tr><tr><td>Storage</td><td>87 MB</td></tr></tbody></table><h2>Model</h2><p><span class="katex">E = mc²</span></p><h2>Decisions</h2><blockquote>Keep the tray concise; move operational detail into drilldown views.</blockquote>`,
      }),
      briefingRunLogDate: async (date) => ({
        ok: true,
        date,
        run: {
          run_id: 'mock-run-001',
          run_dir: `/mock/${date}/runs/mock-run-001`,
          status: date === '2026-04-23' ? 'failed' : 'success',
          reason: date === '2026-04-23' ? 'code 137' : null,
          last_stage: 'day',
        },
        text: `Run mock-run-001\nDate: ${date}\nStatus: ${date === '2026-04-23' ? 'failed' : 'success'}\nEvents:\n{\"kind\":\"stage_enter\",\"stage\":\"gather\"}\n{\"kind\":\"error\",\"source\":\"mock\",\"message\":\"example failure\"}`,
      }),
      decisionGraphDate: async (date) => mockDecisionGraph(date),
      synthesisProfile: async () => ({ ok: true, profile: JSON.parse(JSON.stringify(mockSynthesisProfile)) }),
      synthesisProfileSave: async (profile) => {
        mockSynthesisProfile = JSON.parse(JSON.stringify(profile));
        return { ok: true, profile: JSON.parse(JSON.stringify(mockSynthesisProfile)), suggestions: JSON.parse(JSON.stringify(mockProfileSuggestions)) };
      },
      synthesisProfileSuggestions: async () => ({ ok: true, suggestions: JSON.parse(JSON.stringify(mockProfileSuggestions)) }),
      synthesisProfilePromote: async (id) => {
        const suggestion = mockProfileSuggestions.find((item) => item.id === id);
        if (suggestion) {
          mockSynthesisProfile.interests.push({ id: suggestion.id, type: suggestion.type, name: suggestion.name, aliases: [], notes: suggestion.description, priority: mockSynthesisProfile.interests.length, enabled: true, linked_knowledge_ids: [suggestion.knowledge_id] });
          mockProfileSuggestions = mockProfileSuggestions.filter((item) => item.id !== id);
        }
        return { ok: true, profile: JSON.parse(JSON.stringify(mockSynthesisProfile)), suggestions: JSON.parse(JSON.stringify(mockProfileSuggestions)) };
      },
      synthesisProfileIgnore: async (id) => {
        mockProfileSuggestions = mockProfileSuggestions.filter((item) => item.id !== id);
        return { ok: true, suggestions: JSON.parse(JSON.stringify(mockProfileSuggestions)) };
      },
      synthesisSchedule: async () => ({ ok: true, schedule: JSON.parse(JSON.stringify(state.synthesisSchedule)) }),
      synthesisScheduleSave: async (patch) => {
        state.synthesisSchedule = {
          ...state.synthesisSchedule,
          ...patch,
        };
        state.synthesisSchedule.setup_completed = !!state.synthesisSchedule.setup_completed;
        state.synthesisSchedule.enabled = state.synthesisSchedule.setup_completed ? !!state.synthesisSchedule.enabled : false;
        state.synthesisSchedule.setup_pending = !state.synthesisSchedule.setup_completed;
        emitState();
        return { ok: true, schedule: JSON.parse(JSON.stringify(state.synthesisSchedule)) };
      },
      synthesisScheduleRunDue: async () => {
        state.synthesisSchedule.queued_dates = state.synthesisSchedule.due_dates.slice();
        emitState();
        return { ok: true, status: state.synthesisSchedule.queued_dates.length ? 'queued' : 'idle', schedule: JSON.parse(JSON.stringify(state.synthesisSchedule)) };
      },
      openCaptureDir: () => console.log('[mock] open capture dir'),
      openBriefingRunLogs: async (date) => ({ ok: true, path: `/mock/${date}/runs/latest` }),
      openShellLog: () => console.log('[mock] open shell log'),
      openPermissionSettings: async (permission) => ({ ok: true, permission }),
      quit: () => console.log('[mock] quit'),
      providerList: async () => ({ providers: providerProbe.providers }),
      providerTest: async (name) => {
        const provider = providerProbe.providers.find((p) => p.name === name);
        const ok = name === 'codex-cli' || name === 'ollama';
        if (provider) {
          provider.test = { provider: name, ok, status: ok ? 'available' : 'usage_limited', error: ok ? null : 'usage limit reached' };
          provider.ui = ok
            ? { level: 'green', status: 'available', reason: 'authenticated and returning tokens' }
            : { level: 'yellow', status: 'usage_limited', reason: 'usage limit reached' };
        }
        return { provider: name, ok, status: ok ? 'available' : 'usage_limited', summary: JSON.parse(JSON.stringify(providerProbe)) };
      },
      providerSetActive: async (name) => {
        providerProbe.configured = name;
        if (name === 'auto') {
          const fallback = providerProbe.providers.find((p) => p.enabled !== false && p.available && p.test && p.test.ok);
          providerProbe.auto_resolved = fallback ? fallback.name : null;
          providerProbe.providers.forEach((p) => { p.active = !!fallback && p.name === fallback.name; });
        } else {
          providerProbe.providers.forEach((p) => { p.active = p.name === name; });
        }
        return { ok: true, summary: JSON.parse(JSON.stringify(providerProbe)) };
      },
      providerSetup: async (name, action = null) => ({ ok: true, provider: name, action: action || 'inline' }),
      providerModels: async (name) => {
        const options = name === 'claude-cli'
          ? [{ value: '', label: 'CLI default' }, { value: 'sonnet', label: 'Sonnet' }, { value: 'opus', label: 'Opus' }]
          : (name === 'codex-cli'
          ? [{ value: '', label: 'CLI default' }, { value: 'gpt-5.4', label: 'gpt-5.4' }, { value: 'gpt-5.4-mini', label: 'GPT-5.4 Mini' }]
          : (name === 'anthropic-api'
            ? [{ value: 'claude-sonnet-4-6', label: 'claude-sonnet-4-6' }, { value: 'claude-opus-4-1', label: 'Claude Opus' }]
            : [{ value: 'deepseek-r1:70b', label: 'deepseek-r1:70b' }, { value: 'deepseek-r1:32b', label: 'deepseek-r1:32b' }]));
        const installable_options = name === 'ollama'
          ? [
            { value: 'gemma3', label: 'gemma3', detail: 'The current, most capable model that runs on a single GPU.', input_support: { text: true, image: true, audio: false }, provenance: 'ollama_library' },
            { value: 'llama3.2', label: 'llama3.2', detail: "Meta's Llama 3.2 goes small with 1B and 3B models.", input_support: { text: true, image: false, audio: false }, provenance: 'ollama_library' },
            { value: 'qwen3', label: 'qwen3', detail: 'Qwen3 is the latest generation of large language models in Qwen series, offering a comprehensive suite of dense and mixture-of-experts (MoE) models.', input_support: { text: true, image: false, audio: false }, provenance: 'ollama_library' },
          ]
          : [];
        const cliDefaultOptions = [{ value: '', label: 'CLI default' }];
        const options_by_modality = name === 'ollama'
          ? { text: options, image: [], audio: [] }
          : (name === 'claude-cli' || name === 'codex-cli'
            ? { text: options, image: cliDefaultOptions, audio: cliDefaultOptions }
            : { text: options, image: options, audio: [] });
        return {
          ok: true,
          provider: name,
          source: name === 'ollama' ? 'ollama' : 'mock',
          options,
          options_by_modality,
          installable_options,
        };
      },
      providerInstallModel: async (name, model) => {
        const provider = providerProbe.providers.find((p) => p.name === name);
        if (!provider) return { ok: false, error: 'unknown provider' };
        const field = (provider.config_fields || []).find((item) => item.key === 'text_model' || item.key === 'model');
        if (field && !field.options.some((option) => option.value === model)) {
          field.options.push({ value: model, label: model });
        }
        return {
          ok: true,
          provider: name,
          model,
          status: 'installed',
          models: {
            ok: true,
            provider: name,
            source: 'ollama',
            options: field ? field.options : [{ value: model, label: model }],
            options_by_modality: {
              text: field ? field.options : [{ value: model, label: model }],
              image: [],
              audio: [],
            },
            installable_options: [
              { value: 'gemma3', label: 'gemma3', detail: 'The current, most capable model that runs on a single GPU.', input_support: { text: true, image: true, audio: false }, provenance: 'ollama_library' },
              { value: 'llama3.2', label: 'llama3.2', detail: "Meta's Llama 3.2 goes small with 1B and 3B models.", input_support: { text: true, image: false, audio: false }, provenance: 'ollama_library' },
              { value: 'qwen3', label: 'qwen3', detail: 'Qwen3 is the latest generation of large language models in Qwen series, offering a comprehensive suite of dense and mixture-of-experts (MoE) models.', input_support: { text: true, image: false, audio: false }, provenance: 'ollama_library' },
            ],
          },
          summary: JSON.parse(JSON.stringify(providerProbe)),
        };
      },
      installWhisperModel: async (variant) => {
        const modelFile = `ggml-${variant || 'base.en'}.bin`;
        const audio = extensionState.connectors.find((connector) => connector.component_id === 'alvum.audio/audio');
        const processor = audio && audio.processor_controls && audio.processor_controls[0];
        if (processor) {
          const tokenSetting = processor.settings && processor.settings.find((item) => item.key === 'pyannote_hf_token');
          const tokenConfigured = !!(tokenSetting && tokenSetting.configured);
          processor.readiness = {
            status: tokenConfigured ? 'waiting_on_diarization_install' : 'requires_huggingface_access',
            level: 'warning',
            detail: tokenConfigured
              ? `Local Whisper model is installed at /Users/michael/.alvum/runtime/models/${modelFile}. Install Pyannote for speaker turns.`
              : 'Pyannote Community-1 requires Hugging Face access. Accept the model terms, then enter an HF token and retry.',
            action: { kind: 'install_pyannote', label: tokenConfigured ? 'Install' : 'Retry' },
          };
        }
        return { ok: true, model: 'whisper', variant: variant || 'base.en', status: 'present', connectors: JSON.parse(JSON.stringify(extensionState.connectors)) };
      },
      installPyannote: async () => {
        const audio = extensionState.connectors.find((connector) => connector.component_id === 'alvum.audio/audio');
        const processor = audio && audio.processor_controls && audio.processor_controls[0];
        const tokenSetting = processor && processor.settings && processor.settings.find((item) => item.key === 'pyannote_hf_token');
        if (!tokenSetting || !tokenSetting.configured) {
          return {
            ok: false,
            model: 'pyannote',
            variant: 'community-1',
            status: 'requires_huggingface_access',
            detail: 'Pyannote Community-1 requires Hugging Face access. Accept the model terms, then enter an HF token and retry.',
            error: 'Pyannote Community-1 requires Hugging Face access. Accept the model terms, then enter an HF token and retry.',
            connectors: JSON.parse(JSON.stringify(extensionState.connectors)),
          };
        }
        if (processor) {
          processor.readiness = {
            status: 'ready',
            level: 'ok',
            detail: 'Local Whisper and diarization are configured. Voice evidence uses /Users/michael/.alvum/runtime/speakers.json.',
            action: null,
          };
          const setting = processor.settings.find((item) => item.key === 'pyannote_command');
          if (setting) {
            setting.value = '/Users/michael/.alvum/runtime/pyannote/bin/alvum-pyannote';
            setting.value_label = 'alvum-pyannote';
          }
        }
        return { ok: true, model: 'pyannote', variant: 'community-1', status: 'present', connectors: JSON.parse(JSON.stringify(extensionState.connectors)) };
      },
      openPyannoteTerms: async () => ({ ok: true, url: 'https://huggingface.co/pyannote/speaker-diarization-community-1' }),
      providerConfigure: async (name, payload) => {
        const provider = providerProbe.providers.find((p) => p.name === name);
        if (!provider) return { ok: false, error: 'unknown provider' };
        if (payload && payload.enabled === true) provider.enabled = true;
        const fields = Array.isArray(provider.config_fields) ? provider.config_fields : [];
        for (const [key, value] of Object.entries((payload && payload.settings) || {})) {
          const field = fields.find((item) => item.key === key && !item.secret);
          if (field) {
            field.value = value;
            field.configured = String(value || '').trim() !== '';
          }
        }
        for (const key of Object.keys((payload && payload.secrets) || {})) {
          const field = fields.find((item) => item.key === key && item.secret);
          if (field) field.configured = true;
        }
        if (provider.name === 'anthropic-api' && fields.some((field) => field.key === 'api_key' && field.configured)) {
          provider.available = true;
          provider.ui = { level: 'yellow', status: 'not_checked', reason: 'not checked yet' };
        }
        return { ok: true, provider: name, summary: JSON.parse(JSON.stringify(providerProbe)) };
      },
      updateInstall: async () => {
        state.updateState.status = 'installing';
        emitState();
        return { ok: true, state: JSON.parse(JSON.stringify(state.updateState)) };
      },
      updateCheck: async () => {
        state.updateState.status = scenario === 'update' ? 'downloaded' : 'current';
        state.updateState.checkedAt = new Date().toISOString();
        emitState();
        return { ok: true, available: scenario === 'update', state: JSON.parse(JSON.stringify(state.updateState)) };
      },
      providerSetEnabled: async (name, enabled) => {
        const provider = providerProbe.providers.find((p) => p.name === name);
        if (!provider) return { ok: false, error: 'unknown provider' };
        provider.enabled = !!enabled;
        if (!enabled && provider.active) {
          provider.active = false;
          const fallback = providerProbe.providers.find((p) => p.enabled && p.available && p.test && p.test.ok);
          if (fallback) fallback.active = true;
        }
        return { ok: true, provider: name, enabled: provider.enabled };
      },
      connectorList: async () => JSON.parse(JSON.stringify(extensionState)),
      connectorSetEnabled: async (id, enabled) => {
        const ext = extensionState.connectors.find((item) => item.id === id);
        if (!ext) return { ok: false, error: 'unknown connector', connectors: extensionState.connectors };
        ext.enabled = !!enabled;
        if (Array.isArray(ext.source_controls)) {
          ext.source_controls.forEach((control) => {
            control.enabled = !!enabled;
            const input = state.captureInputs.inputs.find((item) => item.id === control.id);
            if (input) input.enabled = !!enabled;
          });
          ext.enabled_source_count = ext.source_controls.filter((control) => control.enabled).length;
          ext.source_count = ext.source_controls.length;
          ext.aggregate_state = ext.enabled_source_count === 0 ? 'all_off' : (ext.enabled_source_count === ext.source_count ? 'all_on' : 'partial');
          state.captureInputs.enabled = state.captureInputs.inputs.filter((item) => item.enabled).length;
        }
        return { ok: true, id, enabled: ext.enabled, connectors: JSON.parse(JSON.stringify(extensionState.connectors)) };
      },
      connectorProcessorSetSetting: async (component, key, value) => {
        for (const ext of extensionState.connectors) {
          const controls = Array.isArray(ext.processor_controls) ? ext.processor_controls : [];
          const control = controls.find((item) => item.component === component);
          if (!control) continue;
          const setting = Array.isArray(control.settings)
            ? control.settings.find((item) => item.key === key)
            : null;
          if (!setting) continue;
          if (setting.secret) {
            setting.value = null;
            setting.configured = String(value || '').trim() !== '';
            setting.value_label = setting.configured ? 'Configured' : 'Not configured';
            return { ok: true, component, key, value: null, connectors: JSON.parse(JSON.stringify(extensionState.connectors)) };
          }
          setting.value = String(value);
          const option = Array.isArray(setting.options)
            ? setting.options.find((item) => String(item.value) === String(value))
            : null;
          setting.value_label = option ? option.label : String(value);
          return { ok: true, component, key, value: setting.value, connectors: JSON.parse(JSON.stringify(extensionState.connectors)) };
        }
        return { ok: false, error: 'unknown processor setting', connectors: JSON.parse(JSON.stringify(extensionState.connectors)) };
      },
      speakerList: async () => JSON.parse(JSON.stringify(speakerState)),
      speakerSamples: async () => JSON.parse(JSON.stringify(speakerState)),
      speakerRename: async (id, label) => {
        const speaker = speakerState.speakers.find((item) => item.speaker_id === id);
        if (!speaker) return { ok: false, speakers: speakerState.speakers, error: 'unknown speaker' };
        speaker.label = String(label || '').trim() || null;
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerLink: async (id, interestId) => {
        const speaker = speakerState.speakers.find((item) => item.speaker_id === id);
        const interest = (mockSynthesisProfile.interests || []).find((item) => item.id === interestId && (item.type || item.interest_type) === 'person');
        if (!speaker) return { ok: false, speakers: speakerState.speakers, error: 'unknown speaker' };
        if (!interest) return { ok: false, speakers: speakerState.speakers, error: 'voice identity can only link to tracked people' };
        speaker.linked_interest_id = interest.id;
        speaker.linked_interest = { id: interest.id, type: 'person', name: interest.name || interest.id };
        speaker.label = interest.name || speaker.label || null;
        for (const sample of speakerState.samples || []) {
          if (sample.cluster_id === id) {
            sample.linked_interest_id = interest.id;
            sample.linked_interest = { id: interest.id, type: 'person', name: interest.name || interest.id };
            if (sample.assignment_source !== 'user_confirmed_sample') sample.assignment_source = 'user_linked_cluster';
          }
        }
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerLinkSample: async (sampleId, interestId) => {
        const sample = speakerState.samples.find((item) => item.sample_id === sampleId);
        const interest = (mockSynthesisProfile.interests || []).find((item) => item.id === interestId && (item.type || item.interest_type) === 'person');
        if (!sample) return { ok: false, samples: speakerState.samples, error: 'unknown sample' };
        if (!interest) return { ok: false, samples: speakerState.samples, error: 'voice identity can only link to tracked people' };
        sample.linked_interest_id = interest.id;
        sample.linked_interest = { id: interest.id, type: 'person', name: interest.name || interest.id };
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerMoveSample: async (sampleId, clusterId) => {
        const sample = speakerState.samples.find((item) => item.sample_id === sampleId);
        if (!sample) return { ok: false, samples: speakerState.samples, error: 'unknown sample' };
        if (clusterId === 'new') {
          const nextId = `spk_local_${sampleId}`;
          speakerState.speakers.push({ speaker_id: nextId, label: null, linked_interest_id: null, linked_interest: null, fingerprint_count: 1, samples: [], person_candidates: [], duplicate_candidates: [], context_interests: [] });
          sample.cluster_id = nextId;
          if (sample.assignment_source !== 'user_confirmed_sample') {
            sample.linked_interest_id = null;
            sample.linked_interest = null;
          }
        } else {
          const target = speakerState.speakers.find((item) => item.speaker_id === clusterId);
          sample.cluster_id = clusterId;
          if (sample.assignment_source !== 'user_confirmed_sample') {
            sample.linked_interest_id = target && target.linked_interest_id ? target.linked_interest_id : null;
            sample.linked_interest = target && target.linked_interest ? target.linked_interest : null;
          }
        }
        if (sample.assignment_source !== 'user_confirmed_sample') sample.assignment_source = 'user_moved_sample';
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerIgnoreSample: async (sampleId) => {
        const sample = speakerState.samples.find((item) => item.sample_id === sampleId);
        if (!sample) return { ok: false, samples: speakerState.samples, error: 'unknown sample' };
        sample.quality_flags = Array.isArray(sample.quality_flags) ? sample.quality_flags : [];
        if (!sample.quality_flags.includes('ignored_by_user')) sample.quality_flags.push('ignored_by_user');
        sample.assignment_source = 'user_ignored_sample';
        sample.linked_interest_id = null;
        sample.linked_interest = null;
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerSplit: async (clusterId, sampleIds) => {
        const nextId = `spk_local_split_${Date.now()}`;
        speakerState.speakers.push({ speaker_id: nextId, label: null, linked_interest_id: null, linked_interest: null, fingerprint_count: 1, samples: [], person_candidates: [], duplicate_candidates: [], context_interests: [] });
        for (const sample of speakerState.samples) {
          if (sample.cluster_id === clusterId && sampleIds.includes(sample.sample_id)) {
            sample.cluster_id = nextId;
            if (sample.assignment_source !== 'user_confirmed_sample') {
              sample.assignment_source = 'user_split_sample';
              sample.linked_interest_id = null;
              sample.linked_interest = null;
            }
          }
        }
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerRecluster: async () => JSON.parse(JSON.stringify(speakerState)),
      speakerUnlink: async (id) => {
        const speaker = speakerState.speakers.find((item) => item.speaker_id === id);
        if (!speaker) return { ok: false, speakers: speakerState.speakers, error: 'unknown speaker' };
        speaker.linked_interest_id = null;
        speaker.linked_interest = null;
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerMerge: async (sourceId, targetId) => {
        const source = speakerState.speakers.find((item) => item.speaker_id === sourceId);
        const target = speakerState.speakers.find((item) => item.speaker_id === targetId);
        if (!source || !target) return { ok: false, speakers: speakerState.speakers, error: 'unknown speaker' };
        target.fingerprint_count = Number(target.fingerprint_count || 0) + Number(source.fingerprint_count || 0);
        target.samples = (target.samples || []).concat(source.samples || []);
        if (!target.linked_interest_id && source.linked_interest_id) {
          target.linked_interest_id = source.linked_interest_id;
          target.linked_interest = source.linked_interest;
          target.label = source.label || target.label || null;
        }
        speakerState.speakers = speakerState.speakers.filter((item) => item.speaker_id !== sourceId);
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerForget: async (id) => {
        speakerState.speakers = speakerState.speakers.filter((item) => item.speaker_id !== id);
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerReset: async () => {
        speakerState.speakers = [];
        return JSON.parse(JSON.stringify(speakerState));
      },
      speakerSampleAudio: async (id, sampleIndex) => {
        const speaker = speakerState.speakers.find((item) => item.speaker_id === id);
        const index = Number(sampleIndex);
        const sample = speaker && Array.isArray(speaker.samples) && Number.isInteger(index)
          ? speaker.samples[index]
          : null;
        if (!sample || !sample.media_path) return { ok: false, error: 'sample audio is unavailable' };
        return {
          ok: true,
          url: `file://${sample.media_path}`,
          start_secs: sample.start_secs || 0,
          end_secs: sample.end_secs || 0,
          mime: sample.mime || null,
        };
      },
      voiceSampleAudio: async (sampleId) => {
        const sample = speakerState.samples.find((item) => item.sample_id === sampleId);
        if (!sample || !sample.media_path) return { ok: false, error: 'sample audio is unavailable' };
        return {
          ok: true,
          url: `file://${sample.media_path}`,
          start_secs: sample.start_secs || 0,
          end_secs: sample.end_secs || 0,
          mime: sample.mime || null,
        };
      },
      doctor: async () => JSON.parse(JSON.stringify(globalDoctor)),
      openExtensionsDir: () => {
        console.log('[mock] open extensions dir');
        return { ok: true, path: '/Users/michael/.alvum/runtime/extensions' };
      },
      logSnapshot: async (kind) => ({ kind, file: `/mock/${kind}.log`, text: `[mock] ${kind} log\nline 2\nline 3` }),
      resizePopover: () => {},
    };
    window.__initialMockView = ['logs', 'providers', 'extensions', 'briefing'].includes(scenario) ? scenario : 'main';

}
