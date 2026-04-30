export type ViewDirection = 'forward' | 'back' | 'replace';

export interface CaptureInput {
  id: string;
  label?: string;
  kind?: string;
  enabled?: boolean;
  detail?: string;
  settings?: Record<string, unknown>;
  blocked_permissions?: PermissionIssue[];
}

export interface PermissionIssue {
  permission?: string;
  label?: string;
  status?: string;
  source_id?: string;
  source_label?: string;
}

export interface BriefingCalendarDay {
  date: string;
  inMonth?: boolean;
  isToday?: boolean;
  hasCapture?: boolean;
  hasBriefing?: boolean;
  status?: string;
  artifacts?: string;
  failure?: { reason?: string } | null;
}

export interface BriefingCalendar {
  month: string;
  label?: string;
  today?: string;
  days: BriefingCalendarDay[];
}

export interface BriefingProgress {
  briefingDate?: string;
  stage?: string;
  current?: number;
  total?: number;
  [key: string]: unknown;
}

export interface PipelineEvent {
  kind?: string;
  briefingDate?: string;
  provider?: string;
  stage?: string;
  ts?: string;
  [key: string]: unknown;
}

export interface ProviderConfigField {
  key: string;
  label?: string;
  kind?: string;
  secret?: boolean;
  configured?: boolean;
  value?: unknown;
  placeholder?: string;
  detail?: string;
  options?: Array<{ value: unknown; label?: string; detail?: string }>;
}

export interface ProviderSummaryItem {
  name: string;
  display_name?: string;
  enabled?: boolean;
  active?: boolean;
  available?: boolean;
  setup_kind?: string;
  setup_label?: string;
  setup_command?: string;
  setup_url?: string;
  setup_hint?: string;
  auth_hint?: string;
  usage?: number | null;
  test?: Record<string, unknown> | null;
  ui?: { level?: string; status?: string; reason?: string };
  config_fields?: ProviderConfigField[];
  installable_models?: Array<{ value: string; label?: string; detail?: string }>;
  [key: string]: unknown;
}

export interface ProviderSummary {
  providers?: ProviderSummaryItem[];
  configured?: string;
  auto_resolved?: string | null;
  error?: string;
  [key: string]: unknown;
}

export interface ConnectorControl {
  id?: string;
  component?: string;
  label?: string;
  kind?: string;
  enabled?: boolean;
  toggleable?: boolean;
  detail?: string;
  settings?: Array<Record<string, unknown>>;
  blocked_permissions?: PermissionIssue[];
}

export interface ConnectorSummaryItem {
  id: string;
  component_id?: string;
  package_id?: string;
  connector_id?: string;
  kind?: string;
  package_name?: string;
  display_name?: string;
  description?: string;
  version?: string;
  enabled?: boolean;
  read_only?: boolean;
  aggregate_state?: string;
  source_controls?: ConnectorControl[];
  processor_controls?: ConnectorControl[];
  permission_issues?: PermissionIssue[];
  [key: string]: unknown;
}

export interface ConnectorSummary {
  connectors?: ConnectorSummaryItem[];
  error?: string;
  [key: string]: unknown;
}

export interface DecisionGraphData {
  ok?: boolean;
  date?: string;
  decisions?: Array<Record<string, unknown>>;
  edges?: Array<Record<string, unknown>>;
  domains?: string[];
  derived_edges?: number;
  [key: string]: unknown;
}

export interface SynthesisProfile {
  intentions?: Array<Record<string, unknown>>;
  domains?: Array<Record<string, unknown>>;
  interests?: Array<Record<string, unknown>>;
  writing?: Record<string, unknown>;
  advanced_instructions?: string;
  ignored_suggestions?: string[];
  [key: string]: unknown;
}

export interface SynthesisSchedule {
  enabled?: boolean;
  time?: string;
  policy?: string;
  setup_completed?: boolean;
  setup_pending?: boolean;
  due_dates?: string[];
  queued_dates?: string[];
  running_date?: string | null;
  last_error?: string | null;
  [key: string]: unknown;
}

