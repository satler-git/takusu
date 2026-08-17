jest.resetModules();

jest.doMock('expo-notifications', () => ({}));

jest.doMock('@/src/notifications/scheduler', () => ({
  postInProgressNotification: jest.fn(),
  dismissInProgressNotification: jest.fn(),
  dismissTaskNotifications: jest.fn(),
  cancelScheduledTaskNotifications: jest.fn(),
  cancelScheduledStartNotifications: jest.fn(),
  postResultNotification: jest.fn(),
}));

jest.doMock('@sentry/react-native', () => ({
  withScope: jest.fn((cb: (scope: any) => void) =>
    cb({ setTag: jest.fn(), setExtra: jest.fn() }),
  ),
  captureException: jest.fn(),
}));

jest.doMock('@/src/components/haptics', () => ({
  haptic: {
    medium: jest.fn(),
    success: jest.fn(),
    warning: jest.fn(),
  },
}));

const Sentry = require('@sentry/react-native');
const {
  postInProgressNotification,
  dismissInProgressNotification,
  dismissTaskNotifications,
  cancelScheduledTaskNotifications,
  cancelScheduledStartNotifications,
  postResultNotification,
} = require('@/src/notifications/scheduler');
const {
  handleActionButtonResponse,
} = require('@/src/notifications/actionHandler');
const {
  ACTION_START,
  ACTION_DONE,
  ACTION_CANCEL,
} = require('@/src/notifications/categories');

const mockUpdateTask = jest.fn();
const mockGetTask = jest.fn();

function makeResponse(actionId: string, overrides: any = {}) {
  return {
    actionIdentifier: actionId,
    notification: {
      request: {
        identifier: 'notif-1',
        content: {
          title: '実行中: テストタスク',
          data: { taskId: 'task-1' },
        },
      },
    },
    ...overrides,
  };
}

describe('handleActionButtonResponse', () => {
  const haptic = {
    medium: jest.fn(),
    success: jest.fn(),
    warning: jest.fn(),
  };

  beforeEach(() => {
    jest.spyOn(console, 'warn').mockImplementation(() => {});
    jest.clearAllMocks();
    mockUpdateTask.mockResolvedValue(undefined);
    mockGetTask.mockResolvedValue({ id: 'task-1', title: 'テストタスク' });
  });

  afterEach(() => {
    (console.warn as jest.Mock).mockRestore();
  });

  function makeClient(): any {
    return {
      updateTask: mockUpdateTask,
      getTask: mockGetTask,
    };
  }

  it('returns false for an unknown action', async () => {
    const response = makeResponse('unknown');
    const result = await handleActionButtonResponse(response, {
      client: makeClient(),
      inProgressNotifications: true,
      haptic,
    });
    expect(result).toBe(false);
    expect(mockUpdateTask).not.toHaveBeenCalled();
  });

  it('handles START action and posts in-progress notification when enabled', async () => {
    const response = makeResponse(ACTION_START);
    const result = await handleActionButtonResponse(response, {
      client: makeClient(),
      inProgressNotifications: true,
      haptic,
    });
    expect(result).toBe(true);
    expect(haptic.medium).toHaveBeenCalled();
    expect(mockUpdateTask).toHaveBeenCalledWith('task-1', {
      status: 'in_progress',
    });
    expect(dismissTaskNotifications).toHaveBeenCalledWith('task-1');
    expect(cancelScheduledStartNotifications).toHaveBeenCalledWith('task-1');
    expect(mockGetTask).toHaveBeenCalledWith('task-1');
    expect(postInProgressNotification).toHaveBeenCalledWith(
      {
        id: 'task-1',
        title: 'テストタスク',
      },
      undefined,
    );
  });

  it('handles START action without posting in-progress notification when disabled', async () => {
    const response = makeResponse(ACTION_START);
    const result = await handleActionButtonResponse(response, {
      client: makeClient(),
      inProgressNotifications: false,
      haptic,
    });
    expect(result).toBe(true);
    expect(mockUpdateTask).toHaveBeenCalledWith('task-1', {
      status: 'in_progress',
    });
    expect(mockGetTask).not.toHaveBeenCalled();
    expect(postInProgressNotification).not.toHaveBeenCalled();
  });

  it('handles START action with missing taskId by returning true without calling updateTask', async () => {
    const response = makeResponse(ACTION_START, {
      notification: {
        request: {
          identifier: 'notif-1',
          content: { title: '', data: {} },
        },
      },
    });
    const result = await handleActionButtonResponse(response, {
      client: makeClient(),
      inProgressNotifications: true,
      haptic,
    });
    expect(result).toBe(true);
    expect(mockUpdateTask).not.toHaveBeenCalled();
  });

  it('logs an error with Sentry when START updateTask fails', async () => {
    const error = new Error('network down');
    mockUpdateTask.mockRejectedValue(error);
    const response = makeResponse(ACTION_START);
    await handleActionButtonResponse(response, {
      client: makeClient(),
      inProgressNotifications: true,
      haptic,
    });
    expect(Sentry.captureException).toHaveBeenCalledWith(error);
    expect(cancelScheduledStartNotifications).not.toHaveBeenCalled();
  });

  it('handles DONE action with success haptic', async () => {
    const response = makeResponse(ACTION_DONE);
    const result = await handleActionButtonResponse(response, {
      client: makeClient(),
      inProgressNotifications: true,
      haptic,
    });
    expect(result).toBe(true);
    expect(haptic.success).toHaveBeenCalled();
    expect(mockUpdateTask).toHaveBeenCalledWith('task-1', {
      status: 'completed',
    });
    expect(postResultNotification).toHaveBeenCalledWith(
      'task-1',
      'テストタスク',
      'completed',
      undefined,
    );
    expect(dismissInProgressNotification).toHaveBeenCalledWith('task-1');
    expect(dismissTaskNotifications).toHaveBeenCalledWith('task-1');
    expect(cancelScheduledTaskNotifications).toHaveBeenCalledWith('task-1');
  });

  it('handles CANCEL action with warning haptic', async () => {
    const response = makeResponse(ACTION_CANCEL);
    const result = await handleActionButtonResponse(response, {
      client: makeClient(),
      inProgressNotifications: true,
      haptic,
    });
    expect(result).toBe(true);
    expect(haptic.warning).toHaveBeenCalled();
    expect(mockUpdateTask).toHaveBeenCalledWith('task-1', {
      status: 'skipped',
    });
    expect(postResultNotification).toHaveBeenCalledWith(
      'task-1',
      'テストタスク',
      'skipped',
      undefined,
    );
  });

  it('falls back to a default task title when the title is empty', async () => {
    const response = makeResponse(ACTION_DONE, {
      notification: {
        request: {
          identifier: 'notif-1',
          content: { title: '', data: { taskId: 'task-1' } },
        },
      },
    });
    await handleActionButtonResponse(response, {
      client: makeClient(),
      inProgressNotifications: true,
      haptic,
    });
    expect(postResultNotification).toHaveBeenCalledWith(
      'task-1',
      'タスク',
      'completed',
      undefined,
    );
  });
});
