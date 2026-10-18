import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkArchitecture } from "./check-architecture.mjs";
import {
  astGrepCommand,
  capture,
  childRelative,
  windowsAstGrepCommand,
} from "./tools.mjs";

const base = join(tmpdir(), "ssd refresh 路徑");
assert.equal(
  childRelative(base, join(base, "src/verify/summary.rs")),
  "src/verify/summary.rs",
);
assert.equal(
  childRelative(base, join(base, "src/verify/../control.rs")),
  "src/control.rs",
);
assert.throws(
  () => childRelative(base, join(`${base}-other`, "src/control.rs")),
  /outside/,
);
assert.throws(() => childRelative(base, base), /outside/);
assert.throws(
  () => childRelative(base, join(base, "..", "outside.rs")),
  /outside/,
);
if (process.platform === "win32") {
  assert.equal(
    childRelative(
      base.toUpperCase(),
      join(base.toLowerCase(), "src/control.rs"),
    ),
    "src/control.rs",
  );
} else {
  assert.throws(
    () => childRelative(base.toUpperCase(), join(base, "src/control.rs")),
    /outside/,
  );
}
console.log("PATH TESTS: YES");

const fixture = mkdtempSync(join(tmpdir(), "ssd refresh boundary 路徑-"));
const write = (file, text) => writeFileSync(join(fixture, file), text);
try {
  if (process.platform === "win32") {
    const original = astGrepCommand();
    const npmFolder = join(fixture, "npm");
    mkdirSync(join(npmFolder, "node_modules/@ast-grep/cli"), {
      recursive: true,
    });
    writeFileSync(join(npmFolder, "ast-grep.cmd"), "");
    writeFileSync(
      join(npmFolder, "node_modules/@ast-grep/cli/ast-grep"),
      `const { spawnSync } = require('node:child_process');\nconst [command, ...prefix] = ${JSON.stringify(original)};\nconst result = spawnSync(command, [...prefix, ...process.argv.slice(2)], {stdio: 'inherit', windowsHide: true});\nprocess.exit(result.status ?? 1);\n`,
    );
    write("query.rs", "fn npm_entry_test() {}");
    const [command, ...prefix] = windowsAstGrepCommand([
      join(npmFolder, "ast-grep.cmd"),
    ]);
    assert.equal(command, process.execPath);
    const query = capture(command, [
      ...prefix,
      "run",
      "--kind",
      "function_item",
      "--json=compact",
      join(fixture, "query.rs"),
    ]);
    assert.equal(query.status, 0, query.stderr);
    assert.equal(JSON.parse(query.stdout)[0].text, "fn npm_entry_test() {}");
    console.log("WINDOWS NPM ENTRY TEST: YES");
  }
  for (const folder of ["src", "gui/src/views", "gui/src-tauri/src"])
    mkdirSync(join(fixture, folder), { recursive: true });
  for (const module of [
    "common",
    "control",
    "csv",
    "outcome",
    "text",
    "report",
    "progress",
    "reader",
    "scan",
    "refresh",
    "verify",
  ])
    write(`src/${module}.rs`, "");
  for (const module of ["actions", "store", "diagnostics"])
    write(`gui/src/${module}.ts`, "");
  write("gui/src-tauri/src/commands.rs", "");
  write("gui/src/views/view.ts", "");
  checkArchitecture(fixture);
  for (const [file, text, error] of [
    ["src/scan.rs", "use crate::cli;", /Dependency is not allowed/],
    ["src/scan.rs", 'fn test() { println!("test"); }', /writes terminal/],
    ["src/scan.rs", "fn test() { std::io::stdin(); }", /terminal or process/],
    [
      "src/scan.rs",
      "pub fn run() -> io::Result<i32> { Ok(0) }",
      /CLI exit code/,
    ],
    [
      "gui/src/actions.ts",
      'import { view } from "./views/view";',
      /Model imports a view/,
    ],
    [
      "gui/src/diagnostics.ts",
      'document.getElementById("test");',
      /Model uses the DOM/,
    ],
    [
      "gui/src-tauri/src/commands.rs",
      "use ssd_refresh::csv;",
      /GUI commands read CSV/,
    ],
  ]) {
    write(file, text);
    assert.throws(() => checkArchitecture(fixture), error);
    write(file, "");
  }
  write("gui/src/actions.ts", 'import { store } from "./store";');
  write("gui/src/store.ts", 'import { actions } from "./actions";');
  assert.throws(() => checkArchitecture(fixture), /Runtime import cycle/);
  write("gui/src/store.ts", 'import type { Actions } from "./actions";');
  checkArchitecture(fixture);
  console.log("BOUNDARY TESTS: YES (8 rejected cases)");
} finally {
  childRelative(tmpdir(), fixture);
  rmSync(fixture, { recursive: true, force: true });
}
