export type Json =
  | null
  | boolean
  | number
  | string
  | Json[]
  | { [key: string]: Json | undefined };
export type RecordValue = { [key: string]: Json | undefined };
export interface SourceFile {
  path: string;
  content: string;
  sha256: string;
  sizeBytes: number;
}
export interface Ref {
  flow: string;
  node: string;
}
export interface Phase {
  id: string;
  title: string;
  description: string;
  nodes: Ref[];
}
export interface Presentation {
  format: string;
  title: string;
  purpose: string;
  inputsDescription?: string;
  resultDescription?: string;
  phases: Phase[];
}
export interface Plan extends RecordValue {
  path: string;
  id: string;
  entryStep: string;
  exports?: Record<string, { from: string }>;
  nodes: Record<string, RecordValue>;
  limits: RecordValue;
}
export interface Registry {
  path: string;
  id: string;
  tasks: Record<string, RecordValue>;
}
export interface Inspection {
  format: string;
  packageId: string;
  name: string;
  entryFlow: string | null;
  taskRegistry: string | null;
  candidates: { entryFlows: string[]; taskRegistries: string[] };
  valid: boolean;
  diagnostics: {
    level: string;
    code: string;
    message: string;
    path?: string;
    node?: string;
  }[];
  files: SourceFile[];
  plans: Plan[];
  registry: Registry | null;
  setup: Json;
  presentation: Presentation | null;
  unassignedNodes: Ref[];
}
export const pretty = (value: unknown) =>
  JSON.stringify(value, null, 2) ?? "Not supplied";
export const qualified = (ref: Ref) => `${ref.flow}/${ref.node}`;
export function nodesOf(doc: Inspection) {
  return doc.plans.flatMap((plan) =>
    Object.entries(plan.nodes).map(([node, value]) => ({
      flow: plan.id,
      node,
      value,
      plan,
    })),
  );
}
export function promptPath(
  registry: Registry | null,
  task: RecordValue | undefined,
) {
  if (!registry || typeof task?.prompt !== "string") return "";
  const parts = registry.path.split("/").slice(0, -1);
  for (const part of task.prompt.split("/")) {
    if (part === "." || part === "") continue;
    if (part === "..") {
      if (!parts.length) return "";
      parts.pop();
    } else parts.push(part);
  }
  return parts.join("/");
}
export function taskUses(doc: Inspection, key: string) {
  return nodesOf(doc)
    .filter((n) => n.value.taskRef === key)
    .map(qualified);
}
export function promptUses(doc: Inspection, path: string) {
  return Object.entries(doc.registry?.tasks ?? {})
    .filter(([, t]) => promptPath(doc.registry, t) === path)
    .flatMap(([key]) => taskUses(doc, key));
}
export interface Edge {
  id: string;
  source: string;
  target: string;
  label: string;
  external?: boolean;
}
export function edgesOf(plan: Plan): Edge[] {
  const edges: Edge[] = [];
  const occupied = new Set(Object.keys(plan.nodes));
  let syntheticIndex = 0;
  const syntheticId = () => {
    let id: string;
    do {
      id = `@review:${syntheticIndex++}`;
    } while (occupied.has(id));
    occupied.add(id);
    return id;
  };
  const add = (source: string, label: string, value: Json | undefined) => {
    if (value == null) return;
    if (typeof value === "string") {
      edges.push({ id: `${source}:${label}`, source, target: value, label });
      return;
    }
    if (typeof value === "object" && !Array.isArray(value)) {
      const target = syntheticId();
      edges.push({
        id: target,
        source,
        target,
        label: `${label} → ${String(value.status ?? "directive")}`,
        external: true,
      });
      if (typeof value.resumeAt === "string")
        edges.push({
          id: `${target}:resume`,
          source: target,
          target: value.resumeAt,
          label: "explicit resume",
        });
      else if (value.status === "paused")
        edges.push({
          id: `${target}:resume`,
          source: target,
          target: source,
          label: "legacy resume at current node",
        });
    }
  };
  for (const [id, n] of Object.entries(plan.nodes)) {
    for (const key of ["onVerified", "onEmpty", "onComplete", "onLimit"])
      add(id, key, n[key]);
    if (n.onItemOutcome && typeof n.onItemOutcome === "object" && !Array.isArray(n.onItemOutcome))
      for (const [outcome, target] of Object.entries(n.onItemOutcome))
        add(id, `child outcome: ${outcome}`, target);
    if (n.routes && typeof n.routes === "object" && !Array.isArray(n.routes))
      for (const [route, t] of Object.entries(n.routes))
        add(id, `route: ${route}`, t);
    if (
      n.onError &&
      typeof n.onError === "object" &&
      !Array.isArray(n.onError)
    ) {
      const err = n.onError;
      add(id, "onError exhausted", err.then);
      if (typeof err.retry === "number" && err.retry > 0)
        edges.push({
          id: `${id}:retry`,
          source: id,
          target: id,
          label: `retry ≤ ${err.retry}`,
        });
    }
    for (const key of ["body", "between"])
      if (n[key] && typeof n[key] === "object" && !Array.isArray(n[key])) {
        const ref = n[key] as { [key: string]: Json };
        edges.push({
          id: `${id}:${key}`,
          source: id,
          target: syntheticId(),
          label: `${key}: ${String(ref.flowRef)} (returns)`,
          external: true,
        });
      }
  }
  return edges;
}
export function graphSlice(plan: Plan, visible: string[]) {
  const ids = new Set(visible),
    edges = edgesOf(plan);
  const external = new Set(
    edges.filter((e) => ids.has(e.source) && e.external).map((e) => e.target),
  );
  return edges.filter(
    (e) =>
      (ids.has(e.source) || external.has(e.source)) &&
      (ids.has(e.target) || external.has(e.target)),
  );
}

