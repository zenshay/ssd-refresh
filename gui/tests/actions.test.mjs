import assert from "node:assert/strict";
import test from "node:test";
import { createActions, nextStep, speedChange } from "../src/actions.ts";
import { validateVerifyOutcome } from "../src/api.ts";
import { createDiagnostics } from "../src/diagnostics.ts";
import { countOf, truncatePath, wasWere } from "../src/format.ts";
import {
  activeDriveText,
  createStore,
  initialState,
  refreshAction,
  STANDARD_USER_NOTICE,
  verifyAction,
} from "../src/store.ts";
import { mutationStatus } from "./mutation-status.mjs";

const scan = (root) => ({
  reportPath: `${root}-synthetic.csv`,
  totalFiles: 10,
  slowTotal: 2,
  fastAfterWait: 0,
  stillSlowAfterWait: 0,
  slowFiles: [],
  stopped: false,
});

test("mutation scoring rejects build errors and timeouts", () => {
  assert.equal(
    mutationStatus(
      { code: 1, timedOut: false, text: "error: could not compile" },
      "rust",
    ),
    "invalid",
  );
  assert.equal(
    mutationStatus(
      { code: null, timedOut: true, text: "test result: FAILED" },
      "rust",
    ),
    "timeout",
  );
  assert.equal(
    mutationStatus({ code: 0, timedOut: false, text: "" }, "rust"),
    "survived",
  );
  assert.equal(
    mutationStatus(
      { code: 1, timedOut: false, text: "test result: FAILED" },
      "rust",
    ),
    "killed",
  );
  assert.equal(
    mutationStatus(
      {
        code: 1,
        timedOut: false,
        text: "not ok 1 - import error ERR_TEST_FAILURE",
      },
      "frontend",
    ),
    "invalid",
  );
  assert.equal(
    mutationStatus(
      { code: 1, timedOut: false, text: "not ok 1 - assertion ERR_ASSERTION" },
      "frontend",
    ),
    "killed",
  );
});
const storeFor = () =>
  createStore({
    ...initialState,
    selectedRoots: ["X:", "Y:"],
    scans: { "X:": scan("X:"), "Y:": scan("Y:") },
  });
const backend = (overrides = {}) => ({
  listVolumes: async () => [],
  startScan: async (root) => scan(root),
  startRefresh: async () => ({ movedFiles: 2, stopped: false }),
  startVerify: async () => ({}),
  togglePause: async () => true,
  stop: async () => {},
  ...overrides,
});

test("cancel does not call refresh or invalidate reports", async () => {
  const store = storeFor();
  const calls = [];
  await createActions(
    store,
    async () => false,
    backend({ startRefresh: async () => calls.push(1) }),
  ).runRefresh();
  assert.equal(calls.length, 0);
  assert.equal(Object.keys(store.get().scans).length, 2);
  assert.equal(store.get().mode, "idle");
});
test("stop keeps reports for drives that did not run", async () => {
  const store = storeFor();
  const calls = [];
  await createActions(
    store,
    async () => true,
    backend({
      startRefresh: async (_, root) => {
        calls.push(root);
        return { movedFiles: 1, stopped: true };
      },
    }),
  ).runRefresh();
  assert.deepEqual(calls, ["X:"]);
  assert.equal(store.get().scans["X:"], undefined);
  assert.ok(store.get().scans["Y:"]);
  assert.match(store.get().notice.title, /stopped/);
});
test("failure clears transient state and invalidates the attempted report", async () => {
  const store = storeFor();
  await createActions(
    store,
    async () => true,
    backend({
      startRefresh: async () => {
        store.set({ paused: true, stopping: true });
        throw new Error("Synthetic failure");
      },
    }),
  ).runRefresh();
  assert.equal(store.get().mode, "idle");
  assert.equal(store.get().paused, false);
  assert.equal(store.get().stopping, false);
  assert.equal(store.get().scans["X:"], undefined);
  assert.ok(store.get().scans["Y:"]);
  assert.equal(store.get().notice.kind, "error");
});
test("only one confirmation can run at a time", async () => {
  const store = storeFor();
  let close;
  let confirmations = 0;
  const actions = createActions(
    store,
    () => {
      confirmations++;
      return new Promise((resolve) => {
        close = resolve;
      });
    },
    backend(),
  );
  const first = actions.runRefresh();
  await actions.runRefresh();
  assert.equal(confirmations, 1);
  close(false);
  await first;
});
test("stop between scan drives is reported as stopped", async () => {
  const store = storeFor();
  await createActions(
    store,
    async () => true,
    backend({
      startScan: async (root) => {
        store.set({ stopping: true });
        return scan(root);
      },
    }),
  ).runScan();
  assert.match(store.get().notice.title, /stopped/);
});
test("countOf and wasWere use the singular only for 1", () => {
  assert.equal(countOf(0, "file"), "0 files");
  assert.equal(countOf(1, "slow file"), "1 slow file");
  assert.equal(countOf(2, "drive"), "2 drives");
  assert.equal(countOf(1234, "file"), "1,234 files");
  assert.equal(countOf(3, "entry", "entries"), "3 entries");
  assert.equal(wasWere(1), "was");
  assert.equal(wasWere(0), "were");
  assert.equal(wasWere(2), "were");
});

