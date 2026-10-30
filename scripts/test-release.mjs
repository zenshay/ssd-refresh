import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { root } from "./tools.mjs";

const require = createRequire(new URL("../gui/package.json", import.meta.url));
const { parse } = require("yaml");
const release = parse(
  readFileSync(`${root}/.github/workflows/release.yml`, "utf8"),
);
const check = parse(
  readFileSync(`${root}/.github/workflows/check.yml`, "utf8"),
);
const sha = "0123456789abcdef0123456789abcdef01234567";
const validation = release.jobs.validate.steps.find((step) => step.run).run;
const guard = release.jobs.draft.steps.find(
  (step) => step.name === "Check tag and release state",
).run;
const evidence = release.jobs.draft.steps.find(
  (step) => step.name === "Check packages and record evidence",
).run;
mkdirSync(join(root, "scratch"), { recursive: true });
const suiteRoot = mkdtempSync(`${root}/scratch/release-tests-`);
let count = 0;
const packageNames = [
  "ssd-refresh-gui-v1.0.2-windows-x64.zip",
  "ssd-refresh-gui-v1.0.2-windows-x64.exe",
  "ssd-refresh-cli-v1.0.2-windows-x64.zip",
  "ssd-refresh-gui-v1.0.2-windows-arm64.zip",
  "ssd-refresh-gui-v1.0.2-windows-arm64.exe",
  "ssd-refresh-cli-v1.0.2-windows-arm64.zip",
  "ssd-refresh-cli-v1.0.2-linux-x64.tar.gz",
  "ssd-refresh-cli-v1.0.2-macos-universal.tar.gz",
  "LICENSE-MIT",
  "LICENSE-APACHE",
  "ssd-refresh-gui-v1.0.2-windows-x64-NOTICES.txt",
  "ssd-refresh-gui-v1.0.2-windows-arm64-NOTICES.txt",
];

function fixture(name, mismatch) {
  const directory = join(suiteRoot, name);
  mkdirSync(join(directory, "gui/src-tauri"), { recursive: true });
  for (const manifest of ["Cargo.toml", "gui/src-tauri/Cargo.toml"]) {
    const version = mismatch === manifest ? "9.9.9" : "1.0.2";
    writeFileSync(
      join(directory, manifest),
      `[package]\nname = "fixture"\nversion = "${version}"\n\n[dependencies]\n`,
    );
  }
  for (const manifest of [
    "gui/package.json",
    "gui/src-tauri/tauri.conf.json",
  ]) {
    writeFileSync(
      join(directory, manifest),
      JSON.stringify({ version: mismatch === manifest ? "9.9.9" : "1.0.2" }),
    );
  }
  return directory;
}

function execute(directory, script, environment = {}) {
  const scriptFile = join(directory, "run.ps1");
  writeFileSync(scriptFile, `$ErrorActionPreference = 'Stop'\n${script}`);
  return spawnSync(
    "pwsh",
    ["-NoProfile", "-NonInteractive", "-File", scriptFile],
    {
      cwd: directory,
      encoding: "utf8",
      windowsHide: true,
      timeout: 30000,
      env: {
        ...process.env,
        GITHUB_REF_TYPE: "tag",
        RELEASE_TAG: "v1.0.2",
        GITHUB_SHA: sha,
        GITHUB_REPOSITORY: "test/repo",
        GITHUB_SERVER_URL: "https://github.com",
        GITHUB_RUN_ID: "123",
        GITHUB_RUN_ATTEMPT: "1",
        RELEASE_TEST_CASE: "valid",
        ...environment,
      },
    },
  );
}

function expect(name, result, success, errorText) {
  assert.ifError(result.error);
  assert.equal(
    result.status === 0,
    success,
    `${name}: ${result.stdout}\n${result.stderr}`,
  );
  if (errorText)
    assert.ok(result.stderr.includes(errorText), `${name}: ${result.stderr}`);
  count += 1;
  console.log(`PASS: ${name}`);
}

