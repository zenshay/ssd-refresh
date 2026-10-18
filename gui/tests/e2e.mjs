// Browser test with synthetic IPC. This test does not access storage volumes.
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";
import { createServer } from "vite";

const root = fileURLToPath(new URL("../", import.meta.url));
const candidates = [
  process.env.SSD_REFRESH_BROWSER,
  process.env.ProgramFiles &&
    resolve(process.env.ProgramFiles, "Google/Chrome/Application/chrome.exe"),
  process.env["ProgramFiles(x86)"] &&
    resolve(
      process.env["ProgramFiles(x86)"],
      "Microsoft/Edge/Application/msedge.exe",
    ),
  "/usr/bin/chromium",
  "/usr/bin/google-chrome",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
];
const executablePath = candidates.find((path) => path && existsSync(path));
assert.ok(
  executablePath,
  "Set SSD_REFRESH_BROWSER to a Chrome or Edge executable path.",
);
const server = await createServer({
  root,
  configFile: false,
  server: { host: "127.0.0.1", port: 0, strictPort: false },
});
let browser;
try {
  await server.listen();
  browser = await puppeteer.launch({
    executablePath,
    headless: true,
    args: ["--no-first-run"],
  });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  await page.evaluateOnNewDocument(() => {
    const callbacks = new Map();
    const listeners = new Map();
    let id = 0;
    const calls = [];
    const emit = (event, payload) =>
      callbacks.get(listeners.get(event))?.({ event, id: 1, payload });
    window.__TAURI_INTERNALS__ = {
      transformCallback(callback) {
        callbacks.set(++id, callback);
        return id;
      },
      unregisterCallback(callback) {
        callbacks.delete(callback);
      },
      async invoke(command, args) {
        calls.push(command);
        if (command === "plugin:event|listen") {
          listeners.set(args.event, args.handler);
          return id;
        }
        if (command === "environment") return { elevated: false };
        if (command === "list_volumes")
          return [
            {
              root: "X:\\",
              fs: "NTFS",
              totalBytes: 100000,
              freeBytes: 50000,
              isSystem: false,
              model: "Synthetic",
              smart: { level: "healthy", detail: null },
            },
          ];
        if (command === "start_scan") {
          emit("operation-message", "Synthetic core message");
          emit("progress", {
            stage: "read",
            filesDone: 1,
            totalFiles: 2,
            bytesDone: 1024,
            totalBytes: 2048,
            mbps: 12,
            etaSecs: 1,
          });
          return {
            reportPath: "synthetic.csv",
            totalFiles: 2,
            slowTotal: 1,
            slowFiles: [{ path: "X:\\synthetic.dat", size: 1024, mbps: 12 }],
            stopped: false,
          };
        }
        if (command === "start_refresh")
          return {
            logPath: "synthetic-refresh.csv",
            movedFiles: 1,
            stopped: false,
          };
        if (command === "toggle_pause") return true;
        return null;
      },
    };
    window.__mockCalls = calls;
  });
  const address = server.httpServer.address();
  await page.goto(`http://127.0.0.1:${address.port}`, {
    waitUntil: "networkidle0",
  });
  await page.waitForFunction(
    () => !document.querySelector("#btn-scan").disabled,
  );
  await page.click("#btn-scan");
  await page.waitForFunction(
    () => !document.querySelector("#btn-refresh").disabled,
  );
  assert.match(
    await page.$eval("#console-log-view", (node) => node.textContent),
    /Synthetic core message/,
  );
  await page.click("#btn-refresh");
  await page.waitForFunction(
    () => document.querySelector("#confirm-modal").open,
  );
  await page.click("#btn-confirm-cancel");
  assert.equal(
    await page.evaluate(
      () =>
        window.__mockCalls.filter((call) => call === "start_refresh").length,
    ),
    0,
  );
  await page.click("#btn-refresh");
  await page.waitForFunction(
    () => document.querySelector("#confirm-modal").open,
  );
  await page.click("#btn-confirm-ok");
  await page.waitForFunction(
    () =>
      window.__mockCalls.filter((call) => call === "start_refresh").length ===
      1,
  );
  await page.waitForFunction(
    () => document.querySelector("#mode-chip").textContent === "Ready",
  );
  assert.equal(
    await page.evaluate(
      () =>
        window.__mockCalls.filter((call) => call === "start_refresh").length,
    ),
    1,
  );
  assert.deepEqual(errors, []);
  console.log("RESULT: YES (scan, core messages, cancel, confirm, refresh)");
} finally {
  await browser?.close();
  await server.close();
}
