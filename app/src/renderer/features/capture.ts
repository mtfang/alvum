import { createViewFeature } from '../shell/feature-module';
import type { FeatureModule, ViewHandler, ViewName } from '../shell/contracts';

export function createCaptureFeature(handlers: Record<ViewName, ViewHandler>): FeatureModule {
  return createViewFeature('capture', handlers);
}
