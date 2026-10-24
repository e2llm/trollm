import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import type { Inspection } from "../../src/model";
const realURL = process.env.FLOW_REVIEW_REAL_API_URL;
test("real API mounted starter, graph, feedback, prompt revision and tar inspection", async ({
  page,
  request,
}) => {
  test.skip(
    !realURL,
    "Set FLOW_REVIEW_REAL_API_URL and FLOW_REVIEW_BROWSER_URL to a running server mounted with flows/starters.",
  );
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error")
      errors.push(`${message.text()} (${message.location().url})`);
  });
  page.on("response", (response) => {
    if (response.status() >= 400)
      errors.push(`HTTP ${response.status()} ${response.url()}`);
  });
  const notices = await request.get(`${realURL}/third-party-notices.txt`);
  expect(notices.ok()).toBe(true);
  expect(await notices.text()).toContain("Permission is hereby granted");
  const packages = await (await request.get(`${realURL}/api/packages`)).json();
  const mounted = packages.packages.find(
    (p: { name: string }) => p.name === "check-total",
  );
  expect(mounted).toBeTruthy();
  const original: Inspection = await (
    await request.get(`${realURL}/api/packages/${mounted.id}`)
  ).json();
  expect(original.valid).toBe(true);
  expect(original.presentation?.phases).toHaveLength(3);
  await page.goto(realURL!);
  await page.getByLabel("Mounted packages").selectOption(mounted.id);
  await expect(
    page.getByRole("heading", { name: "Check a total", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Execution validation passed", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("note", { name: "Effective task failure defaults" }),
  ).toContainText("4 tasks in this view");
  await expect(
    page.getByRole("button", { name: /Calculate and compare/ }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "sum task sum-cart", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "check-total / sum" }),
  ).toBeVisible();
  await expect(
    page.getByRole("complementary", { name: "Inspector" }).getByRole("note"),
  ).toContainText("retry: 0");
  const sumTask = original.registry!.tasks["sum-cart"];
  const promptPath = sumTask.prompt as string;
  await expect(
    page.getByRole("button", { name: promptPath, exact: true }),
  ).toBeVisible();
  await page
    .getByText("Show read-only graph · 5 of 5 matching nodes", { exact: true })
    .click();
  await expect(page.locator(".react-flow")).toBeVisible();
  await expect(page.locator(".react-flow__edge")).toHaveCount(4);
  await expect(
    page.locator(".react-flow__node").filter({ hasText: "model: coder" }),
  ).toHaveCount(3);
  await expect(
    page.locator(".react-flow__node").filter({ hasText: "model: reasoner" }),
  ).toHaveCount(1);
  await page
    .getByLabel("Comment", { exact: true })
    .fill("Clarify how the two amounts are selected.");
  await page.getByRole("button", { name: "Add comment", exact: true }).click();
  const downloadPromise = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Export 1 comments as Markdown" })
    .click();
  const download = await downloadPromise;
  const markdown = await readFile((await download.path())!, "utf8");
  expect(markdown).toContain(original.packageId);
  expect(markdown).toContain("node:check-total/sum");
  expect(markdown).toContain("Clarify how the two amounts are selected.");
  await page
    .getByRole("heading", { name: "Executable graph", exact: true })
    .scrollIntoViewIfNeeded();
  await page.setViewportSize({ width: 1600, height: 1100 });
  await page.screenshot({
    path:
      process.env.FLOW_REVIEW_SCREENSHOT ?? "/tmp/trollm-flow-review-real.png",
    fullPage: true,
  });
  await page.getByRole("button", { name: "Use current as baseline" }).click();
  const changed = original.files.map((f) => ({
    name: f.path,
    mimeType: "text/plain",
    buffer: Buffer.from(
      f.content +
        (f.path === promptPath
          ? "\nReview clarification: select only the two explicitly stated item amounts.\n"
          : ""),
    ),
  }));
  await page.getByLabel("Open files or tar").setInputFiles(changed);
  await expect(
    page.getByRole("heading", { name: "Before / after comparison" }),
  ).toBeVisible();
  await expect(
    page.getByText(`Source text: ${promptPath}`, { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText(/earlier revision; review manually/),
  ).toBeVisible();
  await expect(
    page.getByText("Execution validation passed", { exact: true }),
  ).toBeVisible();
  await page
    .getByLabel("Open files or tar")
    .setInputFiles(
      resolve(
        "../../../apps/android/app/src/main/assets/flows/check-total.tar",
      ),
    );
  await expect(
    page.getByText(
      "No authored business description. Inspect the executable sources below.",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(
    page.getByText("Execution validation passed", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "sum task sum-cart", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});
