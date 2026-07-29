import { zoomAroundFocalPoint } from '@/src/components/graph/zoom';

describe('zoomAroundFocalPoint', () => {
  it('keeps the world point under the focal fixed when zooming in', () => {
    // Initial state: scale 1, no translation, focal at (200, 200)
    const result = zoomAroundFocalPoint(0, 0, 1, 200, 200, 2, 0.3, 3);
    expect(result.scale).toBe(2);
    // World point under focal before: (200 - 0) / 1 = 200
    // After: (200 - result.translateX) / result.scale should equal 200
    const worldX = (200 - result.translateX) / result.scale;
    const worldY = (200 - result.translateY) / result.scale;
    expect(worldX).toBeCloseTo(200, 6);
    expect(worldY).toBeCloseTo(200, 6);
  });

  it('keeps the world point under the focal fixed when zooming out', () => {
    // Start zoomed in: scale 2, translated so focal (100,100) maps to world (50, 50)
    // translateX = focalX - worldX * scale = 100 - 50 * 2 = 0
    // translateY = 0
    const result = zoomAroundFocalPoint(0, 0, 2, 100, 100, 0.5, 0.3, 3);
    expect(result.scale).toBe(1);
    const worldX = (100 - result.translateX) / result.scale;
    const worldY = (100 - result.translateY) / result.scale;
    expect(worldX).toBeCloseTo(50, 6);
    expect(worldY).toBeCloseTo(50, 6);
  });

  it('clamps scale to the minimum', () => {
    const result = zoomAroundFocalPoint(0, 0, 0.4, 100, 100, 0.5, 0.3, 3);
    expect(result.scale).toBe(0.3);
  });

  it('clamps scale to the maximum', () => {
    const result = zoomAroundFocalPoint(0, 0, 2.5, 100, 100, 2, 0.3, 3);
    expect(result.scale).toBe(3);
  });

  it('preserves the focal world point with existing translation', () => {
    // scale 1.5, translate (30, -20), focal at (120, 80)
    const tx = 30;
    const ty = -20;
    const s = 1.5;
    const fx = 120;
    const fy = 80;
    const worldXBefore = (fx - tx) / s;
    const worldYBefore = (fy - ty) / s;
    const result = zoomAroundFocalPoint(tx, ty, s, fx, fy, 1.5, 0.3, 3);
    const worldXAfter = (fx - result.translateX) / result.scale;
    const worldYAfter = (fy - result.translateY) / result.scale;
    expect(worldXAfter).toBeCloseTo(worldXBefore, 6);
    expect(worldYAfter).toBeCloseTo(worldYBefore, 6);
  });
});
