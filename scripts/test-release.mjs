// Release workflow tests. They run on Windows, Linux, and macOS without PowerShell.
// External commands (gh, git) use stubs. The tests do not use the network.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import {
  cargoVersion,
  changelogForTag,
  evidence,
  expectedPackages,
  guard,
  requireToken,
  validate,
  verifyRemote,
} from "./release.mjs";
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
const baseEnv = {
  GITHUB_REF_TYPE: "tag",
  RELEASE_TAG: "v1.0.2",
  GITHUB_SHA: sha,
  GITHUB_REPOSITORY: "test/repo",
  GITHUB_SERVER_URL: "https://github.com",
  GITHUB_RUN_ID: "123",
  GITHUB_RUN_ATTEMPT: "1",
};
mkdirSync(join(root, "scratch"), { recursive: true });
const suiteRoot = mkdtempSync(`${root}/scratch/release-tests-`);
let count = 0;

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

function expect(name, action, errorText) {
  if (errorText) {
    assert.throws(action, (error) => error.message.includes(errorText), name);
  } else {
    action();
  }
  count += 1;
  console.log(`PASS: ${name}`);
}

// Workflow structure.
assert.ok(Object.hasOwn(check.on, "workflow_call"));
assert.deepEqual(check.on.push.branches, ["main"]);
assert.ok(check.on.push.paths.includes(".github/workflows/check.yml"));
assert.ok(!check.on.push.paths.some((path) => path.startsWith("src/")));
const warmer = check.jobs["warm-release-cache"];
assert.equal(
  warmer.if,
  "github.event_name == 'push' && github.ref == 'refs/heads/main'",
);
const cacheKey = (job) =>
  job.steps.find((step) => step.uses?.startsWith("Swatinem/rust-cache@"))?.with[
    "shared-key"
  ];
assert.equal(cacheKey(warmer), cacheKey(release.jobs["build-windows"]));
assert.ok(!JSON.stringify(warmer).includes("RELEASE_TOKEN"));
assert.ok(
  !warmer.steps.some((step) => step.uses?.includes("action-gh-release")),
);
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

// Each release step calls release.mjs. Node 24 is set before the first call.
const releaseSteps = {
  validate: { "Require a version tag": "validate" },
  "build-windows": {
    "Package Windows Artifacts": `package-windows --target \${{ matrix.target }} --arch \${{ matrix.arch }}`,
  },
  draft: {
    "Check release token": "require-token",
    "Check tag and release state": "guard",
    "Check packages and record evidence": "evidence",
    "Verify remote draft assets": "verify-remote",
  },
};
for (const [jobName, steps] of Object.entries(releaseSteps)) {
  const job = release.jobs[jobName];
  const nodeIndex = job.steps.findIndex(
    (step) =>
      step.uses?.startsWith("actions/setup-node@") &&
      String(step.with?.["node-version"]) === "24",
  );
  assert.ok(nodeIndex >= 0, `${jobName}: set Node 24.`);
  for (const [stepName, command] of Object.entries(steps)) {
    const index = job.steps.findIndex((step) => step.name === stepName);
    assert.ok(index > nodeIndex, `${jobName}: ${stepName} runs after Node.`);
    assert.equal(job.steps[index].run, `node scripts/release.mjs ${command}`);
    assert.equal(job.steps[index].shell, undefined);
  }
}
assert.ok(
  !/\bpwsh\b|powershell/i.test(
    readFileSync(`${root}/.github/workflows/release.yml`, "utf8"),
  ),
  "release.yml must not need PowerShell.",
);
count += 1;
console.log("PASS: release steps call release.mjs");

