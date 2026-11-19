import { expect, type Page } from "@playwright/test";

export function trackPasswordInRequestUrls(page: Page): {
  urls: string[];
  assertClean: () => void;
} {
  const urls: string[] = [];
  page.on("request", (req) => {
    const u = req.url();
    if (/[?&]password=/i.test(u)) urls.push(u);
  });
  return {
    urls,
    assertClean: () => expect(urls).toEqual([]),
  };
}

export function assertNoPasswordInUrl(url: string): void {
  expect(url).not.toMatch(/[?&]password=/i);
  const decoded = decodeURIComponent(url).toLowerCase();
  expect(decoded).not.toMatch(/[?&]password=/);
}