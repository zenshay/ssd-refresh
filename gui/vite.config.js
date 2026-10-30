import { readFileSync } from "node:fs";
import { defineConfig } from "vite";

// package.json is the one source of the version. The release check compares it with the tag.
const { version } = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
);

export default defineConfig({
  clearScreen: false,
  plugins: [
    {
      name: "app-version",
      transformIndexHtml: (html) => html.replaceAll("%APP_VERSION%", version),
    },
  ],
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: ["es2021", "chrome100", "safari13"],
    minify: !process.env.TAURI_DEBUG,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});
