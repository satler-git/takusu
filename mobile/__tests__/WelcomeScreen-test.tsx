jest.mock('react-native/Libraries/Animated/AnimatedExports', () => {
  const View = require('react-native/Libraries/Components/View/View').default;
  const noop = (cb: any) => {
    if (cb) cb({ finished: true });
  };
  const stoppable = { start: noop, stop: noop };

  return {
    __esModule: true,
    default: {
      Value: class {
        value: number;
        constructor(value: number) {
          this.value = value;
        }
        setValue(value: number) {
          this.value = value;
        }
      },
      View,
      timing: () => stoppable,
      sequence: (animations: Array<{ start: (cb: any) => void }>) => ({
        start: (cb: any) => {
          animations.forEach((a) => a.start(() => {}));
          noop(cb);
        },
        stop: noop,
      }),
      delay: () => stoppable,
    },
  };
});

jest.mock('react-native/Libraries/Animated/Easing', () => ({
  __esModule: true,
  default: {
    ease: (t: number) => t,
    inOut: (fn: (t: number) => number) => fn,
  },
}));

import { render } from '@testing-library/react-native';
import { WelcomeScreen } from '@/src/components/WelcomeScreen';

const themes = ['light', 'dark', 'catppuccin', 'aura-soft-dark'] as const;

describe('WelcomeScreen', () => {
  it.each(themes)('renders for the %s theme without crashing', (theme) => {
    const onFinished = jest.fn();
    expect(() =>
      render(
        <WelcomeScreen
          theme={theme}
          backgroundColor="#ffffff"
          onFinished={onFinished}
        />,
      ),
    ).not.toThrow();
  });
});
