import { useMemo, useEffect, useCallback } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  Panel,
  MarkerType,
  Handle,
  Position,
  BaseEdge,
  useReactFlow,
  useStore,
  getViewportForBounds,
  type Rect,
  type EdgeProps,
  type NodeProps,
} from "@xyflow/react";
import type { Plan, Registry } from "./model";
import { layoutGraph } from "./graph-layout";
import { readableViewport } from "./graph-viewport";
import "@xyflow/react/dist/style.css";
import "./graph.css";

function FlowCard({ data }: NodeProps) {
  return (
    <div
      className={`flow-card${data.boundary ? " flow-card-boundary" : ""}${data.entry ? " flow-card-entry" : ""}`}
      style={{ width: Number(data.width), height: Number(data.height) }}
      role={data.authored ? "button" : undefined}
      tabIndex={data.authored ? 0 : undefined}
      onKeyDown={(event) => {
        if (
          (event.key === "Enter" || event.key === " ") &&
          typeof data.activate === "function"
        ) {
          event.preventDefault();
          data.activate();
        }
      }}
    >
      <Handle id="in" type="target" position={Position.Left} />
      <Handle id="out" type="source" position={Position.Right} />
      <Handle
        id="return-in"
        type="target"
        position={Position.Top}
        style={{ left: "32%" }}
      />
      <Handle
        id="return-out"
        type="source"
        position={Position.Top}
        style={{ left: "68%" }}
      />
      {Boolean(data.boundary) && (
        <span className="flow-card-eyebrow">
          Outside phase · {data.authored ? "open node ↗" : "unresolved target"}
        </span>
      )}
      {!data.authored && !data.boundary && (
        <span className="flow-card-eyebrow">Directive / subflow reference</span>
      )}
      <span className="flow-card-label">{String(data.label)}</span>
    </div>
  );
}

function ReturnLane(props: EdgeProps) {
  const { sourceX: sx, sourceY: sy, targetX: tx, targetY: ty, data } = props;
  const top =
    Number(data?.roof ?? Math.min(sy, ty)) - 65 - Number(data?.lane ?? 0) * 16;
  // Separate handles also give self-loops an explicit arrow and readable label.
  const path = `M ${sx} ${sy} C ${sx} ${top}, ${sx} ${top}, ${sx - 20} ${top} L ${tx + 20} ${top} C ${tx} ${top}, ${tx} ${top}, ${tx} ${ty}`;
  return (
    <BaseEdge
      id={props.id}
      path={path}
      markerEnd={props.markerEnd}
      style={props.style}
      label={props.label}
      labelStyle={props.labelStyle}
      labelBgStyle={props.labelBgStyle}
      labelBgPadding={props.labelBgPadding}
      labelBgBorderRadius={props.labelBgBorderRadius}
      labelX={(sx + tx) / 2}
      labelY={top}
    />
  );
}
const nodeTypes = { flowCard: FlowCard };
const edgeTypes = { returnLane: ReturnLane };
const colors = {
  transition: "#47776f",
  route: "#386fac",
  recovery: "#b85a37",
  return: "#805eab",
  call: "#697488",
};

function ViewportControls({ bounds, focus }: { bounds: Rect; focus: Rect }) {
  const { setViewport } = useReactFlow();
  const width = useStore((state) => state.width);
  const height = useStore((state) => state.height);
  const fit = useCallback(() => {
    if (width && height)
      void setViewport(
        getViewportForBounds(bounds, width, height, 0.001, 1, 0.1),
      );
  }, [bounds, width, height, setViewport]);
  const read = useCallback(() => {
    if (width && height)
      void setViewport(readableViewport(bounds, focus, width, height, true));
  }, [bounds, focus, width, height, setViewport]);
  useEffect(() => {
    if (width && height)
      void setViewport(readableViewport(bounds, focus, width, height));
  }, [bounds, focus, width, height, setViewport]);
  return (
    <>
      <Panel position="top-left" className="graph-view-actions">
        <button onClick={fit}>Fit graph</button>
        <button onClick={read}>Read selected node</button>
        <span>Drag to pan · Scroll to zoom</span>
      </Panel>
      <Controls showInteractive={false} showFitView={false} />
      <MiniMap
        pannable
        zoomable
        nodeColor={(node) =>
          node.selected ? "#2377bc" : node.data.boundary ? "#ccd8e0" : "#88a69f"
        }
        maskColor="#15334218"
        ariaLabel="Graph overview"
      />
    </>
  );
}