test("truncatePath keeps short paths and cuts long paths in the middle", () => {
  assert.equal(truncatePath("C:\\a\\b.txt", 50), "C:\\a\\b.txt");
  const long = `C:\\${"d".repeat(80)}\\file-name.bin`;
  const short = truncatePath(long, 50);
  assert.equal(Array.from(short).length, 50);
  assert.ok(short.startsWith("C:\\ddd"));
  assert.ok(short.endsWith("\\file-name.bin"));
  assert.ok(short.includes("…"));
  // A cut must not split a character that uses two UTF-16 units.
  const emoji = truncatePath("😀".repeat(80), 10);
  assert.equal(emoji, `${"😀".repeat(4)}…${"😀".repeat(5)}`);
  assert.ok(emoji.isWellFormed());
});

test("diagnostic models are independent and keep only 500 entries", () => {
  const first = createDiagnostics();
  const second = createDiagnostics();
  let updates = 0;
  const unsubscribe = first.subscribe(() => updates++);
  for (let i = 0; i < 700; i++) first.logMessage("info", `Synthetic ${i}`);
  assert.equal(first.entries().length, 500);
  assert.equal(first.entries()[0].text, "Synthetic 200");
  assert.equal(second.entries().length, 0);
  unsubscribe();
  const count = updates;
  first.clear();
  assert.equal(updates, count);
  assert.equal(first.entries().length, 0);
});
test("refresh counts only the files that stay slow after the wait", async () => {
  const waited = (root, fast) => ({
    ...scan(root),
    slowTotal: 3,
    fastAfterWait: fast,
    stillSlowAfterWait: 3 - fast,
  });
  const store = createStore({
    ...initialState,
    selectedRoots: ["X:", "Y:"],
    scans: { "X:": waited("X:", 2), "Y:": waited("Y:", 3) },
  });
  assert.match(refreshAction(store.get()).title, /\(1 file\)/);
  let asked;
  await createActions(
    store,
    async (request) => {
      asked = request;
      return false;
    },
    backend(),
  ).runRefresh();
  assert.match(asked.title, /^Rewrite 1 slow file across X:\?$/u);
  assert.match(asked.body, /2 files were fast after the wait/);

  store.set({ scans: { "X:": waited("X:", 3), "Y:": waited("Y:", 3) } });
  const locked = refreshAction(store.get());
  assert.equal(locked.disabled, true);
  assert.match(locked.hint, /All 6 slow files were fast after the wait/);
});

test("refresh is locked for a standard user, also with slow files", () => {
  const slow = { ...scan("X:"), slowTotal: 2, stillSlowAfterWait: 2 };
  const s = { ...initialState, selectedRoots: ["X:"], scans: { "X:": slow } };
  const locked = refreshAction({ ...s, elevated: false });
  assert.equal(locked.disabled, true);
  assert.match(locked.hint, /administrator/);
  // Unknown (before the app asks) and administrator do not lock it.
  assert.equal(refreshAction({ ...s, elevated: null }).disabled, false);
  assert.equal(refreshAction({ ...s, elevated: true }).disabled, false);
  assert.equal(STANDARD_USER_NOTICE.kind, "warn");
});

