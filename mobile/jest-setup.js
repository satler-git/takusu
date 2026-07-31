// Jest setup for React Native Reanimated 4 / react-native-worklets.
// The package mocks need a populated native proxy and the new-arch flag
// before they can load in a Node test environment.

// This global is the exact name Reanimated's native module looks for.
// eslint-disable-next-line no-underscore-dangle
global.__reanimatedModuleProxy = new Proxy(
  {},
  {
    get(_target, prop) {
      if (prop === 'getStaticFeatureFlag') {
        return () => false;
      }
      if (prop === 'getViewProp') {
        return () => null;
      }
      if (
        prop === 'registerSensor' ||
        prop === 'registerEventHandler' ||
        prop === 'subscribeForKeyboardEvents'
      ) {
        return () => -1;
      }
      if (prop === 'getSettledUpdates') {
        return () => [];
      }
      return () => {};
    },
  },
);

global.RN$registerCallableModule = jest.fn();
global.RN$Bridgeless = true;

jest.mock('expo-haptics', () => ({
  impactAsync: jest.fn(() => Promise.resolve()),
  selectionAsync: jest.fn(() => Promise.resolve()),
  notificationAsync: jest.fn(() => Promise.resolve()),
  ImpactFeedbackStyle: { Light: 'light', Medium: 'medium', Heavy: 'heavy' },
  NotificationFeedbackType: {
    Success: 'success',
    Warning: 'warning',
    Error: 'error',
  },
}));

jest.mock('react-native-worklets', () => {
  const mock = require('react-native-worklets/lib/module/mock.js');
  return mock;
});

jest.mock('react-native-reanimated', () => {
  const mock = require('react-native-reanimated/mock');
  return mock;
});
