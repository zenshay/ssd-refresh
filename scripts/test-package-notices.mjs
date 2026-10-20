import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  collectNotices,
  readCargoMetadata,
  readSupplement,
  writeNotices,
} from "./package-notices.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
mkdirSync(join(root, "target"), { recursive: true });
const fixture = mkdtempSync(join(root, "target/notices-test-"));
const packages = [];
const nodes = [];
function crate(
  name,
  source = "registry+https://github.com/rust-lang/crates.io-index",
  license = "MIT",
) {
  const directory = join(fixture, "registry", name);
  mkdirSync(directory, { recursive: true });
  writeFileSync(join(directory, "Cargo.toml"), "");
  const item = {
    id: name,
    name,
    version: "1.0.0",
    source,
    license,
    license_file: null,
    manifest_path: join(directory, "Cargo.toml"),
  };
  packages.push(item);
  nodes.push({ id: name, deps: [] });
  return directory;
}
function depends(from, to, kind = null) {
  nodes
    .find((node) => node.id === from)
    .deps.push({ pkg: to, dep_kinds: [{ kind, target: null }] });
}
const core = crate("ssd-refresh", null);
const gui = crate("ssd-refresh-gui", null);
const normal = crate("normal");
const transitive = crate("transitive", undefined, "MIT OR Apache-2.0");
const guiOnly = crate("gui-only");
crate("other-platform");
crate("dev-only");
for (const [directory, files] of [
  [
    normal,
    {
      "LICENSE-MIT": "Full MIT text.\nLine two.\n",
      NOTICE: "Full attribution text.\n",
    },
  ],
  [transitive, { COPYING: "Full transitive license.\n" }],
  [guiOnly, { LICENSE: "GUI license.\n" }],
]) {
  for (const [name, text] of Object.entries(files))
    writeFileSync(join(directory, name), text);
}
mkdirSync(join(normal, "nested"));
writeFileSync(join(normal, "nested/LICENSE"), "Do not scan nested files.");
depends("ssd-refresh", "normal");
depends("normal", "transitive", "build");
depends("ssd-refresh", "dev-only", "dev");
depends("ssd-refresh-gui", "ssd-refresh");
depends("ssd-refresh-gui", "gui-only");
const metadata = {
  packages,
  workspace_members: ["ssd-refresh", "ssd-refresh-gui"],
  resolve: { nodes },
};
const options = {
  root: fixture,
  target: "x86_64-pc-windows-msvc",
  packageName: "ssd-refresh",
  output: join(fixture, "staging/THIRD-PARTY-NOTICES.txt"),
};

let invoked = false;
assert.deepEqual(
  readCargoMetadata(fixture, options.target, (command, args, settings) => {
    invoked = true;
    assert.equal(command, "cargo");
    assert.deepEqual(args, [
      "metadata",
      "--locked",
      "--filter-platform",
      options.target,
      "--format-version",
      "1",
    ]);
    assert.equal(settings.cwd, fixture);
    assert.equal(settings.windowsHide, true);
    return { status: 0, stdout: JSON.stringify(metadata) };
  }),
  metadata,
);
assert.equal(invoked, true);
const cli = writeNotices(options, metadata);
assert.equal(cli.rustCount, 2);
assert.equal(cli.npmCount, 0);
assert.match(cli.text, /Full MIT text\.\nLine two\.\n/);
assert.match(cli.text, /Full attribution text/);
assert.match(cli.text, /Full transitive license/);
assert.match(cli.text, /SPDX: MIT OR Apache-2\.0/);
assert.match(cli.text, /https:\/\/crates\.io\/crates\/normal\/1\.0\.0/);
assert.doesNotMatch(
  cli.text,
  /gui-only|other-platform|dev-only|Do not scan nested/,
);
assert.ok(
  !cli.text.includes(fixture) &&
    !cli.text.includes(core) &&
    !cli.text.includes(gui),
);
assert.equal(readFileSync(options.output, "utf8"), cli.text);

const armOnly = crate("arm-only");
writeFileSync(join(armOnly, "LICENCE"), "Arm target license text.\n");
const armGraph = structuredClone(metadata);
armGraph.resolve.nodes
  .find((node) => node.id === "ssd-refresh")
  .deps.push({
    pkg: "arm-only",
    dep_kinds: [{ kind: null, target: null }],
  });
const universal = collectNotices([metadata, armGraph], {
  ...options,
  targets: ["x86_64-apple-darwin", "aarch64-apple-darwin"],
});
assert.equal(universal.rustCount, 3);
assert.equal((universal.text.match(/Rust: normal 1\.0\.0/g) ?? []).length, 1);
assert.match(universal.text, /Arm target license text/);
assert.match(
  universal.text,
  /Targets: x86_64-apple-darwin, aarch64-apple-darwin/,
);
assert.throws(
  () =>
    collectNotices(metadata, {
      ...options,
      targets: ["x86_64-apple-darwin", "aarch64-apple-darwin"],
    }),
  /one Cargo dependency graph for each target/,
);

