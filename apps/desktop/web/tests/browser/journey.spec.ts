import { expect, test, type Page } from "@playwright/test";
import type { ConnectionStatus, Flow, ModelStatus, Run } from "../../src/api";

const flow: Flow = {
  sha256: "a".repeat(64),
  id: "text-summary",
  title: "Summarize text",
  description: "Write a short summary using a local model.",
  version: 1,
  setup: { input: { label: "Short text to summarize" }, connections: {} },
};
const browserFlow: Flow = {
  sha256: "b".repeat(64),
  id: "browser-summary",
  title: "Browse and summarize",
  description: "Read a page in a connected browser.",
  version: 1,
  setup: {
    input: {
      label: "Browser request",
      fields: [
        { key: "url", label: "Page URL", required: true, kind: "url", maxLength: 2048 },
        { key: "question", label: "Question", required: false, kind: "text", maxLength: 80 },
      ],
    },
    connections: {},
  },
};
const runId = "11111111-2222-4333-8444-555555555555";
const report = "The supplied passage describes a locally run workflow.\n<img src=x onerror=alert(1)>";
const simulatedToken = "simulated-session";
const model: ModelStatus = {
  slot: "coder", name: "Example model", url: "https://models.example.org/weights",
  sha256: "c".repeat(64), sizeBytes: 104857600, downloadedBytes: 104857600, status: "ready",
};
const available: ConnectionStatus = {
  name: "browser", url: "https://mcp.example.org/mcp", mode: "unavailable",
  connected: false, pending: false, status: "Sign in or add a token.",
};

async function simulate(page: Page) {
  const state = {
    needsSetup: true,
    models: [{ ...model }],
    connection: { ...available },
    run: undefined as Run | undefined,
    loseResponse: false,
    omitRegistry: false,
    starts: [] as { requestId: string; flowSha256: string; input: string }[],
    actions: [] as string[],
    modelActions: [] as string[],
    connectionSaves: [] as { url: string; mode: string; token?: string }[],
    after: [] as number[],
  };

  await page.route("**/api/**", async (route) => {
    const req = route.request();
    const method = req.method();
    const url = new URL(req.url());
    const path = url.pathname;
    const reply = (json: unknown, status = 200) => route.fulfill({ json, status });

    if (path === "/api/bootstrap") return reply({ backend: "stub", needsSetup: state.needsSetup, version: "1.1.0" });
    if (path === "/api/session" && method === "POST") {
      state.needsSetup = false;
      return reply({ token: simulatedToken });
    }
    if (req.headers()["authorization"] !== `Bearer ${simulatedToken}`) {
      return reply({ error: { code: "SESSION_REQUIRED", message: "Unlock required." } }, 401);
    }
    expect(req.url()).not.toContain(simulatedToken);
    if (path === "/api/flows") return reply({ flows: [flow, browserFlow] });
    if (path === "/api/session" && method === "DELETE") return reply({});
    if (path === "/api/flows/import") {
      expect(method).toBe("POST");
      expect(req.headers()["content-type"]).toBe("application/x-tar");
      return reply({ flow });
    }
    if (path === `/api/flows/${flow.sha256}/inspect`) return reply({
      diagnostics: [], files: [{ path: "summarize.md", content: "<a href=javascript:alert(1)>Prompt</a>" }],
      plans: [{ id: flow.id, entryStep: "summary", nodes: { summary: { kind: "task", taskRef: "write" } } }],
      registry: { tasks: { write: { model: "coder", prompt: "summarize.md", tools: [] } } },
    });
    if (path === `/api/flows/${browserFlow.sha256}/inspect`) return reply({
      diagnostics: state.omitRegistry
        ? [{ level: "warning", message: "Some source details exceed the inspection limit." }]
        : [],
      files: [],
      plans: [{
        id: browserFlow.id,
        entryStep: "read",
        nodes: {
          read: { kind: "task", taskRef: "read-browser" },
          items: { kind: "foreach", body: { flowRef: "read-item" }, between: { flowRef: "return-to-list" } },
          finished: { kind: "end", outcome: "succeeded" },
        },
      }],
      registry: state.omitRegistry ? null : { tasks: { "read-browser": { tools: ["e2llm.read_page"] } } },
    });
    if (path === `/api/flows/${flow.sha256}` || path === `/api/flows/${browserFlow.sha256}`) {
      const selected = path.endsWith(browserFlow.sha256) ? browserFlow : flow;
      const connections = selected === browserFlow ? [state.connection] : [];
      const canStart = state.models.every((model) => model.status === "ready") && connections.every((item) => item.connected);
      return reply({ flow: selected, models: state.models, connections, canStart, blockers: canStart ? [] : ["Finish model or connection setup."] });
    }
    if (/^\/api\/flows\/[^/]+\/models\/coder\/(download|pause)$/.test(path) && method === "POST") {
      const action = path.endsWith("/pause") ? "pause" : "download";
      state.modelActions.push(action);
      state.models = [{ ...model, status: action === "pause" ? "paused" : "downloading", downloadedBytes: model.sizeBytes / 2 }];
      return reply({ models: state.models });
    }
    if (path === "/api/connections/browser/oauth/start" && method === "POST") {
      expect(req.postDataJSON()).toEqual({ url: available.url });
      state.connection = { ...available, pending: true, status: "Waiting for browser sign-in." };
      return reply({ authorizationUrl: "https://auth.example.org/sign-in" });
    }
    if (path === "/api/connections/browser" && method === "PUT") {
      state.connectionSaves.push(req.postDataJSON());
      state.connection = { ...available, mode: "bearer", connected: true, status: "Available." };
      return reply({ connection: state.connection });
    }
    if (path === "/api/runs" && method === "GET") return reply({ runs: state.run ? [state.run] : [] });
    if (path === "/api/runs" && method === "POST") {
      const start = req.postDataJSON();
      state.starts.push(start);
      state.run = {
        id: runId, flowSha256: start.flowSha256,
        phase: state.loseResponse ? "succeeded" : "running",
        workerActive: !state.loseResponse,
        canResume: false,
        report: state.loseResponse ? report : undefined,
        activity: "Writing a summary",
        lastSeq: 2,
      };
      return state.loseResponse ? route.abort("failed") : reply({ id: runId, phase: state.run.phase });
    }
    if (path === `/api/runs/${runId}` && state.run) {
      const after = Number(url.searchParams.get("after"));
      state.after.push(after);
      return reply({
        run: state.run,
        events: after === 0 ? [{ seq: 1, type: "note", text: "<b>First update</b>" }]
          : after === 1 ? [{ seq: 2, type: "note", text: "Second update" }] : [],
        hasMore: after === 0,
        nextSeq: after === 0 ? 1 : 2,
      });
    }
    if (path === `/api/runs/${runId}/action` && state.run && method === "POST") {
      const { action } = req.postDataJSON();
      state.actions.push(action);
      state.run = {
        ...state.run,
        phase: action === "pause" ? "paused" : "succeeded",
        workerActive: action === "pause",
        canResume: action === "pause",
        report: action === "resume" ? report : undefined,
        error: undefined,
        activity: undefined,
      };
      return reply({ run: state.run });
    }
    return reply({ error: { message: "Unexpected simulated request." } }, 404);
  });
  return state;
}

