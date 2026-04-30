import { createViewFeature } from '../shell/feature-module';
import type { FeatureModule, ViewHandler, ViewName } from '../shell/contracts';

export function createProfileFeature(handlers: Record<ViewName, ViewHandler>): FeatureModule {
  return createViewFeature('profile', handlers);
}
