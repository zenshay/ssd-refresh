import { spawnSync } from "node:child_process";
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

export function astNodes(kind, paths) {
  const result = capture("ast-grep", [
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
