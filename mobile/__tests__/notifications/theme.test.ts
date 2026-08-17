jest.mock('@react-native-async-storage/async-storage', () => ({
  __esModule: true,
  default: {
    getItem: jest.fn(),
    setItem: jest.fn(),
    removeItem: jest.fn(),
  },
}));

import AsyncStorage from '@react-native-async-storage/async-storage';
import { Appearance } from 'react-native';
import {
  notificationColorForTheme,
  getNotificationIconColor,
} from '@/src/notifications/theme';
import {
  COLORS,
  DARK_COLORS,
  CATPPUCCIN_COLORS,
  AURA_SOFT_DARK_COLORS,
} from '@/src/theme';

const asyncStorageGetItem = AsyncStorage.getItem as jest.Mock;

beforeEach(() => {
  asyncStorageGetItem.mockReset();
  jest.spyOn(Appearance, 'getColorScheme').mockReturnValue(null);
});

afterEach(() => {
  jest.restoreAllMocks();
});

describe('notificationColorForTheme', () => {
  it('uses the light brand color for light theme', () => {
    expect(notificationColorForTheme('light')).toBe(COLORS.brand);
  });

  it('uses the dark brand light color for dark theme', () => {
    expect(notificationColorForTheme('dark')).toBe(DARK_COLORS.brandLight);
  });

  it('uses the catppuccin brand light color for catppuccin theme', () => {
    expect(notificationColorForTheme('catppuccin')).toBe(
      CATPPUCCIN_COLORS.brandLight,
    );
  });

  it('uses the aura soft dark brand light color for aura-soft-dark theme', () => {
    expect(notificationColorForTheme('aura-soft-dark')).toBe(
      AURA_SOFT_DARK_COLORS.brandLight,
    );
  });
});

describe('getNotificationIconColor', () => {
  it('uses the stored explicit theme', async () => {
    asyncStorageGetItem.mockImplementation((key: string) =>
      Promise.resolve(
        key === 'takusu.theme'
          ? 'dark'
          : key === 'takusu.darkMode'
            ? null
            : null,
      ),
    );
    expect(await getNotificationIconColor()).toBe(DARK_COLORS.brandLight);
  });

  it('falls back to the system dark color scheme', async () => {
    asyncStorageGetItem.mockResolvedValue(null);
    jest.spyOn(Appearance, 'getColorScheme').mockReturnValue('dark');
    expect(await getNotificationIconColor()).toBe(DARK_COLORS.brandLight);
  });

  it('falls back to the system light color scheme', async () => {
    asyncStorageGetItem.mockResolvedValue(null);
    jest.spyOn(Appearance, 'getColorScheme').mockReturnValue('light');
    expect(await getNotificationIconColor()).toBe(COLORS.brand);
  });

  it('migrates the legacy darkMode key', async () => {
    asyncStorageGetItem.mockImplementation((key: string) =>
      Promise.resolve(
        key === 'takusu.theme'
          ? null
          : key === 'takusu.darkMode'
            ? 'true'
            : null,
      ),
    );
    expect(await getNotificationIconColor()).toBe(DARK_COLORS.brandLight);
  });
});
