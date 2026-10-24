import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

export const root = fileURLToPath(new URL("../", import.meta.url));

export function childRelative(base, path) {
  const result = relative(resolve(base), resolve(path));
  if (
    !result ||
    result === ".." ||
    result.startsWith(`..${sep}`) ||
    isAbsolute(result)
  ) {
    throw new Error(`Path is outside the base directory: ${path}`);
  }
  return result.split(sep).join("/");
}

export function capture(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 32 * 1024 * 1024,
    ...options,
  });
  if (result.error) throw result.error;
  return result;
}

export function runCheck(name, command, args, live = false) {
  console.log(`CHECK: ${name}`);
  const start = performance.now();
  return new Promise((resolveCheck, reject) => {
    let output = "";
    const child = spawn(command, args, {
      cwd: root,
      windowsHide: true,
      stdio: live ? "inherit" : ["ignore", "pipe", "pipe"],
    });
    if (!live) {
      for (const stream of [child.stdout, child.stderr]) {
        stream.setEncoding("utf8");
        stream.on("data", (text) => {
          output += text;
        });
      }
    }
    child.once("error", reject);
    child.once("close", (code, signal) => {
      if (output) process.stdout.write(output);
      console.log(
        `TIME: ${name}: ${((performance.now() - start) / 1000).toFixed(2)}s`,
      );
      if (code === 0) resolveCheck();
      else
        reject(
          new Error(`Check failed: ${name} (exit ${code}, signal ${signal})`),
        );
    });
  });
}

export async function runBoth(first, second) {
  // Wait for both lanes before a failure can end the main process.
  const results = await Promise.allSettled(
    [first, second].map((check) => Promise.resolve().then(check)),
  );
  const failures = results
    .filter((result) => result.status === "rejected")
    .map((result) => result.reason);
  if (failures.length) throw new AggregateError(failures, "Checks failed");
}

export function astNodes(kind, paths) {
  const [program, ...prefix] = astGrepCommand();
  const result = capture(program, [
    ...prefix,
    "run",
    "--kind",
    kind,
    "--json=compact",
    ...paths,
  ]);
  if (![0, 1].includes(result.status)) {
    throw new Error(`AST query failed: ${kind}\n${result.stderr}`);
  }
  return JSON.parse(result.stdout.replace(/^\uFEFF/, "") || "[]");
}

export function astGrepCommand() {
  if (process.platform !== "win32") return ["ast-grep"];
  const result = capture("where.exe", ["ast-grep"]);
  if (result.status !== 0)
    throw new Error("Install ast-grep before the check.");
  return windowsAstGrepCommand(result.stdout.trim().split(/\r?\n/));
}

export function windowsAstGrepCommand(shims) {
  for (const shim of shims) {
    if (shim.toLowerCase().endsWith(".exe")) return [shim];
    for (const entry of [
      "node_modules/@ast-grep/cli/ast-grep",
      "../@ast-grep/cli/ast-grep",
    ]) {
      const script = join(dirname(shim), entry);
      if (existsSync(script)) return [process.execPath, script];
    }
  }
  throw new Error(
    "Cannot find the ast-grep program behind its Windows command file.",
  );
}

// Windows cannot spawn a .cmd file without a shell. Use the installed program.
export function pnpmCommand() {
  if (process.platform !== "win32") return ["pnpm"];
  const result = capture("where.exe", ["pnpm"]);
  if (result.status !== 0) throw new Error("Install pnpm before the check.");
  for (const shim of result.stdout.trim().split(/\r?\n/)) {
    if (shim.endsWith(".exe")) return [shim];
    const folder = dirname(shim);
    for (const entry of ["node_modules/pnpm/pnpm.exe", "../pnpm/pnpm.exe"]) {
      const executable = join(folder, entry);
      if (existsSync(executable)) return [executable];
    }
    for (const entry of [
      "node_modules/pnpm/bin/pnpm.cjs",
      "node_modules/corepack/dist/pnpm.js",
      "../pnpm/bin/pnpm.cjs",
    ]) {
      const script = join(folder, entry);
      if (existsSync(script)) return [process.execPath, script];
    }
  }
  throw new Error(
    "Cannot find the pnpm program behind its Windows command file.",
  );
}
