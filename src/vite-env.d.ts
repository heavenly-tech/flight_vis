/// <reference types="vite/client" />

declare const CESIUM_BASE_URL: string;

interface ImportMetaEnv {
  readonly VITE_CESIUM_ION_TOKEN?: string;
  readonly VITE_E2E_SKIP_GLOBE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
