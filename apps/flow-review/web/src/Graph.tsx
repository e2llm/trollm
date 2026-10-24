import { useMemo } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  MarkerType,
} from "@xyflow/react";
import dagre from "@dagrejs/dagre";
import type { Plan, Registry } from "./model";
import { edgesOf, graphSlice, graphLabel } from "./model";
import "@xyflow/react/dist/style.css";
export default function Graph({
  plan,
  registry,
  visible,
  select,
}: {
  plan: Plan;
  registry: Registry | null;
  visible: string[];
  select: (id: string) => void;
}) {
  const graph = useMemo(() => {
    const edges = edgesOf(plan),
      shown = graphSlice(plan, visible);
    const layout = new dagre.graphlib.Graph({ multigraph: true })
      .setGraph({ rankdir: "TB", ranksep: 45, nodesep: 40 })
      .setDefaultEdgeLabel(() => ({}));
    const all = new Set([
      ...visible,
      ...shown.filter((e) => e.external).map((e) => e.target),
    ]);
    // Graphlib and renderer internals use object keys. Keep authored identities
    // entirely in values, including valid names such as __proto__ / constructor.
    const renderIds = new Map(
      [...all].map((id, index) => [id, `view-${index}`]),
    );
    for (const id of all)
      layout.setNode(renderIds.get(id)!, { width: 220, height: 85 });
    shown.forEach((e, index) =>
      layout.setEdge(
        renderIds.get(e.source)!,
        renderIds.get(e.target)!,
        {},
        `edge-${index}`,
      ),
    );
    dagre.layout(layout);
    return {
      nodes: [...all].map((id) => ({
        id: renderIds.get(id)!,
        position: {
          x: layout.node(renderIds.get(id)!).x - 110,
          y: layout.node(renderIds.get(id)!).y - 42,
        },
        data: {
          sourceId: id,
          label: Object.hasOwn(plan.nodes, id)
            ? graphLabel(id, plan.nodes[id], registry)
            : shown.find((e) => e.target === id)?.label,
        },
        style: {
          width: 220,
          whiteSpace: "pre-wrap" as const,
          borderColor: id === plan.entryStep ? "#16615c" : "#a1b3b1",
          background: Object.hasOwn(plan.nodes, id) ? "white" : "#fff3d6",
        },
      })),
      edges: shown.map((e, index) => ({
        id: `edge-${index}`,
        source: renderIds.get(e.source)!,
        target: renderIds.get(e.target)!,
        label: e.label,
        markerEnd: { type: MarkerType.ArrowClosed },
        style: { stroke: e.label.includes("Error") ? "#a23737" : "#527772" },
        labelStyle: { fontSize: 12 },
        animated: false,
      })),
      hidden: edges.length - shown.length,
    };
  }, [plan, registry, visible]);
  return (
    <>
      <p className="hint">
        Verified, route, error, iteration and explicit resume labels describe
        authored transitions. Body/between calls return to their caller.{" "}
        {graph.hidden > 0 &&
          `${graph.hidden} edges lead outside this view; use All nodes to inspect them.`}
      </p>
      <div className="graph">
        <ReactFlow
          key={JSON.stringify([plan.id, visible])}
          nodes={graph.nodes}
          edges={graph.edges}
          fitView
          fitViewOptions={{ padding: 0.08 }}
          nodesDraggable={false}
          nodesConnectable={false}
          edgesReconnectable={false}
          deleteKeyCode={null}
          onNodeClick={(_, node) =>
            Object.hasOwn(plan.nodes, node.data.sourceId) &&
            select(node.data.sourceId)
          }
          onlyRenderVisibleElements
          minZoom={0.05}
        >
          <Background />
          <Controls showInteractive={false} />
          <MiniMap pannable zoomable />
        </ReactFlow>
      </div>
    </>
  );
}
