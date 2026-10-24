import dagre from "@dagrejs/dagre";
import {
  edgesOf,
  graphLabel,
  type Edge,
  type Plan,
  type Registry,
} from "./model";

export type ConnectionKind =
  | "transition"
  | "route"
  | "recovery"
  | "return"
  | "call";
export function connectionKind(edge: Edge): ConnectionKind {
  if (/Error|retry|onLimit/.test(edge.label)) return "recovery";
  if (/resume/.test(edge.label)) return "return";
  if (/^(body|between):/.test(edge.label)) return "call";
  if (edge.label.startsWith("route:")) return "route";
  return "transition";
}

/** Keep adjacent phase boundaries and complete synthetic directive/call chains. */
export function sliceGraph(plan: Plan, visible: string[], boundaryLimit = 40) {
  const allEdges = edgesOf(plan);
  const primary = new Set(
    visible.filter((id) => Object.hasOwn(plan.nodes, id)),
  );
  const synthetic = new Set<string>();
  const feedsPrimary = new Set(
    allEdges
      .filter((edge) => primary.has(edge.target))
      .map((edge) => edge.source),
  );
  for (const edge of allEdges) {
    if (
      edge.external &&
      (primary.has(edge.source) || feedsPrimary.has(edge.target))
    )
      synthetic.add(edge.target);
  }
  const central = new Set([...primary, ...synthetic]);
  const adjacent = allEdges.filter(
    (edge) => central.has(edge.source) || central.has(edge.target),
  );
  const outside = new Set(
    adjacent
      .flatMap((edge) => [edge.source, edge.target])
      .filter((id) => !central.has(id)),
  );
  const boundary = new Set([...outside].slice(0, boundaryLimit));
  const included = new Set([...central, ...boundary]);
  const edges = adjacent.filter(
    (edge) => included.has(edge.source) && included.has(edge.target),
  );
  return {
    ids: [...included],
    primary,
    boundary,
    edges,
    omittedConnections: adjacent.length - edges.length,
    elsewhereConnections: allEdges.length - adjacent.length,
  };
}

/** Opaque renderer keys protect both graphlib and React Flow from authored IDs. */
export function layoutGraph(
  plan: Plan,
  registry: Registry | null,
  visible: string[],
) {
  const slice = sliceGraph(plan, visible);
  const renderIds = new Map(slice.ids.map((id, i) => [id, `node-${i}`]));
  const layout = new dagre.graphlib.Graph({ multigraph: true })
    .setGraph({
      rankdir: "LR",
      ranksep: 150,
      nodesep: 65,
      marginx: 35,
      marginy: 100,
    })
    .setDefaultEdgeLabel(() => ({}));
  const sizeOf = (id: string) => ({
    width: slice.boundary.has(id) ? 170 : 230,
    height: slice.boundary.has(id) ? 72 : 96,
  });
  for (const id of slice.ids) layout.setNode(renderIds.get(id)!, sizeOf(id));
  // Recovery must not drag the normal path backwards. Add only acyclic normal
  // transitions to the rank graph; every original edge is still rendered below.
  const successors = new Map<string, string[]>();
  const reaches = (from: string, target: string) => {
    const pending = [from],
      visited = new Set<string>();
    while (pending.length) {
      const id = pending.pop()!;
      if (id === target) return true;
      if (visited.has(id)) continue;
      visited.add(id);
      pending.push(...(successors.get(id) ?? []));
    }
    return false;
  };
  const ordered = [...slice.edges].sort(
    (a, b) =>
      Number(a.label.startsWith("route:")) -
      Number(b.label.startsWith("route:")),
  );
  ordered.forEach((edge, i) => {
    const kind = connectionKind(edge);
    if (
      kind === "recovery" ||
      kind === "return" ||
      reaches(edge.target, edge.source)
    )
      return;
    layout.setEdge(
      renderIds.get(edge.source)!,
      renderIds.get(edge.target)!,
      {},
      `rank-${i}`,
    );
    successors.set(edge.source, [
      ...(successors.get(edge.source) ?? []),
      edge.target,
    ]);
  });
  dagre.layout(layout);
  const nodes = slice.ids.map((id) => {
    const dimensions = sizeOf(id),
      point = layout.node(renderIds.get(id)!);
    const boundary = slice.boundary.has(id),
      authored = Object.hasOwn(plan.nodes, id);
    const directive = slice.edges.find(
      (edge) => edge.target === id && edge.external,
    );
    return {
      id: renderIds.get(id)!,
      type: "flowCard",
      position: {
        x: point.x - dimensions.width / 2,
        y: point.y - dimensions.height / 2,
      },
      data: {
        sourceId: id,
        boundary,
        authored,
        entry: id === plan.entryStep,
        label: authored
          ? graphLabel(id, plan.nodes[id], registry)
          : (directive?.label ?? id),
        width: dimensions.width,
        height: dimensions.height,
      },
      width: dimensions.width,
      height: dimensions.height,
    };
  });
  const positions = new Map(nodes.map((node) => [node.id, node.position]));
  const roof = Math.min(0, ...nodes.map((node) => node.position.y));
  const edges = slice.edges.map((edge, i) => {
    const source = renderIds.get(edge.source)!,
      target = renderIds.get(edge.target)!;
    const kind = connectionKind(edge);
    const loop =
      source === target || positions.get(target)!.x <= positions.get(source)!.x;
    const elevated = loop || kind === "recovery" || kind === "return";
    return {
      id: `edge-${i}`,
      source,
      target,
      type: elevated ? "returnLane" : "smoothstep",
      sourceHandle: elevated ? "return-out" : "out",
      targetHandle: elevated ? "return-in" : "in",
      label: `${edge.label}${loop && !/retry|resume/.test(edge.label) ? " · loop/back" : ""}`,
      data: { original: edge, kind, lane: i % 8, loop, roof },
    };
  });
  const left = Math.min(0, ...nodes.map((node) => node.position.x));
  const top = edges.some((edge) => edge.type === "returnLane")
    ? roof - 200
    : roof;
  const right = Math.max(
    230,
    ...nodes.map((node) => node.position.x + node.width),
  );
  const bottom = Math.max(
    96,
    ...nodes.map((node) => node.position.y + node.height),
  );
  return {
    nodes,
    edges,
    bounds: { x: left, y: top, width: right - left, height: bottom - top },
    omittedConnections: slice.omittedConnections,
    elsewhereConnections: slice.elsewhereConnections,
  };
}