// Tag and version checks.
expect("valid version tag", () =>
  validate({ env: baseEnv, cwd: fixture("valid") }),
);
expect(
  "branch dispatch rejected",
  () =>
    validate({
      env: { ...baseEnv, GITHUB_REF_TYPE: "branch" },
      cwd: fixture("branch"),
    }),
  "Select an existing version tag",
);
for (const tag of ["v1.0.2/extra", "V1.0.2", "v1.0", "v1.0.2-", "1.0.2"]) {
  expect(
    `invalid tag ${tag} rejected`,
    () =>
      validate({
        env: { ...baseEnv, RELEASE_TAG: tag },
        cwd: fixture("invalid"),
      }),
    "Use a version tag",
  );
}
for (const [index, manifest] of [
  "Cargo.toml",
  "gui/src-tauri/Cargo.toml",
  "gui/package.json",
  "gui/src-tauri/tauri.conf.json",
].entries()) {
  expect(
    `version mismatch ${manifest}`,
    () =>
      validate({ env: baseEnv, cwd: fixture(`version-${index}`, manifest) }),
    "Version does not match",
  );
}
assert.equal(
  cargoVersion(
    '[workspace]\nversion = "0.0.1"\n\n[package]\r\nname = "x"\r\nversion = "1.2.3"\r\n[deps]\nversion = "9"\n',
  ),
  "1.2.3",
);
assert.equal(cargoVersion('[package]\nname = "x"\n'), undefined);
count += 1;
console.log("PASS: Cargo package version only");

// The command line reports errors on stderr with exit code 1.
const cli = (args, cwd, env) =>
  spawnSync(process.execPath, [join(root, "scripts/release.mjs"), ...args], {
    cwd,
    encoding: "utf8",
    windowsHide: true,
    env: { ...process.env, ...env },
  });
let result = cli(["validate"], fixture("cli-valid"), baseEnv);
assert.equal(result.status, 0, result.stderr);
result = cli(["validate"], fixture("cli-invalid"), {
  ...baseEnv,
  RELEASE_TAG: "bad",
});
assert.equal(result.status, 1);
assert.match(result.stderr, /^ERROR: Use a version tag/);
result = cli(["unknown"], suiteRoot, baseEnv);
assert.equal(result.status, 1);
assert.match(result.stderr, /Use release\.mjs </);
count += 1;
console.log("PASS: command line exit codes");

expect("token present", () =>
  requireToken({ env: { RELEASE_TOKEN: "synthetic" } }),
);
expect(
  "token missing rejected",
  () => requireToken({ env: {} }),
  "RELEASE_TOKEN secret is missing",
);

// Tag guard with stub gh and git.
function guardExec(testCase) {
  const calls = [];
  const exec = (command, args) => {
    calls.push([command, ...args].join(" "));
    if (command === "git") return { status: 0, stdout: `${sha}\n` };
    assert.equal(command, "gh");
    if (testCase === "api-failure") return { status: 1, stdout: "" };
    const request = args.join(" ");
    if (request.includes("/releases")) {
      if (testCase === "releases-failure") return { status: 1, stdout: "" };
      if (testCase === "published")
        return { status: 0, stdout: "v1.0.2\tfalse\nv1.0.1\tfalse\n" };
      if (testCase === "draft") return { status: 0, stdout: "v1.0.2\ttrue\n" };
      if (testCase === "other")
        return { status: 0, stdout: "v1.0.20\tfalse\n" };
      return { status: 0, stdout: "" };
    }
    if (request.includes("/git/tags/"))
      return {
        status: 0,
        stdout: JSON.stringify({ object: { type: "commit", sha } }),
      };
    if (testCase === "annotated")
      return {
        status: 0,
        stdout: JSON.stringify({ object: { type: "tag", sha: "tag-object" } }),
      };
    if (testCase === "moved")
      return {
        status: 0,
        stdout: JSON.stringify({
          object: { type: "commit", sha: "other-commit" },
        }),
      };
    if (testCase === "bad-json") return { status: 0, stdout: "not json" };
    return {
      status: 0,
      stdout: JSON.stringify({ object: { type: "commit", sha } }),
    };
  };
  return { exec, calls };
}
for (const name of ["valid", "annotated", "draft", "other"]) {
  const { exec, calls } = guardExec(name);
  expect(`tag guard ${name}`, () =>
    guard({ env: baseEnv, cwd: suiteRoot, exec }),
  );
  assert.equal(calls[0], "gh api repos/test/repo/git/ref/tags/v1.0.2");
  if (name === "annotated")
    assert.equal(calls[1], "gh api repos/test/repo/git/tags/tag-object");
}
for (const [name, message] of [
  ["moved", "release tag changed"],
  ["published", "already published"],
  ["api-failure", "Cannot read the release tag"],
  ["bad-json", "Cannot read the release tag"],
  ["releases-failure", "Cannot read the release list"],
]) {
  const { exec } = guardExec(name);
  expect(
    `tag guard ${name} rejected`,
    () => guard({ env: baseEnv, cwd: suiteRoot, exec }),
    message,
  );
}

