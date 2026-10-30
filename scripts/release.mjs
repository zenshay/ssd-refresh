// Release workflow steps. The workflow calls this script on each runner.
// Tests import the functions and give a stub for external commands.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { crc32, deflateRawSync } from "node:zlib";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const tagPattern = /^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$/;
const projectFiles = [
  "README.md",
  "README.zh-TW.md",
  "LICENSE-MIT",
  "LICENSE-APACHE",
  "SECURITY.md",
  "SUPPORT.md",
  "CONTRIBUTING.md",
];
const docFiles = ["docs/security.md", "docs/architecture.md"];

function defaultExec(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 64 * 1024 * 1024,
    ...options,
  });
  if (result.error) throw result.error;
  return { status: result.status, stdout: result.stdout ?? "" };
}

function context(options = {}) {
  return {
    env: options.env ?? process.env,
    cwd: options.cwd ?? process.cwd(),
    exec: options.exec ?? defaultExec,
  };
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function readText(path) {
  return readFileSync(path, "utf8").replace(/^﻿/, "");
}

function writeText(path, text) {
  writeFileSync(path, text, "utf8");
}

function sameNames(first, second) {
  const a = [...first].sort();
  const b = [...second].sort();
  return a.length === b.length && a.every((name, index) => name === b[index]);
}

function listFiles(directory) {
  return readdirSync(directory, { withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => entry.name);
}

export function cargoVersion(text) {
  const section = /^\[package\]\s*([\s\S]*?)(?=^\[|$(?![\s\S]))/m.exec(text);
  if (!section) return undefined;
  return /^version\s*=\s*"([^"]+)"/m.exec(section[1])?.[1];
}

// Check the tag name and the version in all four manifests.
export function validate(options) {
  const { env, cwd } = context(options);
  if (env.GITHUB_REF_TYPE !== "tag") {
    throw new Error(
      "Select an existing version tag when you start this workflow.",
    );
  }
  const tag = env.RELEASE_TAG ?? "";
  if (!tagPattern.test(tag)) {
    throw new Error("Use a version tag such as v1.0.3 or v1.0.3-rc.1.");
  }
  const version = tag.slice(1);
  for (const manifest of ["Cargo.toml", "gui/src-tauri/Cargo.toml"]) {
    if (cargoVersion(readText(join(cwd, manifest))) !== version) {
      throw new Error(`Version does not match the tag: ${manifest}`);
    }
  }
  for (const manifest of [
    "gui/package.json",
    "gui/src-tauri/tauri.conf.json",
  ]) {
    if (JSON.parse(readText(join(cwd, manifest))).version !== version) {
      throw new Error(`Version does not match the tag: ${manifest}`);
    }
  }
}

export function requireToken(options) {
  const { env } = context(options);
  if (!env.RELEASE_TOKEN) {
    throw new Error(
      'The RELEASE_TOKEN secret is missing. See docs/architecture.md, "Release token".',
    );
  }
}

function dosDateTime(date) {
  const year = Math.max(date.getFullYear(), 1980);
  return {
    time:
      (date.getHours() << 11) |
      (date.getMinutes() << 5) |
      Math.floor(date.getSeconds() / 2),
    date: ((year - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate(),
  };
}

function filesBelow(directory, prefix = "") {
  const names = [];
  for (const entry of readdirSync(join(directory, prefix), {
    withFileTypes: true,
  })) {
    const name = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) names.push(...filesBelow(directory, name));
    else if (entry.isFile()) names.push(name);
  }
  return names.sort();
}

// Write a ZIP file with all files below a folder. Names use "/" and UTF-8.
// The format has no ZIP64 records, so each file and the archive must stay below 4 GiB.
export function writeZip(output, sourceDirectory) {
  const parts = [];
  const central = [];
  let offset = 0;
  const names = filesBelow(sourceDirectory);
  if (names.length > 0xffff)
    throw new Error("Too many files for one ZIP file.");
  for (const name of names) {
    const path = join(sourceDirectory, name);
    const data = readFileSync(path);
    const packed = deflateRawSync(data, { level: 9 });
    const method = packed.length < data.length ? 8 : 0;
    const body = method === 8 ? packed : data;
    const nameBytes = Buffer.from(name, "utf8");
    const { time, date } = dosDateTime(statSync(path).mtime);
    const checksum = crc32(data);
    if (data.length > 0xfffffffe || offset > 0xfffffffe) {
      throw new Error(`The file is too large for this ZIP writer: ${name}`);
    }
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6);
    local.writeUInt16LE(method, 8);
    local.writeUInt16LE(time, 10);
    local.writeUInt16LE(date, 12);
    local.writeUInt32LE(checksum, 14);
    local.writeUInt32LE(body.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    const header = Buffer.alloc(46);
    header.writeUInt32LE(0x02014b50, 0);
    header.writeUInt16LE(20, 4);
    header.writeUInt16LE(20, 6);
    header.writeUInt16LE(0x0800, 8);
    header.writeUInt16LE(method, 10);
    header.writeUInt16LE(time, 12);
    header.writeUInt16LE(date, 14);
    header.writeUInt32LE(checksum, 16);
    header.writeUInt32LE(body.length, 20);
    header.writeUInt32LE(data.length, 24);
    header.writeUInt16LE(nameBytes.length, 28);
    header.writeUInt32LE(offset, 42);
    parts.push(local, nameBytes, body);
    central.push(header, nameBytes);
    offset += local.length + nameBytes.length + body.length;
  }
  const directory = Buffer.concat(central);
  if (offset + directory.length > 0xfffffffe) {
    throw new Error("The archive is too large for this ZIP writer.");
  }
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(names.length, 8);
  end.writeUInt16LE(names.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  writeFileSync(output, Buffer.concat([...parts, directory, end]));
}

// Make the GUI and CLI ZIP files, the bare GUI program, and its notice file.
export function packageWindows(target, arch, options) {
  const { env, cwd, exec } = context(options);
  if (!target || !arch) throw new Error("Give --target and --arch.");
  const tag = env.RELEASE_TAG;
  if (!tag) throw new Error("RELEASE_TAG is missing.");
  const out = join(cwd, "dist-out");
  mkdirSync(out, { recursive: true });
  const binary = (name) =>
    join(cwd, "target", target, "release", `${name}.exe`);
  for (const kind of ["gui", "cli"]) {
    const staging = join(out, "staging", kind);
    mkdirSync(join(staging, "docs"), { recursive: true });
    for (const file of [...projectFiles, ...docFiles]) {
      copyFileSync(join(cwd, file), join(staging, file));
    }
    const name = kind === "gui" ? "ssd-refresh-gui" : "ssd-refresh";
    const notices = exec(
      process.execPath,
      [
        join(scriptDirectory, "package-notices.mjs"),
        "--target",
        target,
        "--package",
        name,
        "--output",
        join(staging, "THIRD-PARTY-NOTICES.txt"),
      ],
      { cwd, stdio: "inherit" },
    );
    if (notices.status !== 0) {
      throw new Error(`Cannot collect notices for ${name}.`);
    }
    copyFileSync(binary(name), join(staging, `${name}.exe`));
    writeZip(
      join(out, `ssd-refresh-${kind}-${tag}-windows-${arch}.zip`),
      staging,
    );
  }
  const base = `ssd-refresh-gui-${tag}-windows-${arch}`;
  copyFileSync(binary("ssd-refresh-gui"), join(out, `${base}.exe`));
  copyFileSync(
    join(out, "staging/gui/THIRD-PARTY-NOTICES.txt"),
    join(out, `${base}-NOTICES.txt`),
  );
}

function ghJson(exec, cwd, args, message) {
  const result = exec("gh", args, { cwd });
  if (result.status !== 0) throw new Error(message);
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error(message);
  }
}

// Check that the remote tag still points to this commit and the release is not published.
export function guard(options) {
  const { env, cwd, exec } = context(options);
  const repo = env.GITHUB_REPOSITORY;
  const tag = env.RELEASE_TAG;
  let target = ghJson(
    exec,
    cwd,
    ["api", `repos/${repo}/git/ref/tags/${tag}`],
    "Cannot read the release tag.",
  ).object;
  // Annotated tags can point to other tag objects. Stop after 10 levels.
  for (let level = 0; target?.type === "tag"; level += 1) {
    if (level === 10) throw new Error("Cannot read the annotated tag.");
    target = ghJson(
      exec,
      cwd,
      ["api", `repos/${repo}/git/tags/${target.sha}`],
      "Cannot read the annotated tag.",
    ).object;
  }
  const head = exec("git", ["rev-parse", "HEAD"], { cwd });
  if (head.status !== 0) throw new Error("Cannot read the source commit.");
  if (target?.type !== "commit" || target.sha !== head.stdout.trim()) {
    throw new Error("The release tag changed during this run.");
  }
  const releases = exec(
    "gh",
    [
      "api",
      "--paginate",
      `repos/${repo}/releases`,
      "--jq",
      ".[] | [.tag_name, .draft] | @tsv",
    ],
    { cwd },
  );
  if (releases.status !== 0) throw new Error("Cannot read the release list.");
  for (const line of releases.stdout.split(/\r?\n/)) {
    const [name, draft] = line.split("\t");
    if (name === tag && draft !== "true") {
      throw new Error(
        "This release is already published. Use a new version tag.",
      );
    }
  }
}

export function expectedPackages(tag) {
  return [
    `ssd-refresh-gui-${tag}-windows-x64.zip`,
    `ssd-refresh-gui-${tag}-windows-x64.exe`,
    `ssd-refresh-cli-${tag}-windows-x64.zip`,
    `ssd-refresh-gui-${tag}-windows-arm64.zip`,
    `ssd-refresh-gui-${tag}-windows-arm64.exe`,
    `ssd-refresh-cli-${tag}-windows-arm64.zip`,
    `ssd-refresh-cli-${tag}-linux-x64.tar.gz`,
    `ssd-refresh-cli-${tag}-macos-universal.tar.gz`,
    `ssd-refresh-gui-${tag}-windows-x64-NOTICES.txt`,
    `ssd-refresh-gui-${tag}-windows-arm64-NOTICES.txt`,
    "LICENSE-MIT",
    "LICENSE-APACHE",
  ];
}

// Check the package list and write release-metadata.json, SHA256SUMS, and release-notes.txt.
export function evidence(options) {
  const { env, cwd } = context(options);
  const out = join(cwd, "dist-out");
  for (const license of ["LICENSE-MIT", "LICENSE-APACHE"]) {
    copyFileSync(join(cwd, license), join(out, license));
  }
  const expected = expectedPackages(env.RELEASE_TAG);
  if (!sameNames(expected, listFiles(out))) {
    throw new Error("The release package list does not match.");
  }
  const packages = expected.map((name) => {
    const path = join(out, name);
    const bytes = statSync(path).size;
    if (bytes === 0) throw new Error(`The package is empty: ${name}`);
    return { name, bytes, sha256: sha256(path) };
  });
  const run = `${env.GITHUB_SERVER_URL}/${env.GITHUB_REPOSITORY}/actions/runs/${env.GITHUB_RUN_ID}`;
  const metadata = {
    tag: env.RELEASE_TAG,
    commit: env.GITHUB_SHA,
    run,
    attempt: env.GITHUB_RUN_ATTEMPT,
    checks:
      "Windows, Linux, macOS: node check.mjs; native Tauri checks run on Windows only.",
    hardwareTests: "NO",
    packages,
  };
  const metadataPath = join(out, "release-metadata.json");
  writeText(metadataPath, `${JSON.stringify(metadata, null, 2)}\n`);
  const sums = packages.map((entry) => `${entry.sha256}  ${entry.name}`);
  sums.push(`${sha256(metadataPath)}  release-metadata.json`);
  writeText(join(out, "SHA256SUMS"), `${sums.join("\n")}\n`);
  writeText(
    join(cwd, "release-notes.txt"),
    `${[
      `Commit: ${env.GITHUB_SHA}`,
      `Checks: ${run}`,
      "Hardware tests in this run: NO.",
      "Windows programs are not signed. Archives include license texts and third-party notices.",
      "For a standalone Windows program, also download its NOTICES.txt and both LICENSE files.",
      "Review the packages and SHA256SUMS before you publish this draft.",
    ].join("\n")}\n`,
  );
}

// Compare the uploaded draft assets with the local files.
export function verifyRemote(options) {
  const { env, cwd, exec } = context(options);
  if (!env.RELEASE_ID) throw new Error("The release ID is missing.");
  const release = ghJson(
    exec,
    cwd,
    ["api", `repos/${env.GITHUB_REPOSITORY}/releases/${env.RELEASE_ID}`],
    "Cannot read the uploaded release.",
  );
  if (release.draft !== true || release.tag_name !== env.RELEASE_TAG) {
    throw new Error("The uploaded release is not the expected draft.");
  }
  const out = join(cwd, "dist-out");
  const names = listFiles(out);
  const assets = Array.isArray(release.assets) ? release.assets : [];
  if (
    !sameNames(
      names,
      assets.map((asset) => asset.name),
    )
  ) {
    throw new Error("The remote release asset list does not match.");
  }
  for (const name of names) {
    const path = join(out, name);
    const matches = assets.filter((asset) => asset.name === name);
    if (
      matches.length !== 1 ||
      matches[0].size !== statSync(path).size ||
      matches[0].digest !== `sha256:${sha256(path)}`
    ) {
      throw new Error(`The remote asset size or hash does not match: ${name}`);
    }
  }
}

function option(args, name) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
}

export function main(args) {
  const [command, ...rest] = args;
  const commands = {
    validate: () => validate(),
    "require-token": () => requireToken(),
    "package-windows": () =>
      packageWindows(option(rest, "--target"), option(rest, "--arch")),
    guard: () => guard(),
    evidence: () => evidence(),
    "verify-remote": () => verifyRemote(),
  };
  if (!Object.hasOwn(commands, command)) {
    throw new Error(
      `Use release.mjs <${Object.keys(commands).join(" | ")}> [options].`,
    );
  }
  commands[command]();
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`ERROR: ${error.message}`);
    process.exitCode = 1;
  }
}
