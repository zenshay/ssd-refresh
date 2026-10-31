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
    const native = join(
      npmFolder,
      `node_modules/@ast-grep/cli-win32-${process.arch}-msvc/ast-grep.exe`,
    );
    mkdirSync(join(native, ".."), { recursive: true });
    writeFileSync(native, "");
    assert.deepEqual(windowsAstGrepCommand([join(npmFolder, "ast-grep.cmd")]), [
      native,
    ]);
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
    "sys",
  ])
    write(`src/${module}.rs`, "");
  const lib =
    "pub mod common; pub mod control; pub mod csv; pub mod outcome; pub mod text; pub mod report; pub mod progress; pub mod reader; pub mod scan; pub mod refresh; pub mod verify; pub mod sys;";
  write("src/lib.rs", lib);
  for (const module of ["actions", "store", "diagnostics"])
    write(`gui/src/${module}.ts`, "");
  write("gui/src-tauri/src/commands.rs", "");
  write("gui/src/views/view.ts", "");
  checkArchitecture(fixture);
  let rejected = 0;
  for (const [file, text, error] of [
    ["src/scan.rs", "use crate::cli;", /Dependency is not allowed/],
    ["src/scan.rs", "use crate::{text};", /Dependency is not allowed/],
    [
      "src/scan.rs",
      "use crate::{common, text as words};",
      /Dependency is not allowed/,
    ],
    ["src/scan.rs", "use super::text;", /Dependency is not allowed/],
    [
      "src/scan.rs",
      "mod tests { use super::super::{text}; }",
      /Dependency is not allowed/,
    ],
    ["src/scan.rs", "use crate::*;", /Dependency is not allowed/],
    ["src/scan.rs", "use crate as core;", /Dependency is not allowed/],
    ["src/sys.rs", "use crate::{text};", /Dependency is not allowed/],
    ["src/scan.rs", "use std::r#io as console;", /terminal API/],
    ["src/sys.rs", "fn test() { GetStdHandle(0); }", /platform terminal API/],
    [
      "src/sys.rs",
      "fn test() { let fd = libc::STDOUT_FILENO; }",
      /platform terminal API/,
    ],
    ["src/scan.rs", 'fn test() { println!("test"); }', /writes terminal/],
    ["src/scan.rs", "fn test() { std::io::stdin(); }", /terminal or process/],
    [
      "src/scan.rs",
      "use std::io as console; fn test() { console::stdout(); }",
      /terminal API/,
    ],
    ["src/scan.rs", "use std::{io::{self as console}};", /terminal API/],
    ["src/scan.rs", "use std as platform;", /terminal API/],
    ["src/scan.rs", "use std::io; use io as console;", /terminal API/],
    ["src/scan.rs", "use std::io::*;", /terminal API/],
    [
      "src/scan.rs",
      "fn test() { let output = std::io::stdout; }",
      /terminal or process/,
    ],
    ["src/scan.rs", "use std::println as output;", /terminal API/],
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
      "gui/src/actions.ts",
      'export { view } from "./views/view";',
      /Model imports a view/,
    ],
    [
      "gui/src/store.ts",
      'export * from "./views/view";',
      /Model imports a view/,
    ],
    [
      "gui/src/diagnostics.ts",
      'void import("./views/view");',
      /Model imports a view/,
    ],
    [
      "gui/src/actions.ts",
      "void import(target);",
      /Unsupported dynamic import/,
    ],
    [
      "gui/src/actions.ts",
      'void import("./store" + target);',
      /Unsupported dynamic import/,
    ],
    [
      "gui/src/actions.ts",
      "const globals = { document };",
      /Model uses the DOM/,
    ],
    [
      "gui/src/actions.ts",
      'self.document.title = "test";',
      /Model uses the DOM/,
    ],
    ["gui/src/actions.ts", 'import "./views/view";', /Model imports a view/],
    [
      "gui/src/actions.ts",
      'globalThis.document.title = "test";',
      /Model uses the DOM/,
    ],
    [
      "gui/src/actions.ts",
      'const page = document; page.title = "test";',
      /Model uses the DOM/,
    ],
    [
      "gui/src/actions.ts",
      "const { document: page } = window;",
      /Model uses the DOM/,
    ],
    [
      "gui/src-tauri/src/commands.rs",
      "use ssd_refresh::csv;",
      /GUI commands read CSV/,
    ],
    [
      "gui/src-tauri/src/commands.rs",
      "use ssd_refresh::{csv};",
      /GUI commands read CSV/,
    ],
    [
      "gui/src-tauri/src/commands.rs",
      "use ssd_refresh::{csv as rows};",
      /GUI commands read CSV/,
    ],
    [
      "gui/src-tauri/src/commands.rs",
      "use ssd_refresh as core;",
      /GUI commands read CSV/,
    ],
  ]) {
    write(file, text);
    assert.throws(() => checkArchitecture(fixture), error);
    rejected++;
    write(file, "");
  }
  write("src/lib.rs", `${lib}\npub mod extra;`);
  write("src/extra.rs", "");
  assert.throws(() => checkArchitecture(fixture), /Missing core module policy/);
  rejected++;
  write("src/lib.rs", lib);
  write("src/lib.rs", "pub mod common { use crate::cli; }");
  assert.throws(
    () => checkArchitecture(fixture),
    /Unsupported inline core module/,
  );
  rejected++;
  write("src/lib.rs", `${lib}\nmod lifecycle_tests;`);
  assert.throws(() => checkArchitecture(fixture), /Missing core module policy/);
  rejected++;
  write("src/lib.rs", `${lib}\n#[cfg(test)]\nmod lifecycle_tests;`);
  checkArchitecture(fixture);
  write("src/lib.rs", lib);
  write("gui/src/new_model.ts", "const page = globalThis.document;");
  assert.throws(() => checkArchitecture(fixture), /Model uses the DOM/);
  rejected++;
  write("gui/src/new_model.ts", "");
  mkdirSync(join(fixture, "src/sys/windows"), { recursive: true });
  write("src/sys/windows/terminal.rs", "fn width() { GetStdHandle(0); }");
  write("src/sys/unix.rs", "fn width() { let fd = libc::STDOUT_FILENO; }");
  for (const [file, text] of [
    ["src/scan.rs", "use crate::{common::{Meta}, control::Control};"],
    [
      "src/scan.rs",
      "mod tests { use super::*; use super::super::common; mod child { use super::super::*; } }",
    ],
    ["src/scan.rs", "mod nested { use self::child::Value; mod child {} }"],
    ["src/sys.rs", "use crate::{common, control};"],
    ["src/sys.rs", "use std::io::{self, Error};"],
    [
      "gui/src/actions.ts",
      'const message = "document.title"; // window.alert()',
    ],
    ["gui/src/actions.ts", 'export type { Store } from "./store";'],
  ]) {
    write(file, text);
    checkArchitecture(fixture);
    write(file, "");
  }
  write("gui/src/actions.ts", 'import { store } from "./store";');
  write("gui/src/store.ts", 'import { actions } from "./actions";');
  assert.throws(() => checkArchitecture(fixture), /Runtime import cycle/);
  rejected++;
  write("gui/src/store.ts", 'import type { Actions } from "./actions";');
  checkArchitecture(fixture);
  write("gui/src/actions.ts", 'export { store } from "./store";');
  write("gui/src/store.ts", 'export { actions } from "./actions";');
  assert.throws(() => checkArchitecture(fixture), /Runtime import cycle/);
  rejected++;
  write("gui/src/store.ts", 'export { type Actions } from "./actions";');
  checkArchitecture(fixture);
  write("gui/src/actions.ts", 'void import("./store");');
  write("gui/src/store.ts", 'void import("./actions");');
  assert.throws(() => checkArchitecture(fixture), /Runtime import cycle/);
  rejected++;
  write("gui/src/store.ts", 'import { type Actions } from "./actions";');
  checkArchitecture(fixture);
  for (const syntax of ["import", "export"]) {
    for (const name of [
      "type as value",
      "type  as value",
      "type /* name of a value */ as value",
    ]) {
      write("gui/src/actions.ts", `${syntax} { ${name} } from "./store";`);
      write("gui/src/store.ts", `${syntax} { ${name} } from "./actions";`);
      assert.throws(() => checkArchitecture(fixture), /Runtime import cycle/);
      rejected++;
    }
  }
  console.log(`BOUNDARY TESTS: YES (${rejected} rejected cases)`);
} finally {
  childRelative(tmpdir(), fixture);
  rmSync(fixture, { recursive: true, force: true });
}
