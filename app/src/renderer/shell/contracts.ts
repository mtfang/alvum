import type { AlvumApi, BriefingProgress, PipelineEvent, PopoverState, ViewDirection } from '../api/types';

export type ViewName = string;
export type ViewHandler = () => void | Promise<void>;

export interface AppContext {
  api: AlvumApi;
  state: Record<string, unknown>;
  router: {
    activeView(): ViewName;
    setView(view: ViewName, direction?: ViewDirection): void;
    parentViewFor(view: ViewName): ViewName;
    registerViewHandler(view: ViewName, handler: ViewHandler): void;
  };
  notify: {
    show(text: string | null, level?: 'info' | 'warning' | 'error', heading?: string | null): void;
  };
  layout: {
    requestResize(height?: number): void;
  };
  dom: {
    $(id: string): HTMLElement;
  };
}

export interface FeatureModule {
  init(ctx: AppContext): void;
  onViewEnter?(view: ViewName): void | Promise<void>;
  onState?(state: PopoverState): void;
  onProgress?(progress: BriefingProgress): void;
  onEvent?(event: PipelineEvent): void;
}
