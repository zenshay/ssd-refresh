// Windows package step tests. They run on Windows, Linux, and macOS.
// A stub replaces package-notices.mjs. The tests read each ZIP file back.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join } from "node:path";
import { crc32, inflateRawSync } from "node:zlib";
import { packageWindows, writeZip } from "./release.mjs";
import { root } from "./tools.mjs";

// Read a ZIP file without extra packages. Returns { name: Buffer }.
function readZip(path) {
  const data = readFileSync(path);
  const end = data.length - 22;
  assert.equal(data.readUInt32LE(end), 0x06054b50, "ZIP end record");
  const total = data.readUInt16LE(end + 10);
  let position = data.readUInt32LE(end + 16);
  const entries = {};
  for (let index = 0; index < total; index += 1) {
    assert.equal(data.readUInt32LE(position), 0x02014b50, "ZIP central record");
    const flags = data.readUInt16LE(position + 8);
    const method = data.readUInt16LE(position + 10);
    const checksum = data.readUInt32LE(position + 16);
    const packedSize = data.readUInt32LE(position + 20);
    const size = data.readUInt32LE(position + 24);
    const nameLength = data.readUInt16LE(position + 28);
    const extraLength = data.readUInt16LE(position + 30);
    const commentLength = data.readUInt16LE(position + 32);
    const offset = data.readUInt32LE(position + 42);
    const name = data.toString(
      "utf8",
      position + 46,
      position + 46 + nameLength,
    );
    assert.equal(flags & 0x0800, 0x0800, `${name}: UTF-8 flag`);
    assert.ok(!name.includes("\\"), `${name}: use / in names`);
    assert.equal(data.readUInt32LE(offset), 0x04034b50, "ZIP local record");
    const localName = data.readUInt16LE(offset + 26);
    const localExtra = data.readUInt16LE(offset + 28);
    const start = offset + 30 + localName + localExtra;
    const body = data.subarray(start, start + packedSize);
    const content = method === 8 ? inflateRawSync(body) : Buffer.from(body);
    assert.ok(method === 0 || method === 8, `${name}: method ${method}`);
    assert.equal(content.length, size, `${name}: size`);
    assert.equal(crc32(content), checksum, `${name}: CRC-32`);
    entries[name] = content;
    position += 46 + nameLength + extraLength + commentLength;
  }
  return entries;
}

// A second reader that this project did not write: bsdtar on Windows and macOS,
// Python zipfile on Linux. Returns [command, args before the ZIP, args after the ZIP].
function systemReader() {
  if (process.platform === "win32")
    return [
      join(process.env.SystemRoot ?? "C:\\Windows", "System32/tar.exe"),
      ["-xf"],
      ["-C"],
    ];
  if (process.platform === "darwin") return ["/usr/bin/tar", ["-xf"], ["-C"]];
  return ["python3", ["-m", "zipfile", "-e"], []];
}

const output = join(root, "scratch");
mkdirSync(output, { recursive: true });
const fixtureRoot = mkdtempSync(join(output, "release packages & test-"));

// ZIP writer edge cases: empty file, Unicode name, nested folder, and data that does not compress.
const edge = join(fixtureRoot, "zip-edge");
const edgeFiles = {
  "empty.txt": Buffer.alloc(0),
  "docs/中文 name.md": Buffer.from("text\n".repeat(200)),
  "random.bin": Buffer.from(
    Array.from({ length: 4096 }, (_, index) => (index * 7919) % 251),
  ),
};
for (const [name, content] of Object.entries(edgeFiles)) {
  mkdirSync(dirname(join(edge, "in", name)), { recursive: true });
  writeFileSync(join(edge, "in", name), content);
}
writeZip(join(edge, "edge.zip"), join(edge, "in"));
const edgeEntries = readZip(join(edge, "edge.zip"));
assert.deepEqual(
  Object.keys(edgeEntries).sort(),
  Object.keys(edgeFiles).sort(),
);
for (const [name, content] of Object.entries(edgeFiles))
  assert.ok(edgeEntries[name].equals(content), name);
const [reader, before, after] = systemReader();
const extracted = join(edge, "reader-out");
mkdirSync(extracted);
const readerResult = spawnSync(
  reader,
  [...before, join(edge, "edge.zip"), ...after, extracted],
  { encoding: "utf8", windowsHide: true },
);
let independent = `NOT RUN (${basename(reader)} is not available)`;
if (!readerResult.error) {
  assert.equal(readerResult.status, 0, readerResult.stderr);
  for (const [name, content] of Object.entries(edgeFiles))
    assert.ok(
      readFileSync(join(extracted, name)).equals(content),
      `${basename(reader)}: ${name}`,
    );
  independent = `YES (${basename(reader)})`;
}