const volume = (root, lastRefresh = null) => ({
  root,
  fs: "NTFS",
  totalBytes: 1,
  freeBytes: 1,
  isSystem: false,
  model: null,
  smart: { level: "healthy", detail: null },
  lastRefresh,
});
const verified = (overrides = {}) => ({
  logPath: "synthetic.verify.log",
  processedFiles: 12,
  dataSafety: "YES",
  uncheckedFiles: 0,
  faster: "YES",
  ageSecs: 2 * 86400,
  speed: { beforeMbps: 10, nowMbps: 400, slowBefore: 12, slowNow: 0 },
  waitSecs: 0,
  stopped: false,
  ...overrides,
});

test("verify IPC rejects invalid safety claims and data", () => {
  for (const dataSafety of ["YES", "NO", "UNKNOWN", "NOT_CHECKED"])
    assert.equal(
      validateVerifyOutcome(verified({ dataSafety })).dataSafety,
      dataSafety,
    );
  for (const patch of [
    { dataSafety: true },
    { dataSafety: "unexpected" },
    { dataSafety: undefined },
    { processedFiles: 0 },
    { uncheckedFiles: undefined },
    { uncheckedFiles: -1 },
    { uncheckedFiles: 1.5 },
    { uncheckedFiles: 13 },
    // YES needs at least one file that verify compared.
    { uncheckedFiles: 12 },
    { stopped: true },
    { ageSecs: -1 },
    { waitSecs: NaN },
    { speed: { beforeMbps: Infinity, nowMbps: 1, slowBefore: 1, slowNow: 0 } },
    { speed: { beforeMbps: 1, nowMbps: 1, slowBefore: 1, slowNow: 2 } },
  ])
    assert.throws(
      () => validateVerifyOutcome(verified(patch)),
      /Invalid verify result/,
    );
});

test("unknown safety never becomes a positive notice", async () => {
  for (const dataSafety of ["UNKNOWN", "NOT_CHECKED"]) {
    const store = createStore({
      ...initialState,
      volumes: [volume("X:", { logPath: "x.refresh.csv", unixSecs: 1 })],
      selectedRoots: ["X:"],
    });
    await createActions(
      store,
      async () => true,
      backend({ startVerify: async () => verified({ dataSafety }) }),
    ).runVerify();
    assert.match(
      store.get().notice.title,
      new RegExp(`Data safe: ${dataSafety}`),
    );
    assert.equal(store.get().notice.kind, "info");
    assert.doesNotMatch(store.get().notice.title, /Data safe: YES/);
  }
});

test("files that verify could not compare get a calm note, not a warning", async () => {
  const run = async (outcome) => {
    const store = createStore({
      ...initialState,
      volumes: [volume("X:", { logPath: "x.refresh.csv", unixSecs: 1 })],
      selectedRoots: ["X:"],
    });
    await createActions(
      store,
      async () => true,
      backend({ startVerify: async () => outcome }),
    ).runVerify();
    return store.get().notice;
  };
  const calm =
    /3 files could not be compared\. The refresh did not cause this\..*You do not need to do anything\./;
  const yes = await run(verified({ uncheckedFiles: 3 }));
  assert.equal(yes.kind, "ok");
  assert.match(yes.title, /Data safe: YES/);
  assert.match(yes.body, calm);
  const unknown = await run(
    verified({ dataSafety: "UNKNOWN", uncheckedFiles: 3 }),
  );
  assert.equal(unknown.kind, "info");
  assert.match(unknown.body, calm);
  const none = await run(verified());
  assert.doesNotMatch(none.body, /could not be compared/);
  const bad = await run(verified({ dataSafety: "NO", uncheckedFiles: 3 }));
  assert.equal(bad.kind, "error");
  assert.doesNotMatch(bad.body, /did not cause this/);
});

