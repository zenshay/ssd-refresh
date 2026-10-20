import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptRoot = fileURLToPath(new URL("../", import.meta.url));
const args = process.argv.slice(2);
assert.ok(
  !args.length || (args.length === 2 && args[0] === "--root"),
  "Use test-release-packages.mjs [--root project].",
);
if (process.platform !== "win32") {
  console.log("RELEASE PACKAGES: NOT RUN (Windows only).");
} else {
  const root = args.length ? resolve(args[1]) : scriptRoot;
  const require = createRequire(join(root, "gui/package.json"));
  const { parse } = require("yaml");
  const workflow = parse(
    readFileSync(join(root, ".github/workflows/release.yml"), "utf8"),
  );
  const steps = workflow.jobs?.["build-windows"]?.steps;
  assert.ok(Array.isArray(steps), "The Windows build job is missing.");
  const matches = steps.filter(
    (step) => step.name === "Package Windows Artifacts",
  );
  assert.equal(matches.length, 1, "Use one Windows package step.");
  assert.equal(matches[0].shell, "pwsh");
  assert.equal(typeof matches[0].run, "string");
  const target = "synthetic-windows-target";
  const arch = "synthetic64";
  const tag = "v0.0.0-test";
  const step = matches[0].run
    .replaceAll(/\$\{\{ matrix\.target \}\}/g, target)
    .replaceAll(/\$\{\{ matrix\.arch \}\}/g, arch);
  assert.ok(
    !step.includes("${{"),
    "An unresolved workflow expression remains.",
  );
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
  const output = join(scriptRoot, "scratch");
  mkdirSync(output, { recursive: true });
  const fixtureRoot = mkdtempSync(join(output, "release packages & test-"));
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
    const powershell = `
$ErrorActionPreference = 'Stop'
function global:node {
  param([Parameter(ValueFromRemainingArguments = $true)][string[]]$NodeArgs)
  if ($NodeArgs.Count -ne 7 -or $NodeArgs[0] -ne 'scripts/package-notices.mjs' -or $NodeArgs[1] -ne '--target' -or $NodeArgs[3] -ne '--package' -or $NodeArgs[5] -ne '--output') {
    throw 'Only the notice command can use the node stub.'
  }
  $call = @{ target = $NodeArgs[2]; package = $NodeArgs[4]; output = $NodeArgs[6] }
  Add-Content -LiteralPath 'notice-calls.jsonl' -Value ($call | ConvertTo-Json -Compress) -Encoding utf8
  if ($NodeArgs[4] -eq $env:NOTICE_FAILURE) {
    $global:LASTEXITCODE = 17
    return
  }
  [IO.File]::WriteAllText($NodeArgs[6], "NOTICE $($NodeArgs[4]) $($NodeArgs[2])")
  $global:LASTEXITCODE = 0
}
`;
    const inspection = String.raw`
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archives = @{}
foreach ($file in Get-Item -Path 'dist-out/*.zip') {
  $archive = [IO.Compression.ZipFile]::OpenRead($file.FullName)
  try {
    $entries = @{}
    foreach ($entry in $archive.Entries) {
      if ($entry.Name -eq '') { continue }
      $reader = [IO.StreamReader]::new($entry.Open())
      try { $entries[$entry.FullName.Replace('\', '/')] = $reader.ReadToEnd() }
      finally { $reader.Dispose() }
    }
    $archives[$file.Name] = $entries
  } finally { $archive.Dispose() }
}
[IO.File]::WriteAllText('archives.json', ($archives | ConvertTo-Json -Depth 5))
`;
    const script = join(fixture, "package.ps1");
    writeFileSync(script, `${powershell}\n${step}\n${inspection}`, "utf8");
    const result = spawnSync(
      "pwsh",
      ["-NoProfile", "-NonInteractive", "-File", script],
      {
        cwd: fixture,
        encoding: "utf8",
        windowsHide: true,
        timeout: 60000,
        env: {
          ...process.env,
          RELEASE_TAG: tag,
          NOTICE_FAILURE: failure,
        },
      },
    );
    if (result.error) throw result.error;
    writeFileSync(
      join(fixture, "powershell.log"),
      `${result.stdout}\n${result.stderr}`,
    );
    const calls = readFileSync(join(fixture, "notice-calls.jsonl"), "utf8")
      .trim()
      .split(/\r?\n/)
      .map((line) => JSON.parse(line.replace(/^\uFEFF/, "")));
    const expectedPackages =
      failure === "ssd-refresh-gui"
        ? ["ssd-refresh-gui"]
        : ["ssd-refresh-gui", "ssd-refresh"];
    assert.deepEqual(
      calls.map((call) => call.package),
      expectedPackages,
    );
    for (const call of calls) {
      assert.equal(call.target, target);
      const kind = call.package === "ssd-refresh-gui" ? "gui" : "cli";
      assert.equal(
        call.output,
        `dist-out/staging/${kind}/THIRD-PARTY-NOTICES.txt`,
      );
    }
    const filename = (kind, extension) =>
      `ssd-refresh-${kind}-${tag}-windows-${arch}.${extension}`;
    if (failure !== "none") {
      assert.ok(
        result.stderr.includes(`Cannot collect notices for ${failure}.`),
        "The notice error must cause the failure.",
      );
      assert.notEqual(
        result.status,
        0,
        "A notice failure must stop packaging.",
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
    assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
    const archives = JSON.parse(
      readFileSync(join(fixture, "archives.json"), "utf8"),
    );
    assert.deepEqual(
      Object.keys(archives).sort(),
      [filename("gui", "zip"), filename("cli", "zip")].sort(),
    );
    for (const kind of ["gui", "cli"]) {
      const name = kind === "gui" ? "ssd-refresh-gui" : "ssd-refresh";
      const entries = archives[filename(kind, "zip")];
      assert.deepEqual(
        Object.keys(entries).sort(),
        [...files, `${name}.exe`, "THIRD-PARTY-NOTICES.txt"].sort(),
      );
      for (const file of files)
        assert.equal(entries[file], `fixture:${file}\n`);
      assert.equal(entries[`${name}.exe`], `binary:${kind}\n`);
      assert.equal(
        entries["THIRD-PARTY-NOTICES.txt"],
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
  console.log(`FIXTURES: ${fixtureRoot}`);
}
