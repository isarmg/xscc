import assert from "node:assert/strict";
import { expect } from "@playwright/test";

// Exercise the actual WASM/font loader, including recovery by a user action.
// Consumers supply their real built page and any API fixtures it needs.
export async function verifyStartupFailures(browser, url, preparePage = async () => {}, contextOptions = {}) {
  for (const resource of ["font", "wasm", "invalid-wasm"]) {
    const context = await browser.newContext({ ...contextOptions, locale: "en-US" });
    try {
      const page = await context.newPage();
      await preparePage(page);
      let documents = 0, blocked = 0;
      page.on("request", request => { if (request.isNavigationRequest() && request.frame() === page.mainFrame()) documents++; });
      const pattern = resource === "font" ? /\.woff2(?:\?|$)/u : /\.wasm(?:\?|$)/u;
      const reject = route => {
        blocked++;
        return route.fulfill(resource === "invalid-wasm"
          ? { status: 200, contentType: "application/wasm", body: "invalid wasm fixture" }
          : { status: 503, contentType: "text/plain", body: "resource unavailable fixture" });
      };
      await page.route(pattern, reject);
      await page.goto(url, { waitUntil: "domcontentloaded" });
      const panel = page.locator("#xcss-startup-error");
      await expect(panel).toBeVisible({ timeout: 30_000 });
      await expect(panel).toHaveAttribute("role", "alert");
      await expect(panel).toBeFocused();
      await expect(page.locator("html")).toHaveAttribute("data-xcss-startup", "failed");
      await expect(page.locator("html")).toHaveAttribute("data-xcss-fonts", "failed");
      assert.equal(await page.locator("html").getAttribute("data-xcss-runtime"), null);
      assert.equal(await panel.evaluate(node => getComputedStyle(node).visibility), "visible");
      assert.ok(blocked > 0, `The ${resource} fault must reach the actual network request`);
      const failedRequests = blocked;
      await page.clock.install();
      await page.clock.runFor(120_000);
      assert.equal(blocked, failedRequests, "Startup must not retry resources automatically");
      assert.equal(documents, 1, "Startup must not refresh automatically");
      await expect(panel).toHaveCount(1);
      await page.unroute(pattern, reject);
      await page.clock.resume();
      await panel.getByRole("button", { name: "Reload page", exact: true }).focus();
      await panel.getByRole("button", { name: "Reload page", exact: true }).press("Enter");
      await expect(page.locator("html")).toHaveAttribute("data-xcss-startup", "ready", { timeout: 30_000 });
      await expect(page.locator("html")).toHaveAttribute("data-xcss-fonts", "ready");
      await expect(page.locator("html")).toHaveAttribute("data-xcss-runtime", "xcss-web");
      await expect(panel).toHaveCount(0);
      assert.equal(documents, 2, "Exactly one explicit retry should reload the page");
      console.log(`${browser.browserType().name()}: ${resource} failure visible; no automatic retries; keyboard reload recovered`);
    } finally { await context.close(); }
  }
}

export async function verifyStartupTimeout(browser, url, preparePage = async () => {}, contextOptions = {}) {
  const context = await browser.newContext({ ...contextOptions, locale: "en-US" });
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  try {
    const page = await context.newPage();
    await preparePage(page);
    await page.clock.install();
    await page.route(/\.woff2(?:\?|$)/u, async route => { await gate; await route.continue(); });
    await page.goto(url, { waitUntil: "commit" });
    await expect(page.locator("html")).toHaveAttribute("data-xcss-startup", "pending", { timeout: 30_000 });
    await page.clock.runFor(30_001);
    await expect(page.locator("#xcss-startup-error")).toBeVisible();
    await expect(page.locator("#xcss-startup-error")).toHaveAttribute("data-stage", "timeout");
    release();
    await page.evaluate(() => document.fonts.ready);
    await page.clock.runFor(1_000);
    await expect(page.locator("html")).toHaveAttribute("data-xcss-startup", "failed");
    await expect(page.locator("html")).toHaveAttribute("data-xcss-fonts", "failed");
    assert.equal(await page.locator("html").getAttribute("data-xcss-runtime"), null);
    console.log(`${browser.browserType().name()}: bounded font wait; late completion preserves the visible failure`);
  } finally { release(); await context.close(); }
}
