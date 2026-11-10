// tests/setup.ts
// Node >= 25 ships an experimental global `localStorage` getter that returns
// `undefined` unless --localstorage-file is passed, shadowing jsdom's Storage.
// Re-expose jsdom's implementation so jsdom suites behave like a real webview.
const g = globalThis as unknown as {
  jsdom?: { window: { localStorage: Storage; sessionStorage: Storage } };
  localStorage?: Storage;
  sessionStorage?: Storage;
};

if (g.jsdom) {
  for (const key of ['localStorage', 'sessionStorage'] as const) {
    if (!g[key]) {
      Object.defineProperty(globalThis, key, {
        value: g.jsdom.window[key],
        configurable: true,
        writable: true,
      });
    }
  }
}