export interface Change {
  target: string;
  category: string;
  before: string;
  after: string;
}
export function compare(before: Inspection, after: Inspection): Change[] {
  const changes: Change[] = [];
  const diff = (target: string, category: string, a: unknown, b: unknown) => {
    if (pretty(a) !== pretty(b))
      changes.push({
        target,
        category,
        before: typeof a === "string" ? a : pretty(a),
        after: typeof b === "string" ? b : pretty(b),
      });
  };
  const a = new Map(nodesOf(before).map((n) => [qualified(n), n.value])),
    b = new Map(nodesOf(after).map((n) => [qualified(n), n.value]));
  for (const id of new Set([...a.keys(), ...b.keys()])) {
    const old = a.get(id),
      next = b.get(id);
    if (!old || !next) {
      diff(id, old ? "Removed node" : "Added node", old, next);
      continue;
    }
    for (const key of new Set([...Object.keys(old), ...Object.keys(next)]))
      diff(
        `${id}.${key}`,
        [
          "onVerified",
          "onError",
          "routes",
          "body",
          "between",
          "onEmpty",
          "onComplete",
          "onItemOutcome",
          "onLimit",
        ].includes(key)
          ? "Edges / recovery"
          : "Node field",
        old[key],
        next[key],
      );
  }
  for (const id of new Set([
    ...Object.keys(before.registry?.tasks ?? {}),
    ...Object.keys(after.registry?.tasks ?? {}),
  ])) {
    const old = before.registry?.tasks[id],
      next = after.registry?.tasks[id];
    if (!old || !next) {
      diff(`task:${id}`, old ? "Removed task" : "Added task", old, next);
      continue;
    }
    for (const field of new Set([...Object.keys(old), ...Object.keys(next)]))
      diff(
        `task:${id}.${field}`,
        field === "tools"
          ? "Tool grants"
          : field === "completion"
            ? "Completion"
            : field === "mutations"
              ? "Mutation recovery"
              : ["maxOutputTokens", "generationTimeoutS", "loop"].includes(
                    field,
                  )
                ? "Generation / task-loop limits"
                : "Task field",
        old[field],
        next[field],
      );
  }
  for (const id of new Set([
    ...before.plans.map((p) => p.id),
    ...after.plans.map((p) => p.id),
  ])) {
    const p = before.plans.find((p) => p.id === id),
      q = after.plans.find((p) => p.id === id);
    for (const key of [
      "limits",
      "onLimit",
      "models",
      "mcp",
      "input",
      "entryStep",
    ])
      diff(`${id}.${key}`, "Flow / frame / policies", p?.[key], q?.[key]);
  }
  diff("setup", "Setup defaults", before.setup, after.setup);
  diff(
    "presentation",
    "Authored descriptions / phase membership",
    before.presentation,
    after.presentation,
  );
  const oldFiles = new Map(before.files.map((f) => [f.path, f]));
  const newFiles = new Map(after.files.map((f) => [f.path, f]));
  for (const path of new Set([...oldFiles.keys(), ...newFiles.keys()]))
    if (oldFiles.get(path)?.sha256 !== newFiles.get(path)?.sha256)
      diff(
        path,
        "Source text",
        oldFiles.get(path)?.content,
        newFiles.get(path)?.content,
      );
  return changes;
}
export interface Comment {
  id: string;
  revision: string;
  target: string;
  excerpt: string;
  text: string;
}
function fenced(text: string) {
  // A long prompt can contain more matches than a JS call's argument limit.
  let length = 3;
  for (const match of text.matchAll(/`+/g))
    length = Math.max(length, match[0].length + 1);
  const fence = "`".repeat(length);
  return `${fence}text\n${text}\n${fence}`;
}
export function feedback(doc: Inspection, comments: Comment[]) {
  return `# trollm flow review feedback\n\nCurrent package: ${doc.name}\nRevision: ${doc.packageId}\n\nUpdate the authored package sources and flow-review.json in response to these comments. Run existing validation and explain concrete changes. Comments retain their original revision anchors; reloading does not resolve them automatically.\n\n${comments.map((c) => `## ${c.target.replace(/[\r\n]/g, " ")}\n\nOriginal revision: ${c.revision}\n\nCurrent value / excerpt at comment creation:\n\n${fenced(c.excerpt)}\n\nHuman comment:\n\n${fenced(c.text)}`).join("\n\n")}`;
}
export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  const data = await response.json();
  if (!response.ok)
    throw new Error(data.error ?? `Request failed (${response.status})`);
  return data as T;
}
export async function inspectFiles(
  files: { path: string; content: string }[],
  entryFlow?: string,
  taskRegistry?: string,
) {
  return request<Inspection>("/api/inspect", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ files, entryFlow, taskRegistry }),
  });
}

