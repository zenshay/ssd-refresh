import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import {
  basename,
  dirname,
  isAbsolute,
  join,
  relative,
  resolve,
  sep,
} from "node:path";
import { fileURLToPath } from "node:url";

const defaultRoot = fileURLToPath(new URL("../", import.meta.url));
const noticesDirectory = fileURLToPath(new URL("./notices/", import.meta.url));
const packageNames = new Set(["ssd-refresh", "ssd-refresh-gui"]);

function inside(base, candidate) {
  const child = relative(base, candidate);
  return (
    child === "" ||
    (!isAbsolute(child) && child !== ".." && !child.startsWith(`..${sep}`))
  );
}

function readJson(file, label) {
  try {
    return JSON.parse(readFileSync(file, "utf8").replace(/^\uFEFF/, ""));
  } catch {
    throw new Error(`Cannot read package metadata for ${label}.`);
  }
}

function licenseTexts(directory, label, declaredFile) {
  let base;
  let files;
  try {
    base = realpathSync(directory);
    files = readdirSync(base, { withFileTypes: true })
      .filter(
        (entry) =>
          entry.isFile() && /^(LICEN[CS]E|COPYING|NOTICE)/i.test(entry.name),
      )
      .map((entry) => join(base, entry.name));
  } catch {
    throw new Error(`Cannot read license files for ${label}.`);
  }
  if (declaredFile) {
    const declared = resolve(base, declaredFile);
    if (!inside(base, declared))
      throw new Error(`License file is outside the package for ${label}.`);
    try {
      const actual = realpathSync(declared);
      if (!inside(base, actual) || !statSync(actual).isFile())
        throw new Error();
      files.push(actual);
    } catch {
      throw new Error(`Cannot read the declared license file for ${label}.`);
    }
  }
  const texts = [...new Set(files)].sort().map((file) => {
    try {
      return { name: basename(file), text: readFileSync(file, "utf8") };
    } catch {
      throw new Error(`Cannot read license text for ${label}.`);
    }
  });
  if (!texts.some((file) => file.text.trim()))
    throw new Error(`No license text for ${label}.`);
  return texts;
}

export function readSupplement(item, directory = noticesDirectory) {
  const index = readJson(
    join(directory, "index.json"),
    "license supplement index",
  );
  const key = `${item.name}@${item.version}`;
  if (!Object.hasOwn(index, key)) return null;
  const entry = index[key];
  const vcs = readJson(
    join(dirname(item.manifest_path), ".cargo_vcs_info.json"),
    key,
  );
  if (
    item.repository?.replace(/\/+$/, "") !== entry.repository ||
    item.license !== entry.license ||
    vcs.git?.sha1 !== entry.commit ||
    (vcs.path_in_vcs ?? "") !== entry.path ||
    (entry.authors &&
      JSON.stringify(item.authors) !== JSON.stringify(entry.authors)) ||
    !Array.isArray(entry.files) ||
    !entry.files.length
  )
    throw new Error(
      `License supplement does not match the package source for ${key}.`,
    );
  return entry.files.map((file) => {
    if (
      basename(file.file) !== file.file ||
      !/^[a-f0-9]{64}$/.test(file.sha256) ||
      !file.source.startsWith("https://")
    )
      throw new Error(`Invalid license supplement for ${key}.`);
    let data;
    try {
      data = readFileSync(join(directory, file.file));
    } catch {
      throw new Error(`Cannot read the license supplement for ${key}.`);
    }
    if (
      createHash("sha256").update(data).digest("hex") !== file.sha256 ||
      !data.toString("utf8").trim()
    )
      throw new Error(`License supplement checksum failed for ${key}.`);
    return {
      name: file.file,
      text: data.toString("utf8"),
      source: file.source,
      sha256: file.sha256,
      sourceSha256: file.sourceSha256,
      normalization: file.normalization,
      sourceLines: file.sourceLines,
      kind: file.kind,
      packageSource: `${entry.repository}/tree/${entry.commit}/${entry.path}`,
    };
  });
}

export function readCargoMetadata(root, target, run = spawnSync) {
  const result = run(
    "cargo",
    [
      "metadata",
      "--locked",
      "--filter-platform",
      target,
      "--format-version",
      "1",
    ],
    {
      cwd: root,
      encoding: "utf8",
      windowsHide: true,
      maxBuffer: 64 * 1024 * 1024,
    },
  );
  if (result.error || result.status !== 0)
    throw new Error(
      "Cargo metadata failed. Check the lock file and installed packages.",
    );
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error("Cargo metadata did not return valid JSON.");
  }
}

