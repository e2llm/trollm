import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test, expect } from "@playwright/test";
import { fixture } from "../fixture";
test.use({ viewport: { width: 1440, height: 900 } });
const browserErrors = new WeakMap<import("@playwright/test").Page, string[]>();
test.beforeEach(async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  browserErrors.set(page, errors);
});
test.afterEach(async ({ page }) => {
  expect(browserErrors.get(page)).toEqual([]);
});
async function openNodes(page: import("@playwright/test").Page) {
  if (
    !(await page
      .locator(".node-browser")
      .evaluate((el) => (el as HTMLDetailsElement).open))
  )
    await page.locator(".node-browser > summary").click();
}
async function tab(
  page: import("@playwright/test").Page,
  name: string | RegExp,
) {
  await page
    .getByRole("tab", { name, exact: typeof name === "string" })
    .click();
}
test("import, inspect shared prompt, comment, export, compare and keep anchors", async ({
  page,
}) => {
  let doc = fixture();
  await page.route("**/api/packages", (route) =>
    route.fulfill({ json: { packages: [] } }),
  );
  await page.route("**/api/inspect", async (route) => {
    const body = route.request().postDataJSON();
    expect(body.files[0].path).toBe("main.flow.yaml");
    await route.fulfill({ json: doc });
  });
  await page.goto("/");
  await page.getByLabel("Open files or tar").setInputFiles({
    name: "main.flow.yaml",
    mimeType: "text/plain",
    buffer: Buffer.from("source"),
  });
  await expect(
    page.getByRole("heading", { name: "Review example", exact: true }),
  ).toBeVisible();
  await openNodes(page);
  await page.getByRole("button", { name: "start task shared", exact: true }).click();
  await expect(
    page.getByText("Shared task used by: main/start, main/unreachable"),
  ).toBeVisible();
  await expect(
    page.getByText("Read {{ user_input }} <script>alert(1)</script>", {
      exact: true,
    }),
  ).toBeVisible();
  await tab(page, /Feedback/);
  await page
    .getByLabel("Comment", { exact: true })
    .fill("Please clarify the allowed routes.");
  await page.getByRole("button", { name: "Add comment", exact: true }).click();
  const download = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Export 1 comments as Markdown" })
    .click();
  expect((await download).suggestedFilename()).toBe("flow-review-feedback.md");
  await page.getByRole("button", { name: "Use current as baseline" }).click();
  doc = structuredClone(doc);
  doc.packageId = "revision-2";
  doc.plans[0].nodes.start.routes = { finish: "done" };
  await page.getByLabel("Open files or tar").setInputFiles({
    name: "main.flow.yaml",
    mimeType: "text/plain",
    buffer: Buffer.from("revised"),
  });
  await tab(page, /Feedback/);
  await expect(
    page.getByText("earlier revision; review manually", { exact: false }),
  ).toBeVisible();
  await tab(page, "Changes");
  await expect(
    page.getByText("Edges / recovery: main/start.routes", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Unassigned nodes", exact: false })
    .click();
  await openNodes(page);
  await expect(
    page.getByRole("button", { name: "unreachable task shared", exact: true }),
  ).toBeVisible();
});
test("incomplete packages keep diagnostics, source and explicit root selection", async ({
  page,
}) => {
  const doc = fixture();
  doc.valid = false;
  doc.plans = [];
  doc.registry = null;
  doc.entryFlow = null;
  doc.taskRegistry = null;
  doc.diagnostics = [
    {
      level: "error",
      code: "root-selection",
      message: "Choose root and registry",
    },
  ];
  await page.route("**/api/packages", (route) =>
    route.fulfill({
      json: {
        packages: [{ id: "mounted", name: "Incomplete", kind: "source" }],
      },
    }),
  );
  await page.route("**/api/packages/mounted", (route) =>
    route.fulfill({ json: doc }),
  );
  await page.goto("/");
  await page.getByLabel("Mounted packages").selectOption("mounted");
  await expect(page.getByText("Choose root and registry")).toBeVisible();
  await expect(page.getByLabel("Root flow")).toBeVisible();
  await tab(page, "Source");
  await page.getByLabel("Source file").selectOption("prompts/shared.md");
  await expect(
    page
      .getByText("Read {{ user_input }} <script>alert(1)</script>", {
        exact: true,
      })
      .last(),
  ).toBeVisible();
});

test("large cyclic package keeps graph visible and reveals navigation progressively", async ({
  page,
}) => {
  const doc = fixture();
  doc.files[0].content = "Long authored prompt ".repeat(16000);
  doc.plans[0].nodes = Object.fromEntries(
    Array.from({ length: 1500 }, (_, i) => [
      `n${i}`,
      { kind: "task", taskRef: "shared", onVerified: `n${(i + 1) % 1500}` },
    ]),
  );
  doc.plans[0].entryStep = "n0";
  doc.presentation = null;
  doc.unassignedNodes = Object.keys(doc.plans[0].nodes).map((node) => ({
    flow: "main",
    node,
  }));
  await page.route("**/api/packages", (route) =>
    route.fulfill({
      json: { packages: [{ id: "large", name: "Large", kind: "source" }] },
    }),
  );
  await page.route("**/api/packages/large", (route) =>
    route.fulfill({ json: doc }),
  );
  await page.goto("/");
  await page.getByLabel("Mounted packages").selectOption("large");
  await expect(page.locator(".react-flow")).toBeVisible();
  await expect(page.locator(".react-flow__node.selected")).toContainText("n0");
  await expect
    .poll(() =>
      page
        .locator(".react-flow__viewport")
        .evaluate((el) => new DOMMatrix(getComputedStyle(el).transform).a),
    )
    .toBeGreaterThanOrEqual(0.8);
  await openNodes(page);
  await expect(
    page.getByRole("navigation", { name: "Flow nodes" }).getByRole("button"),
  ).toHaveCount(100);
  await page
    .getByRole("button", { name: "Show next 100 nodes (1400 remaining)" })
    .click();
  await expect(
    page.getByRole("navigation", { name: "Flow nodes" }).getByRole("button"),
  ).toHaveCount(200);

  await expect(page.locator(".react-flow")).toBeVisible();
  await page
    .getByRole("button", { name: "n0 task shared", exact: true })
    .click();
  await expect(page.getByRole("heading", { name: "main / n0" })).toBeVisible();
  await expect(
    page.getByText("Long authored prompt ".repeat(16000).trim(), {
      exact: true,
    }),
  ).toBeVisible();
});

test("folder imports preserve package paths and tar imports send raw bytes", async ({
  page,
}) => {
  const directory = await mkdtemp(join(tmpdir(), "trollm-browser-"));
  try {
    await mkdir(join(directory, "prompts"));
    await writeFile(join(directory, "main.flow.yaml"), "flow source");
    await writeFile(join(directory, "prompts", "shared.md"), "prompt source");
    await page.route("**/api/packages", (route) =>
      route.fulfill({ json: { packages: [] } }),
    );
    let folderSeen = false,
      tarSeen = false;
    await page.route("**/api/inspect", (route) => {
      const paths = route
        .request()
        .postDataJSON()
        .files.map((f: { path: string }) => f.path)
        .sort();
      expect(paths).toEqual(["main.flow.yaml", "prompts/shared.md"]);
      folderSeen = true;
      return route.fulfill({ json: fixture() });
    });
    await page.route("**/api/upload", (route) => {
      expect(route.request().headers()["content-type"]).toBe(
        "application/octet-stream",
      );
      expect(route.request().postDataBuffer()?.toString()).toBe(
        "raw tar bytes",
      );
      tarSeen = true;
      return route.fulfill({ json: fixture() });
    });
    await page.goto("/");
    await page.getByLabel("Open folder").setInputFiles(directory);
    await expect(
      page.getByRole("heading", { name: "Review example", exact: true }),
    ).toBeVisible();
    expect(folderSeen).toBe(true);
    await page.getByLabel("Open files or tar").setInputFiles({
      name: "example.tar",
      mimeType: "application/octet-stream",
      buffer: Buffer.from("raw tar bytes"),
    });
    await expect.poll(() => tarSeen).toBe(true);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("default failure notices, authored at-sign graph nodes and multiline comparison stay accurate", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  let doc = fixture();
  doc.presentation = null;
  doc.files[0].content = 'First line\nSecond "quoted" line\n';
  doc.plans[0].entryStep = "@summarize";
  doc.plans[0].nodes = {
    "@summarize": {
      kind: "task",
      taskRef: "shared",
      onError: null,
      onVerified: "step",
    },
    step: {
      kind: "foreach",
      onLimit: { status: "failed" },
      onEmpty: "@step:onLimit",
      body: { flowRef: "child" },
    },
    "@step:onLimit": { kind: "end", outcome: "ok" },
    "@review:0": { kind: "end", outcome: "ok" },
  };
  doc.plans[0].nodes = Object.fromEntries([
    ...Object.entries(doc.plans[0].nodes),
    [
      "__proto__",
      { kind: "task", taskRef: "shared", onVerified: "constructor" },
    ],
    ["constructor", { kind: "end", outcome: "ok" }],
  ]);
  doc.unassignedNodes = Object.keys(doc.plans[0].nodes).map((node) => ({
    flow: "main",
    node,
  }));
  await page.route("**/api/packages", (route) =>
    route.fulfill({
      json: {
        packages: [
          { id: "fixture", name: "Collision fixture", kind: "source" },
        ],
      },
    }),
  );
  await page.route("**/api/packages/fixture", (route) =>
    route.fulfill({ json: doc }),
  );
  await page.goto("/");
  await page.getByLabel("Mounted packages").selectOption("fixture");
  await expect(
    page.getByRole("note", { name: "Effective task failure defaults" }),
  ).toContainText("2 tasks in this view");
  await openNodes(page);
  await page.getByRole("button", { name: "Fit graph", exact: true }).click();
  await expect(page.locator(".react-flow__node")).toHaveCount(8);
  for (const id of [
    "@summarize",
    "__proto__",
    "constructor",
    "@step:onLimit",
  ]) {
    await page.getByRole("button", { name: "Fit graph", exact: true }).click();
    await page.locator(".react-flow__node").filter({ hasText: id }).click();
    await expect(
      page.getByRole("heading", { name: `main / ${id}`, exact: true }),
    ).toBeVisible();
  }
  await expect(
    page.getByRole("complementary", { name: "Inspector" }).getByRole("note"),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "@summarize task shared", exact: true })
    .click();
  await expect(
    page.getByRole("complementary", { name: "Inspector" }).getByRole("note"),
  ).toContainText("retry: 0");
  await page.getByRole("button", { name: "Use current as baseline" }).click();
  doc = structuredClone(doc);
  doc.packageId = "new";
  doc.files[0].content = 'First line\nChanged "quoted" line\n';
  doc.files[0].sha256 = "new";
  await page.getByLabel("Mounted packages").selectOption("fixture");
  const comparison = page.getByRole("group").filter({
    has: page.getByText("Source text: prompts/shared.md", { exact: true }),
  });
  await comparison.locator("summary").click();
  await expect(comparison.locator("pre").nth(0)).toHaveText(
    'First line\nSecond "quoted" line\n',
  );
  await expect(comparison.locator("pre").nth(1)).toHaveText(
    'First line\nChanged "quoted" line\n',
  );
  expect(errors).toEqual([]);
});

for (const viewport of [
  { width: 1440, height: 900 },
  { width: 2560, height: 1440 },
]) {
  test(`desktop ${viewport.width}: graph, cross-phase navigation and independent inspector scrolling`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const doc = fixture();
    doc.files[0].content = "Long instruction with preserved evidence.\n".repeat(
      1000,
    );
    doc.presentation!.phases.push({
      id: "finish",
      title: "Finish phase",
      description: "Review the outcome",
      nodes: [{ flow: "main", node: "done" }],
    });
    doc.unassignedNodes = [{ flow: "main", node: "unreachable" }];
    await page.route("**/api/packages", (route) =>
      route.fulfill({
        json: {
          packages: [{ id: "desktop", name: "Desktop", kind: "source" }],
        },
      }),
    );
    await page.route("**/api/packages/desktop", (route) =>
      route.fulfill({ json: doc }),
    );
    await page.goto("/");
    await page.getByLabel("Mounted packages").selectOption("desktop");
    const canvas = page.locator(".graph-canvas");
    const inspector = page.getByRole("complementary", { name: "Inspector" });
    await expect(canvas).toBeVisible();
    await expect(page.locator(".react-flow__node")).toHaveCount(3);
    const nodeTab = page.getByRole("tab", { name: "Node", exact: true });
    await nodeTab.focus();
    await page.keyboard.press("ArrowRight");
    await expect(
      page.getByRole("tab", { name: "Package", exact: true }),
    ).toBeFocused();
    await page.keyboard.press("End");
    await expect(
      page.getByRole("tab", { name: "Changes", exact: true }),
    ).toHaveAttribute("aria-selected", "true");
    await page.keyboard.press("Home");
    await expect(nodeTab).toBeFocused();
    await expect(nodeTab).toHaveAttribute("aria-selected", "true");
    const canvasBox = (await canvas.boundingBox())!;
    const inspectorBox = (await inspector.boundingBox())!;
    const workspaceBox = (await page.locator(".workspace").boundingBox())!;
    expect(inspectorBox.x).toBeGreaterThanOrEqual(
      canvasBox.x + canvasBox.width,
    );
    expect(workspaceBox.width).toBeGreaterThan(viewport.width * 0.96);
    expect(canvasBox.height).toBeGreaterThan(viewport.height * 0.45);
    expect(inspectorBox.x + inspectorBox.width).toBeLessThanOrEqual(
      viewport.width,
    );
    const edges = page.locator(".react-flow__edge-path");
    expect(await edges.count()).toBeGreaterThanOrEqual(4);
    for (const edge of await edges.all()) {
      await expect(edge).toHaveAttribute("marker-end", /url\(/);
      await expect(edge).toHaveAttribute("d", /[CL]/);
    }
    await expect(
      page.locator(".react-flow__edge-text").filter({ hasText: "again" }),
    ).toBeVisible();
    const beforeScroll = await page.evaluate(() => ({
      x: window.scrollX,
      y: window.scrollY,
    }));
    await page.getByRole("tabpanel").hover();
    await page.mouse.wheel(0, 700);
    await expect
      .poll(() => page.getByRole("tabpanel").evaluate((el) => el.scrollTop))
      .toBeGreaterThan(0);
    expect(
      await page.evaluate(() => ({ x: window.scrollX, y: window.scrollY })),
    ).toEqual(beforeScroll);
    expect(await canvas.boundingBox()).toEqual(canvasBox);
    const outside = page
      .locator(".react-flow__node")
      .filter({ hasText: "done" });
    await expect(outside).toContainText("Outside phase");
    await outside.click();
    await expect(
      page.getByRole("heading", { name: "main / done", exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: /PHASE 2/ })).toHaveClass(
      /active/,
    );
    await expect(page.locator(".react-flow__node.selected")).toContainText(
      "done",
    );
    await expect(
      page.locator(".react-flow__node").filter({ hasText: "start" }),
    ).toContainText("Outside phase");
    await expect(canvas).toBeVisible();
  });
}
