import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { astNodes, capture, childRelative, root } from "./tools.mjs";

const allowed = {
  sys: ["common", "control"],
  common: [],
  control: [],
  csv: ["output_lock"],
  outcome: [],
  output_lock: ["sys"],
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
    "output_lock",
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
    "output_lock",
    "progress",
    "reader",
    "report",
    "sys",
    "text",
  ],
};

// Expand only AST-selected use trees. Do not read comments or string contents.
function usePaths(text) {
  const tokens =
    text
      .replace(/\br#(?=\w)/g, "")
      .replace(/\/\*[\s\S]*?\*\/|\/\/[^\n]*/g, "")
      .replace(/^(?:pub(?:\([^)]*\))?\s+)?use\s+/, "")
      .match(/::|[A-Za-z_][A-Za-z_0-9]*|[{}*,;]/g) ?? [];
  let index = 0;
  const paths = [];
  function tree(prefix = []) {
    const path = [...prefix];
    while (index < tokens.length) {
      const token = tokens[index++];
      if (token === "{") {
        while (tokens[index] !== "}") {
          tree(path);
          if (tokens[index] === ",") index++;
          else if (tokens[index] !== "}")
            throw new Error("Unsupported Rust use tree");
        }
        index++;
        return;
      }
      if (token === "as") {
        const alias = tokens[index++];
        paths.push({ path, alias });
        return;
      }
      if (token !== "::") path.push(token);
      if ([",", "}", ";", undefined].includes(tokens[index])) {
        paths.push({ path });
        return;
      }
    }
  }
  tree();
  return paths;
}

function listFiles(folder, extension) {
  const found = capture("fd", [
    "--type",
    "f",
    "--extension",
    extension,
    ".",
    folder,
  ]);
  if (found.status !== 0)
    throw new Error(`Cannot list module files: ${folder}`);
  return found.stdout.trim().split(/\r?\n/).filter(Boolean);
}

