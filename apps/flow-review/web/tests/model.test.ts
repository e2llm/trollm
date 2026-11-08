import { describe, it, expect } from "vitest";
import {
  type Plan,
  compare,
  edgesOf,
  feedback,
  nodesOf,
  promptUses,
  promptPath,
  graphSlice,
  graphLabel,
  defaultFailureNodes,
  bindingReferences,
  isPresentation,
} from "../src/model";
import { fixture } from "./fixture";

describe("inspection view logic", () => {
  it("shows declared child-outcome exits as graph edges", () => {
    const plan = fixture().plans[0];
    plan.nodes.each = {
      kind: "foreach",
      onItemOutcome: { accepted: "done", pending: { status: "paused", resumeAt: "start" } },
    };
    const edges = edgesOf(plan);
    expect(edges.some((edge) => edge.source === "each" && edge.target === "done" && edge.label === "child outcome: accepted")).toBe(true);
    expect(edges.some((edge) => edge.source === "each" && edge.label.startsWith("child outcome: pending"))).toBe(true);
  });
  it("reports effective default failure for task nodes with absent or null policies only", () => {
    const doc = fixture(),
      plan = doc.plans[0];
    plan.nodes.nullPolicy = { kind: "task", onError: null };
    plan.nodes.endNull = { kind: "end", onError: null };
    plan.nodes.each = { kind: "foreach" };
    expect(defaultFailureNodes(plan, Object.keys(plan.nodes))).toEqual([
      "unreachable",
      "nullPolicy",
    ]);
    expect(
      defaultFailureNodes(plan, ["start", "done", "endNull", "each"]),
    ).toEqual([]);
  });
  it("allocates distinct synthetic vertices even when authored keys match prior or current prefixes", () => {
    const plan: Plan = {
      id: "collision",
      path: "collision.flow.yaml",
      entryStep: "step",
      limits: {},
      nodes: {
        step: {
          kind: "foreach",
          onLimit: { status: "failed" },
          onEmpty: { status: "paused", resumeAt: "@summarize" },
          body: { flowRef: "child" },
          between: { flowRef: "child" },
        },
        "@step:onLimit": { kind: "end" },
        "@summarize": { kind: "task" },
        "@review:0": { kind: "end" },
      },
    };
    const virtual = edgesOf(plan)
      .filter((edge) => edge.external)
      .map((edge) => edge.target);
    expect(new Set(virtual).size).toBe(4);
    expect(virtual.every((id) => !Object.hasOwn(plan.nodes, id))).toBe(true);
    expect(graphSlice(plan, Object.keys(plan.nodes))).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          target: "@summarize",
          label: "explicit resume",
        }),
      ]),
    );
  });
  it("keeps multiline before/after prompt values as original text", () => {
    const before = fixture(),
      after = structuredClone(before);
    before.files[0].content = 'First line\nSecond "quoted" line\n';
    after.files[0].content = 'First line\nRevised "quoted" line\n';
    after.files[0].sha256 = "changed";
    const change = compare(before, after).find(
      (c) => c.category === "Source text",
    );
    expect(change?.before).toBe(before.files[0].content);
    expect(change?.after).toBe(after.files[0].content);
  });
  it("labels graph roles from task definitions rather than task names", () => {
    const doc = fixture();
    doc.registry!.tasks.shared.model = "reasoner";
    expect(
      graphLabel("start", doc.plans[0].nodes.start, doc.registry),
    ).toContain("model: reasoner");
    expect(graphLabel("done", doc.plans[0].nodes.done, doc.registry)).toBe(
      "done\nend\nok",
    );
  });
  it("labels legacy resume and rejects malformed authored presentation", () => {
    const doc = fixture();
    doc.plans[0].nodes.start.onVerified = { status: "paused" };
    expect(edgesOf(doc.plans[0])).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          target: "start",
          label: "legacy resume at current node",
        }),
      ]),
    );
    expect(isPresentation({ format: "trollm.flow-presentation.v1" })).toBe(
      false,
    );
    expect(isPresentation(doc.presentation)).toBe(true);
  });
  it("identifies static binding names without evaluating values", () => {
    expect(
      bindingReferences(
        "Use {{ start.outputs.total }} and {{ user_input }}",
        { items: { from: "items.outputs.list" } },
        {
          toolArguments: {
            "local.calculate": { fromVars: { a: "start.outputs.total" } },
          },
        },
      ),
    ).toEqual(["start.outputs.total", "user_input", "items.outputs.list"]);
  });
  it("preserves error, retry, route, paused directive and explicit resume edges", () => {
    const edges = edgesOf(fixture().plans[0]);
    expect(edges).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          source: "start",
          target: "done",
          label: "onError exhausted",
        }),
        expect.objectContaining({
          source: "start",
          target: "start",
          label: "retry ≤ 2",
        }),
        expect.objectContaining({ target: "done", label: "explicit resume" }),
        expect.objectContaining({ label: "route: wait → paused" }),
      ]),
    );
  });
  it("exposes foreach body and between without inventing execution nodes", () => {
    const plan: Plan = {
      id: "loop",
      path: "loop.flow.yaml",
      entryStep: "each",
      limits: {},
      nodes: {
        each: {
          kind: "foreach",
          body: { flowRef: "body" },
          between: { flowRef: "between" },
          onComplete: "end",
        },
        end: { kind: "end" },
      },
    };
    expect(edgesOf(plan).map((e) => e.label)).toEqual([
      "onComplete",
      "body: body (returns)",
      "between: between (returns)",
    ]);
  });
  it("keeps shared prompts and every unassigned/unreachable node discoverable", () => {
    const doc = fixture();
    expect(promptUses(doc, "prompts/shared.md")).toEqual([
      "main/start",
      "main/unreachable",
    ]);
    expect(nodesOf(doc)).toHaveLength(3);
  });
  it("reports real field changes and renamed nodes as removal/addition", () => {
    const a = fixture(),
      b = structuredClone(a);
    b.packageId = "revision-2";
    b.plans[0].nodes.start.routes = { finish: "done" };
    delete b.plans[0].nodes.unreachable;
    b.plans[0].nodes.renamed = { kind: "task", taskRef: "shared" };
    b.registry!.tasks.shared.tools = ["local.calculate"];
    b.files[0].content = "Changed text";
    b.files[0].sha256 = "b";
    const changes = compare(a, b);
    expect(changes).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          target: "main/start.routes",
          category: "Edges / recovery",
        }),
        expect.objectContaining({
          target: "main/unreachable",
          category: "Removed node",
        }),
        expect.objectContaining({
          target: "main/renamed",
          category: "Added node",
        }),
        expect.objectContaining({
          target: "task:shared.tools",
          category: "Tool grants",
        }),
        expect.objectContaining({
          target: "prompts/shared.md",
          category: "Source text",
        }),
      ]),
    );
    expect(compare(a, a)).toEqual([]);
  });
  it("normalizes registry-relative prompt paths without escaping the package", () => {
    const doc = fixture();
    doc.registry!.path = "tasks/main.tasks.yaml";
    doc.registry!.tasks.shared.prompt = "../prompts/./shared.md";
    expect(promptPath(doc.registry, doc.registry!.tasks.shared)).toBe(
      "prompts/shared.md",
    );
    expect(promptUses(doc, "prompts/shared.md")).toEqual([
      "main/start",
      "main/unreachable",
    ]);
    doc.registry!.tasks.shared.prompt = "../../escape.md";
    expect(promptPath(doc.registry, doc.registry!.tasks.shared)).toBe("");
  });
  it("keeps explicit resume edges through synthetic pause cards in filtered graphs", () => {
    const plan = fixture().plans[0];
    expect(graphSlice(plan, ["start", "done"])).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ label: "explicit resume", target: "done" }),
      ]),
    );
    expect(
      graphSlice(plan, ["start"]).some((e) => e.label === "explicit resume"),
    ).toBe(false);
  });
  it("exports original revision anchors and robust fences after reload", () => {
    const doc = fixture();
    doc.packageId = "new";
    const markdown = feedback(doc, [
      {
        id: "1",
        revision: "old",
        target: "node:main/start",
        excerpt: "```\n# injected heading",
        text: "Revise this ``` prompt",
      },
    ]);
    expect(markdown).toContain("Original revision: old");
    expect(markdown).toContain("````text");
    expect(markdown).toContain("run existing validation".replace("run", "Run"));
  });
  it("handles a 300KB many-node cyclic package with bounded graph extraction", () => {
    const doc = fixture();
    doc.files[0].content = "x".repeat(310000);
    doc.plans[0].nodes = Object.fromEntries(
      Array.from({ length: 1500 }, (_, i) => [
        `n${i}`,
        { kind: "task", taskRef: "shared", onVerified: `n${(i + 1) % 1500}` },
      ]),
    );
    expect(nodesOf(doc)).toHaveLength(1500);
    expect(edgesOf(doc.plans[0])).toHaveLength(1500);
    expect(compare(doc, doc)).toHaveLength(0);
  });
  it("exports large excerpts with many backtick runs without losing comments", () => {
    const excerpt = "`x".repeat(150000) + "\n````\n";
    const markdown = feedback(fixture(), [
      {
        id: "1",
        revision: "original",
        target: "prompt",
        excerpt,
        text: "Keep this comment",
      },
      {
        id: "2",
        revision: "earlier",
        target: "limits",
        excerpt: "20",
        text: "And this one",
      },
    ]);
    expect(markdown).toContain("`````text\n" + excerpt + "\n`````");
    expect(markdown).toContain("Keep this comment");
    expect(markdown).toContain("Original revision: earlier");
    expect(markdown).toContain("And this one");
  });
});
