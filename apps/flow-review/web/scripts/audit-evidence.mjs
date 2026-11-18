import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

export const INVENTORY = "bundle-inventory.json";
export const MODULES = ".bundle-modules.json";
export const PUBLIC_RECEIPT = "audit-prebuild.json";
export const RECEIPT = "node_modules/.cache/trollm-audit/prebuild.json";
export const MAX_AGE_MS = 60 * 60 * 1000;
export const GLOB_PACKAGES = [
  "picomatch",
  "micromatch",
  "minimatch",
  "tinyglobby",
  "fdir",
];
export const sha256 = (bytes) =>
  createHash("sha256").update(bytes).digest("hex");
export function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}
export const object = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value);
export function keys(value, required, optional = []) {
  requireValue(
    object(value) &&
      required.every((key) => Object.hasOwn(value, key)) &&
      Object.keys(value).every(
        (key) => required.includes(key) || optional.includes(key),
      ),
    "Unexpected evidence format",
  );
}
export function relativePath(value) {
  requireValue(
    typeof value === "string" &&
      value.length > 0 &&
      !value.includes("\\") &&
      !value.includes(":") &&
      !value.includes("\0") &&
      value.split("/").every((part) => part && part !== "." && part !== ".."),
    "Unsafe evidence path",
  );
  return value;
}
export function readJson(path) {
  requireValue(lstatSync(path).isFile(), "Evidence must be a regular file");
  return JSON.parse(readFileSync(path, "utf8"));
}
export function readContext(root, now = Date.now()) {
  const policyBytes = readFileSync(join(root, "scripts/audit-exceptions.json"));
  const policy = JSON.parse(policyBytes);
  keys(policy, [
    "schema",
    "advisory",
    "advisoryUrl",
    "package",
    "version",
    "severity",
    "expiresAt",
    "lockfileSha256",
    "paths",
  ]);
  requireValue(
    policy.schema === "trollm.web-audit-exception.v1" &&
      policy.advisory === "GHSA-fc5q-39xc-vjpf" &&
      policy.advisoryUrl ===
        `https://github.com/advisories/${policy.advisory}` &&
      policy.package === "picomatch" &&
      policy.version === "4.0.7" &&
      policy.severity === "high",
    "Unapproved audit exception identity",
  );
  requireValue(
    policy.expiresAt === "2026-11-24T00:00:00Z" &&
      now < Date.parse(policy.expiresAt),
    "Audit exception expired",
  );
  const paths = [
    "node_modules/tinyglobby/node_modules/picomatch",
    "node_modules/vite/node_modules/picomatch",
    "node_modules/vitest/node_modules/picomatch",
  ];
  requireValue(
    Array.isArray(policy.paths) &&
      JSON.stringify(policy.paths) === JSON.stringify(paths),
    "Unapproved audit exception paths",
  );
  const lockBytes = readFileSync(join(root, "package-lock.json"));
  const lockSha256 = sha256(lockBytes);
  requireValue(
    policy.lockfileSha256 ===
      "7070eafefedc72065b039f169455c950cdfe695c783aa4c5154a960ac1de40e7" &&
      lockSha256 === policy.lockfileSha256,
    "Lockfile differs from the approved audit exception",
  );
  const lock = JSON.parse(lockBytes);
  requireValue(
    lock.lockfileVersion === 3 && object(lock.packages),
    "Unexpected lockfile format",
  );
  for (const path of paths) {
    requireValue(
      lock.packages[path]?.version === policy.version &&
        lock.packages[path]?.dev === true,
      "Excepted package must retain its exact version and development-only path",
    );
  }
  const pkg = readJson(join(root, "package.json"));
  for (const field of [
    "dependencies",
    "devDependencies",
    "optionalDependencies",
  ]) {
    requireValue(
      JSON.stringify(pkg[field] ?? {}) ===
        JSON.stringify(lock.packages[""][field] ?? {}),
      "Package dependencies differ from the audited lockfile",
    );
  }
  return { policy, lock, lockSha256, policySha256: sha256(policyBytes) };
}
export function validateReceipt(receipt, context, now = Date.now()) {
  keys(receipt, ["schema", "nonce", "checkedAt", "lockSha256", "policySha256"]);
  const checked = Date.parse(receipt.checkedAt);
  requireValue(
    receipt.schema === "trollm.web-audit-prebuild.v1" &&
      typeof receipt.nonce === "string" &&
      /^[a-f0-9]{32}$/.test(receipt.nonce) &&
      typeof receipt.checkedAt === "string" &&
      Number.isFinite(checked) &&
      checked <= now &&
      now - checked <= MAX_AGE_MS,
    "Missing or stale prebuild audit evidence",
  );
  requireValue(
    receipt.lockSha256 === context.lockSha256 &&
      receipt.policySha256 === context.policySha256,
    "Prebuild audit does not match current lockfile/policy",
  );
  return checked;
}
export function outputFiles(
  directory,
  excluded = new Set([INVENTORY, MODULES]),
) {
  const files = [];
  function visit(relative = "") {
    for (const name of readdirSync(join(directory, relative)).sort()) {
      const file = relative ? `${relative}/${name}` : name;
      relativePath(file);
      const stat = lstatSync(join(directory, file));
      requireValue(
        !stat.isSymbolicLink(),
        "Build output symlinks are not accepted",
      );
      if (stat.isDirectory()) visit(file);
      else {
        requireValue(stat.isFile(), "Unexpected build output type");
        if (!excluded.has(file))
          files.push({
            path: file,
            sha256: sha256(readFileSync(join(directory, file))),
          });
      }
    }
  }
  visit();
  return files.sort((a, b) => a.path.localeCompare(b.path));
}
export function validateInventory(
  inventory,
  context,
  receipt,
  files,
  now = Date.now(),
) {
  keys(inventory, [
    "schema",
    "generatedAt",
    "lockSha256",
    "policySha256",
    "prebuildNonce",
    "moduleCount",
    "packages",
    "files",
  ]);
  const checked = validateReceipt(receipt, context, now);
  const generated = Date.parse(inventory.generatedAt);
  requireValue(
    inventory.schema === "trollm.web-bundle-inventory.v1" &&
      typeof inventory.generatedAt === "string" &&
      Number.isFinite(generated) &&
      generated >= checked &&
      generated <= now &&
      now - generated <= MAX_AGE_MS,
    "Missing or stale build inventory",
  );
  requireValue(
    inventory.lockSha256 === context.lockSha256 &&
      inventory.policySha256 === context.policySha256 &&
      inventory.prebuildNonce === receipt.nonce,
    "Build inventory does not match current prebuild audit",
  );
  requireValue(
    Number.isSafeInteger(inventory.moduleCount) &&
      inventory.moduleCount > 0 &&
      Array.isArray(inventory.packages) &&
      inventory.packages.length > 0,
    "Missing output module inventory",
  );
  const seen = new Set();
  for (const pkg of inventory.packages) {
    keys(pkg, ["name", "version", "path", "modules"]);
    relativePath(pkg.path);
    requireValue(
      typeof pkg.name === "string" &&
        /^(?:@[a-z0-9._-]+\/)?[a-z0-9._-]+$/.test(pkg.name) &&
        typeof pkg.version === "string" &&
        Number.isSafeInteger(pkg.modules) &&
        pkg.modules > 0 &&
        !seen.has(pkg.path),
      "Invalid bundled package identity",
    );
    seen.add(pkg.path);
    requireValue(
      pkg.path.endsWith(`node_modules/${pkg.name}`) &&
        context.lock.packages[pkg.path]?.version === pkg.version,
      "Bundled package differs from the audited lockfile",
    );
    requireValue(
      !GLOB_PACKAGES.includes(pkg.name),
      `Forbidden glob package in production bundle: ${pkg.name}`,
    );
  }
  requireValue(
    inventory.packages.reduce((sum, pkg) => sum + pkg.modules, 0) <=
      inventory.moduleCount,
    "Bundled module counts disagree",
  );
  requireValue(
    Array.isArray(inventory.files) && inventory.files.length > 0,
    "Missing output file inventory",
  );
  for (const file of inventory.files) {
    keys(file, ["path", "sha256"]);
    relativePath(file.path);
    requireValue(
      typeof file.sha256 === "string" && /^[a-f0-9]{64}$/.test(file.sha256),
      "Invalid output hash",
    );
  }
  requireValue(
    inventory.files.some((file) => file.path === "index.html") &&
      inventory.files.some((file) => file.path.endsWith(".js")) &&
      inventory.files.some((file) => file.path === PUBLIC_RECEIPT) &&
      inventory.files.some((file) => file.path === "third-party-notices.txt") &&
      JSON.stringify(inventory.files) === JSON.stringify(files),
    "Build output set or hashes differ from inventory",
  );
}
