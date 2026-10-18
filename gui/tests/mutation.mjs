// Targeted mutation tests. Production source files are never changed.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { astNodes } from "../../scripts/tools.mjs";
import { mutationStatus } from "./mutation-status.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const repo = resolve(root, "..");
const output = join(repo, "target", "mutation");
const offline = process.argv.includes("--offline");
const timeout = Number(process.env.SSD_REFRESH_MUTATION_TIMEOUT_MS ?? 120000);
assert.ok(
  Number.isFinite(timeout) && timeout >= 1000,
  "Invalid mutation timeout",
);
const report = {
  suite: "targeted architecture regression mutations",
  baseline: [],
  mutants: [],
  restored: [],
  result: "NO",
};
let work;

async function execute(command, arguments_, env = {}) {
  return new Promise((resolveResult) => {
    let text = "";
    let timedOut = false;
    const child = spawn(command, arguments_, {
      cwd: repo,
      windowsHide: true,
      detached: process.platform !== "win32",
      env: { ...process.env, SSD_REFRESH_MUTATION_OVERRIDES: "{}", ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (data) => {
      text += data;
    });
    child.stderr.on("data", (data) => {
      text += data;
    });
    const timer = setTimeout(() => {
      timedOut = true;
      if (process.platform === "win32") {
        spawnSync("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], {
          windowsHide: true,
        });
      } else {
        try {
          process.kill(-child.pid, "SIGKILL");
        } catch {
          /* The process can exit before the timeout handler. */
        }
      }
    }, timeout);
    child.once("error", (error) => {
      clearTimeout(timer);
      resolveResult({ code: null, timedOut: false, text: `${text}\n${error}` });
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      resolveResult({ code, timedOut, text });
    });
  });
}

function ast(file, kind, predicate) {
  const nodes = astNodes(kind, [join(repo, file)]).filter(predicate);
  assert.equal(
    nodes.length,
    1,
    `Mutation must match one AST node: ${file} (${nodes.length})`,
  );
  return nodes[0];
}

function body(text, replacement) {
  return `${text.slice(0, text.indexOf("{") + 1)}\n${replacement}\n}`;
}

const mutants = [
  {
    id: "rust-drop-cleanup",
    file: "src/progress.rs",
    kind: "function_item",
    match: (node) => /^fn drop\(/.test(node.text),
    change: (text) => body(text, ""),
  },
  {
    id: "rust-progress-events",
    file: "src/control.rs",
    kind: "function_item",
    match: (node) => /^pub fn report_progress\(/.test(node.text),
    change: (text) => body(text, "let _ = update;"),
  },
  {
    id: "rust-changed-count",
    file: "src/outcome.rs",
    kind: "macro_invocation",
    match: (node) => node.text.startsWith('matches!(status, "OK"'),
    change: () => 'matches!(status, "OK")',
  },
  {
    id: "rust-refresh-validation",
    file: "src/report.rs",
    kind: "function_item",
    match: (node) => /^pub fn read_refresh\(/.test(node.text),
    change: (text) => body(text, "checked_rows(path, &REFRESH_HEADER[..17])"),
  },
  {
    id: "rust-scan-schema",
    file: "src/report.rs",
    kind: "if_expression",
    match: (node) => node.text.startsWith("if !header.iter().any("),
    change: (text) =>
      text.replace("!header.iter().any(|name| name == column)", "false"),
  },
  {
    id: "rust-preview-limit",
    file: "src/report.rs",
    kind: "call_expression",
    match: (node) => node.text === "self.slow_files.truncate(500)",
    change: () => "self.slow_files.truncate(499)",
  },
  {
    id: "rust-eta",
    file: "src/progress.rs",
    kind: "function_item",
    match: (node) => /^pub fn eta_secs\(/.test(node.text),
    change: (text) => body(text, "Some(0)"),
  },
  {
    id: "frontend-cancel",
    file: "gui/src/actions.ts",
    kind: "function_declaration",
    match: (node) => /^async function runRefresh\(/.test(node.text),
    change: (text) =>
      text.replace(
        '!confirmed || store.get().mode !== "idle"',
        'store.get().mode !== "idle"',
      ),
  },
  {
    id: "frontend-unprocessed-reports",
    file: "gui/src/actions.ts",
    kind: "function_declaration",
    match: (node) => /^async function runRefresh\(/.test(node.text),
    change: (text) =>
      text.replace(
        "for (const root of attemptedRoots)",
        "for (const root of drivesWithSlow)",
      ),
  },
  {
    id: "frontend-log-limit",
    file: "gui/src/diagnostics.ts",
    kind: "function_declaration",
    match: (node) => /^function logMessage\(/.test(node.text),
    change: (text) => text.replace("logs.length > MAX_LOGS", "false"),
  },
  {
    id: "frontend-log-clear",
    file: "gui/src/diagnostics.ts",
    kind: "function_declaration",
    match: (node) => /^function clear\(/.test(node.text),
    change: (text) => body(text, ""),
  },
];
if (process.platform === "win32") {
  mutants.push({
    id: "rust-priority-restore",
    file: "src/sys/windows/priority.rs",
    kind: "function_item",
    match: (node) => /^fn drop\(/.test(node.text),
    change: (text) => body(text, ""),
  });
}

async function test(language, overrides = {}) {
  if (language === "rust") {
    const arguments_ = [
      "test",
      "--manifest-path",
      join(work, "Cargo.toml"),
      "--target-dir",
      join(output, "cargo"),
      "--lib",
      "--color",
      "never",
      "--jobs",
      "8",
    ];
    if (offline) arguments_.push("--offline");
    return execute("cargo", arguments_, { RUST_BACKTRACE: "0" });
  }
  return execute(
    process.execPath,
    [
      "--import",
      pathToFileURL(join(root, "tests/register.mjs")).href,
      "--test",
      "--test-reporter=tap",
      join(root, "tests/actions.test.mjs"),
    ],
    { SSD_REFRESH_MUTATION_OVERRIDES: JSON.stringify(overrides) },
  );
}

async function baseline(phase) {
  for (const language of ["rust", "frontend"]) {
    const result = await test(language);
    await writeFile(join(output, `${phase}-${language}.log`), result.text);
    const status = result.code === 0 && !result.timedOut ? "passed" : "failed";
    report[phase].push({ language, status });
    assert.equal(
      status,
      "passed",
      `${phase} failed: ${language}. Read target/mutation/${phase}-${language}.log`,
    );
  }
}

try {
  await mkdir(output, { recursive: true });
  work = await mkdtemp(join(output, "run-"));
  const files = spawnSync(
    "fd",
    ["--type", "f", "--extension", "rs", ".", join(repo, "src")],
    { encoding: "utf8", windowsHide: true },
  );
  assert.equal(files.status, 0, "Cannot list Rust source files");
  for (const file of files.stdout.trim().split(/\r?\n/)) {
    const destination = join(work, relative(repo, file));
    await mkdir(dirname(destination), { recursive: true });
    await copyFile(file, destination);
  }
  const manifest = await readFile(join(repo, "Cargo.toml"), "utf8");
  assert.equal(
    manifest.split('"gui/src-tauri"').length,
    2,
    "Unexpected workspace layout",
  );
  await writeFile(
    join(work, "Cargo.toml"),
    manifest.replace('"gui/src-tauri"', ""),
  );
  await copyFile(join(repo, "Cargo.lock"), join(work, "Cargo.lock"));
  console.log("MUTATION: baseline");
  await baseline("baseline");
  for (const mutant of mutants) {
    const node = ast(mutant.file, mutant.kind, mutant.match);
    const replacement = mutant.change(node.text);
    assert.notEqual(
      replacement,
      node.text,
      `Mutation made no change: ${mutant.id}`,
    );
    const original = await readFile(join(repo, mutant.file));
    const { start, end } = node.range.byteOffset;
    assert.equal(
      original.subarray(start, end).toString("utf8"),
      node.text,
      "AST source range does not match",
    );
    const destination = join(work, mutant.file);
    await mkdir(dirname(destination), { recursive: true });
    await writeFile(
      destination,
      Buffer.concat([
        original.subarray(0, start),
        Buffer.from(replacement),
        original.subarray(end),
      ]),
    );
    let result;
    const language = mutant.file.startsWith("src/") ? "rust" : "frontend";
    try {
      result = await test(language, { [join(repo, mutant.file)]: destination });
    } finally {
      await writeFile(destination, original);
    }
    const status = mutationStatus(result, language);
    await writeFile(join(output, `${mutant.id}.log`), result.text);
    report.mutants.push({
      id: mutant.id,
      file: mutant.file,
      status,
      log: `${mutant.id}.log`,
    });
    console.log(`MUTANT: ${status} ${mutant.id}`);
  }
  await baseline("restored");
  const killed = report.mutants.filter(
    (mutant) => mutant.status === "killed",
  ).length;
  if (killed === mutants.length) report.result = "YES";
  console.log(
    `MUTATION: ${report.result} (${killed}/${mutants.length} killed)`,
  );
  if (report.result !== "YES") process.exitCode = 1;
} catch (error) {
  report.error = String(error);
  console.error(error);
  process.exitCode = 1;
} finally {
  await writeFile(
    join(output, "report.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  if (work) {
    const child = relative(output, work);
    assert.ok(
      !isAbsolute(child) &&
        !child.startsWith(`..${sep}`) &&
        child !== ".." &&
        child.startsWith("run-"),
      "Unsafe mutation cleanup path",
    );
    await rm(work, { recursive: true, force: true });
  }
}
