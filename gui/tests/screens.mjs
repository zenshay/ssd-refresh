// Make one screenshot for each scenario at each window size, for a review of GUI changes.
// The page gets synthetic IPC data. This script does not access storage volumes.
// Run: pnpm -C gui screens [output folder]   (default: output/screens in the repository)
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { resolve } from "node:path";
import puppeteer from "puppeteer-core";
import { browserPath, guiRoot, SIZES, startServer } from "./harness.mjs";
import { openScenario, SCENARIOS } from "./scenarios.mjs";

const output = resolve(
  process.argv[2] ?? resolve(guiRoot, "../output/screens"),
);
mkdirSync(output, { recursive: true });

const server = await startServer();
let browser;
let count = 0;
try {
  browser = await puppeteer.launch({
    executablePath: browserPath(),
    headless: true,
    pipe: true,
    args: ["--no-first-run"],
    // Show the scrollbars: the review must see where a list scrolls.
    ignoreDefaultArgs: ["--hide-scrollbars"],
  });
  for (const [name, scenario] of Object.entries(SCENARIOS))
    for (const [size, viewport] of Object.entries(SIZES)) {
      const { page, errors } = await openScenario(
        browser,
        server.url,
        scenario,
        viewport,
      );
      const file = resolve(output, `${name}-${size}.png`);
      await page.screenshot({ path: file });
      assert.deepEqual(errors, [], `${name}-${size}`);
      console.log(file);
      count++;
      await page.close();
    }
  console.log(`RESULT: YES (${count} screenshots in ${output})`);
} finally {
  await browser?.close();
  await server.close();
}
