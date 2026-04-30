import { createViewFeature } from '../shell/feature-module';
import type { FeatureModule, ViewHandler, ViewName } from '../shell/contracts';

export function createLogsFeature(handlers: Record<ViewName, ViewHandler>): FeatureModule {
  return createViewFeature('logs', handlers);
}
