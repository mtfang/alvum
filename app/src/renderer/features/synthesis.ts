import { createViewFeature } from '../shell/feature-module';
import type { FeatureModule, ViewHandler, ViewName } from '../shell/contracts';

export function createSynthesisFeature(handlers: Record<ViewName, ViewHandler>): FeatureModule {
  return createViewFeature('synthesis', handlers);
}
