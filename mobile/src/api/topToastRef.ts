import type { TopToastContextValue } from '@/src/components/TopToast';

// Escape hatch so non-React code (showError) can imperatively show toasts.
// This mirrors the pattern used by FloatingVoiceButton. If multiple
// TopToastProviders are mounted, the last one to mount wins.
let topToastRef: TopToastContextValue | null = null;

export function setTopToastRef(ref: TopToastContextValue | null): void {
  topToastRef = ref;
}

export function getTopToastRef(): TopToastContextValue | null {
  return topToastRef;
}
