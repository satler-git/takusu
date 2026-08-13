jest.mock('react-native-reanimated', () => {
  const RN = require('react-native');
  const { useEffect, useRef } = require('react');
  type MockReaction = {
    prepare: () => unknown;
    react: (current: unknown, previous: unknown) => void;
    previous: unknown;
  };
  const reactions: MockReaction[] = [];
  const notifyReactions = () => {
    for (const reaction of [...reactions]) {
      const current = reaction.prepare();
      if (current !== reaction.previous) {
        const previous = reaction.previous;
        reaction.previous = current;
        reaction.react(current, previous);
      }
    }
  };
  const useSharedValue = (initial: unknown) => {
    let current = initial;
    return {
      get value() {
        return current;
      },
      set value(next: unknown) {
        current = next;
        notifyReactions();
      },
    };
  };
  const useAnimatedReaction = (
    prepare: () => unknown,
    react: MockReaction['react'],
  ) => {
    const reactionRef = useRef(null as MockReaction | null);
    if (!reactionRef.current) {
      reactionRef.current = { prepare, react, previous: undefined };
    } else {
      reactionRef.current.prepare = prepare;
      reactionRef.current.react = react;
    }
    useEffect(() => {
      const reaction = reactionRef.current!;
      reactions.push(reaction);
      return () => {
        const index = reactions.indexOf(reaction);
        if (index >= 0) reactions.splice(index, 1);
      };
    }, []);
  };
  const identity = (value: unknown) => value;
  return {
    __esModule: true,
    default: {
      View: RN.View,
      createAnimatedComponent: (component: unknown) => component,
    },
    runOnJS: identity,
    useSharedValue,
    useAnimatedStyle: (callback: () => unknown) => callback(),
    useAnimatedReaction,
    useEvent: () => () => {},
    withSpring: identity,
  };
});

jest.mock('@expo/vector-icons', () => ({
  Ionicons: () => null,
}));

jest.mock('@/src/components/CrossFadeIcon', () => ({
  CrossFadeIcon: () => null,
}));

jest.mock('@/src/components/haptics', () => ({
  haptic: {
    light: jest.fn(),
    medium: jest.fn(),
    warning: jest.fn(),
  },
}));

import { act, render } from '@testing-library/react-native';
import { getByGestureTestId } from 'react-native-gesture-handler/lib/commonjs/jestUtils';
import { ThemeProvider } from '@/src/theme';
import { TaskCard } from '@/src/components/TaskCard';
import type { TaskRow } from '@/src/api/types';

const task = {
  id: 'task-1',
  title: 'task',
  status: 'scheduled',
  abandonability: 0.5,
  allows_parallel: false,
  avg_minutes: 30,
  completed_at: null,
  created_at: '2026-08-14T00:00:00Z',
  depends: '[]',
  display_id: 1,
  end_at: '2026-08-14T01:00:00Z',
  fixed: false,
  habit_id: null,
  habit_step_id: null,
  ical_uid: null,
  original_quantity_total: null,
  parallelizable: false,
  quantity_done: 0,
  quantity_total: null,
  quantity_unit: null,
  sigma_minutes: 5,
  split_from_task_id: null,
  start_at: '2026-08-14T00:00:00Z',
  updated_at: '2026-08-14T00:00:00Z',
  user_edited: false,
} as TaskRow;

async function renderTask(overrides: Partial<TaskRow> = {}) {
  const onDone = jest.fn();
  const onStart = jest.fn();
  const onComplete = jest.fn();
  const rendered = await render(
    <ThemeProvider theme="light">
      <TaskCard
        task={{ ...task, ...overrides }}
        isDone={false}
        onPress={jest.fn()}
        onDone={onDone}
        onStart={onStart}
        onComplete={onComplete}
      />
    </ThemeProvider>,
  );
  return { ...rendered, onDone, onStart, onComplete };
}

describe('TaskCard start/done over-slide', () => {
  it('keeps the existing start action between the two thresholds', async () => {
    const { onDone, onStart, onComplete } = await renderTask();
    const pan = getByGestureTestId('task-card-pan-task-1') as any;

    await act(async () => {
      pan.handlers.onEnd({ translationX: 120 });
    });

    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onStart).not.toHaveBeenCalled();
    expect(onComplete).not.toHaveBeenCalled();
  });

  it('reveals Start and Done after the new threshold', async () => {
    const { getByRole, onDone, onStart, onComplete } = await renderTask();
    const pan = getByGestureTestId('task-card-pan-task-1') as any;

    await act(async () => {
      pan.handlers.onEnd({ translationX: 180 });
    });

    expect(onDone).not.toHaveBeenCalled();
    await act(async () => {
      getByRole('button', { name: 'タスクを開始' }).props.onClick();
    });
    expect(onStart).toHaveBeenCalledTimes(1);
    expect(onComplete).not.toHaveBeenCalled();

    await act(async () => {
      getByRole('button', { name: 'タスクを完了' }).props.onClick();
    });
    expect(onComplete).toHaveBeenCalledTimes(1);
  });

  it('resets the revealed panel when the task status changes', async () => {
    const { rerender, getByRole, queryByRole, getByTestId } =
      await renderTask();
    const pan = getByGestureTestId('task-card-pan-task-1') as any;

    await act(async () => {
      pan.handlers.onEnd({ translationX: 180 });
    });
    expect(getByRole('button', { name: 'タスクを開始' })).toBeTruthy();

    await act(async () => {
      rerender(
        <ThemeProvider theme="light">
          <TaskCard
            task={{ ...task, status: 'in_progress' }}
            isDone={false}
            onPress={jest.fn()}
            onDone={jest.fn()}
            onStart={jest.fn()}
            onComplete={jest.fn()}
          />
        </ThemeProvider>,
      );
    });

    expect(queryByRole('button', { name: 'タスクを開始' })).toBeNull();
    expect(getByTestId('task-card-task-1').props.style).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ transform: [{ translateX: 0 }] }),
      ]),
    );
  });

  it('keeps the existing direct completion for pending tasks', async () => {
    const { queryByLabelText, onDone, onStart, onComplete } = await renderTask({
      status: 'pending',
    });
    const pan = getByGestureTestId('task-card-pan-task-1') as any;

    await act(async () => {
      pan.handlers.onEnd({ translationX: 180 });
    });

    expect(onDone).toHaveBeenCalledTimes(1);
    expect(queryByLabelText('タスクを開始')).toBeNull();
    expect(queryByLabelText('タスクを完了')).toBeNull();
    expect(onStart).not.toHaveBeenCalled();
    expect(onComplete).not.toHaveBeenCalled();
  });
});
