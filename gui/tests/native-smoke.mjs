// Native WebView2 smoke test. Storage operation buttons stay disabled.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

assert.equal(process.platform, "win32", "This test needs Windows.");
assert.equal(process.argv.length, 3, "Give the release GUI executable path.");
const binary = resolve(process.argv[2]);
assert(existsSync(binary), "The GUI executable does not exist.");
const root = fileURLToPath(new URL("../../", import.meta.url));
mkdirSync(join(root, "scratch"), { recursive: true });
const output = mkdtempSync(join(root, "scratch/native-smoke-"));
const listener = createServer();
await new Promise((done) => listener.listen(0, "127.0.0.1", done));
const port = listener.address().port;
await new Promise((done) => listener.close(done));
const endpoint = `http://127.0.0.1:${port}`;
const child = spawn(binary, [], {
  cwd: dirname(binary),
  windowsHide: false,
  env: {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
    WEBVIEW2_USER_DATA_FOLDER: join(output, "profile"),
  },
  stdio: [
    "ignore",
    openSync(join(output, "stdout.log"), "w"),
    openSync(join(output, "stderr.log"), "w"),
  ],
});
const result = {
  status: "NO",
  binary_sha256: createHash("sha256")
    .update(readFileSync(binary))
    .digest("hex"),
  ipc_mocked: false,
};
let browser;
const wait = (ms) => new Promise((done) => setTimeout(done, ms));
try {
  let ready = false;
  for (let attempt = 0; attempt < 60; attempt++) {
    try {
      if ((await fetch(`${endpoint}/json/version`)).ok) {
        ready = true;
        break;
      }
    } catch {}
    await wait(500);
  }
  assert(ready, "WebView2 CDP did not start.");
  browser = await puppeteer.connect({
    browserURL: endpoint,
    defaultViewport: null,
  });
  const target = await browser.waitForTarget(
    (t) => t.type() === "page" && t.url().includes("tauri.localhost"),
    { timeout: 20000 },
  );
  const page = await target.page();
  assert(page, "The native page is missing.");
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  await page.waitForFunction(
    () => document.querySelector("#btn-reload")?.disabled === false,
    { timeout: 30000 },
  );
  const environment = await page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("environment"),
  );
  const baseline = await page.evaluate(() => ({
    width: innerWidth,
    height: innerHeight,
    ready: document.readyState,
    refreshDisabled: document.querySelector("#btn-refresh").disabled,
    standby: document.body.textContent.includes("No operation is running"),
    volumeCount: Number(
      document.body.textContent.match(
        /Storage inventory: (\d+) drives detected/,
      )?.[1],
    ),
  }));
  assert.equal(baseline.ready, "complete");
  assert(baseline.standby && baseline.volumeCount > 0);
  if (!environment.elevated) assert(baseline.refreshDisabled);
  await page.evaluate(() => {
    const disable = () => {
      for (const id of ["btn-scan", "btn-refresh", "btn-verify"]) {
        const button = document.getElementById(id);
        if (!button.disabled) button.disabled = true;
      }
    };
    new MutationObserver(disable).observe(document.body, {
      subtree: true,
      attributes: true,
      attributeFilter: ["disabled"],
      childList: true,
    });
    disable();
  });
  await page.screenshot({ path: join(output, "startup.png") });
  const click = async (id) => {
    assert(["btn-notice-dismiss", "btn-win-max", "btn-win-close"].includes(id));
    await page.click(`#${id}`);
  };
  await wait(1000);
  if (await page.$eval("#notice-card", (e) => !e.hidden)) {
    await click("btn-notice-dismiss");
    assert(await page.$eval("#notice-card", (e) => e.hidden));
    result.notice_dismissed = true;
  }
  await click("btn-win-max");
  await page.waitForFunction(
    (s) => innerWidth !== s.width || innerHeight !== s.height,
    { timeout: 10000 },
    baseline,
  );
  const maximized = await page.evaluate(() => ({
    width: innerWidth,
    height: innerHeight,
  }));
  await page.screenshot({ path: join(output, "maximized.png") });
  await wait(1500);
  await click("btn-win-max");
  await page.waitForFunction(
    (s) => innerWidth === s.width && innerHeight === s.height,
    { timeout: 10000 },
    baseline,
  );
  const restored = await page.evaluate(() => ({
    width: innerWidth,
    height: innerHeight,
    standby: document.body.textContent.includes("No operation is running"),
  }));
  assert(restored.standby);
  await page.screenshot({ path: join(output, "restored.png") });
  assert.deepEqual(errors, []);
  await wait(1000);
  const closed = once(child, "exit");
  await click("btn-win-close").catch((error) => {
    if (!/closed|detached|Target/i.test(String(error))) throw error;
  });
  const timer = setTimeout(() => child.kill(), 10000);
  const [exit] = await closed;
  clearTimeout(timer);
  assert.equal(exit, 0, "The native close action failed.");
  Object.assign(result, {
    status: "YES",
    environment,
    baseline,
    maximized,
    restored,
    native_close: true,
    storage_buttons_disabled_by_test: true,
    page_errors: errors,
  });
} catch (error) {
  result.error = String(error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.disconnect();
  if (child.exitCode === null) child.kill();
  writeFileSync(
    join(output, "result.json"),
    `${JSON.stringify(result, null, 2)}\n`,
    "utf8",
  );
  writeFileSync(
    join(root, "scratch/native-smoke-last.json"),
    `${JSON.stringify({ output })}\n`,
    "utf8",
  );
  console.log(`OUTPUT: ${output}`);
  console.log(
    `RESULT: ${result.status} (native WebView2; no storage operation requested)`,
  );
}
