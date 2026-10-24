import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { test } from "node:test";
import { astGrepCommand, runBoth, runCheck } from "./tools.mjs";

test("Both lanes start before either lane finishes", async () => {
  const started = [];
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const lane = async (name) => {
    started.push(name);
    if (started.length === 2) release();
    await gate;
  };
  await runBoth(
    () => lane("first"),
    () => lane("second"),
  );
  assert.deepEqual(started, ["first", "second"]);
});

test("A failed lane waits for the other lane and blocks later work", async () => {
  let completed = false;
  let laterWork = false;
  await assert.rejects(async () => {
    await runBoth(
      () => {
        throw new Error("First lane failed");
      },
      async () => {
        await new Promise((resolve) => setImmediate(resolve));
        completed = true;
      },
    );
    laterWork = true;
  }, AggregateError);
  assert.equal(completed, true);
  assert.equal(laterWork, false);
});

test("Both lane failures are reported", async () => {
  await assert.rejects(
    runBoth(
      () => Promise.reject(new Error("First")),
      () => Promise.reject(new Error("Second")),
    ),
    (error) => {
      assert.deepEqual(
        error.errors.map((item) => item.message),
        ["First", "Second"],
      );
      return true;
    },
  );
});

test("A child process must exit with code zero", async () => {
  await runCheck("Successful child", process.execPath, [
    "-e",
    "process.exit(0)",
  ]);
  await assert.rejects(
    runCheck("Failed child", process.execPath, ["-e", "process.exit(7)"]),
    /exit 7/,
  );
});

test("A process that cannot start fails the check", async () => {
  await assert.rejects(
    runCheck("Missing child", "ssd-refresh-missing-check-program", []),
  );
});

test("Tool lookup cache tracks PATH and PATHEXT and retries failures", {
  skip: process.platform !== "win32",
}, () => {
  const original = childProcess.spawnSync;
  const path = process.env.PATH;
  const extensions = process.env.PATHEXT;
  let calls = 0;
  let failed = false;
  childProcess.spawnSync = (command, args) => {
    assert.equal(command, "where.exe");
    assert.deepEqual(args, ["ast-grep"]);
    calls += 1;
    return { status: failed ? 1 : 0, stdout: "C:/synthetic/ast-grep.exe\r\n" };
  };
  syncBuiltinESMExports();
  try {
    process.env.PATH = `${path};C:/synthetic-cache`;
    const command = astGrepCommand();
    command.push("changed-by-caller");
    assert.deepEqual(astGrepCommand(), ["C:/synthetic/ast-grep.exe"]);
    assert.equal(calls, 1);
    process.env.PATH += "-changed";
    astGrepCommand();
    assert.equal(calls, 2);
    process.env.PATHEXT = `${extensions};.SYNTHETIC`;
    astGrepCommand();
    assert.equal(calls, 3);
    process.env.PATH += "-failure";
    failed = true;
    assert.throws(astGrepCommand, /Install ast-grep/);
    assert.throws(astGrepCommand, /Install ast-grep/);
    assert.equal(calls, 5);
    failed = false;
    astGrepCommand();
    astGrepCommand();
    assert.equal(calls, 6);
  } finally {
    childProcess.spawnSync = original;
    if (path === undefined) delete process.env.PATH;
    else process.env.PATH = path;
    if (extensions === undefined) delete process.env.PATHEXT;
    else process.env.PATHEXT = extensions;
    syncBuiltinESMExports();
  }
});
