import assert from "node:assert/strict";
import test from "node:test";
import {
  audit,
  auditPlan,
  queryOsv,
  reachablePackages,
  runCli,
} from "./audit-rust.mjs";

const source = "registry+https://github.com/rust-lang/crates.io-index";
const windows = "x86_64-pc-windows-msvc";
const linux = "x86_64-unknown-linux-gnu";
const identity = (name) => ({ id: name, name, version: "1.0.0", source });
const edge = (pkg, kind = null) => ({ pkg, dep_kinds: [{ kind }] });
const node = (id, deps = []) => ({ id, deps });
function fixture(target = windows) {
  return {
    workspace_members: ["cli", "gui"],
    packages: [
      { id: "cli", name: "ssd-refresh", source: null },
      { id: "gui", name: "ssd-refresh-gui", source: null },
      identity("common"),
      identity("windows-only"),
      identity("linux-only"),
      identity("glib"),
      identity("dev-only"),
      { ...identity("private"), source: "registry+https://private.example" },
    ],
    resolve: {
      nodes: [
        node("cli", [
          edge("common"),
          edge(target.includes("windows") ? "windows-only" : "linux-only"),
          edge("dev-only", "dev"),
          edge("private"),
        ]),
        node("gui", [edge("cli"), ...(target === linux ? [edge("glib")] : [])]),
        ...[
          "common",
          "windows-only",
          "linux-only",
          "glib",
          "dev-only",
          "private",
        ].map((name) => node(name)),
      ],
    },
  };
}
const silent = () => {};
const response = (results) => ({ ok: true, json: async () => ({ results }) });

test("The plan includes six CLI targets and two Windows GUI targets", () => {
  const plan = auditPlan([]);
  assert.equal(plan.length, 6);
  assert.equal(
    plan.filter((entry) => entry.packages.includes("ssd-refresh-gui")).length,
    2,
  );
  assert.throws(() =>
    auditPlan(["--target", linux, "--package", "ssd-refresh-gui"]),
  );
  assert.throws(() => auditPlan(["--target", "unknown"]));
  assert.throws(() => auditPlan(["--package", "unknown"]));
  assert.throws(() => auditPlan(["--target"]));
  assert.throws(() => auditPlan(["--target", windows, "--target", windows]));
});

test("Only reachable release dependencies from the chosen root are included", () => {
  const packages = reachablePackages(fixture(), ["ssd-refresh"]);
  assert.deepEqual(packages.map((entry) => entry.name).sort(), [
    "common",
    "windows-only",
  ]);
  const metadata = fixture(linux);
  assert.ok(
    !reachablePackages(metadata, ["ssd-refresh"]).some(
      (entry) => entry.name === "glib",
    ),
  );
  assert.ok(
    reachablePackages(metadata, ["ssd-refresh-gui"]).some(
      (entry) => entry.name === "glib",
    ),
  );
});

test("Build dependencies are included and cycles do not lose packages", () => {
  const metadata = fixture();
  metadata.resolve.nodes.find((entry) => entry.id === "common").deps = [
    edge("windows-only", "build"),
  ];
  metadata.resolve.nodes.find((entry) => entry.id === "windows-only").deps = [
    edge("common"),
  ];
  assert.equal(reachablePackages(metadata, ["ssd-refresh"]).length, 2);
});

test("Incomplete Cargo graphs fail", () => {
  assert.throws(() => reachablePackages({}, ["ssd-refresh"]));
  assert.throws(() => reachablePackages(fixture(), ["missing"]));
  const metadata = fixture();
  metadata.resolve.nodes = metadata.resolve.nodes.filter(
    (entry) => entry.id !== "common",
  );
  assert.throws(() => reachablePackages(metadata, ["ssd-refresh"]));
});

test("Targets form a union without duplicate package versions", async () => {
  const seenTargets = [];
  let checked;
  const code = await audit([], {
    readMetadata: (target) => {
      seenTargets.push(target);
      return fixture(target);
    },
    query: async (packages) => {
      checked = packages;
      return [];
    },
    log: silent,
  });
  assert.equal(code, 0);
  assert.equal(seenTargets.length, 6);
  assert.deepEqual(checked.map((entry) => entry.name).sort(), [
    "common",
    "linux-only",
    "windows-only",
  ]);
});

test("OSV receives only package identities", async () => {
  const packages = [
    {
      name: "common",
      version: "1.0.0",
      path: "private/path",
      source: "private",
    },
  ];
  const results = await queryOsv(packages, async (url, options) => {
    assert.equal(url, "https://api.osv.dev/v1/querybatch");
    assert.deepEqual(JSON.parse(options.body), {
      queries: [
        {
          package: { name: "common", ecosystem: "crates.io" },
          version: "1.0.0",
        },
      ],
    });
    return response([{}]);
  });
  assert.deepEqual(results, []);
});

test("OSV HTTP, network, JSON, missing results, and pagination fail", async () => {
  const packages = [{ name: "common", version: "1.0.0" }];
  for (const fetcher of [
    async () => ({ ok: false, status: 503 }),
    async () => {
      throw new Error("Network failed");
    },
    async () => ({
      ok: true,
      json: async () => {
        throw new Error("Bad JSON");
      },
    }),
    async () => response([]),
    async () => response([null]),
    async () => response([{ error: "Query failed" }]),
    async () => response([{ next_page_token: "more" }]),
    async () => response([{ vulns: "wrong" }]),
    async () => response([{ vulns: [{}] }]),
  ]) {
    await assert.rejects(queryOsv(packages, fetcher));
  }
});

test("OSV batches all packages and preserves result order", async () => {
  const packages = Array.from({ length: 501 }, (_, index) => ({
    name: `crate-${index}`,
    version: "1",
  }));
  const sizes = [];
  const findings = await queryOsv(packages, async (_url, options) => {
    const queries = JSON.parse(options.body).queries;
    sizes.push(queries.length);
    return response(
      queries.map((query) =>
        query.package.name === "crate-500" ? { vulns: [{ id: "TEST-1" }] } : {},
      ),
    );
  });
  assert.deepEqual(sizes, [500, 1]);
  assert.deepEqual(findings, [
    { name: "crate-500", version: "1", id: "TEST-1" },
  ]);
});

test("Findings and Cargo failures return a nonzero CLI result", async () => {
  const args = ["--target", windows, "--package", "ssd-refresh"];
  const code = await runCli(args, {
    readMetadata: fixture,
    query: async () => [{ name: "common", version: "1.0.0", id: "TEST-1" }],
    log: silent,
  });
  assert.equal(code, 1);
  assert.equal(
    await runCli(args, {
      readMetadata: () => {
        throw new Error("Cargo failed");
      },
      log: silent,
    }),
    1,
  );
  assert.equal(
    await runCli(args, {
      readMetadata: fixture,
      query: async () => {
        throw new Error("OSV failed");
      },
      log: silent,
    }),
    1,
  );
});
