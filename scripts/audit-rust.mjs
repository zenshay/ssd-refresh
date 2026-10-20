import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const manifest = fileURLToPath(new URL("../Cargo.toml", import.meta.url));
const endpoint = "https://api.osv.dev/v1/querybatch";
export const targets = [
  "x86_64-pc-windows-msvc",
  "aarch64-pc-windows-msvc",
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-gnu",
  "x86_64-apple-darwin",
  "aarch64-apple-darwin",
];
const roots = ["ssd-refresh", "ssd-refresh-gui"];
const registries = new Set([
  "registry+https://github.com/rust-lang/crates.io-index",
  "registry+https://index.crates.io/",
  "sparse+https://index.crates.io/",
]);

export function auditPlan(args) {
  const options = new Map();
  for (let index = 0; index < args.length; index += 2) {
    const flag = args[index];
    if (
      !["--target", "--package"].includes(flag) ||
      options.has(flag) ||
      !args[index + 1]
    ) {
      throw new Error("Use audit-rust.mjs [--target target] [--package name].");
    }
    options.set(flag, args[index + 1]);
  }
  const target = options.get("--target");
  const name = options.get("--package");
  if (target && !targets.includes(target))
    throw new Error("Unsupported target.");
  if (name && !roots.includes(name)) throw new Error("Unsupported package.");
  const plan = (target ? [target] : targets).flatMap((platform) => {
    const names = (name ? [name] : roots).filter(
      (entry) => entry === "ssd-refresh" || platform.includes("windows"),
    );
    return names.length ? [{ target: platform, packages: names }] : [];
  });
  if (!plan.length) throw new Error("The GUI is released only for Windows.");
  return plan;
}

export function readCargoMetadata(target) {
  const result = spawnSync(
    "cargo",
    [
      "metadata",
      "--manifest-path",
      manifest,
      "--locked",
      "--format-version",
      "1",
      "--filter-platform",
      target,
    ],
    {
      encoding: "utf8",
      windowsHide: true,
      stdio: ["ignore", "pipe", "inherit"],
      maxBuffer: 64 * 1024 * 1024,
      timeout: 120000,
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`Cargo metadata failed for ${target}: ${result.status}.`);
  }
  return JSON.parse(result.stdout);
}

export function reachablePackages(metadata, names) {
  if (
    !Array.isArray(metadata?.packages) ||
    !Array.isArray(metadata.workspace_members) ||
    !Array.isArray(metadata.resolve?.nodes)
  ) {
    throw new Error("Cargo metadata has no complete dependency graph.");
  }
  const packages = new Map(metadata.packages.map((entry) => [entry.id, entry]));
  const nodes = new Map(
    metadata.resolve.nodes.map((entry) => [entry.id, entry]),
  );
  const pending = names.map((name) => {
    const matches = metadata.packages.filter(
      (entry) =>
        entry.name === name && metadata.workspace_members.includes(entry.id),
    );
    if (matches.length !== 1)
      throw new Error(`Cargo root is missing: ${name}.`);
    return matches[0].id;
  });
  const visited = new Set();
  const found = new Map();
  while (pending.length) {
    const id = pending.pop();
    if (visited.has(id)) continue;
    visited.add(id);
    const entry = packages.get(id);
    const node = nodes.get(id);
    if (!entry || !Array.isArray(node?.deps)) {
      throw new Error("Cargo metadata has an incomplete dependency node.");
    }
    if (registries.has(entry.source)) {
      if (
        typeof entry.name !== "string" ||
        !entry.name ||
        typeof entry.version !== "string" ||
        !entry.version
      ) {
        throw new Error("Cargo package identity is missing.");
      }
      found.set(`${entry.name}@${entry.version}`, {
        name: entry.name,
        version: entry.version,
      });
    }
    for (const dependency of node.deps) {
      if (
        !Array.isArray(dependency.dep_kinds) ||
        !dependency.dep_kinds.length ||
        dependency.dep_kinds.some(
          (entry) => ![null, "build", "dev"].includes(entry.kind),
        )
      ) {
        throw new Error("Cargo dependency kinds are missing or unknown.");
      }
      if (dependency.dep_kinds.some((entry) => entry.kind !== "dev")) {
        pending.push(dependency.pkg);
      }
    }
  }
  return [...found.values()];
}

export async function queryOsv(packages, fetcher = fetch) {
  const findings = [];
  for (let offset = 0; offset < packages.length; offset += 500) {
    const batch = packages.slice(offset, offset + 500);
    const response = await fetcher(endpoint, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        queries: batch.map(({ name, version }) => ({
          package: { name, ecosystem: "crates.io" },
          version,
        })),
      }),
      signal: AbortSignal.timeout(30000),
    });
    if (!response.ok) throw new Error(`OSV HTTP failed: ${response.status}.`);
    const body = await response.json();
    if (!Array.isArray(body?.results) || body.results.length !== batch.length) {
      throw new Error("OSV did not return every package result.");
    }
    for (let index = 0; index < batch.length; index++) {
      const result = body.results[index];
      if (!result || typeof result !== "object" || Array.isArray(result)) {
        throw new Error("OSV returned an invalid result.");
      }
      if (
        Object.keys(result).some(
          (field) => !["vulns", "next_page_token"].includes(field),
        )
      ) {
        throw new Error("OSV returned unknown result fields.");
      }
      if (result.next_page_token) {
        throw new Error("OSV returned more pages. The audit is incomplete.");
      }
      if (result.vulns !== undefined && !Array.isArray(result.vulns)) {
        throw new Error("OSV returned invalid vulnerability data.");
      }
      for (const vulnerability of result.vulns ?? []) {
        if (typeof vulnerability?.id !== "string" || !vulnerability.id) {
          throw new Error("OSV returned a vulnerability without an ID.");
        }
        findings.push({ ...batch[index], id: vulnerability.id });
      }
    }
  }
  return findings;
}

export async function audit(args, dependencies = {}) {
  const readMetadata = dependencies.readMetadata ?? readCargoMetadata;
  const query = dependencies.query ?? queryOsv;
  const log = dependencies.log ?? console.log;
  const plan = auditPlan(args);
  const identities = new Map();
  const scopes = new Map();
  for (const entry of plan) {
    log(`CARGO: ${entry.target} (${entry.packages.join(", ")})`);
    const metadata = await readMetadata(entry.target);
    for (const name of entry.packages) {
      for (const identity of reachablePackages(metadata, [name])) {
        const key = `${identity.name}@${identity.version}`;
        identities.set(key, identity);
        if (!scopes.has(key)) scopes.set(key, new Set());
        scopes.get(key).add(`${name}:${entry.target}`);
      }
    }
  }
  const packages = [...identities.values()].sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(
      `${right.name}@${right.version}`,
    ),
  );
  const findings = await query(packages);
  for (const finding of findings) {
    const key = `${finding.name}@${finding.version}`;
    log(`FOUND: ${key} ${finding.id}`);
    log(`TARGETS: ${[...scopes.get(key)].join(", ")}`);
  }
  log(`PACKAGES: ${packages.length}. FINDINGS: ${findings.length}.`);
  log(`RESULT: ${findings.length ? "NO" : "YES"}`);
  return findings.length ? 1 : 0;
}

export async function runCli(args, dependencies = {}) {
  try {
    return await audit(args, dependencies);
  } catch (error) {
    (dependencies.log ?? console.error)(`AUDIT ERROR: ${error.message}`);
    return 1;
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  process.exitCode = await runCli(process.argv.slice(2));
}