const supplemental = join(fixture, "supplemental");
mkdirSync(supplemental);
const sourceDirectory = join(fixture, "supplement-source");
mkdirSync(sourceDirectory);
const sourcePackage = {
  name: "supplement-fixture",
  version: "1.0.0",
  repository: "https://github.com/example/synthetic",
  license: "MIT",
  manifest_path: join(sourceDirectory, "Cargo.toml"),
};
const supplementalText = "Synthetic full license text.\n";
writeFileSync(join(supplemental, "LICENSE.txt"), supplementalText);
writeFileSync(
  join(sourceDirectory, ".cargo_vcs_info.json"),
  JSON.stringify({ git: { sha1: "a".repeat(40) }, path_in_vcs: "fixture" }),
);
writeFileSync(
  join(supplemental, "index.json"),
  JSON.stringify({
    "supplement-fixture@1.0.0": {
      repository: sourcePackage.repository,
      commit: "a".repeat(40),
      path: "fixture",
      license: "MIT",
      files: [
        {
          file: "LICENSE.txt",
          source: "https://github.com/example/synthetic/LICENSE",
          sha256: createHash("sha256").update(supplementalText).digest("hex"),
        },
      ],
    },
  }),
);
assert.equal(
  readSupplement(sourcePackage, supplemental)[0].text,
  supplementalText,
);
assert.equal(
  readSupplement({ ...sourcePackage, version: "2.0.0" }, supplemental),
  null,
);
assert.throws(
  () =>
    readSupplement(
      { ...sourcePackage, repository: "https://github.com/example/other" },
      supplemental,
    ),
  /does not match the package source/,
);
writeFileSync(join(supplemental, "LICENSE.txt"), "Changed license text.");
assert.throws(
  () => readSupplement(sourcePackage, supplemental),
  /checksum failed/,
);
writeFileSync(join(supplemental, "LICENSE.txt"), supplementalText);
writeFileSync(
  join(sourceDirectory, ".cargo_vcs_info.json"),
  JSON.stringify({ git: { sha1: "b".repeat(40) }, path_in_vcs: "fixture" }),
);
assert.throws(
  () => readSupplement(sourcePackage, supplemental),
  /does not match the package source/,
);

const storedIndex = JSON.parse(
  readFileSync(join(root, "scripts/notices/index.json"), "utf8"),
);
for (const entry of Object.values(storedIndex))
  for (const file of entry.files) {
    const data = readFileSync(join(root, "scripts/notices", file.file));
    assert.equal(createHash("sha256").update(data).digest("hex"), file.sha256);
  }

function npm(
  name,
  dependencies = {},
  optionalDependencies = {},
  devDependencies = {},
) {
  const directory = join(fixture, "gui/node_modules", name);
  mkdirSync(directory, { recursive: true });
  writeFileSync(
    join(directory, "package.json"),
    JSON.stringify({
      name,
      version: "2.0.0",
      license: "Apache-2.0",
      dependencies,
      optionalDependencies,
      devDependencies,
    }),
  );
  writeFileSync(
    join(directory, "LICENSE-APACHE"),
    `${name} full license text.\n`,
  );
  return directory;
}
const api = npm(
  "@tauri-apps/api",
  { runtime: "*" },
  { "missing-optional": "*" },
  { "missing-development": "*" },
);
npm("runtime", { indirect: "*" });
npm("indirect");
npm("unused");
const desktop = collectNotices(metadata, {
  ...options,
  packageName: "ssd-refresh-gui",
});
assert.equal(desktop.rustCount, 3);
assert.equal(desktop.npmCount, 3);
assert.match(desktop.text, /@tauri-apps\/api full license text/);
assert.match(desktop.text, /runtime full license text/);
assert.match(desktop.text, /indirect full license text/);
assert.doesNotMatch(
  desktop.text,
  /missing-development|missing-optional|JavaScript: unused/,
);
assert.ok(!desktop.text.includes(fixture));

const apiFile = join(api, "package.json");
const apiMetadata = JSON.parse(readFileSync(apiFile, "utf8"));
writeFileSync(
  apiFile,
  JSON.stringify({ ...apiMetadata, dependencies: { "missing-required": "*" } }),
);
assert.throws(
  () =>
    collectNotices(metadata, { ...options, packageName: "ssd-refresh-gui" }),
  /Cannot find the JavaScript dependency missing-required/,
);
const unlicensed = join(fixture, "gui/node_modules/unlicensed");
mkdirSync(unlicensed);
writeFileSync(
  join(unlicensed, "package.json"),
  JSON.stringify({ name: "unlicensed", version: "1.0.0", license: "MIT" }),
);
writeFileSync(
  apiFile,
  JSON.stringify({ ...apiMetadata, dependencies: { unlicensed: "*" } }),
);
assert.throws(
  () =>
    collectNotices(metadata, { ...options, packageName: "ssd-refresh-gui" }),
  /No license text for unlicensed@1\.0\.0/,
);
writeFileSync(apiFile, JSON.stringify(apiMetadata));

const missing = crate("missing-license");
depends("ssd-refresh", "missing-license");
assert.throws(
  () => writeNotices(options, metadata),
  /No license text for missing-license@1\.0\.0/,
);
assert.equal(readFileSync(options.output, "utf8"), cli.text);
writeFileSync(join(missing, "LICENSE"), "\n");
assert.throws(() => collectNotices(metadata, options), /No license text/);
writeFileSync(join(missing, "LICENSE"), "Missing crate license text.\n");
const missingPackage = packages.find((item) => item.name === "missing-license");
missingPackage.license_file = "../outside-license";
assert.throws(() => collectNotices(metadata, options), /outside the package/);
assert.equal(existsSync(join(fixture, "outside-license")), false);
missingPackage.license_file = "custom-permission.txt";
writeFileSync(
  join(missing, "custom-permission.txt"),
  "Declared license text.\n",
);
assert.match(collectNotices(metadata, options).text, /Declared license text/);
assert.throws(
  () => collectNotices({ ...metadata, resolve: null }, options),
  /dependency graph/,
);
assert.throws(
  () => readCargoMetadata(fixture, options.target, () => ({ status: 1 })),
  /Cargo metadata failed/,
);
assert.throws(
  () =>
    readCargoMetadata(fixture, options.target, () => ({
      status: 0,
      stdout: "invalid",
    })),
  /valid JSON/,
);
console.log(
  "RESULT: YES (scope, license texts, missing licenses, dependency graph, paths, JavaScript dependencies)",
);
