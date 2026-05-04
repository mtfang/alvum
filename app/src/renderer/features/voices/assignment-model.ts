import type { VoiceGateProfile, VoiceSampleLike } from './types';
import { enabledTrackedPeople } from './gate-model';

export function voiceTimelineActionsForSample(
  sample: VoiceSampleLike,
  interests: Array<Record<string, unknown>>,
) {
  const assignmentTargets = enabledTrackedPeople({ interests });
  return {
    canAssign: assignmentTargets.length > 0,
    assignmentTargets,
    contextEvidence: Array.isArray(sample && sample.context_interests) ? sample.context_interests! : [],
  };
}

export function voiceAssignmentLabel(sample: VoiceSampleLike | null | undefined): string {
  if (sample && sample.linked_interest && sample.linked_interest.name) return String(sample.linked_interest.name);
  if (sample && sample.linked_interest_id) return String(sample.linked_interest_id);
  const candidate = suggestedVoiceAssignment(sample);
  if (candidate && (candidate.name || candidate.id)) return `${candidate.name || candidate.id}?`;
  return 'Unassigned';
}

export function voiceAssignmentDetail(sample: VoiceSampleLike | null | undefined): string {
  const candidate = suggestedVoiceAssignment(sample);
  if (!candidate || (sample && sample.linked_interest_id)) return '';
  return `${candidate.name || candidate.id} · ${candidateMatchLabel(candidate)} · ${candidateEvidenceDetail(candidate) || 'voice fingerprint match'}`;
}

export function suggestedVoiceAssignment(sample: VoiceSampleLike | null | undefined): Record<string, unknown> | null {
  const candidate = Array.isArray(sample && sample.person_candidates) ? sample!.person_candidates![0] : null;
  return candidate && candidate.id ? candidate : null;
}

export function candidateScore(candidate: Record<string, unknown> | null | undefined): string {
  const score = Number(candidate && candidate.score);
  if (!Number.isFinite(score)) return '';
  return `${Math.round(score * 100)}%`;
}

export function candidateMatchLabel(candidate: Record<string, unknown> | null | undefined): string {
  const confidence = String(candidate && candidate.voice_model_confidence || '').toLowerCase();
  if (confidence === 'high') return 'High confidence voice match';
  if (confidence === 'medium') return 'Medium confidence voice match';
  if (confidence === 'low') return 'Low confidence voice match';
  const score = Number(candidate && candidate.score);
  if (!Number.isFinite(score)) return 'Voice match';
  if (score >= 0.85) return 'Strong voice match';
  if (score >= 0.70) return 'Possible voice match';
  return 'Weak voice match';
}

export function candidateEvidenceDetail(candidate: Record<string, unknown> | null | undefined): string {
  const pieces = [];
  const support = Number(candidate && (candidate.verified_sample_count || candidate.support_count));
  if (Number.isFinite(support) && support > 0) {
    pieces.push(`${support} verified sample${support === 1 ? '' : 's'}`);
  }
  const sources = Number(candidate && candidate.source_count);
  if (Number.isFinite(sources) && sources > 0) {
    pieces.push(`${sources} source${sources === 1 ? '' : 's'}`);
  }
  const accuracy = Number(candidate && candidate.holdout_accuracy);
  if (Number.isFinite(accuracy)) {
    pieces.push(`${Math.round(accuracy * 100)}% holdout`);
  }
  const margin = Number(candidate && candidate.holdout_margin);
  if (Number.isFinite(margin)) {
    pieces.push(`${Math.round(margin * 100)}pt margin`);
  }
  const radius = Number(candidate && candidate.confidence_radius);
  if (Number.isFinite(radius)) {
    if (radius <= 0.12) pieces.push('tight voice model');
    else if (radius <= 0.25) pieces.push('moderate voice model');
    else pieces.push('broad voice model');
  }
  if (candidate && candidate.auto_predict === true) pieces.push('auto-predict ready');
  if (candidate && candidate.reason) pieces.push(String(candidate.reason));
  return pieces.join(' · ');
}

export function voiceAssignmentConfidenceLabel(confidenceValue: unknown, scoreValue: unknown = null): string {
  const confidence = String(confidenceValue || '').toLowerCase();
  if (confidence === 'high') return 'High confidence';
  if (confidence === 'medium' || confidence === 'med') return 'Medium confidence';
  if (confidence === 'low') return 'Low confidence';
  const score = Number(scoreValue);
  if (!Number.isFinite(score)) return '';
  if (score >= 0.85) return 'High confidence';
  if (score >= 0.70) return 'Medium confidence';
  return 'Low confidence';
}

export function voiceModelForInterest(
  interestId: unknown,
  voiceModels: Array<Record<string, unknown>> | null | undefined,
): Record<string, unknown> | null {
  const id = String(interestId || '');
  if (!id || !Array.isArray(voiceModels)) return null;
  return voiceModels.find((model) => String(model && model.linked_interest && (model.linked_interest as Record<string, unknown>).id || '') === id) || null;
}

export function voiceCandidateForInterest(
  sample: VoiceSampleLike | null | undefined,
  interestId: unknown,
): Record<string, unknown> | null {
  const id = String(interestId || '');
  if (!id || !Array.isArray(sample && sample.person_candidates)) return null;
  return sample!.person_candidates!.find((candidate) => String(candidate && candidate.id || '') === id) || null;
}

export function voiceAssignmentEvidenceForPerson(
  sample: VoiceSampleLike | null | undefined,
  person: Record<string, unknown> | null | undefined,
  voiceModels: Array<Record<string, unknown>> | null | undefined = [],
): string {
  const personId = person && person.id;
  const candidate = voiceCandidateForInterest(sample, personId);
  if (candidate) {
    return voiceAssignmentConfidenceLabel(candidate.voice_model_confidence, candidate.score);
  }
  const model = voiceModelForInterest(personId, voiceModels);
  if (!model) return '';
  return voiceAssignmentConfidenceLabel(model.confidence);
}

export function assignmentTargets(profile: VoiceGateProfile | null | undefined): Array<Record<string, unknown>> {
  return enabledTrackedPeople(profile);
}
