import { test } from "node:test";
import assert from "node:assert/strict";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  checkAuditExit,
  evaluateAudits,
  validateAudit,
  validateFinalEvidence,
  validatePublishedInventory,
} from "./audit-gate.mjs";
import { collectBundle, finalizeInventory } from "./bundle-inventory.mjs";
import {
  GLOB_PACKAGES,
  INVENTORY,
  MAX_AGE_MS,
  MODULES,
  PUBLIC_RECEIPT,
  RECEIPT,
  outputFiles,
  readContext,
  readJson,
  sha256,
  validateInventory,
} from "./audit-evidence.mjs";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const now = Date.parse("2026-11-17T22:00:00Z");
const context = readContext(root, now);
const clone = (value) => structuredClone(value);
const leaf = {
  source: 1247710,
  name: "picomatch",
  dependency: "picomatch",
  title: "Known advisory",
  url: context.policy.advisoryUrl,
  severity: "high",
  range: "<=4.0.7",
};
function vulnerability(name, via, nodes, severity = "high") {
  return {
    name,
    severity,
    isDirect: false,
    via,
    effects: [],
    range: "*",
    nodes,
    fixAvailable: false,
  };
}
function audit(vulnerabilities = {}) {
  const counts = {
    info: 0,
    low: 0,
    moderate: 0,
    high: 0,
    critical: 0,
    total: Object.keys(vulnerabilities).length,
  };
  for (const value of Object.values(vulnerabilities)) counts[value.severity]++;
  return {
    auditReportVersion: 2,
    vulnerabilities,
    metadata: {
      vulnerabilities: counts,
      dependencies: {
        prod: 1,
        dev: 2,
        optional: 0,
        peer: 0,
        peerOptional: 0,
        total: 3,
      },
    },
  };
}
function full() {
  return audit({
    picomatch: vulnerability(
      "picomatch",
      [clone(leaf)],
      [...context.policy.paths],
    ),
    vite: vulnerability("vite", ["picomatch"], ["node_modules/vite"]),
  });
}
function receipt() {
  return {
    schema: "trollm.web-audit-prebuild.v1",
    nonce: "a".repeat(32),
    checkedAt: new Date(now - 1000).toISOString(),
    lockSha256: context.lockSha256,
    policySha256: context.policySha256,
  };
}
function inventory() {
  return {
    schema: "trollm.web-bundle-inventory.v1",
    generatedAt: new Date(now).toISOString(),
    lockSha256: context.lockSha256,
    policySha256: context.policySha256,
    prebuildNonce: receipt().nonce,
    moduleCount: 2,
    packages: [
      {
        name: "react",
        version: context.lock.packages["node_modules/react"].version,
        path: "node_modules/react",
        modules: 1,
      },
    ],
    files: [
      "assets/app.js",
      PUBLIC_RECEIPT,
      "index.html",
      "third-party-notices.txt",
    ].map((path) => ({ path, sha256: sha256(path) })),
  };
}
function temp(fn) {
  const directory = mkdtempSync(join(tmpdir(), "trollm-audit-gate-"));
  try {
    mkdirSync(join(directory, "scripts"));
    for (const file of [
      "package.json",
      "package-lock.json",
      "scripts/audit-exceptions.json",
    ])
      copyFileSync(join(root, file), join(directory, file));
    fn(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test("only the approved high advisory and its complete dev via chains are waived", () => {
  assert.deepEqual(evaluateAudits(full(), audit(), context), [
    "picomatch",
    "vite",
  ]);
  assert.deepEqual(evaluateAudits(audit(), audit(), context), []);
  checkAuditExit(full(), 1);
  checkAuditExit(audit(), 0);
  assert.throws(() => checkAuditExit(full(), 0), /expected status/);
  assert.throws(() => checkAuditExit(audit(), 1), /expected status/);
  assert.throws(() => checkAuditExit(audit(), null), /expected status/);
});
test("critical severity, other advisory identities and mixed leaves fail", () => {
  for (const mutate of [
    (value) => {
      value.vulnerabilities.picomatch.via[0].url =
        "https://github.com/advisories/GHSA-other";
    },
    (value) => {
      value.vulnerabilities.picomatch.via[0].severity = "critical";
    },
    (value) => {
      value.vulnerabilities.vite.severity = "critical";
      value.metadata.vulnerabilities.high--;
      value.metadata.vulnerabilities.critical++;
    },
    (value) => {
      value.vulnerabilities.vite.via.push({
        ...leaf,
        severity: "low",
        url: "https://github.com/advisories/GHSA-other",
      });
    },
  ]) {
    const value = full();
    mutate(value);
    assert.throws(
      () => evaluateAudits(value, audit(), context),
      /Critical|Unapproved/,
    );
  }
});
test("production findings and a production or unknown dependency in a waived chain fail", () => {
  assert.throws(
    () => evaluateAudits(full(), full(), context),
    /Production audit/,
  );
  const low = audit({
    react: vulnerability(
      "react",
      [{ ...leaf, severity: "low" }],
      ["node_modules/react"],
      "low",
    ),
  });
  assert.throws(() => evaluateAudits(full(), low, context), /Production audit/);
  for (const path of [context.policy.paths[0], "node_modules/vite"]) {
    const changed = clone(context);
    changed.lock.packages[path].dev = false;
    assert.throws(() => evaluateAudits(full(), audit(), changed), /production/);
  }
  const changed = full();
  changed.vulnerabilities.vite.nodes = ["node_modules/unknown"];
  assert.throws(() => evaluateAudits(changed, audit(), context), /unknown/);
});
test("changed picomatch version and any path change fail", () => {
  const changed = clone(context);
  changed.lock.packages[context.policy.paths[0]].version = "4.0.8";
  assert.throws(
    () => evaluateAudits(full(), audit(), changed),
    /version changed/,
  );
  const value = full();
  value.vulnerabilities.picomatch.nodes.pop();
  assert.throws(() => evaluateAudits(value, audit(), context), /paths changed/);
});
test("cyclic, missing, empty and malformed via chains fail closed", () => {
  const cases = [
    (value) => {
      value.vulnerabilities.picomatch.via = ["vite"];
    },
    (value) => {
      value.vulnerabilities.vite.via = ["missing"];
    },
    (value) => {
      value.vulnerabilities.vite.via = [];
    },
    (value) => {
      value.vulnerabilities.vite.via = [42];
    },
    (value) => {
      value.metadata.vulnerabilities.high = 0;
    },
    (value) => {
      value.auditReportVersion = 3;
    },
    (value) => {
      value.error = "unexpected";
    },
  ];
  for (const mutate of cases) {
    const value = full();
    mutate(value);
    assert.throws(() => validateAudit(value));
  }
});
test("policy expires at the exact boundary and lock drift is never silently approved", () =>
  temp((directory) => {
    const expiry = Date.parse(context.policy.expiresAt);
    assert.doesNotThrow(() => readContext(directory, expiry - 1));
    assert.throws(() => readContext(directory, expiry), /expired/);
    assert.throws(() => readContext(directory, expiry + 1), /expired/);
    writeFileSync(
      join(directory, "package-lock.json"),
      readFileSync(join(directory, "package-lock.json"), "utf8") + "\n",
    );
    assert.throws(() => readContext(directory, now), /Lockfile differs/);
  }));
test("policy version, severity, path and unknown fields cannot broaden the exception", () => {
  for (const mutate of [
    (policy) => {
      policy.version = "4.0.8";
    },
    (policy) => {
      policy.severity = "critical";
    },
    (policy) => {
      policy.paths.push("node_modules/picomatch");
    },
    (policy) => {
      policy.allowAll = true;
    },
  ])
    temp((directory) => {
      const policy = clone(context.policy);
      mutate(policy);
      writeFileSync(
        join(directory, "scripts/audit-exceptions.json"),
        JSON.stringify(policy),
      );
      assert.throws(() => readContext(directory, now));
    });
});
test("fresh inventory is tied to current nonce, lock, policy and exact output bytes", () => {
  const valid = inventory();
  assert.doesNotThrow(() =>
    validateInventory(valid, context, receipt(), valid.files, now),
  );
  for (const mutate of [
    (value) => {
      value.prebuildNonce = "b".repeat(32);
    },
    (value) => {
      value.lockSha256 = "0".repeat(64);
    },
    (value) => {
      value.policySha256 = "0".repeat(64);
    },
    (value) => {
      value.generatedAt = new Date(now - 2000).toISOString();
    },
    (value) => {
      value.generatedAt = new Date(now + 1).toISOString();
    },
    (value) => {
      value.files[0].sha256 = "0".repeat(64);
    },
    (value) => {
      value.packages[0].path = "/private/build/node_modules/react";
    },
    (value) => {
      value.packages[0].version = "0.0.0";
    },
    (value) => {
      value.moduleCount = 0;
    },
  ]) {
    const value = inventory();
    mutate(value);
    assert.throws(() =>
      validateInventory(value, context, receipt(), valid.files, now),
    );
  }
  assert.throws(
    () =>
      validateInventory(
        valid,
        context,
        receipt(),
        valid.files,
        now + MAX_AGE_MS + 1,
      ),
    /stale/,
  );
  assert.throws(
    () =>
      validateInventory(
        valid,
        context,
        receipt(),
        [...valid.files, { path: "extra.js", sha256: "0".repeat(64) }],
        now,
      ),
    /output set/,
  );
  assert.throws(() =>
    validateInventory(null, context, receipt(), valid.files, now),
  );
  assert.throws(() =>
    validateInventory(valid, context, null, valid.files, now),
  );
});
test("every forbidden glob package fails even if it has a valid package identity", () => {
  for (const name of GLOB_PACKAGES) {
    const value = inventory(),
      changed = clone(context),
      path = `node_modules/${name}`;
    changed.lock.packages[path] = { version: "1.0.0", dev: true };
    value.packages.push({ name, version: "1.0.0", path, modules: 1 });
    assert.throws(
      () => validateInventory(value, changed, receipt(), value.files, now),
      /Forbidden glob package/,
    );
  }
});
test("actual module records produce portable inventory and changes after Vite fail", () =>
  temp((directory) => {
    const pkg = join(directory, "node_modules/react");
    mkdirSync(pkg, { recursive: true });
    writeFileSync(
      join(pkg, "package.json"),
      JSON.stringify({
        name: "react",
        version: context.lock.packages["node_modules/react"].version,
      }),
    );
    const bundle = {
      "app.js": {
        type: "chunk",
        fileName: "app.js",
        code: "export const ready = true;",
        modules: { [join(pkg, "index.js")]: {} },
      },
      "index.html": {
        type: "asset",
        fileName: "index.html",
        source: "<script src='app.js'></script>",
      },
    };
    const raw = collectBundle(directory, bundle, context, receipt(), now);
    assert.equal(raw.packages[0].path, "node_modules/react");
    assert.ok(!JSON.stringify(raw).includes(directory));
    mkdirSync(join(directory, "dist"));
    mkdirSync(dirname(join(directory, RECEIPT)), { recursive: true });
    writeFileSync(join(directory, RECEIPT), JSON.stringify(receipt()));
    for (const output of Object.values(bundle))
      writeFileSync(
        join(directory, "dist", output.fileName),
        output.code ?? output.source,
      );
    writeFileSync(
      join(directory, "dist", "third-party-notices.txt"),
      "Copyright fixture",
    );
    writeFileSync(join(directory, "dist", MODULES), JSON.stringify(raw));
    writeFileSync(join(directory, "dist", "app.js"), "changed");
    assert.throws(() => finalizeInventory(directory, now), /Outputs changed/);
    writeFileSync(join(directory, "dist", "app.js"), bundle["app.js"].code);
    finalizeInventory(directory, now);
    assert.doesNotThrow(() => validateFinalEvidence(directory, context, now));
    assert.equal(readJson(join(directory, "dist", INVENTORY)).files.length, 4);
    assert.equal(outputFiles(join(directory, "dist")).length, 4);
    rmSync(join(directory, RECEIPT));
    assert.doesNotThrow(() =>
      validatePublishedInventory(join(directory, "dist"), context, now),
    );
    assert.throws(() => validateFinalEvidence(directory, context, now));
    rmSync(join(directory, "dist", INVENTORY));
    assert.throws(() => validateFinalEvidence(directory, context, now));
  }));
