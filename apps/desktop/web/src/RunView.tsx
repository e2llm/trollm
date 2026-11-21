import { useCallback, useEffect, useRef, useState } from "react";
import { json, message, runPath, type Api, type Run, type RunEvent, type RunUpdates } from "./api";
import { PageTitle, Problem, ReadFailure, runLabel, useResource } from "./common";

const retainedEvents = 160;

interface Observation extends RunUpdates {
  omitted: number;
}

const pollRun = (page: Observation) =>
  page.hasMore ? 150 : page.run.workerActive || page.run.phase === "running" ? 1200 : false;

type Action = "pause" | "resume" | "cancel";

export function RunView({ api, id, title }: { api: Api; id: string; title: (sha: string) => string }) {
  const last = useRef<Observation | null>(null);
  const load = useCallback(async (signal: AbortSignal) => {
    const previous = last.current;
    const after = previous?.nextSeq ?? 0;
    const page = await api<RunUpdates>(`${runPath(id)}?after=${after}`, { signal });
    if (signal.aborted) throw signal.reason;
    const events = [...(previous?.events ?? []), ...page.events.filter((event) => event.seq > after)];
    const dropped = Math.max(events.length - retainedEvents, 0);
    const next: Observation = {
      ...page,
      hasMore: page.hasMore && page.nextSeq > after,
      nextSeq: Math.max(after, page.nextSeq),
      events: dropped ? events.slice(-retainedEvents) : events,
      omitted: (previous?.omitted ?? 0) + dropped,
    };
    last.current = next;
    return next;
  }, [api, id]);
  const page = useResource(load, pollRun);
  const [sending, setSending] = useState<Action>();
  const [waiting, setWaiting] = useState<Action>();
  const [error, setError] = useState("");
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [copy, setCopy] = useState("");
  const observation = page.value;
  const run = observation?.run;
  const terminal = run?.phase === "succeeded" || run?.phase === "failed";
  const waitResolved = terminal || run?.phase === "interrupted" ||
    (waiting === "pause" && run?.phase === "paused");

  useEffect(() => {
    if (waitResolved) setWaiting(undefined);
  }, [waitResolved]);

  async function act(action: Action) {
    if (sending || page.loading || page.error) return;
    setSending(action);
    setConfirmCancel(false);
    setError("");
    try {
      const { run: updated } = await api<{ run: Run }>(`${runPath(id)}/action`, json("POST", { action }));
      if ((action === "pause" && updated.phase === "running") ||
          (action === "cancel" && updated.phase !== "failed" && updated.phase !== "succeeded")) {
        setWaiting(action);
      }
    } catch (error) {
      setWaiting(undefined);
      setError(message(error));
    } finally {
      page.refresh();
      setSending(undefined);
    }
  }

  async function copyReport() {
    if (!run?.report) return;
    try {
      await navigator.clipboard.writeText(run.report);
      setCopy("Report copied.");
    } catch {
      setCopy("Clipboard access is unavailable. You can select and copy the report text.");
    }
  }

  const disabled = !!sending || page.loading || !!page.error;
  const canPause = run?.phase === "running" && run.workerActive;
  const canResume = run?.canResume === true;
  const canCancel = !!run?.workerActive && (run.phase === "running" || run.phase === "paused");

  return <>
    <a className="back" href="#history">← Run history</a>
    {page.loading && !run && <p role="status">Loading run…</p>}
    {page.error && <ReadFailure text={page.error} retry={page.refresh} />}
    {observation && run && <>
      <PageTitle title={title(run.flowSha256)} actions={<span className={`pill ${run.phase}`} role="status">{runLabel(run)}</span>} />
      {run.report && <section className="panel report-panel" aria-labelledby="report-heading">
        <div className="section-heading">
          <h2 id="report-heading">Run report</h2>
          <button className="secondary compact" onClick={() => void copyReport()}>Copy report</button>
        </div>
        <div className="report-text">{run.report}</div>
        <div className="subtle" role="status">{copy}</div>
      </section>}
      {run.phase === "succeeded" && !run.report && <p className="notice">The run finished without a saved report.</p>}
      {(!terminal || canResume) && <section className="panel" aria-labelledby="progress-heading">
        <h2 id="progress-heading">Run progress</h2>
        <div role="status" className="run-status">
          {run.activity || statusText(run.phase)}
        </div>
        {waiting === "pause" && <p className="muted" role="status">Pause requested. The run will pause at its next safe point.</p>}
        {waiting === "cancel" && <p className="muted" role="status">Cancellation requested. The run is stopping.</p>}
        <div className="actions">
          {canPause && <button className="secondary" disabled={disabled || !!waiting} onClick={() => void act("pause")}>Pause run</button>}
          {canResume && <button disabled={disabled || !!waiting} onClick={() => void act("resume")}>Resume run</button>}
          {canCancel && !confirmCancel && <button className="quiet" disabled={disabled || waiting === "cancel"} onClick={() => setConfirmCancel(true)}>Cancel run…</button>}
        </div>
        {confirmCancel && <div className="notice">
          <p>Stop this run? Any changes already made in connected services will remain.</p>
          <div className="actions">
            <button className="danger" disabled={disabled} onClick={() => void act("cancel")}>Cancel run</button>
            <button className="secondary" onClick={() => setConfirmCancel(false)}>Keep run</button>
          </div>
        </div>}
      </section>}
      {error && <Problem>{error}</Problem>}
      {run.error && run.error !== "Cancelled." && <Problem>{run.error}</Problem>}
      <Activity events={observation.events} omitted={observation.omitted} more={observation.hasMore} />
    </>}
  </>;
}

function statusText(phase: Run["phase"]) {
  switch (phase) {
    case "interrupted": return "The run stopped before it finished.";
    case "paused": return "The run is paused.";
    case "failed": return "Saved progress is available.";
    default: return "The run is in progress.";
  }
}

function Activity({ events, omitted, more }: { events: RunEvent[]; omitted: number; more: boolean }) {
  return <details className="panel activity">
    <summary>Technical activity</summary>
    {omitted > 0 && <p className="muted">Earlier activity is omitted from this view.</p>}
    {events.length > 0 ? <ol className="event-list">
      {events.map((event) => <li key={event.seq}><span>{event.type}</span><span>{event.text}</span></li>)}
    </ol> : <p className="muted">No activity has been recorded yet.</p>}
    {more && <p className="muted" role="status">Reading saved activity…</p>}
  </details>;
}
