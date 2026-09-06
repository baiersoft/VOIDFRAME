import "@testing-library/jest-dom";

// recharts' <ResponsiveContainer> (used by ResultsVisualizer's bar charts)
// requires a global ResizeObserver, which jsdom does not provide. This is a
// minimal stub sufficient for tests: it only needs to exist and not throw —
// jsdom has no real layout engine, so actual resize callbacks are never
// needed for assertions to pass.
class ResizeObserverStub implements ResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = ResizeObserverStub;
}

// uPlot (used by FrameTimeChart) calls `window.matchMedia(...)` at import
// time to track devicePixelRatio changes, which jsdom does not implement.
// This is a minimal stub sufficient for tests: it only needs to exist and
// return an object with the on/off (addEventListener/removeEventListener)
// surface uPlot expects -- no real media-query matching is needed for
// assertions to pass.
if (typeof window !== "undefined" && typeof window.matchMedia !== "function") {
  window.matchMedia = ((query: string): MediaQueryList => {
    return {
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    } as MediaQueryList;
  }) as typeof window.matchMedia;
}
