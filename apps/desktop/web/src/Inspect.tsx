import { useCallback, useState } from "react";
import { flowPath, type Api, type Inspection, type InspectionNode } from "./api";
import { flowHref, PageTitle, ReadFailure, useResource } from "./common";

function ToolPermissions({ tools }: { tools?: string[] }) {
  if (!Array.isArray(tools)) {
    return <p className="muted">Tool permissions unavailable in this inspection.</p>;
  }
  return <>
    <p>Granted tools</p>
    {tools.length ? <ul>{tools.map((tool, index) => <li key={index}>{tool}</li>)}</ul> :
      <p className="muted">None declared.</p>}
  </>;
}

function StepDetails({ node, inspection }: { node: InspectionNode; inspection: Inspection }) {
  if (node.kind === "foreach") {
    const body = node.body?.flowRef;
    const between = node.between?.flowRef;
    return <>
      {body ? <p>
        Per-item flow · {body}
        {!inspection.plans.some((plan) => plan.id === body) && <span className="muted"> · Details unavailable</span>}
      </p> : <p className="muted">Per-item flow details unavailable in this inspection.</p>}
      {between && <p>
        Between-item flow · {between}
        {!inspection.plans.some((plan) => plan.id === between) && <span className="muted"> · Details unavailable</span>}
      </p>}
    </>;
  }
  if (node.kind === "end") {
    return <p>Ends the flow{node.outcome && <> · {node.outcome}</>}</p>;
  }
  if (node.kind !== "task") {
    return <p className="muted">Step details unavailable in this inspection.</p>;
  }

  const tasks = inspection.registry?.tasks;
  const task = node.taskRef && tasks && Object.hasOwn(tasks, node.taskRef) ? tasks[node.taskRef] : undefined;
  return <>
    {node.taskRef && <p>Task · {node.taskRef}</p>}
    {task?.model && <p>Model · {task.model}</p>}
    {task?.prompt && <p>Prompt file · {task.prompt}</p>}
    <ToolPermissions tools={task?.tools} />
    {node.promptSupplement && <pre>{node.promptSupplement}</pre>}
  </>;
}

export function Inspect({ api, sha, title }: { api: Api; sha: string; title: string }) {
  const load = useCallback((signal: AbortSignal) => api<Inspection>(`${flowPath(sha)}/inspect`, { signal }), [api, sha]);
  const resource = useResource(load);
  const [path, setPath] = useState("");
  const document = resource.value;
  const source = document?.files.find((file) => file.path === path);

  return <>
    <a className="back" href={flowHref(sha)}>← Prepare flow</a>
    <PageTitle title="Inspect flow" description={title} />
    {resource.loading && !document && <p role="status">Loading inspection…</p>}
    {resource.error && <ReadFailure text={resource.error} retry={resource.refresh} />}
    {document && <div className="panel-stack">
      {document.diagnostics.length > 0 && <section className="panel">
        <h2>Package notes</h2>
        <ul>{document.diagnostics.map(({ level, message, path }, index) => <li key={index}>
          <strong>{level}</strong> · {message} {path && <small>({path})</small>}
        </li>)}</ul>
      </section>}
      <section className="panel">
        <h2>Steps and tool access</h2>
        {document.plans.length === 0 && <p className="muted">No step information is available.</p>}
        {document.plans.map((plan) => <div className="inspection-plan" key={plan.id}>
          <h3>{plan.id}</h3>
          <ul className="requirements">
            {Object.entries(plan.nodes).map(([id, node]) => <li key={id}>
              <details>
                <summary>{id}{id === plan.entryStep && <span className="subtle"> · Entry step</span>}</summary>
                <div className="inspection-detail"><StepDetails node={node} inspection={document} /></div>
              </details>
            </li>)}
          </ul>
        </div>)}
      </section>
      {document.files.length > 0 && <section className="panel form-stack">
        <h2>Bundle source</h2>
        <label>Source file
          <select value={path} onChange={(event) => setPath(event.target.value)}>
            <option value="">Choose a file</option>
            {document.files.map((file) => <option key={file.path} value={file.path}>{file.path}</option>)}
          </select>
        </label>
        {source && (source.content === null
          ? <p className="muted">Source text is unavailable for this file.</p>
          : <pre className="source" tabIndex={0} aria-label={source.path}>{source.content}</pre>)}
      </section>}
    </div>}
  </>;
}