test("a slower result warns even when data safety is not YES", async () => {
  const store = createStore({
    ...initialState,
    volumes: [volume("X:", { logPath: "x.refresh.csv", unixSecs: 1 })],
    selectedRoots: ["X:"],
  });
  await createActions(
    store,
    async () => true,
    backend({
      startVerify: async () =>
        verified({ dataSafety: "UNKNOWN", faster: "NO" }),
    }),
  ).runVerify();
  assert.equal(store.get().notice.kind, "warn");
});

test("the verify notice puts the speed first, one fact per line", () => {
  assert.equal(speedChange(1298, 255), "1298 → 255 MB/s (5.1 times slower)");
  assert.equal(speedChange(10, 400), "10 → 400 MB/s (40.0 times faster)");
  // Few slow files and too soon: a later verify can decide when all files agree.
  const few = verified({
    faster: "NOT DECIDED",
    speed: { beforeMbps: 1298, nowMbps: 255, slowBefore: 3, slowNow: 0 },
    waitSecs: 40000,
  });
  assert.match(nextStep([few]), /^Next step: verify again after /);
  // Enough time, but the few files have different results.
  const mixed = verified({
    faster: "NOT DECIDED",
    speed: { beforeMbps: 10, nowMbps: 90, slowBefore: 3, slowNow: 1 },
    waitSecs: 0,
  });
  assert.match(nextStep([mixed]), /all must have the same result/);
  // Enough time, the measured files agree, but one file had no measurement.
  const partial = verified({
    faster: "NOT DECIDED",
    speed: { beforeMbps: 10, nowMbps: 90, slowBefore: 2, slowNow: 0 },
    waitSecs: 0,
  });
  assert.match(nextStep([partial]), /could not measure all slow files/);
  // Too soon: give the time to verify again.
  const now = new Date(2026, 9, 31, 0, 8).getTime();
  const soon = verified({ faster: "NOT DECIDED", waitSecs: 12 * 3600 });
  assert.equal(
    nextStep([soon], now),
    "Next step: verify again after Oct 31, 12:08. New data is always fast at first.",
  );
  assert.equal(nextStep([verified()]), "The log has the full summary.");
});

test("verify is locked until a selected drive has a refresh log", () => {
  const store = createStore({
    ...initialState,
    volumes: [
      volume("X:"),
      volume("Y:", { logPath: "y.refresh.csv", unixSecs: 1000 }),
    ],
    selectedRoots: ["X:"],
  });
  assert.equal(verifyAction(store.get()).disabled, true);
  store.set({ selectedRoots: ["X:", "Y:"] });
  const ready = verifyAction(store.get(), 1000 + 3600);
  assert.equal(ready.disabled, false);
  assert.match(
    ready.hint,
    /Refreshed 1 hour ago\. Speed result after 12 hours/,
  );
  assert.match(verifyAction(store.get(), 1000 + 3 * 86400).hint, /3 days ago/);
  store.set({ mode: "refreshing" });
  assert.equal(verifyAction(store.get()).disabled, true);
});

test("the active drive card names the current operation", () => {
  assert.equal(activeDriveText("scanning"), "Scanning…");
  assert.equal(activeDriveText("refreshing"), "Refreshing…");
  assert.equal(activeDriveText("verifying"), "Verifying…");
  assert.equal(activeDriveText("idle"), null);
});

test("verify names a refreshed drive that is not selected", () => {
  const store = createStore({
    ...initialState,
    volumes: [
      volume("X:"),
      volume("Y:", { logPath: "y.refresh.csv", unixSecs: 1 }),
      volume("Z:"),
    ],
    selectedRoots: ["X:"],
  });
  const one = verifyAction(store.get());
  assert.equal(one.disabled, true);
  assert.equal(one.hint, "Y: has a refresh log. Select it to verify.");
  store.set({
    volumes: [
      volume("X:"),
      volume("Y:", { logPath: "y.refresh.csv", unixSecs: 1 }),
      volume("Z:", { logPath: "z.refresh.csv", unixSecs: 1 }),
    ],
  });
  assert.equal(
    verifyAction(store.get()).hint,
    "Y:, Z: have a refresh log. Select them to verify.",
  );
  store.set({ volumes: [volume("X:")] });
  assert.equal(verifyAction(store.get()).hint, "Run Selective Refresh first.");
});

