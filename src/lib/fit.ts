// Shrinking mail laid out wider than the screen (MessageBody, on a phone).

/** No smaller than this: below it text is unreadable, and the body scrolls sideways instead. */
export const MIN_SCALE = 0.4;

/** The zoom that fits content `contentWidth` wide into `available`: 1 when it already fits. */
export function fitScale(available: number, contentWidth: number): number {
  if (available <= 0 || contentWidth <= available + 1) return 1;
  return Math.max(MIN_SCALE, Math.floor((available / contentWidth) * 1000) / 1000);
}
