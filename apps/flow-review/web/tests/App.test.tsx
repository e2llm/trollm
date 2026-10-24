// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
  waitFor,
} from "@testing-library/react";
import { App } from "../src/App";
import { fixture } from "./fixture";
// Graph rendering and layout are exercised in browser and layout tests.
vi.mock("../src/Graph", () => ({
  default: () => <div aria-label="Graph fixture" />,
}));
function openNodes() {
  (document.querySelector(".node-browser") as HTMLDetailsElement).open = true;
}
function feedback() {
  fireEvent.click(screen.getByRole("tab", { name: /Feedback/ }));
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
describe("review page", () => {
  it("shows view defaults and selected-task defaults without assigning a task policy to end nodes", async () => {
    const doc = fixture();
    doc.plans[0].nodes.unreachable.onError = null;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => ({
        ok: true,
        json: async () =>
          path === "/api/packages"
            ? {
                packages: [
                  { id: "fixture", name: "Mounted fixture", kind: "source" },
                ],
              }
            : doc,
      })),
    );
    render(<App />);
    await screen.findByRole("option", { name: "Mounted fixture (source)" });
    fireEvent.change(screen.getByLabelText("Mounted packages"), {
      target: { value: "fixture" },
    });
    await screen.findByRole("heading", { name: "Review example" });
    openNodes();
    fireEvent.click(screen.getByRole("button", { name: /All nodes/ }));
    expect(
      screen.getByRole("note", { name: "Effective task failure defaults" })
        .textContent,
    ).toContain("1 task in this view");
    fireEvent.click(screen.getByRole("button", { name: /unreachable task/ }));
    const inspector = screen.getByRole("complementary", { name: "Inspector" });
    expect(within(inspector).getByRole("note").textContent).toContain(
      "retry: 0",
    );
    fireEvent.click(screen.getByRole("button", { name: /done end/ }));
    expect(within(inspector).queryByRole("note")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /PHASE 1/ }));
    expect(
      screen.queryByRole("note", { name: "Effective task failure defaults" }),
    ).toBeNull();
  });
  it("bounds file count before submitting an inspection", async () => {
    const fetch = vi.fn(async () => ({
      ok: true,
      json: async () => ({ packages: [] }),
    }));
    vi.stubGlobal("fetch", fetch);
    render(<App />);
    fireEvent.change(screen.getByLabelText("Open files or tar"), {
      target: {
        files: Array.from(
          { length: 1025 },
          (_, i) => new File(["x"], `p${i}.md`),
        ),
      },
    });
    expect((await screen.findByRole("alert")).textContent).toContain(
      "at most 1024 text files",
    );
    expect(fetch).toHaveBeenCalledTimes(1);
  });
  it("loads mounted packages, renders imported instructions as text and retains unassigned nodes", async () => {
    const doc = fixture();
    doc.presentation!.phases[0].id = "@unassigned";
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => ({
        ok: true,
        json: async () =>
          path === "/api/packages"
            ? {
                packages: [
                  { id: "fixture", name: "Mounted fixture", kind: "source" },
                ],
              }
            : doc,
      })),
    );
    render(<App />);
    await screen.findByRole("option", { name: "Mounted fixture (source)" });
    fireEvent.change(screen.getByLabelText("Mounted packages"), {
      target: { value: "fixture" },
    });
    await screen.findByRole("heading", { name: "Review example" });
    openNodes();
    fireEvent.click(screen.getByRole("button", { name: /start task/ }));
    expect(
      screen.getByText("Read {{ user_input }} <script>alert(1)</script>"),
    ).toBeTruthy();
    expect(document.querySelector("script")).toBeNull();
    expect(
      screen.getByText("Shared task used by: main/start, main/unreachable"),
    ).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: "Comment on shared task" }),
    );
    expect(
      screen
        .getByRole("tab", { name: /Feedback/ })
        .getAttribute("aria-selected"),
    ).toBe("true");
    expect(
      (screen.getByLabelText("Comment target") as HTMLInputElement).value,
    ).toBe("task:shared");
    fireEvent.click(screen.getByRole("tab", { name: /^Node$/ }));
    fireEvent.click(
      screen.getByRole("button", { name: /^prompts\/shared\.md$/ }),
    );
    expect(
      screen.getByRole("tab", { name: "Source" }).getAttribute("aria-selected"),
    ).toBe("true");
    expect(screen.getByLabelText("Graph fixture")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /PHASE 1/ }));
    expect(
      screen.getByRole("navigation", { name: "Flow nodes" }).children,
    ).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: /Unassigned nodes/ }));
    expect(
      screen.getByRole("button", { name: /unreachable task/ }),
    ).toBeTruthy();
  });
  it("keeps revision comments after loading changed sources", async () => {
    let doc = fixture();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => ({
        ok: true,
        json: async () =>
          path === "/api/packages"
            ? {
                packages: [
                  { id: "fixture", name: "Mounted fixture", kind: "source" },
                ],
              }
            : doc,
      })),
    );
    render(<App />);
    await screen.findByRole("option", { name: "Mounted fixture (source)" });
    fireEvent.change(screen.getByLabelText("Mounted packages"), {
      target: { value: "fixture" },
    });
    await screen.findByRole("heading", { name: "Review example" });
    openNodes();
    feedback();
    fireEvent.change(screen.getByLabelText("Comment"), {
      target: { value: "Clarify the budget" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add comment" }));
    doc = structuredClone(doc);
    doc.packageId = "next-revision";
    fireEvent.change(screen.getByLabelText("Mounted packages"), {
      target: { value: "fixture" },
    });
    await waitFor(() =>
      expect(document.querySelector("main")?.getAttribute("aria-busy")).toBe(
        "false",
      ),
    );
    feedback();
    await screen.findByText(/earlier revision; review manually/);
    expect(screen.getByText("Clarify the budget")).toBeTruthy();
  });
  it("preserves an unsaved draft's revision, node and excerpt across selection and reload", async () => {
    let doc = fixture();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => ({
        ok: true,
        json: async () =>
          path === "/api/packages"
            ? {
                packages: [
                  { id: "fixture", name: "Mounted fixture", kind: "source" },
                ],
              }
            : doc,
      })),
    );
    render(<App />);
    await screen.findByRole("option", { name: "Mounted fixture (source)" });
    fireEvent.change(screen.getByLabelText("Mounted packages"), {
      target: { value: "fixture" },
    });
    await screen.findByRole("heading", { name: "Review example" });
    openNodes();
    fireEvent.click(screen.getByRole("button", { name: /start task/ }));
    fireEvent.click(screen.getByRole("button", { name: "Comment on node" }));
    fireEvent.change(screen.getByLabelText("Comment"), {
      target: { value: "Explain this original route" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Unassigned nodes/ }));
    fireEvent.click(screen.getByRole("button", { name: /done end/ }));
    feedback();
    expect(
      (screen.getByLabelText("Comment target") as HTMLInputElement).value,
    ).toBe("node:main/start");
    doc = structuredClone(doc);
    doc.packageId = "next-revision";
    doc.plans[0].nodes.start.routes = { finish: "done" };
    fireEvent.change(screen.getByLabelText("Mounted packages"), {
      target: { value: "fixture" },
    });
    await waitFor(() =>
      expect(document.querySelector("main")?.getAttribute("aria-busy")).toBe(
        "false",
      ),
    );
    fireEvent.click(screen.getByRole("tab", { name: "Package" }));
    await screen.findByText("Revision next-revision");
    feedback();
    expect(screen.getByRole("status").textContent).toContain(
      "Draft revision: revision-1",
    );
    const anchor = screen.getByText("Anchor value / excerpt").parentElement!;
    expect(anchor.textContent).toContain('"again"');
    fireEvent.click(screen.getByRole("button", { name: "Add comment" }));
    const article = screen
      .getByText("Explain this original route")
      .closest("article")!;
    expect(article.textContent).toContain("node:main/start");
    expect(article.textContent).toContain("Original revision: revision-1");
    expect(article.textContent).toContain("earlier revision; review manually");
    feedback();
    fireEvent.change(screen.getByLabelText("Comment"), {
      target: { value: "New revision comment" },
    });
    expect(screen.getByRole("status").textContent).toContain(
      "Draft revision: next-revision",
    );
    fireEvent.click(screen.getByRole("button", { name: "Discard draft" }));
    expect(
      (screen.getByLabelText("Comment") as HTMLTextAreaElement).value,
    ).toBe("");
    expect(article.textContent).toContain("Explain this original route");
  });
});
