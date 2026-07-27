import type { TopToastContextValue } from '@/src/components/TopToast';

let topToastRef: TopToastContextValue | null = null;

export function setTopToastRef(ref: TopToastContextValue | null): void {
  topToastRef = ref;
}

export function getTopToastRef(): TopToastContextValue | null {
  return topToastRef;
}
