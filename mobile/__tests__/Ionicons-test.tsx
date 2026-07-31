import { render } from '@testing-library/react-native';
import { Ionicons } from '@expo/vector-icons';

describe('Ionicons', () => {
  it('renders an icon', async () => {
    const { toJSON } = await render(
      <Ionicons name="add" size={24} color="#000" />,
    );
    expect(toJSON()).toBeTruthy();
  });
});
