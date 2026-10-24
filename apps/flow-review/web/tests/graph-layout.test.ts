import { describe, expect, it } from "vitest";
import { edgesOf, type Plan } from "../src/model";
import { layoutGraph, sliceGraph } from "../src/graph-layout";
import { fixture } from "./fixture";

function plan(nodes: Plan["nodes"]): Plan {
  return {
    id: "test",
    path: "test.flow.yaml",
    entryStep: Object.keys(nodes)[0],
    limits: {},
    nodes,
  };
}

describe("readable directed graph layout", () => {
  it("places success forks left to right on distinct lanes and rejoins them", () => {
    const p = plan({
      start: { routes: { left: "left", right: "right" } },
      left: { onVerified: "finish" },
      right: { onVerified: "finish" },
      finish: {},
    });
    const graph = layoutGraph(p, null, Object.keys(p.nodes));
    const positions = new Map(
      graph.nodes.map((node) => [node.data.sourceId, node.position]),
    );
    expect(positions.get("left")!.x).toBeGreaterThan(positions.get("start")!.x);
    expect(positions.get("right")!.x).toBeGreaterThan(
      positions.get("start")!.x,
    );
    expect(positions.get("left")!.y).not.toBe(positions.get("right")!.y);
    expect(positions.get("finish")!.x).toBeGreaterThan(
      positions.get("right")!.x,
    );
  });

  it("preserves cycle and retry self-loop edges in separate return lanes", () => {
    const p = plan({
      a: { onVerified: "b", onError: { retry: 2, then: "c" } },
      b: { onVerified: "c" },
      c: { routes: { again: "a" } },
    });
    const graph = layoutGraph(p, null, Object.keys(p.nodes));
    expect(graph.edges.map((edge) => edge.data.original)).toEqual(edgesOf(p));
    expect(
      graph.edges.find((edge) => edge.data.original.label === "retry ≤ 2")
        ?.type,
    ).toBe("returnLane");
    expect(
      graph.edges.find(
        (edge) => edge.data.original.label === "onError exhausted",
      )?.type,
    ).toBe("returnLane");
    const cycle = graph.edges.find(
      (edge) => edge.data.original.source === "c",
    )!;
    expect(cycle.type).toBe("returnLane");
    expect(cycle.label).toContain("loop/back");
    expect(graph.bounds.y).toBeLessThan(Number(cycle.data.roof) - 177);
    expect(
      graph.nodes.every(
        (node) =>
          Number.isFinite(node.position.x) && Number.isFinite(node.position.y),
      ),
    ).toBe(true);
  });

  it("discloses incoming, outgoing and cross-phase recovery boundaries", () => {
    const p = plan({
      before: { onVerified: "current" },
      current: { onVerified: "next", onError: { then: "recover" } },
      next: {},
      recover: { onVerified: "elsewhere" },
      elsewhere: {},
    });
    const slice = sliceGraph(p, ["current"]);
    expect([...slice.boundary]).toEqual(["before", "next", "recover"]);
    expect(slice.edges.map((edge) => edge.label)).toContain(
      "onError exhausted",
    );
    expect(slice.elsewhereConnections).toBe(1);
    expect(slice.omittedConnections).toBe(0);
  });

  it("preserves explicit resume outside the phase and body/between semantics without invented edges", () => {
    const p = fixture().plans[0];
    p.nodes.start.body = { flowRef: "child" };
    p.nodes.start.between = { flowRef: "other" };
    const graph = layoutGraph(p, null, ["start"]);
    expect(graph.edges.map((edge) => edge.data.original)).toEqual(edgesOf(p));
    expect(
      graph.nodes.find((node) => node.data.sourceId === "done")?.data.boundary,
    ).toBe(true);
    expect(
      graph.edges.filter((edge) => edge.data.kind === "call"),
    ).toHaveLength(2);
    expect(
      graph.edges.filter((edge) => edge.data.kind === "return"),
    ).toHaveLength(1);
    const incoming = sliceGraph(p, ["done"]);
    expect(
      incoming.edges.some((edge) => edge.label === "explicit resume"),
    ).toBe(true);
    expect(
      incoming.edges.some((edge) => edge.label === "route: wait → paused"),
    ).toBe(true);
  });

  it("caps boundary nodes and explicitly counts omitted adjacent connections", () => {
    const routes = Object.fromEntries(
      Array.from({ length: 60 }, (_, i) => [`r${i}`, `n${i}`]),
    );
    const p = plan({
      start: { routes },
      ...Object.fromEntries(Object.values(routes).map((id) => [id, {}])),
    });
    const graph = layoutGraph(p, null, ["start"]);
    expect(graph.nodes).toHaveLength(41);
    expect(graph.edges).toHaveLength(40);
    expect(graph.omittedConnections).toBe(20);
    expect(graph.elsewhereConnections).toBe(0);
  });

  it("uses opaque IDs while preserving prototype-like and synthetic-looking source keys", () => {
    const p = plan(
      JSON.parse(
        '{"__proto__":{"onVerified":"constructor","onEmpty":{"status":"paused","resumeAt":"@review:0"}},"constructor":{"onVerified":"@review:0"},"@review:0":{}}',
      ),
    );
    const graph = layoutGraph(p, null, Object.keys(p.nodes));
    expect(graph.nodes.map((node) => node.data.sourceId)).toEqual(
      expect.arrayContaining(["__proto__", "constructor", "@review:0"]),
    );
    expect(graph.nodes.every((node) => /^node-\d+$/.test(node.id))).toBe(true);
    expect(new Set(graph.nodes.map((node) => node.id)).size).toBe(4);
    expect(graph.edges.map((edge) => edge.data.original)).toEqual(edgesOf(p));
    expect(
      graph.edges.every(
        (edge) =>
          graph.nodes.some((node) => node.id === edge.source) &&
          graph.nodes.some((node) => node.id === edge.target),
      ),
    ).toBe(true);
  });
});