assert.ok(Object.hasOwn(check.on, "workflow_call"));
assert.equal(release.permissions.contents, "read");
assert.equal(release.jobs.check.uses, "./.github/workflows/check.yml");
assert.equal(release.jobs.check.needs, "validate");
for (const job of ["build-windows", "build-linux", "build-macos"]) {
  assert.equal(release.jobs[job].needs, "check");
  assert.ok(release.jobs.draft.needs.includes(job));
  assert.ok(
    !release.jobs[job].steps.some((step) =>
      step.uses?.includes("action-gh-release"),
    ),
  );
}
// The draft job writes with the RELEASE_TOKEN secret, not with GITHUB_TOKEN.
assert.equal(release.jobs.draft.permissions.contents, "read");
const draftStep = release.jobs.draft.steps.find(
  (step) => step.name === "Create or update draft",
);
assert.equal(draftStep.with.draft, true);
assert.equal(draftStep.with.token, `\${{ secrets.RELEASE_TOKEN }}`);
assert.ok(
  release.jobs.draft.steps.some((step) => step.name === "Check release token"),
);
for (const [name, job] of Object.entries(release.jobs))
  if (name !== "draft")
    assert.ok(
      !JSON.stringify(job).includes("RELEASE_TOKEN"),
      `Only the draft job can read RELEASE_TOKEN: ${name}`,
    );
assert.ok(!JSON.stringify(release.jobs.draft).includes("github.token"));
for (const workflow of [check, release])
  for (const job of Object.values(workflow.jobs)) {
    for (const step of job.steps || [])
      if (step.uses?.startsWith("actions/checkout@")) {
        assert.equal(step.with.ref, `\${{ github.sha }}`);
        assert.equal(step.with["persist-credentials"], false);
      }
  }
count += 1;
console.log("PASS: workflow gates and permissions");
expect("valid version tag", execute(fixture("valid"), validation), true);
expect(
  "branch dispatch rejected",
  execute(fixture("branch"), validation, { GITHUB_REF_TYPE: "branch" }),
  false,
  "Select an existing version tag",
);
expect(
  "invalid tag rejected",
  execute(fixture("invalid"), validation, { RELEASE_TAG: "v1.0.2/extra" }),
  false,
  "Use a version tag",
);
for (const [index, manifest] of [
  "Cargo.toml",
  "gui/src-tauri/Cargo.toml",
  "gui/package.json",
  "gui/src-tauri/tauri.conf.json",
].entries()) {
  expect(
    `version mismatch ${manifest}`,
    execute(fixture(`version-${index}`, manifest), validation),
    false,
    "Version does not match",
  );
}

const stub = `
function git { $global:LASTEXITCODE = 0; '${sha}' }
function gh {
  $global:LASTEXITCODE = 0
  $request = $args -join ' '
  if ($env:RELEASE_TEST_CASE -eq 'api-failure') { $global:LASTEXITCODE = 1; return }
  if ($request.Contains('/releases')) {
    if ($env:RELEASE_TEST_CASE -eq 'published') { "v1.0.2\tfalse" }
    if ($env:RELEASE_TEST_CASE -eq 'draft') { "v1.0.2\ttrue" }
    return
  }
  if ($request.Contains('/git/tags/')) { '{"object":{"type":"commit","sha":"${sha}"}}'; return }
  if ($env:RELEASE_TEST_CASE -eq 'annotated') { '{"object":{"type":"tag","sha":"tag-object"}}'; return }
  if ($env:RELEASE_TEST_CASE -eq 'moved') { '{"object":{"type":"commit","sha":"other-commit"}}'; return }
  '{"object":{"type":"commit","sha":"${sha}"}}'
}
`;
for (const name of ["valid", "annotated", "draft"]) {
  expect(
    `tag guard ${name}`,
    execute(fixture(`guard-${name}`), stub + guard, {
      RELEASE_TEST_CASE: name,
    }),
    true,
  );
}
expect(
  "moved tag rejected",
  execute(fixture("guard-moved"), stub + guard, { RELEASE_TEST_CASE: "moved" }),
  false,
  "release tag changed",
);
expect(
  "published release rejected",
  execute(fixture("guard-published"), stub + guard, {
    RELEASE_TEST_CASE: "published",
  }),
  false,
  "already published",
);
expect(
  "API failure rejected",
  execute(fixture("guard-api"), stub + guard, {
    RELEASE_TEST_CASE: "api-failure",
  }),
  false,
  "Cannot read the release tag",
);

