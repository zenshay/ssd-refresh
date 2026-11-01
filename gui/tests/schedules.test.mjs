import assert from "node:assert/strict";
import test from "node:test";
import { createSchedules, taskResult } from "../src/schedules.ts";

test("a pending request blocks duplicate schedule changes", async () => {
  let finish;
  let creates = 0;
  const model = createSchedules({
    listSchedules: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
    createVerifySchedule: async () => {
      creates++;
      return [];
    },
    removeSchedules: async () => [],
  });
  let state;
  model.subscribe((value) => {
    state = value;
  });
  const loading = model.load();
  await model.create("synthetic.csv");
  assert.equal(creates, 0);
  assert.equal(state.busy, true);
  finish([]);
  await loading;
  assert.equal(state.busy, false);
  await model.create("synthetic.csv");
  assert.equal(creates, 1);
});

test("schedule errors remain visible and allow a later retry", async () => {
  let fail = true;
  const model = createSchedules({
    listSchedules: async () => {
      if (fail) throw new Error("Access denied");
      return [];
    },
    createVerifySchedule: async () => [],
    removeSchedules: async () => [],
  });
  let state;
  model.subscribe((value) => {
    state = value;
  });
  await model.load();
  assert.equal(state.message, "Access denied");
  assert.equal(state.busy, false);
  fail = false;
  await model.load();
  assert.notEqual(state.message, "Access denied");
});

test("task completion never claims that file data is safe", () => {
  const task = { state: "Ready", lastRun: "2026-11-01", result: 0 };
  assert.match(taskResult(task), /Read the verify log/);
  assert.match(taskResult({ ...task, result: 3 }), /Data mismatch/);
  assert.match(taskResult({ ...task, result: 1 }), /Did not finish/);
  assert.match(taskResult({ ...task, result: 2 }), /Unsupported/);
  assert.match(taskResult({ ...task, result: 4 }), /Stopped/);
  assert.match(
    taskResult({ ...task, result: 5 }),
    /Incomplete: the tool could not check all data/,
  );
  assert.equal(taskResult({ ...task, lastRun: "" }), "Not run");
});

test("a busy task shows the lock and retry policy", () => {
  assert.equal(
    taskResult({ state: "Ready", lastRun: "2026-11-01", result: 6 }),
    "Busy: another operation holds the lock. Windows can retry up to 3 times at 30-minute intervals.",
  );
});
