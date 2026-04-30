import type { FeatureModule, ViewHandler, ViewName } from './contracts';

export function createViewFeature(name: string, handlers: Record<ViewName, ViewHandler>): FeatureModule {
  return {
    init(ctx) {
      for (const [view, handler] of Object.entries(handlers)) {
        ctx.router.registerViewHandler(view, handler);
      }
    },
  };
}