/** Static names only: runtime owns value resolution; no expressions execute here. */
export function bindingReferences(
  prompt: string,
  node: RecordValue,
  task: RecordValue | undefined,
) {
  const found = new Set(
    Array.from(prompt.matchAll(/\{\{\s*([^{}]+?)\s*\}\}/g), (m) => m[1].trim()),
  );
  const walk = (value: unknown, key = "") => {
    if (key === "from" && typeof value === "string") found.add(value);
    else if (key === "fromVars" && value && typeof value === "object")
      Object.values(value).forEach((v) => {
        if (typeof v === "string") found.add(v);
      });
    else if (value && typeof value === "object")
      for (const [k, v] of Object.entries(value)) walk(v, k);
  };
  walk({ node, arguments: task?.toolArguments });
  return [...found];
}

/** Defensive API boundary for presentation metadata; executable plans remain visible. */
export function isPresentation(value: unknown): value is Presentation {
  if (!value || typeof value !== "object") return false;
  const p = value as Record<string, unknown>;
  const text = (v: unknown) => typeof v === "string" && v.trim().length > 0;
  return (
    p.format === "trollm.flow-presentation.v1" &&
    text(p.title) &&
    text(p.purpose) &&
    Array.isArray(p.phases) &&
    p.phases.every((phase) => {
      if (!phase || typeof phase !== "object") return false;
      return (
        text(phase.id) &&
        text(phase.title) &&
        text(phase.description) &&
        Array.isArray(phase.nodes) &&
        phase.nodes.every(
          (ref: unknown) =>
            !!ref &&
            typeof ref === "object" &&
            text((ref as Ref).flow) &&
            text((ref as Ref).node),
        )
      );
    })
  );
}

export function graphLabel(
  id: string,
  node: RecordValue,
  registry: Registry | null,
) {
  const task =
    typeof node.taskRef === "string"
      ? registry?.tasks[node.taskRef]
      : undefined;
  return `${id}\n${node.kind}${task ? ` · model: ${task.model}` : ""}\n${node.taskRef ?? node.outcome ?? ""}`;
}

/** Effective executor behavior, separate from authored graph edges. */
export function defaultFailureNodes(plan: Plan, ids: string[]) {
  return ids.filter(
    (id) =>
      Object.hasOwn(plan.nodes, id) &&
      plan.nodes[id].kind === "task" &&
      plan.nodes[id].onError == null,
  );
}
