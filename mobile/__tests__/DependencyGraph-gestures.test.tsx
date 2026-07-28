jest.mock('@shopify/react-native-skia', () => {
  const { View } = require('react-native');
  const Mock = (props: any) => <View {...props} />;
  return {
    __esModule: true,
    Canvas: Mock,
    Group: Mock,
    Circle: Mock,
    Path: Mock,
    Paragraph: Mock,
    DashPathEffect: Mock,
    useCanvasRef: () => ({ current: null }),
    Skia: {
      Color: () => 0,
      ParagraphBuilder: {
        Make: () => {
          const builder = {
            pushStyle: () => builder,
            addText: () => builder,
            pop: () => builder,
            build: () => ({ layout: () => {}, getHeight: () => 20 }),
          };
          return builder;
        },
      },
      Path: { Make: () => ({ addRRect: () => {} }) },
      XYWHRect: () => ({}),
      RRectXY: () => ({}),
    },
    TextAlign: { Center: 1 },
  };
});

jest.mock('react-native-reanimated', () => {
  const RN = require('react-native');
  const NOOP = () => {};
  const ID = (x: any) => x;
  const useSharedValue = (init: any) => {
    const value = { value: init };
    return new Proxy(value, {
      get(target, prop) {
        if (prop === 'value') return target.value;
        if (prop === 'get') return () => target.value;
        if (prop === 'set')
          return (newValue: any) => {
            if (typeof newValue === 'function') {
              target.value = newValue(target.value);
            } else {
              target.value = newValue;
            }
          };
        return undefined;
      },
      set(target, prop: string, newValue) {
        if (prop === 'value') {
          target.value = newValue;
          return true;
        }
        return false;
      },
    });
  };
  return {
    __esModule: true,
    default: {
      View: RN.View,
      Text: RN.Text,
      Image: RN.Image,
      ScrollView: RN.Animated?.ScrollView ?? RN.View,
      FlatList: RN.Animated?.FlatList ?? RN.View,
      createAnimatedComponent: ID,
    },
    runOnJS: ID,
    runOnUI: ID,
    useSharedValue,
    useDerivedValue: (processor: () => any) => ({
      value: processor(),
      get: () => processor(),
    }),
    useEvent: () => NOOP,
    useAnimatedProps: (cb: any) => cb(),
    setGestureState: NOOP,
  };
});

import React from 'react';
import { render, fireEvent, waitFor, act } from '@testing-library/react-native';
import type { TestInstance } from 'test-renderer';
import {
  getByGestureTestId,
  fireGestureHandler,
} from 'react-native-gesture-handler/lib/commonjs/jestUtils';
import { DependencyGraph } from '@/src/components/graph/DependencyGraph';

function getCircles(container: TestInstance): TestInstance[] {
  // Only return fill circles — border/highlight circles use style="stroke"
  // and would inflate the count (#1174 added node borders).
  return container.queryAll(
    (i) =>
      i.type === 'View' &&
      typeof i.props.cx === 'number' &&
      i.props.style !== 'stroke',
  );
}