const target = "synthetic-windows-target";
const arch = "synthetic64";
const tag = "v0.0.0-test";
const files = [
  "README.md",
  "README.zh-TW.md",
  "LICENSE-MIT",
  "LICENSE-APACHE",
  "SECURITY.md",
  "SUPPORT.md",
  "CONTRIBUTING.md",
  "docs/security.md",
  "docs/architecture.md",
];
const filename = (kind, extension) =>
  `ssd-refresh-${kind}-${tag}-windows-${arch}.${extension}`;

for (const failure of ["none", "ssd-refresh-gui", "ssd-refresh"]) {
  const fixture = join(fixtureRoot, failure);
  mkdirSync(fixture);
  for (const file of files) {
    const path = join(fixture, file);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, `fixture:${file}\n`);
  }
  for (const kind of ["gui", "cli"]) {
    const name = kind === "gui" ? "ssd-refresh-gui" : "ssd-refresh";
    const path = join(fixture, "target", target, "release", `${name}.exe`);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, `binary:${kind}\n`);
  }
  const calls = [];
  const exec = (command, args, options) => {
    assert.equal(command, process.execPath);
    assert.equal(basename(args[0]), "package-notices.mjs");
    assert.deepEqual(
      [args[1], args[3], args[5], args.length],
      ["--target", "--package", "--output", 7],
    );
    assert.equal(options.cwd, fixture);
    calls.push({ target: args[2], package: args[4], output: args[6] });
    if (args[4] === failure) return { status: 17, stdout: "" };
    writeFileSync(args[6], `NOTICE ${args[4]} ${args[2]}`);
    return { status: 0, stdout: "" };
  };
  const run = () =>
    packageWindows(target, arch, {
      env: { RELEASE_TAG: tag },
      cwd: fixture,
      exec,
    });
  const expectedPackages =
    failure === "ssd-refresh-gui"
      ? ["ssd-refresh-gui"]
      : ["ssd-refresh-gui", "ssd-refresh"];
  if (failure !== "none") {
    assert.throws(run, { message: `Cannot collect notices for ${failure}.` });
    assert.deepEqual(
      calls.map((call) => call.package),
      expectedPackages,
    );
    const failedKind = failure === "ssd-refresh-gui" ? "gui" : "cli";
    assert.ok(
      !existsSync(join(fixture, "dist-out", filename(failedKind, "zip"))),
      "A failed package must not produce a ZIP.",
    );
    assert.ok(
      !existsSync(join(fixture, "dist-out", filename("gui", "exe"))),
      "A notice failure must stop the bare executable copy.",
    );
    continue;
  }
  run();
  assert.deepEqual(
    calls.map((call) => call.package),
    expectedPackages,
  );
  for (const call of calls) {
    assert.equal(call.target, target);
    const kind = call.package === "ssd-refresh-gui" ? "gui" : "cli";
    assert.equal(
      call.output,
      join(fixture, "dist-out", "staging", kind, "THIRD-PARTY-NOTICES.txt"),
    );
  }
  for (const kind of ["gui", "cli"]) {
    const name = kind === "gui" ? "ssd-refresh-gui" : "ssd-refresh";
    const entries = readZip(join(fixture, "dist-out", filename(kind, "zip")));
    assert.deepEqual(
      Object.keys(entries).sort(),
      [...files, `${name}.exe`, "THIRD-PARTY-NOTICES.txt"].sort(),
    );
    for (const file of files)
      assert.equal(entries[file].toString("utf8"), `fixture:${file}\n`);
    assert.equal(entries[`${name}.exe`].toString("utf8"), `binary:${kind}\n`);
    assert.equal(
      entries["THIRD-PARTY-NOTICES.txt"].toString("utf8"),
      `NOTICE ${name} ${target}`,
    );
  }
  assert.equal(
    readFileSync(join(fixture, "dist-out", filename("gui", "exe")), "utf8"),
    "binary:gui\n",
  );
  assert.equal(
    readFileSync(
      join(
        fixture,
        "dist-out",
        `ssd-refresh-gui-${tag}-windows-${arch}-NOTICES.txt`,
      ),
      "utf8",
    ),
    `NOTICE ssd-refresh-gui ${target}`,
  );
}
console.log("RELEASE PACKAGES: YES (2 ZIP files, 2 notice failures).");
console.log(`ZIP EDGE CASES: YES. SYSTEM READER: ${independent}.`);
console.log(`FIXTURES: ${fixtureRoot}`);
