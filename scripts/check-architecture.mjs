import { existsSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { astNodes, capture, childRelative, root } from "./tools.mjs";

const allowed = {
  common: [],
  control: [],
  csv: [],
  outcome: [],
  text: [],
  report: ["common", "csv", "outcome"],
  progress: ["control"],
  reader: ["common", "control", "sys"],
  scan: [
    "common",
    "control",
    "csv",
    "outcome",
    "progress",
    "reader",
    "report",
    "sys",
  ],
  refresh: [
    "common",
    "control",
    "csv",
    "outcome",
    "progress",
    "reader",
    "report",
    "sys",
  ],
  verify: [
    "common",
    "control",
    "csv",
    "outcome",
    "progress",
    "reader",
    "report",
    "sys",
    "text",
  ],
};

export function checkArchitecture(base = root) {
  const paths = Object.keys(allowed).flatMap((module) => {
    const file = join(base, `src/${module}.rs`);
    if (!existsSync(file)) throw new Error(`Missing core module: ${file}`);
    const folder = join(base, "src", module);
    if (!existsSync(folder) || !statSync(folder).isDirectory()) return [file];
    const found = capture("fd", [
      "--type",
      "f",
      "--extension",
      "rs",
      ".",
      folder,
    ]);
    if (found.status !== 0)
      throw new Error(`Cannot list module files: ${folder}`);
    return [file, ...found.stdout.trim().split(/\r?\n/).filter(Boolean)];
  });
  for (const kind of ["scoped_identifier", "scoped_type_identifier"]) {
    for (const node of astNodes(kind, paths)) {
      const match = /^crate::([a-z_]+)/.exec(node.text);
      if (!match) continue;
      const dependency = match[1];
      const module = childRelative(join(base, "src"), node.file)
        .split("/")[0]
        .replace(/\.rs$/, "");
      if (dependency !== module && !allowed[module].includes(dependency)) {
        throw new Error(
          `Dependency is not allowed: ${module} -> ${dependency}`,
        );
      }
    }
  }
  const reject = (kind, pattern, message, files = paths) => {
    for (const node of astNodes(kind, files)) {
      if (pattern.test(node.text)) throw new Error(`${message}: ${node.file}`);
    }
  };
  reject(
    "macro_invocation",
    /^(?:std::)?e?print(?:ln)?!/,
    "Core module writes terminal output",
  );
  reject(
    "call_expression",
    /^(?:(?:std::)?io::(?:stdin|stdout|stderr)|(?:std::)?process::exit)\s*\(/,
    "Core module uses a terminal or process API",
  );
  reject(
    "use_declaration",
    /\b(stdin|stdout|stderr|IsTerminal)\b/,
    "Core module imports a terminal API",
  );
  reject(
    "function_item",
    /^pub fn run\([^)]*\)\s*->\s*io::Result<i32>/,
    "Core operation returns a CLI exit code",
  );

  const gui = join(base, "gui/src");
  const graph = new Map();
  const models = ["actions.ts", "store.ts", "diagnostics.ts"];
  for (const node of astNodes("import_statement", [gui])) {
    const module = childRelative(gui, node.file);
    if (!graph.has(module)) graph.set(module, []);
    const target = /from\s+["']([^"']+)["']/.exec(node.text)?.[1];
    if (!target?.startsWith(".")) continue;
    const file = resolve(dirname(node.file), `${target}.ts`);
    if (!existsSync(file)) throw new Error(`Missing import target: ${target}`);
    const dependency = childRelative(gui, file);
    if (models.includes(module) && dependency.startsWith("views/")) {
      throw new Error(`Model imports a view: ${module} -> ${dependency}`);
    }
    if (!/^import\s+type\b/.test(node.text)) graph.get(module).push(dependency);
  }
  const visiting = new Set();
  const visited = new Set();
  function visit(module) {
    if (visiting.has(module))
      throw new Error(`Runtime import cycle: ${module}`);
    if (visited.has(module)) return;
    visiting.add(module);
    for (const dependency of graph.get(module) ?? []) visit(dependency);
    visiting.delete(module);
    visited.add(module);
  }
  for (const module of graph.keys()) visit(module);
  reject(
    "member_expression",
    /^(document|window)\./,
    "Model uses the DOM",
    models.map((file) => join(gui, file)),
  );
  reject(
    "scoped_identifier",
    /^(?:ssd_refresh::)?csv(?:::|$)/,
    "GUI commands read CSV results",
    [join(base, "gui/src-tauri/src/commands.rs")],
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  const args = process.argv.slice(2);
  if (
    args.length &&
    (args.length !== 2 || !["--root", "-Root"].includes(args[0]))
  ) {
    throw new Error("Use check-architecture.mjs [--root <path>].");
  }
  checkArchitecture(args[1] ? resolve(args[1]) : root);
  console.log("ARCHITECTURE: YES");
}
