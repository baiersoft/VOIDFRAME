/**
 * Formats a raw backend metric value (an unrounded float, or null/undefined
 * when the metric couldn't be computed) for display. Real rig data comes
 * back with 10+ decimal digits (e.g. 430.28571428571433) -- this rounds to a
 * fixed, readable precision and falls back to an em dash for missing data,
 * matching this file's existing `frame_time_cv` formatting precedent
 * (`.toFixed(1)`).
 */
export function fmt(n: number | null | undefined, digits = 1): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "—";
  return n.toFixed(digits);
}
