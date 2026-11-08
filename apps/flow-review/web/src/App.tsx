import { Suspense, lazy, useEffect, useMemo, useRef, useState } from "react";
import {
  type Comment,
  type Inspection,
  type RecordValue,
  compare,
  defaultFailureNodes,
  isPresentation,
  bindingReferences,
  feedback,
  inspectFiles,
  nodesOf,
  pretty,
  promptPath,
  promptUses,
  qualified,
  request,
  taskUses,
} from "./model";
import "./style.css";
const Graph = lazy(() => import("./Graph"));
function Detail({ title, value }: { title: string; value: unknown }) {
  return (
    <details>
      <summary>{title}</summary>
      <pre>{typeof value === "string" ? value : pretty(value)}</pre>
    </details>
  );
}
function Download({
  text,
  name,
  label,
}: {
  text: string;
  name: string;
  label: string;
}) {
  return (
    <button
      onClick={() => {
        const url = URL.createObjectURL(
          new Blob([text], { type: "text/markdown;charset=utf-8" }),
        );
        const a = document.createElement("a");
        a.href = url;
        a.download = name;
        a.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
      }}
    >
      {label}
    </button>
  );
}
export function App() {
  const [inspectorTab, setInspectorTab] = useState<
    "node" | "package" | "source" | "feedback" | "changes"
  >("node");
  const inspectorScroll = useRef<HTMLDivElement>(null);
  const [doc, setDoc] = useState<Inspection | null>(null),
    [baseline, setBaseline] = useState<Inspection | null>(null),
    [catalog, setCatalog] = useState<
      { id: string; name: string; kind: string }[]
    >([]),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [selected, setSelected] = useState(""),
    [flow, setFlow] = useState(""),
    [phase, setPhase] = useState(""),
    [search, setSearch] = useState(""),
    [count, setCount] = useState(100),
    [comments, setComments] = useState<Comment[]>([]),
    [draft, setDraft] = useState({
      revision: "",
      target: "package",
      excerpt: "",
      text: "",
    }),
    [source, setSource] = useState(""),
    [roots, setRoots] = useState({ entryFlow: "", taskRegistry: "" });
  const { target, excerpt, text: comment } = draft;
  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const accept = (next: Inspection) => {
    if (next.format !== "trollm.flow-review.v1")
      throw new Error("Unsupported inspection format");
    if (next.presentation && !isPresentation(next.presentation))
      next = {
        ...next,
        presentation: null,
        unassignedNodes: nodesOf(next).map(({ flow, node }) => ({
          flow,
          node,
        })),
        diagnostics: [
          ...next.diagnostics,
          {
            level: "warning",
            code: "presentation-browser-shape",
            message:
              "Presentation metadata is malformed; all nodes remain available.",
          },
        ],
      };
    setDoc(next);
    const initialPlan =
      next.plans.find((p) => p.path === next.entryFlow) ?? next.plans[0];
    const initialPhase = next.presentation?.phases.find((p) =>
      p.nodes.some(
        (n) => n.flow === initialPlan?.id && n.node === initialPlan?.entryStep,
      ),
    );
    setFlow(initialPlan?.id ?? "");
    setPhase(initialPhase ? `@authored:${initialPhase.id}` : "");
    setSelected(initialPlan?.entryStep ?? "");
    setCount(100);
    setSearch("");
    setInspectorTab(!next.valid ? "package" : baseline ? "changes" : "node");
    setSource("");
    setDraft((current) =>
      current.text.trim()
        ? current
        : {
            revision: next.packageId,
            target: "package",
            excerpt: next.name,
            text: "",
          },
    );
    setRoots({
      entryFlow: next.entryFlow ?? "",
      taskRegistry: next.taskRegistry ?? "",
    });
  };
  const loadCatalog = () =>
    request<{ packages: typeof catalog }>("/api/packages").then((d) =>
      setCatalog(d.packages),
    );
  useEffect(() => {
    loadCatalog().catch((e) => setError(String(e)));
  }, []);
  const plan = doc?.plans.find((p) => p.id === flow),
    node = plan?.nodes[selected],
    task =
      typeof node?.taskRef === "string"
        ? doc?.registry?.tasks[node.taskRef]
        : undefined;
  const path = promptPath(doc?.registry ?? null, task),
    prompt = doc?.files.find((f) => f.path === path);
  const allNodes = useMemo(() => (doc ? nodesOf(doc) : []), [doc]);
  const filtered = useMemo(
    () =>
      allNodes.filter(
        (n) =>
          n.flow === flow &&
          (!phase ||
            (phase === "@unassigned"
              ? doc?.unassignedNodes
              : doc?.presentation?.phases.find((p) => p.id === phase.slice(10))
                  ?.nodes
            )?.some((r) => qualified(r) === qualified(n))) &&
          `${n.node} ${n.value.taskRef ?? ""}`
            .toLowerCase()
            .includes(search.toLowerCase()),
      ),
    [allNodes, doc, flow, phase, search],
  );
  const visible = useMemo(
    () => filtered.slice(0, count).map((n) => n.node),
    [filtered, count],
  );
  const defaultFailures = useMemo(
    () => (plan ? defaultFailureNodes(plan, visible) : []),
    [plan, visible],
  );
  const changes = useMemo(
    () => (doc && baseline ? compare(baseline, doc) : []),
    [doc, baseline],
  );
  const anchor = (name: string, value: unknown) => {
    setDraft((current) =>
      current.text.trim()
        ? current
        : {
            revision: doc?.packageId ?? "",
            target: name,
            excerpt: typeof value === "string" ? value : pretty(value),
            text: "",
          },
    );
  };
  const selectNode = (id: string) => {
    if (!plan || !Object.hasOwn(plan.nodes, id)) return;
    if (!visible.includes(id)) {
      // A boundary card or binding can lead to a node outside this phase/filter.
      const destination = doc?.presentation?.phases.find((p) =>
        p.nodes.some((n) => n.flow === plan.id && n.node === id),
      );
      setPhase(destination ? `@authored:${destination.id}` : "");
      setSearch("");
      const destinationNodes = destination
        ? new Set(
            destination.nodes
              .filter((n) => n.flow === plan.id)
              .map((n) => n.node),
          )
        : null;
      const ids = Object.keys(plan.nodes).filter(
        (nodeId) => !destinationNodes || destinationNodes.has(nodeId),
      );
      setCount(Math.max(100, Math.ceil((ids.indexOf(id) + 1) / 100) * 100));
    }
    setSelected(id);
    setInspectorTab("node");
    if (plan) anchor(`node:${plan.id}/${id}`, plan.nodes[id]);
  };
  const importFiles = (files: FileList | null) => {
    if (!files?.length) return;
    void run(async () => {
      const entries = Array.from(files);
      if (entries.length === 1 && entries[0].name.endsWith(".tar")) {
        accept(
          await request<Inspection>("/api/upload", {
            method: "POST",
            headers: { "Content-Type": "application/octet-stream" },
            body: entries[0],
          }),
        );
        return;
      }
      if (
        entries.length > 1024 ||
        entries.reduce((n, f) => n + f.size, 0) > 32 * 1024 * 1024
      )
        throw new Error("Select at most 1024 text files and 32 MiB.");
      const relative = entries.map((f) => f.webkitRelativePath || f.name);
      const root = relative.every(
        (p) => p.includes("/") && p.split("/")[0] === relative[0].split("/")[0],
      );
      const texts = await Promise.all(
        entries.map(async (f, i) => ({
          path: root ? relative[i].split("/").slice(1).join("/") : relative[i],
          content: await f.text(),
        })),
      );
      accept(await inspectFiles(texts));
    });
  };
  const phaseSelect = (id: string) => {
    setPhase(id);
    setSearch("");
    setCount(100);
    const selectedPhase = id.startsWith("@authored:")
      ? doc?.presentation?.phases.find((p) => p.id === id.slice(10))
      : undefined;
    const ref = selectedPhase?.nodes[0];
    if (ref) setFlow(ref.flow);
    setSelected(ref?.node ?? "");
    setInspectorTab("node");
    anchor(
      selectedPhase
        ? `phase:${selectedPhase.id}`
        : id === "@unassigned"
          ? "unassigned-nodes"
          : "package",
      selectedPhase ??
        (id === "@unassigned" ? doc?.unassignedNodes : doc?.name),
    );
  };
  const commentOn = (name: string, value: unknown) => {
    anchor(name, value);
    setInspectorTab("feedback");
  };
  const openSource = (path: string) => {
    setSource(path);
    setInspectorTab("source");
  };
  const activePhase = doc?.presentation?.phases.find(
    (p) => phase === `@authored:${p.id}`,
  );
  useEffect(() => {
    if (inspectorScroll.current) inspectorScroll.current.scrollTop = 0;
  }, [selected, flow, source, inspectorTab]);
  return (
    <>
      <header>
        <div className="brand">
          trollm <span>FLOW REVIEW</span>
        </div>
        <div className="header-actions">
          <label className="file-button">
            Open files / tar
            <input
              aria-label="Open files or tar"
              disabled={busy}
              type="file"
              multiple
              onChange={(e) => {
                importFiles(e.target.files);
                e.target.value = "";
              }}
            />
          </label>
          <label className="file-button">
            Open folder
            <input
              aria-label="Open folder"
              disabled={busy}
              type="file"
              multiple
              {...{ webkitdirectory: "" }}
              onChange={(e) => {
                importFiles(e.target.files);
                e.target.value = "";
              }}
            />
          </label>
          <button
            disabled={!doc}
            onClick={() => {
              setBaseline(doc);
              setInspectorTab("changes");
            }}
          >
            Use current as baseline
          </button>
          <Download
            text={doc ? feedback(doc, comments) : ""}
            name="flow-review-feedback.md"
            label="Export feedback"
          />
        </div>
      </header>
      <main aria-busy={busy} className={doc ? "has-package" : ""}>
        <section className="catalog">
          <label>
            Mounted packages{" "}
            <select
              aria-label="Mounted packages"
              value=""
              disabled={busy}
              onChange={(e) => {
                const id = e.target.value;
                if (id)
                  void run(async () =>
                    accept(
                      await request<Inspection>(
                        `/api/packages/${encodeURIComponent(id)}`,
                      ),
                    ),
                  );
              }}
            >
              <option value="">Choose a package…</option>
              {catalog.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name} ({p.kind})
                </option>
              ))}
            </select>
          </label>
          <button disabled={busy} onClick={() => void run(loadCatalog)}>
            Refresh catalog
          </button>
          {busy && <span role="status">Inspecting package…</span>}
        </section>
        {error && (
          <p role="alert" className="error">
            {error}
          </p>
        )}
        {!doc ? (
          <section className="welcome">
            <p className="eyebrow">AUTHOR WITH AN LLM · REVIEW WITH PEOPLE</p>
            <h1>
              Understand the flow.
              <br />
              Give precise feedback.
            </h1>
            <p>
              Open a source folder, text files or an uncompressed trollm tar
              bundle. Review the business phases, inspect the executable
              instructions, and export comments for the authoring LLM.
            </p>
            <p className="hint">
              Inspection runs on this server. Comments and baselines stay in
              this browser session; export feedback before closing the page.
            </p>
          </section>
        ) : (
          <>
            <div className="package-heading">
              <div>
                <h1>{doc.presentation?.title ?? doc.name}</h1>
                <p className="package-purpose">
                  {doc.presentation?.purpose ??
                    "Inspect the graph and source, then give feedback to the authoring LLM."}
                </p>
              </div>
              <button
                className="validation-status"
                onClick={() => setInspectorTab("package")}
              >
                {doc.valid
                  ? "Execution validation passed"
                  : "Execution validation incomplete / failed"}
                <small>
                  {allNodes.length} nodes · {doc.plans.length} flows ·{" "}
                  {doc.diagnostics.length} diagnostics
                </small>
              </button>
            </div>
            <div className="workspace">
              <div
                className="review-navigation"
                role="region"
                aria-label="Phase and node navigation"
              >
                <div className="row">
                  <label>
                    Flow{" "}
                    <select
                      aria-label="Flow"
                      value={flow}
                      onChange={(e) => {
                        setFlow(e.target.value);
                        setSelected("");
                        setPhase("");
                        setSearch("");
                        setCount(100);
                      }}
                    >
                      {doc.plans.map((p) => (
                        <option key={p.id}>{p.id}</option>
                      ))}
                    </select>
                  </label>
                  <label>
                    Search nodes{" "}
                    <input
                      value={search}
                      onChange={(e) => {
                        setSearch(e.target.value);
                        setCount(100);
                      }}
                    />
                  </label>
                </div>
                <section>
                  <h2>
                    Business phases <small>authored</small>
                  </h2>
                  <div className="phases">
                    {doc.presentation?.phases.map((p, i) => (
                      <button
                        className={
                          phase === `@authored:${p.id}`
                            ? "phase active"
                            : "phase"
                        }
                        key={p.id}
                        onClick={() => phaseSelect(`@authored:${p.id}`)}
                      >
                        <span className="eyebrow">
                          PHASE {i + 1} · {p.nodes.length} NODES
                        </span>
                        <strong>{p.title}</strong>
                        <span>{p.description}</span>
                      </button>
                    ))}
                    <button
                      className={
                        phase === "@unassigned" ? "phase active" : "phase"
                      }
                      onClick={() => phaseSelect("@unassigned")}
                    >
                      <strong>Unassigned nodes</strong>
                      <span>
                        {doc.unassignedNodes.length} nodes remain inspectable
                      </span>
                    </button>
                    <button className="phase" onClick={() => phaseSelect("")}>
                      <strong>All nodes</strong>
                      <span>Including unassigned and unreachable nodes</span>
                    </button>
                  </div>
                </section>
                <details className="node-browser" open={Boolean(search)}>
                  <summary>
                    Node navigation · {visible.length} of {filtered.length}
                  </summary>
                  <nav aria-label="Flow nodes" className="node-list">
                    {filtered.slice(0, count).map((n) => (
                      <button
                        key={n.node}
                        aria-label={`${n.node} ${n.value.kind} ${n.value.taskRef ?? n.value.outcome ?? ""}`}
                        aria-pressed={selected === n.node}
                        onClick={() => selectNode(n.node)}
                      >
                        <strong>{n.node}</strong>
                        <span>
                          {String(n.value.kind)} ·{" "}
                          {String(n.value.taskRef ?? n.value.outcome ?? "")}{" "}
                          {n.node === plan?.entryStep ? "· entry" : ""}
                        </span>
                      </button>
                    ))}
                  </nav>
                  {filtered.length > count && (
                    <button onClick={() => setCount((n) => n + 100)}>
                      Show next 100 nodes ({filtered.length - count} remaining)
                    </button>
                  )}
                </details>
              </div>
              <section className="graph-pane" aria-label="Graph workspace">
                <div className="graph-heading">
                  <div>
                    <h2>Executable graph</h2>
                    <p>
                      {activePhase?.title ??
                        (phase === "@unassigned"
                          ? "Unassigned nodes"
                          : "All nodes")}
                    </p>
                  </div>
                  <span>
                    {visible.length} of {filtered.length} matching nodes
                  </span>
                </div>
                {activePhase && (
                  <p className="phase-description">{activePhase.description}</p>
                )}
                {plan ? (
                  <>
                    {defaultFailures.length > 0 && (
                      <div
                        className="default-notice"
                        role="note"
                        aria-label="Effective task failure defaults"
                      >
                        <strong>
                          Effective runtime default · not authored
                        </strong>
                        <p>
                          {defaultFailures.length} task
                          {defaultFailures.length === 1 ? "" : "s"} in this view
                          omit onError or set it to null. No retries (retry: 0);
                          a task failure transitions to failed with its runtime
                          error class.
                        </p>
                        <Detail
                          title="Tasks using default failure behavior"
                          value={defaultFailures.join("\n")}
                        />
                      </div>
                    )}
                    <Suspense fallback={<p role="status">Loading graph…</p>}>
                      <Graph
                        registry={doc.registry}
                        plan={plan}
                        visible={visible}
                        selected={selected}
                        select={selectNode}
                      />
                    </Suspense>
                  </>
                ) : (
                  <p>
                    Select package roots in Package details to inspect a graph.
                  </p>
                )}
              </section>
              <aside aria-label="Inspector" className="inspector-pane">
                <div
                  role="tablist"
                  aria-label="Detail panels"
                  className="inspector-tabs"
                >
                  {(
                    [
                      ["node", "Node"],
                      ["package", "Package"],
                      ["source", "Source"],
                      ["feedback", `Feedback (${comments.length})`],
                      ["changes", "Changes"],
                    ] as const
                  ).map(([id, label]) => (
                    <button
                      key={id}
                      role="tab"
                      id={`detail-tab-${id}`}
                      aria-selected={inspectorTab === id}
                      aria-controls={`detail-${id}`}
                      tabIndex={inspectorTab === id ? 0 : -1}
                      onClick={() => setInspectorTab(id)}
                      onKeyDown={(event) => {
                        const ids = [
                          "node",
                          "package",
                          "source",
                          "feedback",
                          "changes",
                        ] as const;
                        const index = ids.indexOf(id);
                        const next =
                          event.key === "ArrowRight"
                            ? (index + 1) % ids.length
                            : event.key === "ArrowLeft"
                              ? (index + ids.length - 1) % ids.length
                              : event.key === "Home"
                                ? 0
                                : event.key === "End"
                                  ? ids.length - 1
                                  : null;
                        if (next !== null) {
                          event.preventDefault();
                          setInspectorTab(ids[next]);
                          event.currentTarget.parentElement
                            ?.querySelector<HTMLButtonElement>(
                              `#detail-tab-${ids[next]}`,
                            )
                            ?.focus();
                        }
                      }}
                    >
                      {label}
                    </button>
                  ))}
                </div>
                <div
                  ref={inspectorScroll}
                  className="inspector-scroll"
                  role="tabpanel"
                  aria-labelledby={`detail-tab-${inspectorTab}`}
                  id={`detail-${inspectorTab}`}
                  tabIndex={0}
                >
                  {inspectorTab === "node" && (
                    <>
                      <h2>
                        {selected ? `${flow} / ${selected}` : "Node inspector"}
                      </h2>
                      {!node ? (
                        <p>
                          Select a node to inspect instructions and executable
                          contracts.
                        </p>
                      ) : (
                        <>
                          <p className="badge">
                            {String(node.kind)} ·{" "}
                            {String(task?.model ?? "runtime control")}
                          </p>
                          <button
                            className="comment-node"
                            onClick={() =>
                              commentOn(`node:${flow}/${selected}`, node)
                            }
                          >
                            Comment on node
                          </button>
                          <p>
                            Task reference: {String(node.taskRef ?? "none")} ·
                            Completion:{" "}
                            {String(task?.completion ?? "not a task")}
                          </p>
                          {typeof node.taskRef === "string" && (
                            <>
                              <p>
                                Shared task used by:{" "}
                                {taskUses(doc, node.taskRef).join(", ")}
                              </p>
                              <button
                                onClick={() =>
                                  commentOn(`task:${node.taskRef}`, task)
                                }
                              >
                                Comment on shared task
                              </button>
                            </>
                          )}
                          <h3>Instructions · static preview</h3>
                          <p className="hint">
                            Runtime bindings are unresolved placeholders. This
                            preview does not execute templates or include live
                            observations. Runtime context is appended for nodes
                            with supplements or routes.
                          </p>
                          {path && (
                            <>
                              <p>
                                Base prompt:{" "}
                                <button
                                  onClick={() => {
                                    openSource(path);
                                    anchor(
                                      `prompt:${path}`,
                                      prompt?.content ?? "Missing prompt",
                                    );
                                  }}
                                >
                                  {path}
                                </button>
                              </p>
                              <p className="hint">
                                Affected nodes:{" "}
                                {promptUses(doc, path).join(", ")}
                              </p>
                              <pre>
                                {prompt?.content ?? "Prompt file unavailable"}
                              </pre>
                            </>
                          )}
                          {node.promptSupplement != null && (
                            <>
                              <h4>Node prompt supplement</h4>
                              <pre>{String(node.promptSupplement)}</pre>
                              <button
                                onClick={() =>
                                  commentOn(
                                    `field:${flow}/${selected}.promptSupplement`,
                                    node.promptSupplement,
                                  )
                                }
                              >
                                Comment on supplement
                              </button>
                            </>
                          )}
                          <h3>Data inputs / unresolved bindings</h3>
                          <ul>
                            {bindingReferences(
                              `${prompt?.content ?? ""}\n${node.promptSupplement ?? ""}`,
                              node,
                              task,
                            ).map((ref) => {
                              const sourceNode = ref.split(".")[0];
                              return (
                                <li key={ref}>
                                  <code>{ref}</code>
                                  {plan?.nodes[sourceNode] ? (
                                    <>
                                      {" "}
                                      ·{" "}
                                      <button
                                        onClick={() => selectNode(sourceNode)}
                                      >
                                        Source node {sourceNode}
                                      </button>
                                    </>
                                  ) : (
                                    <>
                                      {" "}
                                      · supplied at runtime / unresolved in
                                      static review
                                    </>
                                  )}
                                </li>
                              );
                            })}
                          </ul>
                          <h3>Granted tools</h3>
                          {task?.model === "reasoner" && (
                            <p>
                              The reasoner supplies advisory prose and has no
                              tools.
                            </p>
                          )}
                          <pre>{pretty(task?.tools ?? [])}</pre>
                          <Detail
                            title="Tool arguments (bound values and modelFields)"
                            value={task?.toolArguments}
                          />
                          <Detail
                            title="Bindings / outputs and artifact requirements"
                            value={{
                              outputs: node.outputs,
                              verify: node.verify,
                              outputContract: task?.outputContract,
                            }}
                          />
                          <h3>Generation / task-loop limits</h3>
                          {task ? (
                            <p>
                              Output ceiling:{" "}
                              {String(task?.maxOutputTokens ?? 2048)} tokens per
                              generation
                              {task?.maxOutputTokens == null
                                ? " (omitted: runtime default)"
                                : ""}
                              . Generation deadline:{" "}
                              {task?.generationTimeoutS == null
                                ? "remaining run deadline (no additional task deadline)"
                                : `${task.generationTimeoutS} s, capped by remaining run time (cooperative between batches/tokens)`}
                              .
                            </p>
                          ) : (
                            <p>No loaded model task at this node.</p>
                          )}
                          <Detail
                            title="Task loop (omission means single shot)"
                            value={task?.loop}
                          />
                          <Detail
                            title="Mutations / journaling"
                            value={task?.mutations}
                          />
                          {node.kind === "task" && node.onError == null && (
                            <p role="note">
                              Effective runtime default (onError omitted or
                              null): no retries (retry: 0); a task failure
                              transitions to failed with its runtime error
                              class. This behavior is supplied by the executor,
                              not an authored edge.
                            </p>
                          )}
                          <Detail
                            title="Routes, verified transitions, recovery, pause and resume"
                            value={{
                              onVerified: node.onVerified,
                              routes: node.routes,
                              onError: node.onError,
                              onEmpty: node.onEmpty,
                              onComplete: node.onComplete,
                              onItemOutcome: node.onItemOutcome,
                              onLimit: node.onLimit,
                            }}
                          />
                          {["body", "between"].map((key) => {
                            const ref = node[key] as RecordValue | undefined;
                            return (
                              ref && (
                                <div key={key}>
                                  <p>
                                    {key}: {String(ref.flowRef)} · child frame
                                    returns to the foreach caller
                                  </p>
                                  <button
                                    disabled={
                                      !doc.plans.some(
                                        (p) => p.id === ref.flowRef,
                                      )
                                    }
                                    onClick={() => {
                                      setFlow(String(ref.flowRef));
                                      setPhase("");
                                      setSelected("");
                                      setSearch("");
                                      setCount(100);
                                    }}
                                  >
                                    Open {key} subflow
                                  </button>
                                </div>
                              )
                            );
                          })}
                          <Detail
                            title="Complete node source structure (includes iteration settings)"
                            value={node}
                          />
                          <Detail
                            title="Complete shared task structure"
                            value={task}
                          />
                          <button
                            onClick={() => {
                              openSource(plan!.path);
                              anchor(`node:${flow}/${selected}`, node);
                            }}
                          >
                            View plan source
                          </button>
                        </>
                      )}
                    </>
                  )}
                  {inspectorTab === "package" && (
                    <>
                      <section className="overview">
                        <p className="eyebrow">PACKAGE OVERVIEW</p>
                        <h2>Package details</h2>
                        <p className="revision">Revision {doc.packageId}</p>
                        <p className="badge">
                          {doc.valid
                            ? "Execution validation passed"
                            : "Execution validation incomplete / failed"}
                        </p>
                        <p>
                          {doc.presentation?.purpose ??
                            "No authored business description. Inspect the executable sources below."}
                        </p>
                        {doc.presentation && (
                          <p className="hint">
                            Purpose, inputs, results and phase descriptions are
                            authored explanations. Executable source facts
                            appear separately.
                          </p>
                        )}
                        <div className="overview-grid">
                          <div>
                            <h3>Inputs · authored</h3>
                            <p>
                              {doc.presentation?.inputsDescription ??
                                "Not described"}
                            </p>
                          </div>
                          <div>
                            <h3>Expected result · authored</h3>
                            <p>
                              {doc.presentation?.resultDescription ??
                                "Not described"}
                            </p>
                          </div>
                          <div>
                            <h3>Executable facts</h3>
                            <p>
                              {allNodes.length} nodes · {doc.plans.length} flows
                              · {Object.keys(doc.registry?.tasks ?? {}).length}{" "}
                              shared tasks
                            </p>
                            <p>
                              Model slots:{" "}
                              {[
                                ...new Set(
                                  Object.values(doc.registry?.tasks ?? {}).map(
                                    (t) => String(t.model),
                                  ),
                                ),
                              ].join(", ") || "None loaded"}
                            </p>
                            <p>
                              Granted tools:{" "}
                              {[
                                ...new Set(
                                  Object.values(
                                    doc.registry?.tasks ?? {},
                                  ).flatMap((t) =>
                                    Array.isArray(t.tools)
                                      ? t.tools.map(String)
                                      : [],
                                  ),
                                ),
                              ].join(", ") || "None"}
                            </p>
                          </div>
                        </div>
                        <Detail
                          title="Package budgets and consequential recovery / mutations"
                          value={{
                            flows: doc.plans.map((p) => ({
                              flow: p.id,
                              frameLimits: p.limits,
                              onLimit: p.onLimit,
                            })),
                            tasks: Object.fromEntries(
                              Object.entries(doc.registry?.tasks ?? {})
                                .filter(([, t]) => t.mutations != null)
                                .map(([id, t]) => [
                                  id,
                                  {
                                    grantedTools: t.tools,
                                    mutations: t.mutations,
                                  },
                                ]),
                            ),
                          }}
                        />
                        <button onClick={() => commentOn("package", doc.name)}>
                          Comment on package
                        </button>
                        <Detail
                          title="Setup / pinned model and connection defaults"
                          value={doc.setup}
                        />
                        <p className="hint">
                          Setup describes package defaults; a phone can have its
                          own overrides.
                        </p>
                      </section>
                      <details
                        className="diagnostics"
                        open={!doc.valid || doc.diagnostics.length > 0}
                      >
                        <summary>
                          Validation · {doc.diagnostics.length} diagnostics
                        </summary>
                        <p>
                          Execution and package checks:{" "}
                          {doc.valid ? "passed" : "incomplete / failed"}.
                          Presentation diagnostics are listed separately by
                          their presentation- code.
                        </p>
                        <ul>
                          {doc.diagnostics.map((d, i) => (
                            <li key={i}>
                              <strong>
                                {d.level} · {d.code}
                              </strong>{" "}
                              {d.message}{" "}
                              {d.path && (
                                <button onClick={() => openSource(d.path!)}>
                                  {d.path}
                                </button>
                              )}{" "}
                              {d.node}
                            </li>
                          ))}
                        </ul>
                        <div className="row">
                          <label>
                            Root flow
                            <select
                              aria-label="Root flow"
                              value={roots.entryFlow}
                              onChange={(e) =>
                                setRoots({
                                  ...roots,
                                  entryFlow: e.target.value,
                                })
                              }
                            >
                              <option value="">Select root…</option>
                              {doc.candidates.entryFlows.map((p) => (
                                <option key={p}>{p}</option>
                              ))}
                            </select>
                          </label>
                          <label>
                            Task registry
                            <select
                              aria-label="Task registry"
                              value={roots.taskRegistry}
                              onChange={(e) =>
                                setRoots({
                                  ...roots,
                                  taskRegistry: e.target.value,
                                })
                              }
                            >
                              <option value="">Select registry…</option>
                              {doc.candidates.taskRegistries.map((p) => (
                                <option key={p}>{p}</option>
                              ))}
                            </select>
                          </label>
                          <button
                            disabled={busy}
                            onClick={() =>
                              void run(async () =>
                                accept(
                                  await inspectFiles(
                                    doc.files.map(({ path, content }) => ({
                                      path,
                                      content,
                                    })),
                                    roots.entryFlow || undefined,
                                    roots.taskRegistry || undefined,
                                  ),
                                ),
                              )
                            }
                          >
                            Inspect selected roots
                          </button>
                        </div>
                      </details>
                      {plan && (
                        <>
                          <details>
                            <summary>
                              Flow frame budgets / model slots / policies
                            </summary>
                            <p>
                              Each flow frame enforces its own limits. Frame
                              timeout is checked at node boundaries; it is not a
                              hard preemption of tool/model calls. Generation
                              deadlines are separate.
                            </p>
                            <Detail title="Frame limits" value={plan.limits} />
                            <Detail
                              title="On flow budget limit"
                              value={plan.onLimit}
                            />
                            <Detail
                              title="Model slots and context"
                              value={plan.models}
                            />
                            <Detail
                              title="MCP policies / mutation classes"
                              value={plan.mcp}
                            />
                            <Detail
                              title="Reserved inactive input.promptTemplate (not used for generation)"
                              value={plan.input}
                            />
                            <button
                              onClick={() =>
                                commentOn(
                                  `field:${plan.id}.limits`,
                                  plan.limits,
                                )
                              }
                            >
                              Comment on frame limits
                            </button>
                          </details>
                        </>
                      )}
                    </>
                  )}
                  {inspectorTab === "feedback" && (
                    <>
                      <section className="feedback">
                        <h2>Human feedback</h2>
                        <p className="hint">
                          Comments request source changes from the authoring
                          LLM. They remain anchored to the revision where they
                          were created, including after a reload.
                        </p>
                        {comment.trim() && (
                          <p role="status">
                            Draft revision: {draft.revision}. Save or discard
                            this draft before choosing another anchor.
                          </p>
                        )}
                        <label>
                          Target{" "}
                          <input
                            aria-label="Comment target"
                            value={target}
                            onChange={(e) =>
                              setDraft({ ...draft, target: e.target.value })
                            }
                          />
                        </label>
                        <Detail
                          title="Anchor value / excerpt"
                          value={excerpt}
                        />
                        <label>
                          Comment
                          <textarea
                            aria-label="Comment"
                            value={comment}
                            onChange={(e) =>
                              setDraft({ ...draft, text: e.target.value })
                            }
                            placeholder="Describe the desired change and why it matters…"
                          />
                        </label>
                        <button
                          disabled={!comment.trim()}
                          onClick={() => {
                            setComments([
                              ...comments,
                              {
                                id: Array.from(
                                  crypto.getRandomValues(new Uint32Array(4)),
                                ).join("-"),
                                revision: draft.revision,
                                target,
                                excerpt,
                                text: comment.trim(),
                              },
                            ]);
                            setDraft({
                              revision: doc.packageId,
                              target: "package",
                              excerpt: doc.name,
                              text: "",
                            });
                          }}
                        >
                          Add comment
                        </button>
                        {comment.trim() && (
                          <button
                            onClick={() =>
                              setDraft({
                                revision: doc.packageId,
                                target: "package",
                                excerpt: doc.name,
                                text: "",
                              })
                            }
                          >
                            Discard draft
                          </button>
                        )}
                        {comments.map((c) => (
                          <article key={c.id}>
                            <strong>{c.target}</strong>
                            <p>{c.text}</p>
                            <small>
                              Original revision: {c.revision}
                              {c.revision !== doc.packageId
                                ? " · earlier revision; review manually"
                                : ""}
                            </small>
                            <button
                              onClick={() =>
                                setComments(
                                  comments.filter((x) => x.id !== c.id),
                                )
                              }
                            >
                              Remove comment
                            </button>
                          </article>
                        ))}
                        <Download
                          text={feedback(doc, comments)}
                          name="flow-review-feedback.md"
                          label={`Export ${comments.length} comments as Markdown`}
                        />
                      </section>
                    </>
                  )}
                  {inspectorTab === "changes" && (
                    <>
                      {baseline && (
                        <section>
                          <h2>Before / after comparison</h2>
                          <p>
                            Baseline {baseline.packageId} → Current{" "}
                            {doc.packageId}
                          </p>
                          <p>
                            {changes.length} concrete changes. Text changes do
                            not establish semantic equivalence. Renamed nodes
                            are removed and added.
                          </p>
                          <button onClick={() => setBaseline(null)}>
                            Clear baseline
                          </button>
                          {changes.map((c, i) => (
                            <details key={i}>
                              <summary>
                                {c.category}: {c.target}
                              </summary>
                              <div className="comparison">
                                <div>
                                  <h4>Before</h4>
                                  <pre>{c.before}</pre>
                                </div>
                                <div>
                                  <h4>After</h4>
                                  <pre>{c.after}</pre>
                                </div>
                              </div>
                            </details>
                          ))}
                        </section>
                      )}
                      {!baseline && (
                        <section>
                          <h2>Before / after comparison</h2>
                          <p>
                            Use the current package as a baseline, then open a
                            revised package to inspect changes.
                          </p>
                        </section>
                      )}
                    </>
                  )}
                  {inspectorTab === "source" && (
                    <>
                      <section>
                        <h2>Package source files</h2>
                        <label>
                          Source{" "}
                          <select
                            aria-label="Source file"
                            value={source}
                            onChange={(e) => {
                              setSource(e.target.value);
                              anchor(
                                `file:${e.target.value}`,
                                doc.files.find((f) => f.path === e.target.value)
                                  ?.content,
                              );
                            }}
                          >
                            <option value="">Select a file…</option>
                            {doc.files.map((f) => (
                              <option key={f.path}>{f.path}</option>
                            ))}
                          </select>
                        </label>
                        {source && (
                          <>
                            <p>
                              {doc.files.find((f) => f.path === source)
                                ?.sizeBytes ?? 0}{" "}
                              bytes · SHA-256{" "}
                              {doc.files.find((f) => f.path === source)?.sha256}
                            </p>
                            <pre>
                              {doc.files.find((f) => f.path === source)
                                ?.content ?? "File unavailable"}
                            </pre>
                            <button
                              onClick={() =>
                                commentOn(
                                  `file:${source}`,
                                  doc.files.find((f) => f.path === source)
                                    ?.content,
                                )
                              }
                            >
                              Comment on source
                            </button>
                          </>
                        )}
                      </section>
                    </>
                  )}
                </div>
              </aside>
            </div>
          </>
        )}
      </main>
      <footer>
        <a href="/third-party-notices.txt">Dependency notices</a> · Read-only
        review · Comments stay in this browser tab; export feedback before
        closing.
      </footer>
    </>
  );
}
