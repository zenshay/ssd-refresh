import { randomUUID } from "node:crypto";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import {
  pnpmCommand,
  root,
  runCheck as run,
  runBoth,
} from "./scripts/tools.mjs";

const options = new Set(process.argv.slice(2));
for (const option of options) {
  if (!["--core-only", "--skip-browser", "--offline"].includes(option)) {
    throw new Error(`Unknown check option: ${option}`);
  }
}
const node = (name, file, args = [], live = false) =>
  run(name, process.execPath, [join(root, file), ...args], live);
const manifest = ["--manifest-path", join(root, "Cargo.toml")];
const offline = options.has("--offline") ? ["--offline"] : [];
// A unique path prevents an old fixture from passing a new contract check.
const contractPath = join(
  root,
  "target/contracts",
  `verify-${process.pid}-${randomUUID()}.json`,
);
if (!options.has("--core-only"))
  process.env.SSD_REFRESH_CONTRACT_OUTPUT = contractPath;
const [pnpm, ...prefix] = pnpmCommand();

async function sharedChecks() {
  await node("Check runner regression tests", "scripts/test-check-runner.mjs");
  await node("Line ending regression tests", "scripts/test-eol.mjs");
  await node("Line endings", "scripts/check-eol.mjs");
  await node(
    "Path and architecture regression tests",
    "scripts/test-common.mjs",
  );
  await node("Architecture", "scripts/check-architecture.mjs");
  await node("Rust audit regression tests", "scripts/test-audit-rust.mjs");
  await node(
    "Package notice regression tests",
    "scripts/test-package-notices.mjs",
  );
}

async function releaseChecks() {
  await node("Release workflow regression tests", "scripts/test-release.mjs");
  await node("Release package tests", "scripts/test-release-packages.mjs");
}

async function applicationChecks() {
  await run("Rust format", "cargo", ["fmt", ...manifest, "--all", "--check"]);
  // Tauri needs the frontend files before the GUI can compile.
  await run("Frontend build", pnpm, [
    ...prefix,
    "--dir",
    join(root, "gui"),
    "run",
    "build",
  ]);
  const scope = options.has("--core-only")
    ? ["-p", "ssd-refresh"]
    : ["--workspace"];
  await run("Rust tests", "cargo", [
    "test",
    "--locked",
    ...manifest,
    ...offline,
    ...scope,
    "--all-targets",
  ]);
  if (!options.has("--core-only")) {
    await run("Verify IPC contract", process.execPath, [
      "--import",
      pathToFileURL(join(root, "gui/tests/register.mjs")).href,
      join(root, "gui/tests/verify-contract.mjs"),
      contractPath,
    ]);
  }
  await run("Rust lint", "cargo", [
    "clippy",
    "--locked",
    ...manifest,
    ...offline,
    ...scope,
    "--all-targets",
    "--",
    "-D",
    "warnings",
  ]);
  await run("CLI help", "cargo", [
    "run",
    ...manifest,
    "--locked",
    ...offline,
    "-p",
    "ssd-refresh",
    "--bin",
    "ssd-refresh",
    "--",
    "help",
  ]);
  await run("Shared script lint", pnpm, [
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
    ...(!options.has("--skip-browser") ? ["test:e2e", "test:layout"] : []),
  ]) {
    await run(`Frontend ${task}`, pnpm, [
      ...prefix,
      "--dir",
      join(root, "gui"),
      "run",
      task,
    ]);
  }
}

// Release cases run on all platforms. On Windows they run beside the other checks
// to balance the two lanes. On Linux and macOS they run after the shared checks.
await runBoth(
  async () => {
    await sharedChecks();
    if (process.platform === "win32") await applicationChecks();
    else await releaseChecks();
  },
  process.platform === "win32" ? releaseChecks : applicationChecks,
);
// Mutations share one copy and one Cargo target directory. Keep them serial.
await node("Targeted mutation tests", "gui/tests/mutation.mjs", offline, true);
console.log("RESULT: YES");
