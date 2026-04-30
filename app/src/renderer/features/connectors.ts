import { createViewFeature } from '../shell/feature-module';
import type { FeatureModule, ViewHandler, ViewName } from '../shell/contracts';

export function createConnectorsFeature(handlers: Record<ViewName, ViewHandler>): FeatureModule {
  return createViewFeature('connectors', handlers);
}
