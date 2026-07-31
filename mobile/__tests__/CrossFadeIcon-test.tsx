import { render } from '@testing-library/react-native';
import { CrossFadeIcon } from '@/src/components/CrossFadeIcon';

describe('CrossFadeIcon', () => {
  it('renders the requested icon with the given color', async () => {
    const { toJSON } = await render(
      <CrossFadeIcon name="add" size={24} color="#ff0000" />,
    );
    expect(toJSON()).toBeTruthy();
  });
});