function packageFixture(name, omit, empty = false, extra = false) {
  const directory = fixture(name);
  for (const license of ["LICENSE-MIT", "LICENSE-APACHE"]) {
    writeFileSync(join(directory, license), `Synthetic ${license}`);
  }
  mkdirSync(join(directory, "dist-out"));
  for (const filename of packageNames) {
    if (filename === omit && !empty) continue;
    writeFileSync(
      join(directory, "dist-out", filename),
      filename === omit && empty ? "" : `synthetic ${filename}`,
    );
  }
  if (extra)
    writeFileSync(join(directory, "dist-out/unexpected.txt"), "unexpected");
  return directory;
}
const good = packageFixture("packages");
expect("package evidence", execute(good, evidence), true);
const metadata = JSON.parse(
  readFileSync(join(good, "dist-out/release-metadata.json"), "utf8"),
);
assert.equal(metadata.commit, sha);
assert.equal(metadata.hardwareTests, "NO");
assert.equal(metadata.packages.length, packageNames.length);
for (const entry of metadata.packages) {
  assert.equal(
    entry.sha256,
    createHash("sha256")
      .update(readFileSync(join(good, "dist-out", entry.name)))
      .digest("hex"),
  );
}
for (const line of readFileSync(join(good, "dist-out/SHA256SUMS"), "utf8")
  .trim()
  .split(/\r?\n/)) {
  const [hash, filename] = line.split("  ");
  assert.equal(
    hash,
    createHash("sha256")
      .update(readFileSync(join(good, "dist-out", filename)))
      .digest("hex"),
  );
}
assert.ok(existsSync(join(good, "release-notes.txt")));
count += 1;
console.log("PASS: commit and all checksums");
expect(
  "missing package rejected",
  execute(packageFixture("missing", packageNames[0]), evidence),
  false,
  "package list does not match",
);
expect(
  "extra package rejected",
  execute(packageFixture("extra", undefined, false, true), evidence),
  false,
  "package list does not match",
);
expect(
  "empty package rejected",
  execute(packageFixture("empty", packageNames[0], true), evidence),
  false,
  "package is empty",
);
const remoteCheck = release.jobs.draft.steps.find(
  (step) => step.name === "Verify remote draft assets",
).run;
const remoteStub = `
function gh {
  $global:LASTEXITCODE = 0
  if ($env:RELEASE_TEST_CASE -eq 'remote-api') { $global:LASTEXITCODE = 1; return }
  $assets = @(Get-ChildItem -LiteralPath dist-out -File | ForEach-Object {
    @{ name = $_.Name; size = $_.Length; digest = 'sha256:' + (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
  })
  if ($env:RELEASE_TEST_CASE -eq 'remote-extra') { $assets += @{name='stale.exe'; size=1; digest='sha256:bad'} }
  if ($env:RELEASE_TEST_CASE -eq 'remote-missing') { $assets = @($assets | Select-Object -Skip 1) }
  if ($env:RELEASE_TEST_CASE -eq 'remote-hash') { $assets[0].digest = 'sha256:bad' }
  if ($env:RELEASE_TEST_CASE -eq 'remote-size') { $assets[0].size = 0 }
  @{ draft = ($env:RELEASE_TEST_CASE -ne 'remote-published'); tag_name=$env:RELEASE_TAG; assets=$assets } | ConvertTo-Json -Depth 5
}
`;
for (const [name, message] of [
  ["valid", undefined],
  ["remote-api", "Cannot read the uploaded release"],
  ["remote-extra", "remote release asset list does not match"],
  ["remote-missing", "remote release asset list does not match"],
  ["remote-hash", "remote asset size or hash does not match"],
  ["remote-size", "remote asset size or hash does not match"],
  ["remote-published", "not the expected draft"],
]) {
  expect(
    `remote draft ${name}`,
    execute(good, remoteStub + remoteCheck, {
      RELEASE_TEST_CASE: name,
      RELEASE_ID: "123",
    }),
    name === "valid",
    message,
  );
}
console.log(`RESULT: YES (${count} checks)`);
