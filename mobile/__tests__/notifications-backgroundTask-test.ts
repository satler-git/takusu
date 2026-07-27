jest.resetModules();

jest.doMock('expo-task-manager', () => ({
  defineTask: jest.fn(),
  isTaskRegisteredAsync: jest.fn(),
  unregisterTaskAsync: jest.fn(),
}));

jest.doMock('expo-notifications', () => ({
  registerTaskAsync: jest.fn(),
  unregisterTaskAsync: jest.fn(),
}));

jest.doMock('@sentry/react-native', () => ({
  withScope: jest.fn((cb: (scope: any) => void) =>
    cb({ setTag: jest.fn(), setExtra: jest.fn() }),
  ),
  captureException: jest.fn(),
}));

jest.doMock('@/src/api/settingsStore', () => ({
  loadSettings: jest.fn(),
}));

jest.doMock('@/src/api/server', () => ({
  ensureLocalServer: jest.fn(),
  waitForLocalServerReady: jest.fn().mockResolvedValue(undefined),
}));

jest.doMock('@/src/notifications/actionHandler', () => ({
  handleActionButtonResponse: jest.fn(),
  NOOP_HAPTIC: { medium: jest.fn(), success: jest.fn(), warning: jest.fn() },
}));

const TaskManager = require('expo-task-manager');
const Notifications = require('expo-notifications');
const Sentry = require('@sentry/react-native');
const { loadSettings } = require('@/src/api/settingsStore');
const {
  ensureLocalServer,
  waitForLocalServerReady,
} = require('@/src/api/server');
const {
  handleActionButtonResponse,
  NOOP_HAPTIC,
} = require('@/src/notifications/actionHandler');
const backgroundTask = require('@/src/notifications/backgroundTask');

const mockedDefineTask = TaskManager.defineTask as jest.Mock;
const mockedIsTaskRegisteredAsync =
  TaskManager.isTaskRegisteredAsync as jest.Mock;
const mockedRegisterTaskAsync = Notifications.registerTaskAsync as jest.Mock;
const mockedNotificationsUnregister =
  Notifications.unregisterTaskAsync as jest.Mock;

// Capture the defineTask call and executor at module-load time before any test clears mocks.
const defineTaskCall = mockedDefineTask.mock.calls[0];
const taskExecutor = defineTaskCall?.[1];

describe('isActionResponse', () => {
  it('returns true for a notification response payload', () => {
    expect(
      backgroundTask.isActionResponse({
        actionIdentifier: 'action_done',
        notification: { request: { identifier: 'n1', content: { data: {} } } },
      }),
    ).toBe(true);
  });

  it('returns false for a remote data payload', () => {
    expect(
      backgroundTask.isActionResponse({
        data: { dataString: '{}' },
        notification: null,
      }),
    ).toBe(false);
  });

  it('returns false for null', () => {
    expect(backgroundTask.isActionResponse(null)).toBe(false);
  });
});

describe('registerNotificationBackgroundTask', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it('registers the task when it is not already registered', async () => {
    mockedIsTaskRegisteredAsync.mockResolvedValue(false);
    await backgroundTask.registerNotificationBackgroundTask();
    expect(mockedIsTaskRegisteredAsync).toHaveBeenCalledWith(
      backgroundTask.BACKGROUND_NOTIFICATION_TASK,
    );
    expect(mockedRegisterTaskAsync).toHaveBeenCalledWith(
      backgroundTask.BACKGROUND_NOTIFICATION_TASK,
    );
  });

  it('does not register the task when already registered', async () => {
    mockedIsTaskRegisteredAsync.mockResolvedValue(true);
    await backgroundTask.registerNotificationBackgroundTask();
    expect(mockedRegisterTaskAsync).not.toHaveBeenCalled();
  });
});

describe('unregisterNotificationBackgroundTask', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it('unregisters the task when it is registered', async () => {
    mockedIsTaskRegisteredAsync.mockResolvedValue(true);
    await backgroundTask.unregisterNotificationBackgroundTask();
    expect(mockedNotificationsUnregister).toHaveBeenCalledWith(
      backgroundTask.BACKGROUND_NOTIFICATION_TASK,
    );
  });

  it('does not unregister the task when not registered', async () => {
    mockedIsTaskRegisteredAsync.mockResolvedValue(false);
    await backgroundTask.unregisterNotificationBackgroundTask();
    expect(mockedNotificationsUnregister).not.toHaveBeenCalled();
  });
});

it('defines a task with the expected name', () => {
  expect(defineTaskCall[0]).toBe(backgroundTask.BACKGROUND_NOTIFICATION_TASK);
  expect(typeof defineTaskCall[1]).toBe('function');
});

describe('background notification task executor', () => {
  const mockClient = { updateTask: jest.fn() };

  beforeEach(() => {
    jest.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    (console.warn as jest.Mock).mockRestore();
  });

  function makeResponse(actionId: string) {
    return {
      actionIdentifier: actionId,
      notification: {
        request: {
          identifier: 'notif-1',
          content: { title: '実行中: テスト', data: { taskId: 'task-1' } },
        },
      },
    };
  }

  beforeEach(() => {
    jest.clearAllMocks();
    loadSettings.mockResolvedValue({
      workersUrl: 'https://example.com',
      workersToken: 'token',
      notifications: { inProgress: true },
    });
    ensureLocalServer.mockReturnValue(mockClient);
    handleActionButtonResponse.mockResolvedValue(undefined);
  });

  it('processes a known action response', async () => {
    const response = makeResponse('action_done');
    await taskExecutor({ data: response });
    expect(ensureLocalServer).toHaveBeenCalledWith({
      workersUrl: 'https://example.com',
      rootToken: 'token',
    });
    expect(waitForLocalServerReady).toHaveBeenCalledWith(mockClient, {
      maxWaitMs: 3000,
    });
    expect(handleActionButtonResponse).toHaveBeenCalledWith(
      response,
      expect.objectContaining({
        client: mockClient,
        inProgressNotifications: true,
        haptic: NOOP_HAPTIC,
      }),
    );
  });

  it('ignores an unknown action response', async () => {
    await taskExecutor({ data: makeResponse('unknown') });
    expect(ensureLocalServer).not.toHaveBeenCalled();
    expect(handleActionButtonResponse).not.toHaveBeenCalled();
  });

  it('ignores a non-action payload', async () => {
    await taskExecutor({
      data: { data: { dataString: '{}' }, notification: null },
    });
    expect(ensureLocalServer).not.toHaveBeenCalled();
    expect(handleActionButtonResponse).not.toHaveBeenCalled();
  });

  it('does nothing when workersUrl or workersToken is missing', async () => {
    loadSettings.mockResolvedValue({
      workersUrl: '',
      workersToken: '',
      notifications: { inProgress: true },
    });
    await taskExecutor({ data: makeResponse('action_done') });
    expect(ensureLocalServer).not.toHaveBeenCalled();
    expect(handleActionButtonResponse).not.toHaveBeenCalled();
  });

  it('captures an error when ensureLocalServer fails', async () => {
    const error = new Error('server failed');
    ensureLocalServer.mockImplementation(() => {
      throw error;
    });
    await taskExecutor({ data: makeResponse('action_done') });
    expect(handleActionButtonResponse).not.toHaveBeenCalled();
    expect(Sentry.captureException).toHaveBeenCalledWith(error);
  });
});
