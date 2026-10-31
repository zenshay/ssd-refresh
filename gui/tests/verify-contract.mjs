// Read actual Rust DTO JSON. Do not maintain a second copy of its fields here.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createActions } from "../src/actions.ts";
import { validateVerifyOutcome } from "../src/api.ts";
import { createStore, initialState } from "../src/store.ts";

const fixtures = JSON.parse(await readFile(process.argv[2], "utf8"));
assert.equal(fixtures.length, 4);
for (const [index, safety] of [
  "YES",
  "NO",
  "UNKNOWN",
  "NOT_CHECKED",
].entries()) {
  const outcome = validateVerifyOutcome(fixtures[index]);
  assert.equal(outcome.dataSafety, safety);
  const store = createStore({
    ...initialState,
    selectedRoots: ["X:"],
    volumes: [
      {
        root: "X:",
        lastRefresh: { logPath: "synthetic.refresh.csv", unixSecs: 1 },
      },
    ],
  });
  await createActions(store, async () => true, {
    startVerify: async () => outcome,
  }).runVerify();
  const notice = store.get().notice;
  if (safety === "NO") {
    assert.equal(notice.kind, "error");
    assert.match(notice.body, /Data safe: NO/);
  } else {
    assert.match(notice.title, new RegExp(`Data safe: ${safety}`));
  }
  if (safety !== "YES") assert.doesNotMatch(notice.title, /Data safe: YES/);
  // The Rust fixture gives UNKNOWN one file that verify could not compare.
  assert.equal(outcome.uncheckedFiles, safety === "UNKNOWN" ? 1 : 0);
  if (safety === "UNKNOWN")
    assert.match(
      notice.body,
      /1 file could not be compared\. The refresh did not cause this\./,
    );
}
console.log("VERIFY CONTRACT: YES (4 Rust DTO results and frontend notices)");
