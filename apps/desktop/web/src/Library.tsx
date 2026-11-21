import { useCallback, useState, type FormEvent } from "react";
import { message, type Api, type Flow, type Run } from "./api";
import { flowHref, PageTitle, Problem, ReadFailure, runHref, runLabel, useResource } from "./common";
import type { PendingStart } from "./session";

export function Library({ flows }: { flows: Flow[] }) {
  const sorted = [...flows].sort((a, b) =>
    Number(b.id === "text-summary") - Number(a.id === "text-summary") ||
    a.title.localeCompare(b.title),
  );

  return <>
    <PageTitle
      title="Flow library"
      description="Choose a flow, prepare its inputs, and follow it through to a report."
      actions={<a className="button secondary" href="#import">Import flow</a>}
    />
    {sorted.length ? (
      <ul className="flow-grid">
        {sorted.map((flow) => (
          <li className="panel flow-card" key={flow.sha256}>
            <div>
              <h2><a href={flowHref(flow.sha256)}>{flow.title}</a></h2>
              {flow.description && <p>{flow.description}</p>}
            </div>
            <div className="card-footer">
              {flow.version && <span>Version {flow.version}</span>}
              <a className="button secondary" href={flowHref(flow.sha256)} aria-label={`Open ${flow.title}`}>
                Open flow <span aria-hidden="true">↗</span>
              </a>
            </div>
          </li>
        ))}
      </ul>
    ) : <div className="panel"><p>Your library is empty. Import a flow bundle to get started.</p></div>}
  </>;
}

export function ImportFlow({ api, installed }: { api: Api; installed: (flow: Flow) => void }) {
  const [file, setFile] = useState<File>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!file || busy) return;
    setBusy(true);
    setError("");
    try {
      const result = await api<{ flow: Flow }>("/flows/import", {
        method: "POST",
        headers: { "Content-Type": "application/x-tar" },
        body: file,
      });
      installed(result.flow);
    } catch (error) {
      setError(message(error));
    } finally {
      setBusy(false);
    }
  }

  return <>
    <a className="back" href="#">← Flow library</a>
    <PageTitle title="Import a flow" description="Add an uncompressed .tar flow bundle to your library." />
    <form onSubmit={submit} className="panel form-stack narrow">
      <p className="muted">Choose a bundle from an author you trust. A flow can use its granted tools to read or change data on connected services.</p>
      <label>Flow bundle
        <input type="file" accept=".tar,application/x-tar" required disabled={busy} onChange={(event) => setFile(event.target.files?.[0])} />
      </label>
      {error && <Problem>{error}</Problem>}
      <div className="actions"><button disabled={!file || busy}>{busy ? "Importing…" : "Import bundle"}</button></div>
    </form>
  </>;
}

const pollHistory = () => 5000;

export function History({
  api,
  flows,
  pending,
  clearPending,
}: {
  api: Api;
  flows: Flow[];
  pending: PendingStart | null;
  clearPending: () => void;
}) {
  const load = useCallback((signal: AbortSignal) => api<{ runs: Run[] }>("/runs", { signal }), [api]);
  const history = useResource(load, pollHistory);
  const [confirm, setConfirm] = useState(false);

  return <>
    <PageTitle title="Run history" description="Open a saved run to read its progress or report." />
    {pending && <section className="notice" aria-labelledby="start-review">
      <h2 id="start-review">Start could not be confirmed</h2>
      <p>Review the recent runs below before starting again. The previous request may already have started a run.</p>
      {confirm ? <>
        <p>If you start another run, it could repeat actions from the earlier run. Continue only once you have checked its outcome.</p>
        <div className="actions">
          <button type="button" className="secondary" disabled={!history.value || !!history.error} onClick={clearPending}>Allow new starts</button>
          <button type="button" className="quiet" onClick={() => setConfirm(false)}>Keep blocked</button>
        </div>
      </> : <button type="button" className="secondary" disabled={!history.value || !!history.error} onClick={() => setConfirm(true)}>I reviewed the history</button>}
    </section>}
    {history.loading && <p role="status">Loading runs…</p>}
    {history.error && <ReadFailure text={history.error} retry={history.refresh} />}
    {history.value && (history.value.runs.length ? (
      <ul className="panel rows">
        {history.value.runs.map((run) => (
          <li key={run.id} className="list-row">
            <a href={runHref(run.id)}>{flows.find((flow) => flow.sha256 === run.flowSha256)?.title ?? "Saved run"}</a>
            <span className={`pill ${run.phase}`}>{runLabel(run)}</span>
          </li>
        ))}
      </ul>
    ) : <div className="panel"><p>No saved runs yet.</p></div>)}
  </>;
}