function cargoNotices(metadata, packageName) {
  const workspace = new Set(metadata.workspace_members ?? []);
  const roots = metadata.packages.filter(
    (item) => item.name === packageName && workspace.has(item.id),
  );
  if (roots.length !== 1 || !metadata.resolve?.nodes)
    throw new Error(
      "Cannot find one selected workspace package and its dependency graph.",
    );
  const packages = new Map(metadata.packages.map((item) => [item.id, item]));
  const nodes = new Map(metadata.resolve.nodes.map((item) => [item.id, item]));
  const visited = new Set();
  const queue = [roots[0].id];
  const notices = [];
  const failures = [];
  while (queue.length) {
    const id = queue.pop();
    if (visited.has(id)) continue;
    visited.add(id);
    const item = packages.get(id);
    const node = nodes.get(id);
    if (!item || !Array.isArray(node?.deps))
      throw new Error("Cargo metadata has an incomplete dependency graph.");
    for (const dependency of node.deps) {
      if (!Array.isArray(dependency.dep_kinds))
        throw new Error("Cargo metadata has no dependency kinds.");
      if (dependency.dep_kinds.some((kind) => kind.kind !== "dev"))
        queue.push(dependency.pkg);
    }
    if (workspace.has(id)) continue;
    if (
      !item.source?.startsWith("registry+") &&
      !item.source?.startsWith("sparse+")
    ) {
      throw new Error(
        `Unsupported dependency source for ${item.name}@${item.version}.`,
      );
    }
    const label = `${item.name}@${item.version}`;
    try {
      let texts;
      try {
        texts = licenseTexts(
          dirname(item.manifest_path),
          label,
          item.license_file,
        );
      } catch (error) {
        if (!error.message.startsWith("No license text for")) throw error;
        texts = readSupplement(item);
        if (!texts) throw error;
      }
      notices.push({
        ecosystem: "Rust",
        name: item.name,
        version: item.version,
        authors: item.authors ?? [],
        license: item.license || "Not specified; see the license text.",
        url: `https://crates.io/crates/${encodeURIComponent(item.name)}/${encodeURIComponent(item.version)}`,
        texts,
      });
    } catch (error) {
      failures.push(
        `${label} [SPDX: ${item.license || "Not specified"}]: ${error.message}`,
      );
    }
  }
  return { notices, failures };
}

function npmDirectory(modules, from, name) {
  if (
    !/^(@[a-z0-9._-]+\/)?[a-z0-9._-]+$/i.test(name) ||
    name === "." ||
    name === ".."
  )
    throw new Error("Invalid JavaScript dependency name.");
  let directory = from;
  while (inside(modules, directory)) {
    const candidate = join(directory, "node_modules", name);
    if (existsSync(join(candidate, "package.json"))) return candidate;
    const parent = dirname(directory);
    if (parent === directory) break;
    directory = parent;
  }
  const candidate = join(modules, name);
  return existsSync(join(candidate, "package.json")) ? candidate : null;
}

function npmNotices(root) {
  let modules;
  try {
    modules = realpathSync(join(root, "gui/node_modules"));
  } catch {
    throw new Error("Install the GUI packages before collecting notices.");
  }
  const queue = [
    { directory: join(modules, "@tauri-apps/api"), name: "@tauri-apps/api" },
  ];
  const visited = new Set();
  const notices = [];
  while (queue.length) {
    const current = queue.pop();
    let directory;
    try {
      directory = realpathSync(current.directory);
    } catch {
      throw new Error(`Cannot find the JavaScript package ${current.name}.`);
    }
    if (!inside(modules, directory))
      throw new Error(
        `JavaScript package is outside node_modules: ${current.name}.`,
      );
    if (visited.has(directory)) continue;
    visited.add(directory);
    const item = readJson(join(directory, "package.json"), current.name);
    if (item.name !== current.name || !item.version)
      throw new Error(
        `Invalid JavaScript package metadata for ${current.name}.`,
      );
    const label = `${item.name}@${item.version}`;
    notices.push({
      ecosystem: "JavaScript",
      name: item.name,
      version: item.version,
      license:
        typeof item.license === "string"
          ? item.license
          : "Not specified; see the license text.",
      url: `https://www.npmjs.com/package/${item.name}/v/${encodeURIComponent(item.version)}`,
      texts: licenseTexts(directory, label),
    });
    const dependencies = new Set([
      ...Object.keys(item.dependencies ?? {}),
      ...Object.keys(item.optionalDependencies ?? {}),
      ...Object.keys(item.peerDependencies ?? {}),
    ]);
    for (const name of dependencies) {
      const dependency = npmDirectory(modules, directory, name);
      const optional =
        Object.hasOwn(item.optionalDependencies ?? {}, name) ||
        item.peerDependenciesMeta?.[name]?.optional;
      if (!dependency && optional) continue;
      if (!dependency)
        throw new Error(
          `Cannot find the JavaScript dependency ${name} for ${label}.`,
        );
      queue.push({ directory: dependency, name });
    }
  }
  return notices;
}

function selectedTargets({ target, targets = [target] }) {
  if (
    !targets.length ||
    targets.some((value) => !/^[a-z0-9_]+(?:-[a-z0-9_]+)+$/i.test(value))
  )
    throw new Error("Select Rust target triples.");
  return [...new Set(targets)];
}

