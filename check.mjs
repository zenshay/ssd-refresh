import { join } from "node:path";
import { capture, pnpmCommand, root } from "./scripts/tools.mjs";

const options = new Set(process.argv.slice(2));
for (const option of options) {
  if (!["--core-only", "--skip-browser", "--offline"].includes(option)) {
    throw new Error(`Unknown check option: ${option}`);
  }
}
function run(name, command, args) {
  console.log(`CHECK: ${name}`);
  const result = capture(command, args, { stdio: "inherit" });
  if (result.status !== 0)
    throw new Error(`Check failed: ${name} (exit ${result.status})`);
}
const node = (name, file, args = []) =>
  run(name, process.execPath, [join(root, file), ...args]);
node("Path and architecture regression tests", "scripts/test-common.mjs");
node("Architecture", "scripts/check-architecture.mjs");
const manifest = ["--manifest-path", join(root, "Cargo.toml")];
const offline = options.has("--offline") ? ["--offline"] : [];
run("Rust format", "cargo", ["fmt", ...manifest, "--all", "--check"]);
const [pnpm, ...prefix] = pnpmCommand();
// Tauri needs the frontend files when it compiles in a clean checkout.
run("Frontend build", pnpm, [
  ...prefix,
  "--dir",
  join(root, "gui"),
  "run",
  "build",
]);
const scope = options.has("--core-only")
  ? ["-p", "ssd-refresh"]
  : ["--workspace"];
run("Rust tests", "cargo", [
  "test",
  ...manifest,
  ...offline,
  ...scope,
  "--all-targets",
]);
run("Rust lint", "cargo", [
  "clippy",
  ...manifest,
  ...offline,
  ...scope,
  "--all-targets",
  "--",
  "-D",
  "warnings",
]);
run("CLI help", "cargo", [
  "run",
  ...manifest,
  ...offline,
  "-p",
  "ssd-refresh",
  "--bin",
  "ssd-refresh",
  "--",
  "help",
]);
run("Shared script lint", pnpm, [
  ...prefix,
  "--dir",
  join(root, "gui"),
  "exec",
  "biome",
  "check",
  join(root, "check.mjs"),
  join(root, "scripts"),
]);
for (const task of [
  "typecheck",
  "lint",
  "test",
  ...(!options.has("--skip-browser") ? ["test:e2e"] : []),
]) {
  run(`Frontend ${task}`, pnpm, [
    ...prefix,
    "--dir",
    join(root, "gui"),
    "run",
    task,
  ]);
}
node("Targeted mutation tests", "gui/tests/mutation.mjs", offline);
console.log("RESULT: YES");
