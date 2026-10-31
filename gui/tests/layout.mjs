// Layout checks: in each scenario and window size, no area is cut, squeezed or too wide.
// The page gets synthetic IPC data. This script does not access storage volumes.
// Run: pnpm -C gui test:layout
import assert from "node:assert/strict";
import puppeteer from "puppeteer-core";
import { browserPath, SIZES, startServer } from "./harness.mjs";
import { openScenario, SCENARIOS } from "./scenarios.mjs";

/** The console height floor in style.css: 180 px next to the slow file list, else 120 px. */
const MIN_CONSOLE_PX = { slowList: 180, other: 120 };
/** The operation cards must stay wide and flat: width / height. */
const MIN_CARD_RATIO = 1.5;

const server = await startServer();
let browser;
let checks = 0;
try {
  browser = await puppeteer.launch({
    executablePath: browserPath(),
    headless: true,
    pipe: true,
    args: ["--no-first-run"],
  });
  for (const [name, scenario] of Object.entries(SCENARIOS))
    for (const [size, viewport] of Object.entries(SIZES)) {
      const where = `${name} at ${size} ${viewport.width}x${viewport.height}`;
      const { page, errors } = await openScenario(
        browser,
        server.url,
        scenario,
        viewport,
      );
      const facts = await page.evaluate(() => {
        const box = (el) => el.getBoundingClientRect();
        // An element is cut when its content is wider than its box.
        const cut = (selector) =>
          [...document.querySelectorAll(selector)]
            .filter((el) => !el.closest("[hidden]"))
            .filter((el) => el.scrollWidth > el.clientWidth + 1)
            .map((el) => el.id || el.className);
        const content = document.querySelector(".layout-content");
        const slow = document.querySelector("#slow-files-card");
        const table = document.querySelector(".table-container");
        return {
          pageWidth: document.documentElement.scrollWidth,
          cut: cut(
            ".console-toolbar, .drive-top, .drive-meta, .card-header, .notice-content, .action-card",
          ),
          cards: [...document.querySelectorAll(".action-card")].map((el) => ({
            id: el.id,
            ratio: box(el).width / box(el).height,
            clipped: el.scrollHeight > el.clientHeight + 1,
          })),
          icons: [...document.querySelectorAll(".action-icon")].filter(
            (el) => el.getBoundingClientRect().width > 0,
          ).length,
          lockedButNext:
            document.querySelector("#btn-refresh").disabled &&
            document.querySelector("#app").dataset.next === "refresh",
          console: box(document.querySelector(".console-card")).height,
          slowShown: !slow.hidden,
          tableScrolls: table.scrollHeight > table.clientHeight,
          pageScrolls: content.scrollHeight > content.clientHeight + 1,
        };
      });
      assert.deepEqual(errors, [], `${where}: page errors`);
      assert.ok(
        facts.pageWidth <= viewport.width,
        `${where}: page is ${facts.pageWidth} px wide`,
      );
      assert.deepEqual(facts.cut, [], `${where}: content is cut`);
      for (const card of facts.cards) {
        assert.ok(!card.clipped, `${where}: ${card.id} text is cut`);
        assert.ok(
          card.ratio >= MIN_CARD_RATIO,
          `${where}: ${card.id} is ${card.ratio.toFixed(2)}:1, not wide`,
        );
      }
      // Only the minimum window hides the card icons.
      if (size !== "min")
        assert.equal(facts.icons, 3, `${where}: card icons are hidden`);
      assert.ok(
        !facts.lockedButNext,
        `${where}: a locked refresh card has the next-step color`,
      );
      assert.ok(
        facts.console >=
          MIN_CONSOLE_PX[facts.slowShown ? "slowList" : "other"] - 1,
        `${where}: console is ${Math.round(facts.console)} px high`,
      );
      if (facts.slowShown)
        assert.ok(facts.tableScrolls, `${where}: the long list must scroll`);
      // The minimum window may scroll. The other sizes show all areas.
      if (size !== "min")
        assert.ok(!facts.pageScrolls, `${where}: the page must not scroll`);
      checks++;
      await page.close();
    }
  console.log(`RESULT: YES (layout: ${checks} scenario and window size pairs)`);
} finally {
  await browser?.close();
  await server.close();
}