export function checkArchitecture(base = root) {
  const source = join(base, "src");
  const rootTests = new Set(["gui_messages_tests", "lifecycle_tests"]);
  const lib = readFileSync(join(source, "lib.rs"));
  for (const node of astNodes("mod_item", [join(source, "lib.rs")])) {
    const name = /\bmod\s+(\w+)/.exec(node.text)?.[1];
    const testOnly =
      rootTests.has(name) &&
      /#\[cfg\(test\)\]\s*$/.test(
        lib.subarray(0, node.range.byteOffset.start).toString("utf8"),
      );
    if (!Object.hasOwn(allowed, name) && !testOnly)
      throw new Error(`Missing core module policy: ${name}`);
    if (node.text.includes("{"))
      throw new Error(`Unsupported inline core module: ${name}`);
  }
  const paths = Object.keys(allowed).flatMap((module) => {
    let file = join(base, `src/${module}.rs`);
    if (!existsSync(file)) file = join(source, module, "mod.rs");
    if (!existsSync(file)) throw new Error(`Missing core module: ${file}`);
    const folder = join(base, "src", module);
    if (!existsSync(folder) || !statSync(folder).isDirectory()) return [file];
    return [...new Set([file, ...listFiles(folder, "rs")])];
  });
  const modules = astNodes("mod_item", paths).sort(
    (a, b) => a.range.byteOffset.start - b.range.byteOffset.start,
  );
  function location(node) {
    const parts = childRelative(source, node.file)
      .replace(/\.rs$/, "")
      .split("/");
    if (parts.at(-1) === "mod") parts.pop();
    for (const scope of modules) {
      if (
        scope.file === node.file &&
        scope.text.includes("{") &&
        scope.range.byteOffset.start < node.range.byteOffset.start &&
        scope.range.byteOffset.end >= node.range.byteOffset.end
      ) {
        parts.push(/\bmod\s+(\w+)/.exec(scope.text)[1]);
      }
    }
    return parts;
  }
  function dependency(node, path, namespace = false) {
    const here = location(node);
    const module = here[0];
    let target;
    if (path[0] === "crate") target = path.slice(1);
    else if (path[0] === "self") target = [...here, ...path.slice(1)];
    else if (path[0] === "super") {
      target = [...here];
      while (path[0] === "super") {
        target.pop();
        path = path.slice(1);
      }
      target.push(...path);
    } else return;
    const name = target[0];
    if (!name && namespace) return;
    if (
      !name ||
      name === "*" ||
      (name !== module && !allowed[module].includes(name))
    )
      throw new Error(
        `Dependency is not allowed: ${module} -> ${name ?? "crate"}`,
      );
  }
  for (const node of astNodes("use_declaration", paths)) {
    for (const { path, alias } of usePaths(node.text)) {
      dependency(node, path);
      // Namespace aliases need name resolution. Reject them at this boundary.
      if (
        (path[0] === "crate" && path.length === 1) ||
        (alias &&
          (["io", "process"].includes(path[0]) ||
            (path[0] === "std" &&
              (path.length === 1 ||
                ["self", "io", "process"].includes(path[1]))))) ||
        (path.at(-1) === "*" && ["std", "io", "process"].includes(path[0]))
      )
        throw new Error(
          `Core module imports a terminal API or namespace alias: ${node.file}`,
        );
    }
  }
  for (const kind of ["scoped_identifier", "scoped_type_identifier"]) {
    for (const node of astNodes(kind, paths)) {
      dependency(node, node.text.replace(/\br#(?=\w)/g, "").split("::"), true);
    }
  }
  const reject = (kind, pattern, message, files = paths) => {
    for (const node of astNodes(kind, files)) {
      const text =
        node.language === "Rust"
          ? node.text.replace(/\br#(?=\w)/g, "")
          : node.text;
      if (pattern.test(text)) throw new Error(`${message}: ${node.file}`);
    }
  };
  // These two files own the existing platform terminal width calls.
  const platformTerminals = new Set(["sys/windows/terminal.rs", "sys/unix.rs"]);
  reject(
    "scoped_identifier",
    /^libc::(?:STDIN_FILENO|STDOUT_FILENO|STDERR_FILENO|isatty)$/,
    "Core module uses a platform terminal API",
    paths.filter((file) => !platformTerminals.has(childRelative(source, file))),
  );
  reject(
    "call_expression",
    /^(?:GetStdHandle|GetConsoleScreenBufferInfo)\s*\(/,
    "Core module uses a platform terminal API",
    paths.filter((file) => !platformTerminals.has(childRelative(source, file))),
  );
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
    "scoped_identifier",
    /^(?:(?:std::)?io::(?:stdin|stdout|stderr)|(?:std::)?process::exit)$/,
    "Core module uses a terminal or process API",
  );
  reject(
    "use_declaration",
    /\b(stdin|stdout|stderr|IsTerminal|print|println|eprint|eprintln)\b/,
    "Core module imports a terminal API",
  );
  reject(
    "function_item",
    /^pub fn run\([^)]*\)\s*->\s*io::Result<i32>/,
    "Core operation returns a CLI exit code",
  );

  const gui = join(base, "gui/src");
  const graph = new Map();
  const adapters = new Set(["main.ts", "dom.ts", "icons.ts"]);
  const isModel = (file) => !file.startsWith("views/") && !adapters.has(file);
  const modelFiles = listFiles(gui, "ts").filter((file) =>
    isModel(childRelative(gui, file)),
  );
  const edges = [
    ...astNodes("import_statement", [gui]),
    ...astNodes("export_statement", [gui]).filter((node) =>
      /\bfrom\s+["']/.test(node.text),
    ),
    ...astNodes("call_expression", [gui]).filter((node) =>
      /^import\s*\(/.test(node.text),
    ),
  ];
  for (const node of edges) {
    const module = childRelative(gui, node.file);
    if (!graph.has(module)) graph.set(module, []);
    const dynamic = /^import\s*\(/.test(node.text);
    const target = dynamic
      ? /^import\s*\(\s*["']([^"']+)["']\s*\)$/.exec(node.text)?.[1]
      : /(?:from\s+|^import\s*)["']([^"']+)["']/.exec(node.text)?.[1];
    if (dynamic && !target)
      throw new Error(`Unsupported dynamic import: ${module}`);
    if (!target?.startsWith(".")) continue;
    const file = resolve(
      dirname(node.file),
      target.endsWith(".ts") ? target : `${target}.ts`,
    );
    if (!existsSync(file)) throw new Error(`Missing import target: ${target}`);
    const dependency = childRelative(gui, file);
    if (
      isModel(module) &&
      (dependency.startsWith("views/") || adapters.has(dependency))
    ) {
      throw new Error(`Model imports a view: ${module} -> ${dependency}`);
    }
    const named = /^(?:import|export)\s*\{([^}]+)\}/s.exec(node.text);
    const typeOnly =
      /^(?:import|export)\s+type\b/.test(node.text) ||
      named?.[1]
        .replace(/\/\*[\s\S]*?\*\/|\/\/[^\n]*/g, " ")
        .split(",")
        .filter((part) => part.trim())
        .every((part) => /^\s*type\s+(?!\s*as\b)/.test(part));
    if (!typeOnly) graph.get(module).push(dependency);
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
  for (const kind of ["identifier", "shorthand_property_identifier"])
    reject(
      kind,
      /^(document|window|globalThis|self)$/,
      "Model uses the DOM",
      modelFiles,
    );
  reject(
    "scoped_identifier",
    /^(?:ssd_refresh::)?csv(?:::|$)/,
    "GUI commands read CSV results",
    [join(base, "gui/src-tauri/src/commands.rs")],
  );
  for (const node of astNodes("use_declaration", [
    join(base, "gui/src-tauri/src/commands.rs"),
  ])) {
    for (const { path } of usePaths(node.text)) {
      if (
        path[0] === "ssd_refresh" &&
        (path.length === 1 || ["csv", "*"].includes(path[1]))
      )
        throw new Error(`GUI commands read CSV results: ${node.file}`);
    }
  }
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
