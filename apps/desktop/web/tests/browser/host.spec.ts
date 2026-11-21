import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Bootstrap } from "../../src/api";

const liveHost = process.env.TROLLM_DESKTOP_BROWSER_URL;
const native = process.env.TROLLM_DESKTOP_BROWSER_REAL_MODE === "llama";

const hash = (bytes: string) => createHash("sha256").update(bytes).digest("hex");

function calculatorBundle() {
  const dir = mkdtempSync(join(tmpdir(), "trollm-desktop-browser-"));
  const entryFlow = "desktop-smoke.flow.yaml";
  const taskRegistry = "desktop-smoke.tasks.yaml";
  const setup = JSON.stringify({
    format: "trollm.flow-setup.v1",
    title: "Add two amounts",
    description: "Browser smoke fixture for local arithmetic.",
    models: {},
    connections: {},
  });
  const files = {
    [entryFlow]: `
schemaVersion: ledger.execution-plan.v1
id: desktop-smoke
version: 1
setup: {path: setup.json, sha256: "${hash(setup)}"}
limits: {maxSteps: 1, maxReasonerCalls: 0, maxTokensTotal: 0, timeoutS: 30, artifactMaxMb: 1}
onLimit: abort
models: {}
mcp:
  policies:
    local.calculate: {class: read, resultArtifact: calculation}
entryStep: calculate
nodes:
  calculate: {kind: task, taskRef: add, onVerified: done}
  done: {kind: end, outcome: succeeded}
`,
    [taskRegistry]: `
schemaVersion: ledger.task-registry.v1
tasks:
  add:
    execution: tool
    completion: tool-result
    tools: [local.calculate]
    toolArguments:
      local.calculate:
        modelFields: []
        fixed: {operation: add, left: '39.99', right: '40.10'}
    outputContract:
      outputs: {calculation: {fromArtifact: calculation, format: json, type: object}}
      requireArtifactComplete: [calculation]
`,
    "setup.json": setup,
  };
  const manifest = {
    format: "trollm.flow-bundle.v1", entryFlow, taskRegistry,
    files: Object.entries(files).map(([path, contents]) => ({ path, sha256: hash(contents) })),
  };
  try {
    writeFileSync(join(dir, "manifest.json"), JSON.stringify(manifest));
    for (const [path, contents] of Object.entries(files)) writeFileSync(join(dir, path), contents);
    return execFileSync("tar", ["--format=ustar", "-cf", "-", "--", "manifest.json", ...Object.keys(files)], { cwd: dir });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("actual desktop host opens an installed flow and runs it explicitly", async ({ page, request }) => {
  test.skip(!liveHost, "Set TROLLM_DESKTOP_BROWSER_URL for a disposable desktop host.");
  test.setTimeout(native ? 240000 : 45000);
  const bootstrapResponse = await request.get("/api/bootstrap");
  expect(bootstrapResponse.ok()).toBeTruthy();
  const bootstrap: Bootstrap = await bootstrapResponse.json();
  expect(bootstrap.backend).toBe(native ? "llama" : "stub");
  const anonymous = await request.get("/api/flows");
  expect(anonymous.status()).toBe(401);
  const passphrase = process.env.TROLLM_DESKTOP_BROWSER_PASSPHRASE ?? "disposable browser test vault passphrase";

  await page.goto("/");
  if (bootstrap.needsSetup) {
    await page.getByLabel("New passphrase", { exact: true }).fill(passphrase);
    await page.getByLabel("Confirm passphrase", { exact: true }).fill(passphrase);
    await page.getByRole("button", { name: "Create passphrase" }).click();
  } else {
    await page.getByLabel("Passphrase", { exact: true }).fill(passphrase);
    await page.getByRole("button", { name: "Unlock" }).click();
  }
  await expect(page.getByRole("heading", { name: "Flow library" })).toBeVisible({ timeout: 10000 });

  const name = native ? (process.env.TROLLM_DESKTOP_BROWSER_FLOW ?? "Summarize text") : "Add two amounts";
  if (native) {
    await expect(page.getByText("Local models", { exact: true })).toBeVisible();
    await page.getByRole("link", { name: `Open ${name}`, exact: true }).click();
    await page.getByLabel("Short text to summarize").fill("This local test uses a small model on a Linux host to summarize supplied text.");
  } else {
    await expect(page.getByText("Simulated models", { exact: true })).toBeVisible();
    await page.getByRole("link", { name: "Import flow" }).click();
    await page.getByLabel("Flow bundle").setInputFiles({
      name: "desktop-smoke.tar", mimeType: "application/x-tar", buffer: calculatorBundle(),
    });
    await page.getByRole("button", { name: "Import bundle" }).click();
    await expect(page.getByRole("heading", { name, exact: true })).toBeVisible();
    await page.getByRole("link", { name: "Inspect flow" }).click();
    await page.getByText("calculate · Entry step").click();
    await expect(page.getByText("local.calculate", { exact: true })).toBeVisible();
    await page.getByRole("link", { name: "Prepare flow" }).click();
  }

  const start = page.getByRole("button", { name: "Start run" });
  await expect(start).toBeEnabled();
  await start.click();
  await expect(page.getByText("Completed", { exact: true })).toBeVisible({ timeout: native ? 200000 : 10000 });
  if (native) await expect(page.locator(".report-text")).toHaveText(/\S/);
  await page.getByRole("link", { name: "Run history" }).click();
  const saved = page.getByRole("link", { name, exact: true }).first();
  await expect(saved).toBeVisible();
  await saved.click();
  await expect(page.getByText("Completed", { exact: true })).toBeVisible();
  if (native) await expect(page.getByRole("heading", { name: "Run report" })).toBeVisible();
});