async function setup(page: Page) {
  await page.goto("/");
  const passphrase = page.getByLabel("New passphrase", { exact: true });
  await expect(passphrase).toHaveAccessibleDescription("Use a long, unique passphrase.");
  await passphrase.fill("simulated vault passphrase");
  await page.getByLabel("Confirm passphrase", { exact: true }).fill("simulated vault passphrase");
  await page.getByRole("button", { name: "Create passphrase" }).click();
  await expect(page.getByRole("heading", { name: "Flow library" })).toBeVisible();
  await expect(page.getByText("Simulated models")).toBeVisible();
}

test("simulated library, inspected source, explicit run, pause, resume and saved report", async ({ page, context }) => {
  const state = await simulate(page);
  await setup(page);
  await page.getByRole("link", { name: "Open Summarize text" }).click();
  await page.getByLabel("Short text to summarize").fill("Input for the local workflow.");
  await page.getByRole("link", { name: "Inspect flow" }).click();
  await page.getByText("summary · Entry step").click();
  await expect(page.getByText("None declared.")).toBeVisible();
  await page.getByLabel("Source file").selectOption("summarize.md");
  await expect(page.locator("pre")).toHaveText("<a href=javascript:alert(1)>Prompt</a>");
  await expect(page.locator("pre a")).toHaveCount(0);
  await page.getByRole("link", { name: "Prepare flow" }).click();
  await expect(page.getByLabel("Short text to summarize")).toHaveValue("Input for the local workflow.");
  expect(state.starts).toHaveLength(0);

  await page.getByRole("button", { name: "Start run" }).click();
  await expect(page.getByText("Writing a summary")).toBeVisible();
  expect(state.starts).toHaveLength(1);
  expect(state.starts[0]).toEqual({
    flowSha256: flow.sha256,
    input: "Input for the local workflow.",
    requestId: expect.stringMatching(/^[a-f0-9-]{36}$/),
  });
  await page.getByRole("button", { name: "Pause run" }).click();
  await expect(page.getByText("Paused", { exact: true })).toBeVisible();
  expect(state.actions).toEqual(["pause"]);
  await page.reload();
  await expect(page.getByText("Paused", { exact: true })).toBeVisible();
  expect(state.actions).toEqual(["pause"]);
  await page.getByRole("button", { name: "Resume run" }).click();

  await expect(page.locator(".report-text")).toHaveText(report);
  await expect(page.locator(".report-text img")).toHaveCount(0);
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.getByRole("button", { name: "Copy report" }).click();
  await expect(page.getByText("Report copied.")).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(report);
  await page.getByText("Technical activity").click();
  await expect(page.getByText("<b>First update</b>")).toBeVisible();
  await expect(page.getByText("Second update")).toBeVisible();
  expect(state.after).toContain(1);

  await page.getByRole("link", { name: "Run history" }).click();
  await expect(page.getByText("Completed", { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "Summarize text" }).click();
  await expect(page.getByRole("heading", { name: "Run report" })).toBeVisible();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("heading", { name: "Unlock trollm" })).toBeVisible();
});

