import { randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  INVENTORY,
  MODULES,
  PUBLIC_RECEIPT,
  RECEIPT,
  keys,
  object,
  outputFiles,
  readContext,
  readJson,
  relativePath,
  requireValue,
  sha256,
  validateInventory,
} from "./audit-evidence.mjs";

const severities = ["info", "low", "moderate", "high", "critical"];
export function validateAudit(report) {
  keys(report, ["auditReportVersion", "vulnerabilities", "metadata"]);
  requireValue(
    report.auditReportVersion === 2 && object(report.vulnerabilities),
    "Unexpected npm audit format",
  );
  keys(report.metadata, ["vulnerabilities", "dependencies"]);
  keys(report.metadata.vulnerabilities, [...severities, "total"]);
  keys(report.metadata.dependencies, [
    "prod",
    "dev",
    "optional",
    "peer",
    "peerOptional",
    "total",
  ]);
  requireValue(
    object(report.metadata.dependencies) &&
      Object.values(report.metadata.dependencies).every(
        (n) => Number.isSafeInteger(n) && n >= 0,
      ),
    "Invalid audit dependency counts",
  );
  const counts = Object.fromEntries(
    severities.map((severity) => [severity, 0]),
  );
  for (const [name, vuln] of Object.entries(report.vulnerabilities)) {
    keys(vuln, [
      "name",
      "severity",
      "isDirect",
      "via",
      "effects",
      "range",
      "nodes",
      "fixAvailable",
    ]);
    requireValue(
      vuln.name === name &&
        severities.includes(vuln.severity) &&
        typeof vuln.isDirect === "boolean" &&
        typeof vuln.range === "string" &&
        Array.isArray(vuln.effects) &&
        vuln.effects.every((effect) => typeof effect === "string") &&
        Array.isArray(vuln.via) &&
        vuln.via.length > 0 &&
        Array.isArray(vuln.nodes) &&
        vuln.nodes.length > 0 &&
        new Set(vuln.nodes).size === vuln.nodes.length,
      "Invalid npm vulnerability entry",
    );
    for (const node of vuln.nodes) {
      relativePath(node);
      requireValue(
        node.startsWith("node_modules/"),
        "Invalid audit package path",
      );
    }
    if (typeof vuln.fixAvailable !== "boolean") {
      keys(vuln.fixAvailable, ["name", "version", "isSemVerMajor"]);
      requireValue(
        typeof vuln.fixAvailable.name === "string" &&
          typeof vuln.fixAvailable.version === "string" &&
          typeof vuln.fixAvailable.isSemVerMajor === "boolean",
        "Invalid npm fix metadata",
      );
    }
    counts[vuln.severity]++;
    for (const via of vuln.via) {
      if (typeof via === "string") continue;
      keys(
        via,
        ["source", "name", "dependency", "title", "url", "severity", "range"],
        ["cwe", "cvss"],
      );
      requireValue(
        Number.isSafeInteger(via.source) &&
          typeof via.name === "string" &&
          typeof via.dependency === "string" &&
          typeof via.title === "string" &&
          typeof via.url === "string" &&
          typeof via.range === "string" &&
          severities.includes(via.severity),
        "Invalid npm advisory entry",
      );
      if (Object.hasOwn(via, "cwe"))
        requireValue(
          Array.isArray(via.cwe) &&
            via.cwe.every((cwe) => typeof cwe === "string"),
          "Invalid advisory CWE metadata",
        );
      if (Object.hasOwn(via, "cvss")) {
        keys(via.cvss, ["score", "vectorString"]);
        requireValue(
          (via.cvss.score === null ||
            (typeof via.cvss.score === "number" &&
              via.cvss.score >= 0 &&
              via.cvss.score <= 10)) &&
            (via.cvss.vectorString === null ||
              typeof via.cvss.vectorString === "string"),
          "Invalid advisory CVSS metadata",
        );
      }
    }
  }
  for (const severity of severities)
    requireValue(
      report.metadata.vulnerabilities[severity] === counts[severity],
      "Audit vulnerability counts disagree",
    );
  requireValue(
    report.metadata.vulnerabilities.total ===
      Object.keys(report.vulnerabilities).length,
    "Audit total disagrees",
  );
  const leaves = new Map(),
    active = new Set();
  function visit(name) {
    requireValue(
      Object.hasOwn(report.vulnerabilities, name),
      "Missing audit via-chain entry",
    );
    requireValue(!active.has(name), "Cyclic audit via chain");
    if (leaves.has(name)) return leaves.get(name);
    active.add(name);
    const result = [
      ...new Set(
        report.vulnerabilities[name].via.flatMap((via) =>
          typeof via === "string" ? visit(via) : [via],
        ),
      ),
    ];
    active.delete(name);
    requireValue(result.length > 0, "Empty audit advisory chain");
    leaves.set(name, result);
    return result;
  }
  for (const name of Object.keys(report.vulnerabilities)) visit(name);
  return leaves;
}

