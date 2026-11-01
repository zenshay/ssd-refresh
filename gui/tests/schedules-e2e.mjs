import assert from "node:assert/strict";
import puppeteer from "puppeteer-core";
import { browserPath, startServer } from "./harness.mjs";
import { openScenario, SCENARIOS } from "./scenarios.mjs";

const server = await startServer();
const browser = await puppeteer.launch({
  executablePath: browserPath(),
  headless: true,
  pipe: true,
});
try {
  const { page, errors } = await openScenario(
    browser,
    server.url,
    { data: SCENARIOS["verify-faster-no"].data },
    { width: 980, height: 640 },
  );
  await page.click("#schedule-open");
  assert.match(
    await page.$eval("#schedule-panel", (el) => el.textContent),
    /Scheduled Verify cannot read files that need administrator rights\. Run Verify manually for those files\./,
  );
  await page.waitForFunction(
    () => !document.querySelector("#schedule-create").disabled,
  );
  await page.click("#schedule-create");
  await page.waitForFunction(
    () => document.querySelectorAll("#schedule-list li").length === 1,
  );
  assert.match(
    await page.$eval("#schedule-list", (el) => el.textContent),
    /Not run/,
  );
  const layout = await page.$eval("#schedule-panel", (el) => ({
    fits: el.scrollWidth <= el.clientWidth + 1,
    bottom: el.getBoundingClientRect().bottom,
  }));
  assert.equal(layout.fits, true);
  assert.ok(layout.bottom <= 640);
  await page.click("#schedule-remove");
  await page.waitForFunction(
    () => document.querySelectorAll("#schedule-list li").length === 0,
  );
  await page.click("#schedule-close");
  assert.equal(await page.$eval("#schedule-panel", (el) => el.open), false);
  assert.deepEqual(errors, []);
  await page.close();
  const idle = await openScenario(browser, server.url, SCENARIOS.idle, {
    width: 1080,
    height: 780,
  });
  await idle.page.click("#schedule-open");
  assert.equal(
    await idle.page.$eval("#schedule-create", (el) => el.disabled),
    true,
  );
  await idle.page.keyboard.press("Escape");
  assert.equal(
    await idle.page.$eval("#schedule-panel", (el) => el.open),
    false,
  );
  assert.deepEqual(idle.errors, []);
  console.log(
    "RESULT: YES (schedule create, status, removal, close, minimum size and missing refresh log)",
  );
} finally {
  await browser.close();
  await server.close();
}