test("simulated inspection marks omitted tool grants and child plans as unavailable", async ({ page }) => {
  const state = await simulate(page);
  await setup(page);
  await page.getByRole("link", { name: "Open Browse and summarize" }).click();
  await page.getByRole("link", { name: "Inspect flow" }).click();
  await page.getByText("read · Entry step").click();
  await expect(page.getByText("e2llm.read_page", { exact: true })).toBeVisible();

  state.omitRegistry = true;
  await page.reload();
  await page.getByText("read · Entry step").click();
  await expect(page.getByText("Tool permissions unavailable in this inspection.")).toBeVisible();
  await expect(page.getByText("None declared.")).toHaveCount(0);
  await page.getByText("items", { exact: true }).click();
  await expect(page.getByText("Per-item flow · read-item · Details unavailable")).toBeVisible();
  await expect(page.getByText("Between-item flow · return-to-list · Details unavailable")).toBeVisible();
  await page.getByText("finished", { exact: true }).click();
  await expect(page.getByText("Ends the flow · succeeded")).toBeVisible();
  await expect(page.getByText("Granted tools")).toHaveCount(0);
});

test("simulated recovery offers Resume only when the server admits it", async ({ page }) => {
  const state = await simulate(page);
  state.run = {
    id: runId, flowSha256: flow.sha256, phase: "interrupted",
    workerActive: false, canResume: false, lastSeq: 2,
  };
  await setup(page);
  await page.getByRole("link", { name: "History", exact: true }).click();
  await page.getByRole("link", { name: "Summarize text", exact: true }).click();
  const resume = page.getByRole("button", { name: "Resume run" });
  await expect(page.getByText("The run stopped before it finished.")).toBeVisible();
  await expect(resume).toHaveCount(0);

  state.run = { ...state.run, phase: "failed", error: "The worker stopped." };
  await page.reload();
  await expect(page.getByText("The worker stopped.", { exact: true })).toBeVisible();
  await expect(resume).toHaveCount(0);

  state.run = { ...state.run, canResume: true };
  await page.reload();
  await expect(page.getByText("Saved progress is available.")).toBeVisible();
  await expect(resume).toBeEnabled();
  expect(state.actions).toEqual([]);
  await resume.click();
  await expect(page.getByRole("heading", { name: "Run report" })).toBeVisible();
  expect(state.actions).toEqual(["resume"]);
});

test("simulated lost Start response remains fenced across reload and history is recoverable", async ({ page }) => {
  const state = await simulate(page);
  state.loseResponse = true;
  await setup(page);
  await page.getByRole("link", { name: "Open Summarize text" }).click();
  await page.getByLabel("Short text to summarize").fill("Sensitive simulated input.");
  await page.getByRole("button", { name: "Start run" }).click();
  await expect(page.getByRole("heading", { name: "Start could not be confirmed" })).toBeVisible();
  const saved = await page.evaluate(() => JSON.parse(sessionStorage.getItem("trollm.desktop.pending-start") ?? "{}"));
  expect(saved).toEqual({ requestId: state.starts[0].requestId, flowSha256: flow.sha256 });

  await page.reload();
  await expect(page.getByRole("link", { name: "Summarize text" })).toBeVisible();
  expect(state.starts).toHaveLength(1);
  await page.getByRole("link", { name: "Summarize text" }).click();
  await expect(page.locator(".report-text")).toHaveText(report);

  await page.getByRole("link", { name: "Flows", exact: true }).click();
  await page.getByRole("link", { name: "Open Summarize text" }).click();
  await expect(page.getByRole("button", { name: "Start run" })).toBeDisabled();
  await page.getByRole("link", { name: "Open run history" }).click();
  await page.getByRole("button", { name: "I reviewed the history" }).click();
  await page.getByRole("button", { name: "Allow new starts" }).click();
  await expect(page.getByRole("heading", { name: "Start could not be confirmed" })).toHaveCount(0);
  expect(state.starts).toHaveLength(1);
});

