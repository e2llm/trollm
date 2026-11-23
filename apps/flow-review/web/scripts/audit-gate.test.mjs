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
const now = Date.parse("2026-12-01T12:00:00Z");
const context = readContext(root);
const clone = (value) => structuredClone(value);
const leaf = {
  source: 1247710,
  name: "picomatch",
  dependency: "picomatch",
  title: "Known advisory",
  url: "https://github.com/advisories/GHSA-fc5q-39xc-vjpf",
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
      [
        "node_modules/tinyglobby/node_modules/picomatch",
        "node_modules/vite/node_modules/picomatch",
        "node_modules/vitest/node_modules/picomatch",
      ],
    ),
    vite: vulnerability("vite", ["picomatch"], ["node_modules/vite"]),
  });
}
function receipt() {
  return {
    schema: "trollm.web-audit-prebuild.v2",
    nonce: "a".repeat(32),
    checkedAt: new Date(now - 1000).toISOString(),
    lockSha256: context.lockSha256,
    policySha256: context.policySha256,
  };
}
function inventory() {
  return {
    schema: "trollm.web-bundle-inventory.v2",
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
      "scripts/audit-policy.json",
    ])
      copyFileSync(join(root, file), join(directory, file));
    fn(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test("full and production audits require zero findings at every severity", () => {
  assert.doesNotThrow(() => evaluateAudits(audit(), audit()));
  for (const severity of ["info", "low", "moderate", "high", "critical"]) {
    const advisory = {
      ...leaf,
      name: "fixture",
      dependency: "fixture",
      url: "https://github.com/advisories/GHSA-test-test-test",
      severity,
    };
    const affected = audit({
      fixture: vulnerability("fixture", [advisory], ["node_modules/fixture"], severity),
    });
    assert.throws(() => evaluateAudits(affected, audit()), /Full audit must have zero findings/);
    assert.throws(() => evaluateAudits(audit(), affected), /Production audit must have zero findings/);
    assert.doesNotThrow(() => checkAuditExit(affected, 1));
    assert.throws(() => checkAuditExit(affected, 0), /expected status/);
  }
});

test("the withdrawn Picomatch advisory blocks if the registry reports it again", () => {
  const affected = full();
  assert.throws(() => evaluateAudits(affected, audit()), /Full audit must have zero findings/);
  assert.throws(() => evaluateAudits(audit(), affected), /Production audit must have zero findings/);
  assert.throws(() => checkAuditExit(affected, 0), /expected status/);
});

test("clean reports still require their expected exit status and metadata", () => {
  assert.doesNotThrow(() => checkAuditExit(audit(), 0));
  assert.throws(() => checkAuditExit(audit(), 1), /expected status/);
  assert.throws(() => checkAuditExit(audit(), null), /expected status/);
  const badCount = audit();
  badCount.metadata.vulnerabilities.low = 1;
  assert.throws(() => validateAudit(badCount), /counts disagree/);
  const badTotal = audit();
  badTotal.metadata.vulnerabilities.total = 1;
  assert.throws(() => validateAudit(badTotal), /total disagrees/);
  assert.throws(() => validateAudit({ error: "registry unavailable" }), /Unexpected evidence/);
});

test("the strict policy pins the audited lockfile and admits no exception fields", () =>
  temp((directory) => {
    const file = join(directory, "scripts/audit-policy.json");
    const loaded = readContext(directory);
    assert.equal(loaded.policy.mode, "zero-findings");
    for (const changed of [
      {...context.policy, mode: "allow-high"},
      {...context.policy, schema: "trollm.web-audit-exception.v1"},
      {...context.policy, expiresAt: "2099-01-01T00:00:00Z"},
    ]) {
      writeFileSync(file, JSON.stringify(changed));
      assert.throws(() => readContext(directory));
    }
    copyFileSync(join(root, "scripts/audit-policy.json"), file);
    const lock = join(directory, "package-lock.json");
    writeFileSync(lock, readFileSync(lock, "utf8") + "\n");
    assert.throws(() => readContext(directory), /Lockfile differs/);
  }));

test("exception-era receipts and inventories cannot be qualified", () => {
  const valid = inventory();
  const oldReceipt = {...receipt(), schema: "trollm.web-audit-prebuild.v1"};
  const oldInventory = {...valid, schema: "trollm.web-bundle-inventory.v1"};
  assert.throws(() => validateInventory(valid, context, oldReceipt, valid.files, now));
  assert.throws(() => validateInventory(oldInventory, context, receipt(), valid.files, now));
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
