import assert from "node:assert/strict";
import { test } from "node:test";
import { runBoth, runCheck } from "./tools.mjs";

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