test("simulated preparation renders dynamic input, download progress and service setup", async ({ page }) => {
  const state = await simulate(page);
  state.models = [{ ...model, status: "missing", downloadedBytes: 0 }];
  await setup(page);
  await page.getByRole("link", { name: "Open Browse and summarize" }).click();
  await page.getByLabel("Page URL").fill("ftp://example.org/");
  await page.getByLabel("Question").fill("What happened?");
  await expect(page.getByRole("button", { name: "Start run" })).toBeDisabled();
  await page.getByRole("button", { name: "Download model" }).click();
  await expect(page.getByRole("progressbar", { name: "Download progress for Example model" })).toBeVisible();
  await expect(page.getByText("50% · 50 MiB of 100 MiB")).toBeVisible();
  await page.getByRole("button", { name: "Pause download" }).click();
  await page.getByRole("button", { name: "Resume download" }).click();
  expect(state.modelActions).toEqual(["download", "pause", "download"]);
  state.models = [{ ...model }];

  await page.getByRole("button", { name: "Sign in with browser" }).click();
  await expect(page.getByRole("link", { name: "Open sign-in page" })).toHaveAttribute("href", "https://auth.example.org/sign-in");
  await page.getByText("Manual connection").click();
  await page.getByLabel("Bearer token", { exact: true }).fill("test-simulated-credential");
  await page.getByRole("button", { name: "Save connection" }).click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  expect(state.connectionSaves).toEqual([{ url: available.url, mode: "bearer", token: "test-simulated-credential" }]);
  await page.getByRole("button", { name: "Start run" }).click();
  await expect(page.getByText("Enter an HTTP or HTTPS URL without embedded credentials.")).toBeVisible();
  expect(state.starts).toHaveLength(0);
  await page.getByLabel("Page URL").fill("https://example.org/");
  await page.getByRole("button", { name: "Start run" }).click();
  await expect(page.getByText("Writing a summary")).toBeVisible();
  expect(JSON.parse(state.starts[0].input)).toEqual({ url: "https://example.org/", question: "What happened?" });
});

test("simulated OAuth clears completed links and shows a fresh link during re-sign-in", async ({ page, context }) => {
  const state = await simulate(page);
  await setup(page);
  await page.getByRole("link", { name: "Open Browse and summarize" }).click();

  const saved = { ...available, connected: true, mode: "oauth" as const, status: "OAuth account connected." };
  let result = "connected";
  await context.route("https://auth.example.org/sign-in", (route) => {
    state.connection = saved;
    const callback = new URL(`/?oauth=${result}`, page.url()).href;
    return route.fulfill({ status: 302, headers: { location: callback } });
  });
  const link = page.getByRole("link", { name: "Open sign-in page" });
  const startSignIn = page.getByRole("button", { name: "Sign in with browser" });
  const savedStatus = page.getByText("OAuth account connected.", { exact: true });
  const authorizationAnchor = page.locator('a[href="https://auth.example.org/sign-in"]');

  async function finishSignIn(callbackHeading: string) {
    await expect(page.getByText("Waiting for browser sign-in.", { exact: true })).toBeVisible();
    await expect(link).toBeVisible();
    const popupPromise = page.waitForEvent("popup");
    await link.click();
    const popup = await popupPromise;
    await expect(popup.getByRole("heading", { name: callbackHeading })).toBeVisible();
    await popup.close();
    await expect(savedStatus).toBeVisible();
    await expect(authorizationAnchor).toHaveCount(0);
    await expect(page.getByText("Open the sign-in page in a new tab", { exact: false })).toHaveCount(0);
  }

  await startSignIn.click();
  await finishSignIn("Connection saved");

  result = "failed";
  await page.getByText("Manage connection", { exact: true }).click();
  await startSignIn.click();
  await finishSignIn("Sign-in could not be completed");
  expect(state.connectionSaves).toHaveLength(0);
  expect(state.starts).toHaveLength(0);
});

test("simulated import sends the tar directly and opens the installed flow", async ({ page }) => {
  await simulate(page);
  await setup(page);
  await page.getByRole("link", { name: "Import flow" }).click();
  await page.getByLabel("Flow bundle").setInputFiles({
    name: "simulated.tar", mimeType: "application/x-tar", buffer: Buffer.from("simulated fixture"),
  });
  await page.getByRole("button", { name: "Import bundle" }).click();
  await expect(page.getByRole("heading", { name: "Summarize text", exact: true })).toBeVisible();
});
