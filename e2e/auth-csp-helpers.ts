import type { Page } from "@playwright/test";

const PASSWORD_IN_URL = /[?&]password=/i;

export function installPasswordUrlGuard(page: Page): string[] {
  const leaks: string[] = [];
  page.on("request", (req) => {
    const url = req.url();
    if (PASSWORD_IN_URL.test(url)) {
      leaks.push(url);
    }
  });
  return leaks;
}