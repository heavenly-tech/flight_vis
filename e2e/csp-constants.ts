/** Production nginx CSP on flight.heavenly.cl (must match the vhost). */
export const PROD_HTML_CSP =
  "default-src 'self'; " +
  "script-src 'self' 'wasm-unsafe-eval'; " +
  "worker-src 'self' blob:; " +
  "style-src 'self' 'unsafe-inline'; " +
  "img-src 'self' https://services.arcgisonline.com; " +
  "connect-src 'self' https://services.arcgisonline.com https://elevation3d.arcgis.com; " +
  "font-src 'self'; " +
  "frame-ancestors 'none'; " +
  "base-uri 'none'; " +
  "form-action 'self'";