export function evaluateAudits(full, production, context) {
  const leaves = validateAudit(full);
  validateAudit(production);
  requireValue(
    Object.keys(production.vulnerabilities).length === 0,
    "Production audit must have zero findings",
  );
  const waived = [];
  for (const [name, vuln] of Object.entries(full.vulnerabilities)) {
    const advisoryLeaves = leaves.get(name);
    requireValue(
      vuln.severity !== "critical" &&
        advisoryLeaves.every((leaf) => leaf.severity !== "critical"),
      "Critical advisory is never excepted",
    );
    if (
      vuln.severity !== "high" &&
      !advisoryLeaves.some((leaf) => leaf.severity === "high")
    )
      continue;
    requireValue(
      vuln.severity === "high" &&
        advisoryLeaves.every(
          (leaf) =>
            leaf.name === context.policy.package &&
            leaf.dependency === context.policy.package &&
            leaf.url === context.policy.advisoryUrl &&
            leaf.severity === "high",
        ),
      `Unapproved high advisory or mixed advisory chain: ${name}`,
    );
    for (const node of vuln.nodes) {
      requireValue(
        context.lock.packages[node]?.dev === true &&
          node.endsWith(`node_modules/${name}`),
        "Excepted via chain reaches a production or unknown package path",
      );
    }
    waived.push(name);
  }
  if (waived.length) {
    const root = full.vulnerabilities[context.policy.package];
    requireValue(
      root &&
        JSON.stringify([...root.nodes].sort()) ===
          JSON.stringify([...context.policy.paths].sort()),
      "Excepted package paths changed",
    );
    for (const node of root.nodes)
      requireValue(
        context.lock.packages[node]?.version === context.policy.version,
        "Excepted package version changed",
      );
  }
  return waived;
}

export function checkAuditExit(report, status) {
  validateAudit(report);
  const counts = report.metadata.vulnerabilities;
  requireValue(
    status === (counts.high + counts.critical > 0 ? 1 : 0),
    "npm audit did not complete with its expected status",
  );
}

export function validateFinalEvidence(root, context, now = Date.now()) {
  requireValue(
    sha256(readFileSync(join(root, RECEIPT))) ===
      sha256(readFileSync(join(root, "dist", PUBLIC_RECEIPT))),
    "Published audit receipt differs from the original prebuild record",
  );
  validatePublishedInventory(join(root, "dist"), context, now);
}
export function validatePublishedInventory(
  directory,
  context,
  now = Date.now(),
) {
  requireValue(
    !existsSync(join(directory, MODULES)),
    "Unfinished module inventory",
  );
  validateInventory(
    readJson(join(directory, INVENTORY)),
    context,
    readJson(join(directory, PUBLIC_RECEIPT)),
    outputFiles(directory),
    now,
  );
}

