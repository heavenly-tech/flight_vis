(() => {
  const fixes = [
    { t: 1_700_000_000_000, lat: -33.4, lon: -70.6, alt: 1200, alt_ref: "wgs84_ellipsoid" },
    { t: 1_700_000_001_000, lat: -33.41, lon: -70.61, alt: 1210, alt_ref: "wgs84_ellipsoid" },
  ];
  class MockEventSource {
    readyState = 1;
    url;
    onerror = null;
    #listeners = new Map();
    constructor(url) {
      this.url = url;
      queueMicrotask(() => {
        for (const fix of fixes) {
          this.#emit("fix", JSON.stringify(fix));
        }
      });
    }
    addEventListener(type, fn) {
      const list = this.#listeners.get(type) ?? [];
      list.push(fn);
      this.#listeners.set(type, list);
    }
    close() {}
    #emit(type, data) {
      for (const fn of this.#listeners.get(type) ?? []) {
        fn({ data });
      }
    }
  }
  globalThis.EventSource = MockEventSource;
})();