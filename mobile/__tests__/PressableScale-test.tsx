import { render, fireEvent } from '@testing-library/react-native';
import { Text, View } from 'react-native';
import { PressableScale } from '@/src/components/PressableScale';

describe('PressableScale', () => {
  it('renders children', async () => {
    const { getByText } = await render(
      <PressableScale onPress={jest.fn()}>
        <Text>Tap me</Text>
      </PressableScale>,
    );
    expect(getByText('Tap me')).toBeTruthy();
  });

  it('fires onPress when pressed', async () => {
    const onPress = jest.fn();
    const { getByText } = await render(
      <PressableScale onPress={onPress}>
        <Text>Tap me</Text>
      </PressableScale>,
    );
    fireEvent.press(getByText('Tap me'));
    expect(onPress).toHaveBeenCalledTimes(1);
  });

  it('supports a render child with the pressed state', async () => {
    const { getByText } = await render(
      <PressableScale onPress={jest.fn()}>
        {({ pressed }) => <Text>{pressed ? 'pressed' : 'idle'}</Text>}
      </PressableScale>,
    );
    expect(getByText('idle')).toBeTruthy();
  });

  it('forwards ref to the underlying Pressable', async () => {
    const ref = { current: null } as React.MutableRefObject<View | null>;
    await render(
      <PressableScale ref={ref} onPress={jest.fn()}>
        <Text>Tap me</Text>
      </PressableScale>,
    );
    expect(ref.current).toBeDefined();
  });
});