export function collectNotices(metadata, options) {
  const { root, packageName } = options;
  const targets = selectedTargets(options);
  if (!packageNames.has(packageName))
    throw new Error("Select ssd-refresh or ssd-refresh-gui.");
  const graphs = Array.isArray(metadata) ? metadata : [metadata];
  if (graphs.length !== targets.length)
    throw new Error("Provide one Cargo dependency graph for each target.");
  const results = graphs.map((graph) => cargoNotices(graph, packageName));
  const failures = [
    ...new Set(results.flatMap((result) => result.failures)),
  ].sort();
  if (failures.length)
    throw new Error(
      `License collection failed for ${failures.length} Rust packages:\n${failures.join("\n")}`,
    );
  const unique = new Map();
  for (const item of results.flatMap((result) => result.notices)) {
    const key = `${item.ecosystem}:${item.name}:${item.version}`;
    const previous = unique.get(key);
    if (previous && JSON.stringify(previous) !== JSON.stringify(item))
      throw new Error(
        `License texts differ between targets for ${item.name}@${item.version}.`,
      );
    unique.set(key, item);
  }
  const notices = [...unique.values()];
  if (packageName === "ssd-refresh-gui") notices.push(...npmNotices(root));
  notices.sort((left, right) =>
    `${left.ecosystem}:${left.name}:${left.version}`.localeCompare(
      `${right.ecosystem}:${right.name}:${right.version}`,
      "en",
    ),
  );
  const sections = [
    "Third-party notices",
    `Package: ${packageName}`,
    `Targets: ${targets.join(", ")}`,
    "This file collects package license texts. It does not verify legal compliance.",
    "",
  ];
  for (const item of notices) {
    sections.push(
      "=".repeat(72),
      `${item.ecosystem}: ${item.name} ${item.version}`,
      `SPDX: ${item.license}`,
      `Source: ${item.url}`,
      "",
    );
    if (item.authors?.length)
      sections.push(
        `Package authors (metadata): ${item.authors.join("; ")}`,
        "",
      );
    if (item.ecosystem === "Rust")
      sections.push(
        `Source archive: https://crates.io/api/v1/crates/${encodeURIComponent(item.name)}/${encodeURIComponent(item.version)}/download`,
        "",
      );
    for (const file of item.texts) {
      if (file.kind) sections.push(`Text type: ${file.kind}`);
      if (file.source)
        sections.push(
          `Package source: ${file.packageSource}`,
          `License source: ${file.source}`,
          `SHA-256: ${file.sha256}`,
        );
      if (file.sourceLines)
        sections.push(`Source lines: ${file.sourceLines.join("-")}`);
      if (file.sourceSha256)
        sections.push(`Upstream SHA-256: ${file.sourceSha256}`);
      if (file.normalization)
        sections.push(`Stored text change: ${file.normalization}`);
      sections.push(`--- ${file.name} ---`, file.text, "");
    }
  }
  return {
    text: `${sections.join("\n")}\n`,
    rustCount: notices.filter((item) => item.ecosystem === "Rust").length,
    npmCount: notices.filter((item) => item.ecosystem === "JavaScript").length,
  };
}

export function writeNotices(options, metadata) {
  const targets = selectedTargets(options);
  const graphs =
    metadata ??
    targets.map((target) => readCargoMetadata(options.root, target));
  const result = collectNotices(graphs, { ...options, targets });
  mkdirSync(dirname(options.output), { recursive: true });
  writeFileSync(options.output, result.text, "utf8");
  return result;
}

function hostTarget() {
  const result = spawnSync("rustc", ["-vV"], {
    encoding: "utf8",
    windowsHide: true,
  });
  const target = result.stdout?.match(/^host: (\S+)$/m)?.[1];
  if (result.error || result.status !== 0 || !target)
    throw new Error("Cannot read the Rust host target. Use --target.");
  return target;
}

function main(args) {
  if (args.includes("--help")) {
    console.log(
      "Usage: node scripts/package-notices.mjs [--root DIR] [--target TRIPLE] [--package NAME] [--output FILE]",
    );
    console.log(
      "Default package: ssd-refresh. Default target: the Rust host target.",
    );
    console.log("Repeat --target to collect the union of target dependencies.");
    console.log(
      "Default output: dist-out/staging/THIRD-PARTY-NOTICES.txt under the project root.",
    );
    return;
  }
  const values = {};
  const targets = [];
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index];
    if (
      !["--root", "--target", "--package", "--output"].includes(key) ||
      !args[index + 1] ||
      args[index + 1].startsWith("--")
    )
      throw new Error("Use --help to read the supported options.");
    if (key === "--target") targets.push(args[index + 1]);
    else values[key] = args[index + 1];
  }
  const root = resolve(values["--root"] ?? defaultRoot);
  const result = writeNotices({
    root,
    targets: targets.length ? targets : [hostTarget()],
    packageName: values["--package"] ?? "ssd-refresh",
    output: resolve(
      root,
      values["--output"] ?? "dist-out/staging/THIRD-PARTY-NOTICES.txt",
    ),
  });
  console.log(
    `RESULT: YES (Rust packages: ${result.rustCount}; JavaScript packages: ${result.npmCount})`,
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`RESULT: NO (${error.message})`);
    process.exitCode = 1;
  }
}
