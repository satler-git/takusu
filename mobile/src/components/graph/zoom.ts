/**
 * Compute new pan/zoom transform values that zoom around a focal point
 * (the midpoint of the pinch gesture) so the content under the user's
 * fingers stays in place (#1310).
 *
 * Without this adjustment, pinch zoom scales around the canvas origin
 * (top-left corner), which feels unnatural.
 */
export function zoomAroundFocalPoint(
  translateX: number,
  translateY: number,
  scale: number,
  focalX: number,
  focalY: number,
  scaleChange: number,
  minScale: number,
  maxScale: number,
): { translateX: number; translateY: number; scale: number } {
  'worklet';
  const newScale = Math.max(minScale, Math.min(maxScale, scale * scaleChange));
  // World point currently under the focal point
  const focalWorldX = (focalX - translateX) / scale;
  const focalWorldY = (focalY - translateY) / scale;
  // Adjust translation so that world point stays under the focal
  return {
    translateX: focalX - focalWorldX * newScale,
    translateY: focalY - focalWorldY * newScale,
    scale: newScale,
  };
}
