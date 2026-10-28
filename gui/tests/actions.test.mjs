import assert from "node:assert/strict";
import test from "node:test";
import { createActions } from "../src/actions.ts";
import { createDiagnostics } from "../src/diagnostics.ts";
import { countOf, truncatePath, wasWere } from "../src/format.ts";
import { createStore, initialState, refreshAction } from "../src/store.ts";
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
