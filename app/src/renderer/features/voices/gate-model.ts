import type {
  VoiceGateConnector,
  VoiceGateProfile,
  VoiceGateSetting,
  VoiceSampleLike,
  VoiceGateSummary,
} from './types';
import { isIgnoredVoiceSample, sampleDay } from './timeline-model';

export function voiceGateSummary(
  connectorSummary: { connectors?: VoiceGateConnector[] } | null | undefined,
  synthesisProfile: VoiceGateProfile | null | undefined,
  samples: VoiceSampleLike[] = [],
): VoiceGateSummary {
  const settings = audioProcessorSettings(connectorSummary);
  const mode = stringSetting(settings, 'mode') || 'local';
  const diarizationReady = mode === 'provider'
    ? true
    : mode === 'local' && booleanSetting(settings, 'diarization_enabled', true);
  const people = enabledTrackedPeople(synthesisProfile);
  const reviewable = samples.filter((sample) => sample && !isIgnoredVoiceSample(sample));
  const linkedIds = new Set(
    reviewable
      .map((sample) => sample.linked_interest_id)
      .filter(Boolean)
      .map(String),
  );
  const recentEvidenceDay = reviewable
    .map(sampleDay)
    .filter(Boolean)
    .sort((a, b) => String(b).localeCompare(String(a)))[0] || null;
  return {
    visible: mode !== 'off' && diarizationReady && people.length > 0,
    mode,
    diarizationReady,
    enabledPeople: people.length,
    pendingReviewCount: reviewable.filter((sample) => !sample.linked_interest_id).length,
    linkedPersonCount: linkedIds.size,
    recentEvidenceDay,
  };
}

export function enabledTrackedPeople(profile: VoiceGateProfile | null | undefined): Array<Record<string, unknown>> {
  return (Array.isArray(profile && profile.interests) ? profile!.interests! : [])
    .filter((interest) => interest && interest.enabled !== false)
    .filter((interest) => String(interest.type || interest.interest_type || '').toLowerCase() === 'person');
}

function audioProcessorSettings(
  connectorSummary: { connectors?: VoiceGateConnector[] } | null | undefined,
): VoiceGateSetting[] {
  const connectors = Array.isArray(connectorSummary && connectorSummary.connectors)
    ? connectorSummary!.connectors!
    : [];
  const audio = connectors.find((connector) => connector && (
    connector.component_id === 'alvum.audio/audio'
    || (connector.package_id === 'alvum.audio' && connector.connector_id === 'audio')
    || connector.id === 'alvum.audio/audio'
  ));
  const processors = audio && Array.isArray(audio.processor_controls) ? audio.processor_controls : [];
  const processor = processors.find((control) => control && control.component === 'alvum.audio/whisper');
  return processor && Array.isArray(processor.settings) ? processor.settings : [];
}

function stringSetting(settings: VoiceGateSetting[], key: string): string {
  const setting = settings.find((item) => item && item.key === key);
  if (!setting || setting.value == null) return '';
  return String(setting.value || '').trim();
}

function booleanSetting(settings: VoiceGateSetting[], key: string, fallback: boolean): boolean {
  const setting = settings.find((item) => item && item.key === key);
  if (!setting || setting.value == null) return fallback;
  if (typeof setting.value === 'boolean') return setting.value;
  const normalized = String(setting.value).trim().toLowerCase();
  if (normalized === 'true' || normalized === '1' || normalized === 'on' || normalized === 'yes') return true;
  if (normalized === 'false' || normalized === '0' || normalized === 'off' || normalized === 'no') return false;
  return fallback;
}