export default function Graph({
  plan,
  registry,
  visible,
  select,
  selected,
}: {
  plan: Plan;
  registry: Registry | null;
  visible: string[];
  select: (id: string) => void;
  selected?: string;
}) {
  const graph = useMemo(
    () => layoutGraph(plan, registry, visible),
    [plan, registry, visible],
  );
  const focus = useMemo(() => {
    const node =
      graph.nodes.find((node) => node.data.sourceId === selected) ??
      graph.nodes.find((node) => node.data.entry) ??
      graph.nodes.find((node) => node.data.authored && !node.data.boundary) ??
      graph.nodes[0];
    return node
      ? { ...node.position, width: node.width, height: node.height }
      : graph.bounds;
  }, [graph, selected]);
  const nodes = useMemo(
    () =>
      graph.nodes.map((node) => ({
        ...node,
        selected: node.data.sourceId === selected,
        data: { ...node.data, activate: () => select(node.data.sourceId) },
        ariaLabel: `${node.data.boundary ? "Outside phase: " : ""}${node.data.label}`,
        focusable: false,
      })),
    [graph, selected, select],
  );
  const edges = useMemo(
    () =>
      graph.edges.map((edge) => ({
        ...edge,
        markerEnd: {
          type: MarkerType.ArrowClosed,
          color: colors[edge.data.kind],
          width: 18,
          height: 18,
        },
        style: {
          stroke: colors[edge.data.kind],
          strokeWidth: 1.7,
          strokeDasharray:
            edge.data.kind === "recovery"
              ? "7 4"
              : edge.data.loop ||
                  edge.data.kind === "return" ||
                  edge.data.kind === "call"
                ? "3 4"
                : undefined,
        },
        labelStyle: { fill: "#243b41", fontSize: 11, fontWeight: 550 },
        labelBgStyle: { fill: "#ffffff", fillOpacity: 0.96 },
        labelBgPadding: [5, 3] as [number, number],
        labelBgBorderRadius: 3,
      })),
    [graph],
  );
  return (
    <div className="flow-graph">
      <div className="graph-legend" aria-label="Graph legend">
        <span>
          <b>→</b> Left to right
        </span>
        <span className="legend-transition">━ Transition</span>
        <span className="legend-route">━ Route</span>
        <span className="legend-recovery">┄ Error / limit / retry</span>
        <span className="legend-return">┈ Resume / loop / call</span>
      </div>
      <p className="graph-context">
        Body/between references return to their caller. Select an outside-phase
        node to inspect it.
        {graph.omittedConnections > 0 && (
          <strong>
            {" "}
            {graph.omittedConnections} adjacent connections omitted at the 40
            boundary-node limit; use All nodes.
          </strong>
        )}
        {graph.elsewhereConnections > 0 && (
          <span>
            {" "}
            {graph.elsewhereConnections} connections elsewhere in this flow; use
            All nodes.
          </span>
        )}
      </p>
      <div className="graph-canvas">
        <ReactFlow
          key={JSON.stringify([plan.id, visible])}
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          nodesDraggable={false}
          nodesConnectable={false}
          edgesReconnectable={false}
          deleteKeyCode={null}
          onNodeClick={(_, node) => {
            if (node.data.authored) select(node.data.sourceId);
          }}
          onlyRenderVisibleElements
          minZoom={0.001}
          maxZoom={1.8}
        >
          <Background gap={24} color="#dfe6eb" />
          <ViewportControls bounds={graph.bounds} focus={focus} />
        </ReactFlow>
      </div>
    </div>
  );
}
