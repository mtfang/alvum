import type { AlvumApi } from './types';

declare global {
  interface Window {
    alvum?: AlvumApi;
    __initialMockView?: string;
  }
}

export function getAlvumApi(): AlvumApi {
  if (!window.alvum) {
    throw new Error('window.alvum bridge is not available');
  }
  return window.alvum;
}