export interface PopoverState {
  captureRunning?: boolean;
  captureStartedAt?: string | null;
  briefingRunning?: boolean;
  briefingRuns?: Record<string, Record<string, unknown>>;
  briefingCatchupPending?: number;
  briefingCatchupDates?: string[];
  captureStats?: Record<string, unknown>;
  captureInputs?: { inputs?: CaptureInput[]; [key: string]: unknown };
  permissions?: Record<string, unknown>;
  stats?: string;
  latestBriefing?: Record<string, unknown> | null;
  briefingTargets?: Array<Record<string, unknown>>;
  briefingCalendar?: BriefingCalendar;
  providerSummary?: ProviderSummary;
  providerStats?: Record<string, unknown>;
  providerIssue?: { level?: string; message?: string };
  synthesisSchedule?: SynthesisSchedule | null;
  updateState?: Record<string, unknown>;
}

export interface AlvumApi {
  onState(cb: (state: PopoverState) => void): void;
  onProgress(cb: (progress: BriefingProgress) => void): void;
  onEvent(cb: (event: PipelineEvent) => void): void;
  onPopoverShow(cb: () => void): void;
  requestState(): void;
  resizePopover?(height: number): void;
  toggleCapture(): void;
  captureInputs(): Promise<unknown>;
  toggleCaptureInput(id: string): Promise<unknown>;
  captureInputSetSetting(id: string, key: string, value: unknown): Promise<unknown>;
  chooseDirectory(defaultPath?: string): Promise<unknown>;
  startBriefing(): void;
  startBriefingDate(date: string): Promise<unknown>;
  briefingCalendarMonth(month?: string): Promise<BriefingCalendar>;
  openBriefing(): void;
  openBriefingDate(date: string): Promise<unknown>;
  readBriefingDate(date: string): Promise<unknown>;
  briefingRunLogDate(date: string): Promise<unknown>;
  openBriefingRunLogs(date: string): Promise<unknown>;
  decisionGraphDate(date: string): Promise<DecisionGraphData>;
  synthesisProfile(): Promise<unknown>;
  synthesisProfileSave(profile: SynthesisProfile): Promise<unknown>;
  synthesisProfileSuggestions(): Promise<unknown>;
  synthesisProfilePromote(id: string): Promise<unknown>;
  synthesisProfileIgnore(id: string): Promise<unknown>;
  synthesisSchedule(): Promise<unknown>;
  synthesisScheduleSave(patch: SynthesisSchedule): Promise<unknown>;
  synthesisScheduleRunDue(): Promise<unknown>;
  openBriefingLog(): void;
  openCaptureDir(): void;
  openShellLog(): void;
  openPermissionSettings(permission?: string): Promise<unknown>;
  quit(): void;
  providerList(): Promise<ProviderSummary>;
  providerTest(name: string): Promise<unknown>;
  providerSetActive(name: string): Promise<unknown>;
  providerSetEnabled(name: string, enabled: boolean): Promise<unknown>;
  providerConfigure(name: string, payload: Record<string, unknown>): Promise<unknown>;
  providerModels(name: string): Promise<unknown>;
  providerInstallModel(name: string, model: string): Promise<unknown>;
  installWhisperModel(): Promise<unknown>;
  providerSetup(name: string, action?: string | null): Promise<unknown>;
  updateCheck(): Promise<unknown>;
  updateInstall(): Promise<unknown>;
  logSnapshot(kind: string): Promise<unknown>;
  extensionList(): Promise<unknown>;
  extensionSetEnabled(id: string, enabled: boolean): Promise<unknown>;
  extensionDoctor(): Promise<unknown>;
  openExtensionsDir(): Promise<unknown>;
  connectorList(): Promise<ConnectorSummary>;
  connectorSetEnabled(id: string, enabled: boolean): Promise<unknown>;
  connectorProcessorSetSetting(component: string, key: string, value: unknown): Promise<unknown>;
  doctor(): Promise<unknown>;
}