function audit(root, production) {
  const args = [
    "audit",
    "--audit-level=high",
    "--json",
    "--registry=https://registry.npmjs.org",
    "--fetch-retries=1",
    "--fetch-timeout=30000",
    "--cache",
    join(root, "node_modules/.cache/trollm-audit/npm"),
  ];
  if (production) args.push("--omit=dev");
  const npm = process.env.npm_execpath;
  const result = spawnSync(
    npm ? process.execPath : "npm",
    npm ? [npm, ...args] : args,
    {
      cwd: root,
      encoding: "utf8",
      timeout: 300000,
      maxBuffer: 16 * 1024 * 1024,
    },
  );
  requireValue(
    !result.error && result.signal === null && [0, 1].includes(result.status),
    "npm audit failed to complete",
  );
  let report;
  try {
    report = JSON.parse(result.stdout);
  } catch {
    throw new Error("npm audit did not return JSON evidence");
  }
  checkAuditExit(report, result.status);
  writeFileSync(
    join(
      root,
      "node_modules/.cache/trollm-audit",
      production ? "production.json" : "full.json",
    ),
    JSON.stringify(report, null, 2) + "\n",
  );
  return report;
}

export function runGate(root, phase, imageDirectory) {
  requireValue(
    ["prebuild", "final", "image"].includes(phase),
    "Use audit:prebuild, audit:final or audit:image",
  );
  if (phase === "image") {
    requireValue(
      typeof imageDirectory === "string" && imageDirectory.length > 0,
      "audit:image requires the extracted image web directory",
    );
    validatePublishedInventory(resolve(imageDirectory), readContext(root));
    console.log(
      "Provided frontend payload inventory, original public receipt and output hashes verified. audit:image requires the image's extracted web directory and does not replace its prebuild/final audits.",
    );
    return;
  }
  const state = join(root, "node_modules/.cache/trollm-audit");
  mkdirSync(state, { recursive: true });
  if (phase === "prebuild") {
    rmSync(join(root, RECEIPT), { force: true });
    rmSync(join(root, "dist", INVENTORY), { force: true });
    rmSync(join(root, "dist", MODULES), { force: true });
    rmSync(join(root, "dist", PUBLIC_RECEIPT), { force: true });
  }
  const context = readContext(root);
  if (phase === "final") validateFinalEvidence(root, context);
  const full = audit(root, false),
    production = audit(root, true);
  const current = readContext(root);
  const waived = evaluateAudits(full, production, current);
  if (phase === "prebuild") {
    writeFileSync(
      join(root, RECEIPT),
      JSON.stringify(
        {
          schema: "trollm.web-audit-prebuild.v1",
          nonce: randomBytes(16).toString("hex"),
          checkedAt: new Date().toISOString(),
          lockSha256: current.lockSha256,
          policySha256: current.policySha256,
        },
        null,
        2,
      ) + "\n",
    );
  } else {
    validateFinalEvidence(root, current);
  }
  if (waived.length) {
    console.log(
      `WAIVED ${current.policy.advisory}: ${current.policy.package}@${current.policy.version}, exactly 3 development-only paths, expires ${current.policy.expiresAt}.`,
    );
    console.log(
      `Full npm audit remains exit 1 (${waived.length} affected package entries); production audit has zero findings. This is not an audit-clean result.`,
    );
  } else
    console.log(
      "Full npm audit passed without a waiver; production audit has zero findings.",
    );
  console.log(
    phase === "prebuild"
      ? "Prebuild admission is provisional; audit:final and fresh bundle evidence remain mandatory."
      : "Final audit gate passed with current lockfile and verified build inventory.",
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    runGate(
      dirname(dirname(fileURLToPath(import.meta.url))),
      process.argv[2],
      process.argv[3],
    );
  } catch (error) {
    console.error(`Dependency audit gate failed: ${error.message}`);
    process.exitCode = 1;
  }
}