// Package evidence.
expect("changelog keeps only the exact version and its subheadings", () => {
  const section = "## 1.1.0 — 2026-11-01\n\n### Added\n\n- Scheduled Verify.";
  assert.equal(
    changelogForTag(
      `# Changes\n\n${section}\n\n## 1.0.7\n\n- Old change.\n`,
      "v1.1.0",
    ),
    section,
  );
  assert.equal(
    changelogForTag("## 1.0.2\r\n\r\n- Change.\r\n", "v1.0.2"),
    "## 1.0.2\n\n- Change.",
  );
  assert.equal(
    changelogForTag("## v1.1.0-rc.1\n\n- Test.\n", "v1.1.0-rc.1"),
    "## v1.1.0-rc.1\n\n- Test.",
  );
});
for (const [name, text] of [
  ["missing", "## 1.0.20\n\n- Other version."],
  ["duplicate", "## 1.0.2\n\n- One.\n\n## 1.0.2\n\n- Two."],
  ["empty", "## 1.0.2\n\n## 1.0.1\n\n- Old."],
]) {
  expect(
    `changelog ${name} rejected`,
    () => changelogForTag(text, "v1.0.2"),
    name === "empty" ? "is empty" : "Expected one changelog section",
  );
}
const packageNames = expectedPackages("v1.0.2");
function packageFixture(name, omit, empty = false, extra = false) {
  const directory = fixture(name);
  writeFileSync(
    join(directory, "CHANGELOG.md"),
    "# Changes\n\n## 1.0.2\n\n- Synthetic release change.\n\n## 1.0.1\n\n- Old change.\n",
  );
  for (const license of ["LICENSE-MIT", "LICENSE-APACHE"]) {
    writeFileSync(join(directory, license), `Synthetic ${license}`);
  }
  mkdirSync(join(directory, "dist-out"));
  for (const filename of packageNames) {
    if (filename.startsWith("LICENSE")) continue;
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
const hash = (path) =>
  createHash("sha256").update(readFileSync(path)).digest("hex");
const good = packageFixture("packages");
const missingNotes = packageFixture("missing-version-notes");
writeFileSync(
  join(missingNotes, "CHANGELOG.md"),
  "## 9.0.0\n\n- Other version.\n",
);
expect(
  "package evidence rejects missing version notes before it writes output",
  () => evidence({ env: baseEnv, cwd: missingNotes }),
  "Expected one changelog section",
);
assert.ok(!existsSync(join(missingNotes, "release-notes.txt")));
assert.ok(!existsSync(join(missingNotes, "dist-out/release-metadata.json")));
expect("package evidence", () => evidence({ env: baseEnv, cwd: good }));
const metadata = JSON.parse(
  readFileSync(join(good, "dist-out/release-metadata.json"), "utf8"),
);
assert.equal(metadata.tag, "v1.0.2");
assert.equal(metadata.commit, sha);
assert.equal(metadata.run, "https://github.com/test/repo/actions/runs/123");
assert.equal(metadata.attempt, "1");
assert.equal(metadata.hardwareTests, "NO");
assert.deepEqual(
  metadata.packages.map((entry) => entry.name),
  packageNames,
);
for (const entry of metadata.packages) {
  const path = join(good, "dist-out", entry.name);
  assert.equal(entry.sha256, hash(path));
  assert.equal(entry.bytes, statSync(path).size);
}
const sums = readFileSync(join(good, "dist-out/SHA256SUMS"), "utf8")
  .trimEnd()
  .split("\n");
assert.equal(sums.length, packageNames.length + 1);
for (const line of sums) {
  const [value, filename] = line.split("  ");
  assert.equal(value, hash(join(good, "dist-out", filename)));
}
assert.ok(sums.at(-1).endsWith("  release-metadata.json"));
const notes = readFileSync(join(good, "release-notes.txt"), "utf8");
assert.ok(notes.startsWith("## 1.0.2\n\n- Synthetic release change.\n"));
assert.ok(notes.includes(`\n## Build and checks\n\nCommit: ${sha}\n`));
assert.ok(!notes.includes("Old change."));
assert.ok(notes.includes("Hardware tests in this run: NO."));
count += 1;
console.log("PASS: commit and all checksums");
expect(
  "missing package rejected",
  () =>
    evidence({ env: baseEnv, cwd: packageFixture("missing", packageNames[0]) }),
  "package list does not match",
);
expect(
  "extra package rejected",
  () =>
    evidence({
      env: baseEnv,
      cwd: packageFixture("extra", undefined, false, true),
    }),
  "package list does not match",
);
expect(
  "empty package rejected",
  () =>
    evidence({
      env: baseEnv,
      cwd: packageFixture("empty", packageNames[0], true),
    }),
  "package is empty",
);

// Remote draft check with a stub gh that reports the local files.
function remoteExec(testCase) {
  return (command, args) => {
    assert.equal(command, "gh");
    assert.deepEqual(args, ["api", "repos/test/repo/releases/123"]);
    if (testCase === "remote-api") return { status: 1, stdout: "" };
    const directory = join(good, "dist-out");
    let assets = readdirSync(directory).map((name) => ({
      name,
      size: statSync(join(directory, name)).size,
      digest: `sha256:${hash(join(directory, name))}`,
    }));
    if (testCase === "remote-extra")
      assets.push({ name: "stale.exe", size: 1, digest: "sha256:bad" });
    if (testCase === "remote-missing") assets = assets.slice(1);
    if (testCase === "remote-duplicate") assets.push({ ...assets[0] });
    if (testCase === "remote-hash") assets[0].digest = "sha256:bad";
    if (testCase === "remote-size") assets[0].size = 0;
    return {
      status: 0,
      stdout: JSON.stringify({
        draft: testCase !== "remote-published",
        tag_name: testCase === "remote-tag" ? "v9.9.9" : "v1.0.2",
        assets,
      }),
    };
  };
}
for (const [name, message] of [
  ["valid", undefined],
  ["remote-api", "Cannot read the uploaded release"],
  ["remote-extra", "remote release asset list does not match"],
  ["remote-missing", "remote release asset list does not match"],
  ["remote-duplicate", "remote release asset list does not match"],
  ["remote-hash", "remote asset size or hash does not match"],
  ["remote-size", "remote asset size or hash does not match"],
  ["remote-published", "not the expected draft"],
  ["remote-tag", "not the expected draft"],
]) {
  expect(
    `remote draft ${name}`,
    () =>
      verifyRemote({
        env: { ...baseEnv, RELEASE_ID: "123" },
        cwd: good,
        exec: remoteExec(name),
      }),
    message,
  );
}
expect(
  "remote draft without release ID rejected",
  () => verifyRemote({ env: baseEnv, cwd: good, exec: remoteExec("valid") }),
  "release ID is missing",
);
assert.ok(existsSync(join(good, "dist-out/SHA256SUMS")));
console.log(`RESULT: YES (${count} checks)`);
