import type { Page } from "@playwright/test";

const PASSWORD_IN_URL = /password/i;

export function installPasswordUrlGuard(page: Page) {
  page.on("request", (req) => {
    const url = req.url();
    if (PASSWORD_IN_URL.test(url)) {
      throw new Error(`password leaked in request URL: ${url}`);
    }
  });
}