test("at start, the drive list selects the drives with a refresh log", async () => {
  const refreshed = [
    volume("C:"),
    volume("D:", { logPath: "d.refresh.csv", unixSecs: 1 }),
    volume("E:", { logPath: "e.refresh.csv", unixSecs: 1 }),
  ];
  const store = createStore({ ...initialState });
  const actions = createActions(
    store,
    async () => true,
    backend({ listVolumes: async () => refreshed }),
  );
  await actions.loadVolumes();
  assert.deepEqual(store.get().selectedRoots, ["D:", "E:"]);
  assert.equal(verifyAction(store.get()).disabled, false);

  // A reload keeps the selection of the user.
  store.set({ selectedRoots: ["C:"] });
  await actions.loadVolumes();
  assert.deepEqual(store.get().selectedRoots, ["C:"]);

  // Without a refresh log, the first drive is selected.
  const plain = createStore({ ...initialState });
  await createActions(
    plain,
    async () => true,
    backend({ listVolumes: async () => [volume("C:"), volume("D:")] }),
  ).loadVolumes();
  assert.deepEqual(plain.get().selectedRoots, ["C:"]);
});

test("verify reads only the drives with a refresh log", async () => {
  const store = createStore({
    ...initialState,
    volumes: [
      volume("X:"),
      volume("Y:", { logPath: "y.refresh.csv", unixSecs: 1 }),
    ],
    selectedRoots: ["X:", "Y:"],
  });
  const calls = [];
  await createActions(
    store,
    async () => true,
    backend({
      startVerify: async (logPath) => {
        calls.push(logPath);
        return verified({
          faster: "NOT DECIDED",
          ageSecs: 3600,
          waitSecs: 11 * 3600,
        });
      },
    }),
  ).runVerify();
  assert.deepEqual(calls, ["y.refresh.csv"]);
  assert.equal(store.get().mode, "idle");
  assert.equal(store.get().notice.kind, "info");
  assert.equal(store.get().notice.title, "Data safe: YES. Faster: NOT DECIDED");
  assert.equal(
    store.get().notice.body.split("\n")[0],
    "Read speed: 10 → 400 MB/s (40.0 times faster)",
  );
  assert.match(store.get().notice.body, /\nSlow files: 12 → 0\n/);
  assert.match(store.get().notice.body, /Next step: verify again after /);
});

test("verify reports changed data as an error and stops", async () => {
  const store = createStore({
    ...initialState,
    volumes: [
      volume("X:", { logPath: "x.refresh.csv", unixSecs: 1 }),
      volume("Y:", { logPath: "y.refresh.csv", unixSecs: 1 }),
    ],
    selectedRoots: ["X:", "Y:"],
  });
  const calls = [];
  await createActions(
    store,
    async () => true,
    backend({
      startVerify: async (logPath) => {
        calls.push(logPath);
        return verified({ dataSafety: "NO" });
      },
    }),
  ).runVerify();
  assert.deepEqual(calls, ["x.refresh.csv"]);
  assert.equal(store.get().notice.kind, "error");
  assert.match(store.get().notice.body, /Data safe: NO on X:/);
});

test("refresh makes its log the newest refresh of the drive", async () => {
  const store = createStore({
    ...initialState,
    volumes: [volume("X:"), volume("Y:")],
    selectedRoots: ["X:"],
    scans: { "X:": scan("X:") },
  });
  await createActions(
    store,
    async () => true,
    backend({
      startRefresh: async () => ({
        logPath: "x.refresh.csv",
        movedFiles: 2,
        stopped: false,
      }),
    }),
  ).runRefresh();
  assert.equal(store.get().volumes[0].lastRefresh.logPath, "x.refresh.csv");
  assert.equal(store.get().volumes[1].lastRefresh, null);
  assert.equal(verifyAction(store.get()).disabled, false);
  assert.match(store.get().notice.body, /Run Verify after 1, 7 and 30 days/);
});