describe('DependencyGraph gestures', () => {
  it('pan gesture is configured for single-pointer only', async () => {
    const node = {
      id: 'n1',
      label: 'task',
      color: '#ff0000',
      x: 100,
      y: 100,
      vx: 0,
      vy: 0,
    };
    const { container, root } = await render(
      <DependencyGraph nodes={[node]} edges={[]} />,
    );

    await act(async () => {
      await fireEvent(root!, 'layout', {
        nativeEvent: { layout: { width: 400, height: 400 } },
      });
    });
    await waitFor(() => expect(getCircles(container).length).toBe(1));

    const panGesture = getByGestureTestId('graph-pan') as any;
    expect(panGesture.config.maxPointers).toBe(1);
  });

  it('drags a node with one finger', async () => {
    const node = {
      id: 'n1',
      label: 'task',
      color: '#ff0000',
      x: 100,
      y: 100,
      vx: 0,
      vy: 0,
    };
    const { container, root } = await render(
      <DependencyGraph nodes={[node]} edges={[]} />,
    );

    await act(async () => {
      await fireEvent(root!, 'layout', {
        nativeEvent: { layout: { width: 400, height: 400 } },
      });
    });
    await waitFor(() => expect(getCircles(container).length).toBe(1));

    const circle = getCircles(container)[0];
    const startX = circle.props.cx as number;
    const startY = circle.props.cy as number;

    const panGesture = getByGestureTestId('graph-pan') as any;
    await act(async () => {
      fireGestureHandler(panGesture, [
        { x: startX, y: startY, numberOfPointers: 1 },
        { x: startX, y: startY, numberOfPointers: 1 },
        {
          x: startX + 50,
          y: startY + 50,
          numberOfPointers: 1,
          translationX: 50,
          translationY: 50,
        },
      ]);
    });

    const moved = getCircles(container)[0];
    expect(moved.props.cx).toBeCloseTo(startX + 50, 0);
    expect(moved.props.cy).toBeCloseTo(startY + 50, 0);
  });

  it('cancels node drag when a second finger lands', async () => {
    const node = {
      id: 'n1',
      label: 'task',
      color: '#ff0000',
      x: 100,
      y: 100,
      vx: 0,
      vy: 0,
    };
    const { container, root } = await render(
      <DependencyGraph nodes={[node]} edges={[]} />,
    );

    await act(async () => {
      await fireEvent(root!, 'layout', {
        nativeEvent: { layout: { width: 400, height: 400 } },
      });
    });
    await waitFor(() => expect(getCircles(container).length).toBe(1));

    const circle = getCircles(container)[0];
    const startX = circle.props.cx as number;
    const startY = circle.props.cy as number;

    const panGesture = getByGestureTestId('graph-pan') as any;

    // First drag the node 50 points with one finger.
    await act(async () => {
      fireGestureHandler(panGesture, [
        { x: startX, y: startY, numberOfPointers: 1 },
        { x: startX, y: startY, numberOfPointers: 1 },
        {
          x: startX + 50,
          y: startY + 50,
          numberOfPointers: 1,
          translationX: 50,
          translationY: 50,
        },
      ]);
    });

    await waitFor(() => {
      const c = getCircles(container)[0];
      expect(c.props.cx).toBeCloseTo(startX + 50, 0);
    });

    // Now start a new gesture on the node, then send a two-finger update.
    // The node must not move because draggingNodeId is reset.
    await act(async () => {
      fireGestureHandler(panGesture, [
        { x: startX + 50, y: startY + 50, numberOfPointers: 1 },
        { x: startX + 50, y: startY + 50, numberOfPointers: 1 },
        {
          x: startX + 100,
          y: startY + 100,
          numberOfPointers: 2,
          translationX: 50,
          translationY: 50,
        },
      ]);
    });

    const afterTwoFinger = getCircles(container)[0];
    expect(afterTwoFinger.props.cx).toBeCloseTo(startX + 50, 0);
    expect(afterTwoFinger.props.cy).toBeCloseTo(startY + 50, 0);

    // A following single-finger update now pans the canvas, not the node.
    // Start well above-left of the node so the label area is not hit.
    await act(async () => {
      fireGestureHandler(panGesture, [
        { x: startX - 100, y: startY - 100, numberOfPointers: 1 },
        { x: startX - 100, y: startY - 100, numberOfPointers: 1 },
        {
          x: startX - 50,
          y: startY - 50,
          numberOfPointers: 1,
          translationX: 50,
          translationY: 50,
        },
      ]);
    });

    const afterPan = getCircles(container)[0];
    expect(afterPan.props.cx).toBeCloseTo(startX + 50, 0);
    expect(afterPan.props.cy).toBeCloseTo(startY + 50, 0);
  });

  it('pan onFinalize resets draggingNodeId when cancelled', async () => {
    const node = {
      id: 'n1',
      label: 'task',
      color: '#ff0000',
      x: 100,
      y: 100,
      vx: 0,
      vy: 0,
    };
    const { container, root } = await render(
      <DependencyGraph nodes={[node]} edges={[]} />,
    );

    await act(async () => {
      await fireEvent(root!, 'layout', {
        nativeEvent: { layout: { width: 400, height: 400 } },
      });
    });
    await waitFor(() => expect(getCircles(container).length).toBe(1));

    const circle = getCircles(container)[0];
    const startX = circle.props.cx as number;
    const startY = circle.props.cy as number;

    const panGesture = getByGestureTestId('graph-pan') as any;

    await act(() => {
      panGesture.handlers.onStart({
        x: startX,
        y: startY,
        numberOfPointers: 1,
      });
    });

    await act(() => {
      panGesture.handlers.onFinalize({}, false);
    });

    // After a cancelled finalize, the next one-finger update should pan the
    // canvas instead of dragging the node.
    await act(() => {
      panGesture.handlers.onChange({
        x: startX + 50,
        y: startY + 50,
        numberOfPointers: 1,
        changeX: 50,
        changeY: 50,
      });
    });

    const unchanged = getCircles(container)[0];
    expect(unchanged.props.cx).toBeCloseTo(startX, 0);
    expect(unchanged.props.cy).toBeCloseTo(startY, 0);
  });

  it('long-press drag is configured for single-pointer only', async () => {
    const node = {
      id: 'n1',
      label: 'task',
      color: '#ff0000',
      x: 100,
      y: 100,
      vx: 0,
      vy: 0,
    };
    const onAddEdge = jest.fn();
    const { container, root } = await render(
      <DependencyGraph
        nodes={[node]}
        edges={[]}
        editMode={true}
        onAddEdge={onAddEdge}
      />,
    );

    await act(async () => {
      await fireEvent(root!, 'layout', {
        nativeEvent: { layout: { width: 400, height: 400 } },
      });
    });
    await waitFor(() => expect(getCircles(container).length).toBe(1));

    const longPressGesture = getByGestureTestId('graph-long-press-drag') as any;
    expect(longPressGesture.config.maxPointers).toBe(1);
  });

  it('does not add an edge when the long-press drag starts with two fingers', async () => {
    const nodes = [
      {
        id: 'n1',
        label: 'a',
        color: '#ff0000',
        x: 100,
        y: 100,
        vx: 0,
        vy: 0,
      },
      {
        id: 'n2',
        label: 'b',
        color: '#00ff00',
        x: 300,
        y: 100,
        vx: 0,
        vy: 0,
      },
    ];
    const onAddEdge = jest.fn();
    const { container, root } = await render(
      <DependencyGraph
        nodes={nodes}
        edges={[]}
        editMode={true}
        onAddEdge={onAddEdge}
      />,
    );

    await act(async () => {
      await fireEvent(root!, 'layout', {
        nativeEvent: { layout: { width: 400, height: 400 } },
      });
    });
    await waitFor(() => expect(getCircles(container).length).toBe(2));

    const circles = getCircles(container);
    const [first, second] = circles.sort(
      (a, b) => (a.props.cx as number) - (b.props.cx as number),
    );

    const longPressGesture = getByGestureTestId('graph-long-press-drag') as any;
    await act(async () => {
      fireGestureHandler(longPressGesture, [
        { x: first.props.cx, y: first.props.cy, numberOfPointers: 2 },
        { x: first.props.cx, y: first.props.cy, numberOfPointers: 2 },
        { x: second.props.cx, y: second.props.cy, numberOfPointers: 1 },
      ]);
    });

    expect(onAddEdge).not.toHaveBeenCalled();
  });
});
