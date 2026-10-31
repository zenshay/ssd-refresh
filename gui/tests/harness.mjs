// Shared parts of the browser scripts: find a browser, serve the page, and fake the IPC.
// The page gets synthetic data only. These helpers do not access storage volumes.
import assert from "node:assert/strict";
import { once } from "node:events";
import { existsSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import viteConfig from "../vite.config.js";

export const guiRoot = fileURLToPath(new URL("../", import.meta.url));

/** The window sizes to check: default and minimum from tauri.conf.json, and a large one. */
export const SIZES = {
  default: { width: 1080, height: 780 },
  min: { width: 980, height: 640 },
  large: { width: 1440, height: 1000 },
};

export function browserPath() {
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
  const path = candidates.find((p) => p && existsSync(p));
  assert.ok(
    path,
    "Set SSD_REFRESH_BROWSER to a Chrome or Edge executable path.",
  );
  return path;
}

/** Serve the GUI with Vite on 127.0.0.1. Returns the URL and a close function. */
export async function startServer() {
  const vite = await createServer({
    root: guiRoot,
    configFile: false,
    // The plugins put the version from package.json in the page.
    plugins: viteConfig.plugins,
    server: { host: "127.0.0.1", middlewareMode: true, hmr: false },
  });
  const server = createHttpServer(vite.middlewares);
  await once(server.listen(0, "127.0.0.1"), "listening");
  const { address, port } = server.address();
  assert.equal(address, "127.0.0.1");
  return {
    url: `http://${address}:${port}/`,
    async close() {
      await vite.close();
      if (server.listening)
        await new Promise((done, fail) => {
          server.close((error) => (error ? fail(error) : done()));
        });
    },
  };
}

/**
 * Fake the Tauri IPC before the page loads.
 * data: { volumes, messages?, scan?, verify? } — the results of the commands.
 */
export function installIpc(page, data) {
  return page.evaluateOnNewDocument((data) => {
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
        if (command === "start_scan" || command === "start_verify") {
          for (const message of data.messages ?? [])
            emit("operation-message", message);
          return command === "start_scan" ? data.scan : data.verify;
        }
        return null;
      },
    };
  }, data);
}
