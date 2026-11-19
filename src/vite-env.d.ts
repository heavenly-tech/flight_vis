/// <reference types="vite/client" />

declare const CESIUM_BASE_URL: string;

interface ImportMetaEnv {
  readonly VITE_CESIUM_ION_TOKEN?: string;
  /** Set to `1` with a token to use Cesium ion terrain (not prod CSP default). */
  readonly VITE_USE_CESIUM_ION_TERRAIN?: string;
  readonly VITE_E2E_SKIP_GLOBE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
