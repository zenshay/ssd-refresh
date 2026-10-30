// Make the GUI screenshot for the README.
// The page gets synthetic IPC data. This script does not access storage volumes.
// Run: pnpm -C gui screenshot
import assert from "node:assert/strict";
import { once } from "node:events";
import { existsSync, statSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";
import { createServer } from "vite";

const root = fileURLToPath(new URL("../", import.meta.url));
const output = resolve(root, "../docs/images/gui-scan.png");
// The default window size in gui/src-tauri/tauri.conf.json.
const width = 1080;
const height = 780;
const candidates = [
  process.env.SSD_REFRESH_BROWSER,
  process.env.ProgramFiles &&
    resolve(process.env.ProgramFiles, "Google/Chrome/Application/chrome.exe"),
  process.env["ProgramFiles(x86)"] &&
    resolve(
      process.env["ProgramFiles(x86)"],
      "Microsoft/Edge/Application/msedge.exe",
    ),
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
];
const executablePath = candidates.find((path) => path && existsSync(path));
assert.ok(
  executablePath,
  "Set SSD_REFRESH_BROWSER to a Chrome or Edge executable path.",
);

// The GUI shows sizes in decimal units.
const GB = 1e9;
const MB = 1e6;
const demo = {
  volumes: [
    {
      root: "C:\\",
      fs: "NTFS",
      totalBytes: 1000 * GB,
      freeBytes: 412 * GB,
      isSystem: true,
      model: "Demo NVMe SSD 1TB",
      smart: { level: "healthy", detail: null },
      lastRefresh: null,
    },
    {
      root: "D:\\",
      fs: "NTFS",
      totalBytes: 2000 * GB,
      freeBytes: 655 * GB,
      isSystem: false,
      model: "Demo SATA SSD 2TB",
      smart: { level: "healthy", detail: null },
      lastRefresh: null,
    },
  ],
  messages: [
    "Step 1/3  Find the files",
    "Found 48,213 files (1.21 TB).",
    "Step 2/3  Read each file one time",
    "Step 3/3  Check the files on the check list again",
  ],
  scan: {
    reportPath: "D:\\ssd-refresh-logs\\scan-D-demo.csv",
    totalFiles: 48213,
    slowTotal: 5,
    fastAfterWait: 0,
    stillSlowAfterWait: 5,
    slowFiles: [
      ["D:\\Games\\DemoGame\\Content\\Paks\\pakchunk0.pak", 18.4 * GB, 6.2],
      ["D:\\Photos\\2019\\Trip\\VID_20190714_101522.mp4", 2.1 * GB, 9.8],
      ["D:\\Installers\\demo-suite-2018-setup.iso", 4.7 * GB, 11.5],
      ["D:\\Backup\\2020\\mail-archive.pst", 1.3 * GB, 14.0],
      ["D:\\Music\\Archive\\2017\\album-flac.zip", 820 * MB, 21.3],
    ].map(([path, size, mbps]) => ({
      path,
      size: Math.round(size),
      mbps,
      fastAfterWait: false,
    })),
    stopped: false,
  },
};

const vite = await createServer({
  root,
  configFile: false,
  server: { host: "127.0.0.1", middlewareMode: true, hmr: false },
});
const server = createHttpServer(vite.middlewares);
let browser;
try {
  await once(server.listen(0, "127.0.0.1"), "listening");
  const address = server.address();
  browser = await puppeteer.launch({
    executablePath,
    headless: true,
    pipe: true,
    args: ["--no-first-run", "--hide-scrollbars"],
  });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  await page.setViewport({ width, height, deviceScaleFactor: 2 });
  await page.evaluateOnNewDocument((data) => {
    const callbacks = new Map();
    const listeners = new Map();
    let id = 0;
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
        if (command === "plugin:event|listen") {
          listeners.set(args.event, args.handler);
          return id;
        }
        if (command === "environment") return { elevated: true };
        if (command === "list_volumes") return data.volumes;
        if (command === "start_scan") {
          for (const message of data.messages) {
            emit("operation-message", message);
          }
          return data.scan;
        }
        return null;
      },
    };
  }, demo);
  await page.goto(`http://127.0.0.1:${address.port}`, {
    waitUntil: "networkidle0",
  });
  await page.waitForFunction(
    () => !document.querySelector("#btn-scan").disabled,
  );
  // Select D: only. The app selects the first drive at start.
  await page.click('[data-root="D:\\\\"]');
  await page.click('[data-root="C:\\\\"]');
  await page.click("#btn-scan");
  await page.waitForFunction(
    () => !document.querySelector("#slow-files-card").hidden,
  );
  await page.waitForFunction(
    () => document.querySelector("#mode-chip").textContent === "Ready",
  );
  // Let fonts and transitions finish.
  await page.evaluate(() => document.fonts.ready);
  await new Promise((done) => setTimeout(done, 500));
  // The slow files card must be fully in the image.
  const overflow = await page.evaluate(
    () => document.body.scrollHeight - window.innerHeight,
  );
  assert.ok(overflow <= 0, `Page is ${overflow} px taller than the image.`);
  await page.screenshot({ path: output, type: "png" });
  assert.deepEqual(errors, []);
  const kb = Math.round(statSync(output).size / 1024);
  console.log(`RESULT: YES (${output}, ${width * 2}x${height * 2}, ${kb} KB)`);
} finally {
  await browser?.close();
  await vite.close();
  if (server.listening) {
    await new Promise((done, fail) => {
      server.close((error) => (error ? fail(error) : done()));
    });
  }
}
