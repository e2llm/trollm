import { useCallback, useEffect, useState, type FormEvent } from "react";
import {
  connectionPath, flowPath, json, message, type Api, type ConnectionStatus,
  type Flow, type InputField, type ModelStatus, type Preparation,
} from "./api";
import { inspectHref, PageTitle, Problem, ReadFailure, useResource } from "./common";

export interface Draft {
  text: string;
  fields: ReadonlyMap<string, string>;
}

export const emptyDraft: Draft = { text: "", fields: new Map() };

const modelLabels: Record<ModelStatus["status"], string> = {
  missing: "Download required",
  downloading: "Downloading",
  paused: "Download paused",
  ready: "Ready",
  error: "Download failed",
};

function size(bytes: number) {
  const unit = bytes >= 1024 ** 3 ? "GiB" : "MiB";
  const value = bytes / 1024 ** (unit === "GiB" ? 3 : 2);
  return `${new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(value)} ${unit}`;
}

function Models({ api, sha, models, refresh, setChanging }: {
  api: Api;
  sha: string;
  models: ModelStatus[];
  refresh: () => void;
  setChanging: (change: boolean) => void;
}) {
  const [working, setWorking] = useState("");
  const [error, setError] = useState("");

  async function update(slot: string, action: "download" | "pause") {
    setWorking(slot);
    setChanging(true);
    setError("");
    try {
      await api(`${flowPath(sha)}/models/${encodeURIComponent(slot)}/${action}`, { method: "POST" });
    } catch (error) {
      setError(message(error));
    } finally {
      refresh();
      setChanging(false);
      setWorking("");
    }
  }

  return <section className="panel" aria-labelledby="models-heading">
    <h2 id="models-heading">Local models</h2>
    {models.length === 0 ? <p className="muted">No model downloads are needed for this flow.</p> : (
      <ul className="requirements">
        {models.map((model) => {
          const progress = model.sizeBytes > 0 ? Math.min(model.downloadedBytes / model.sizeBytes, 1) : 0;
          return <li key={model.slot}>
            <div className="requirement-heading">
              <div>
                <strong>{model.name}</strong>
                {model.sizeBytes > 0 && <p className="subtle">{size(model.sizeBytes)}</p>}
              </div>
              <span className={`pill ${model.status}`}>{modelLabels[model.status]}</span>
            </div>
            {(model.status === "downloading" || model.status === "paused") && <>
              <progress aria-label={`Download progress for ${model.name}`} max={1} value={progress} />
              <p className="subtle">{Math.floor(progress * 100)}% · {size(model.downloadedBytes)} of {size(model.sizeBytes)}</p>
            </>}
            {model.error && <p className="error-text">{model.error}</p>}
            {model.status !== "ready" && <button
              type="button"
              className="secondary compact"
              disabled={!!working}
              onClick={() => void update(model.slot, model.status === "downloading" ? "pause" : "download")}
            >{working === model.slot ? "Updating…" :
                model.status === "downloading" ? "Pause download" :
                model.status === "paused" ? "Resume download" :
                model.status === "error" ? "Retry download" : "Download model"}</button>}
          </li>;
        })}
      </ul>
    )}
    {error && <Problem>{error}</Problem>}
  </section>;
}

function httpUrl(value: string, httpsOnly = false): string | null {
  const authority = value.match(/^https?:\/\/([^/?#]+)/i)?.[1];
  if (!authority || authority.includes("@") || /[\s\p{Cc}\\]/u.test(value)) return null;
  try {
    const url = new URL(value);
    if (!url.hostname || (httpsOnly && url.protocol !== "https:")) return null;
    return url.href;
  } catch {
    return null;
  }
}

function Connection({
  api, connection, defaults, statusConfirmed, refresh, setChanging,
}: {
  api: Api;
  connection: ConnectionStatus;
  defaults?: NonNullable<Flow["setup"]>["connections"][string];
  statusConfirmed: boolean;
  refresh: () => void;
  setChanging: (change: boolean) => void;
}) {
  const [override, setOverride] = useState<string>();
  const [mode, setMode] = useState<"bearer" | "anonymous">(connection.mode === "anonymous" && !defaults?.auth ? "anonymous" : "bearer");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [authorization, setAuthorization] = useState("");
  const [error, setError] = useState("");
  const [disconnect, setDisconnect] = useState(false);
  const url = override ?? (connection.url || defaults?.url || "");
  const path = connectionPath(connection.name);
  const allowOAuth = defaults?.auth !== "bearer";
  const allowManual = defaults?.auth !== "oauth";
  const signInPending = connection.pending === true;

  useEffect(() => {
    if (statusConfirmed && !signInPending) setAuthorization("");
  }, [signInPending, statusConfirmed]);

  async function connect(action: "save" | "oauth" | "disconnect") {
    if (busy) return;
    const endpoint = url.trim();
    if (action !== "disconnect" && !httpUrl(endpoint, true)) {
      setError("Enter a valid HTTPS endpoint without embedded credentials.");
      return;
    }
    if (action === "save" && mode === "bearer" && !token) {
      setError("Enter a bearer token.");
      return;
    }
    setBusy(true);
    setChanging(true);
    setError("");
    setAuthorization("");
    try {
      if (action === "oauth") {
        const result = await api<{ authorizationUrl: string }>(`${path}/oauth/start`, json("POST", { url: endpoint }));
        const safe = httpUrl(result.authorizationUrl, true);
        if (!safe) throw new Error("Unexpected authorization URL");
        setAuthorization(safe);
      } else if (action === "save") {
        await api(path, json("PUT", { url: endpoint, mode, ...(mode === "bearer" ? { token } : {}) }));
        setOverride(undefined);
      } else {
        await api(path, { method: "DELETE" });
        setDisconnect(false);
        setOverride(undefined);
      }
    } catch (error) {
      setError(message(error));
    } finally {
      setToken("");
      setBusy(false);
      refresh();
      setChanging(false);
    }
  }

  return <li>
    <div className="requirement-heading">
      <strong>{connection.name}</strong>
      <span className={`pill ${connection.connected ? "ready" : ""}`}>
        {connection.connected ? "Connected" : "Connection needed"}
      </span>
    </div>
    {connection.status && <p className="subtle">{connection.status}</p>}
    <details open={connection.connected ? undefined : true} onToggle={(event) => {
      if (!event.currentTarget.open) setToken("");
    }}>
      <summary>{connection.connected ? "Manage connection" : "Set up connection"}</summary>
      <form className="form-stack connection-form" onSubmit={(event) => {
        event.preventDefault();
        if (allowManual) void connect("save");
      }}>
        <label>Server URL
          <input
            type="url" value={url} required autoComplete="off" spellCheck={false}
            disabled={busy} onChange={(event) => {
              setOverride(event.target.value);
              setAuthorization("");
            }}
          />
        </label>
        {allowOAuth && <div className="actions">
          <button type="button" className="secondary compact" disabled={busy} onClick={() => void connect("oauth")}>
            Sign in with browser
          </button>
        </div>}
        {authorization && signInPending && <div className="notice">
          <p>Open the sign-in page in a new tab, then return here when it finishes.</p>
          <div className="actions">
            <a className="button secondary compact" href={authorization} target="_blank" rel="noopener noreferrer">Open sign-in page ↗</a>
            <button type="button" className="quiet" onClick={refresh}>Refresh connection</button>
          </div>
        </div>}
        {allowManual && <details open={defaults?.auth === "bearer" || undefined} onToggle={(event) => {
          if (!event.currentTarget.open) setToken("");
        }}>
          <summary>Manual connection</summary>
          <div className="form-stack connection-form">
            {!defaults?.auth && <label>Authentication
              <select value={mode} onChange={(event) => {
                setMode(event.target.value === "anonymous" ? "anonymous" : "bearer");
                setToken("");
              }} disabled={busy}>
                <option value="bearer">Bearer token</option>
                <option value="anonymous">No authentication</option>
              </select>
            </label>}
            {mode === "bearer" && <label>Bearer token
              <input type="password" autoComplete="off" spellCheck={false} value={token} disabled={busy} onChange={(event) => setToken(event.target.value)} />
            </label>}
            <div className="actions">
              <button className="secondary compact" disabled={busy || (mode === "bearer" && !token)}>Save connection</button>
            </div>
          </div>
        </details>}
        {connection.connected && (disconnect ? (
          <div className="notice">
            <p>Remove saved access for {connection.name}?</p>
            <div className="actions">
              <button type="button" className="danger compact" disabled={busy} onClick={() => void connect("disconnect")}>Disconnect</button>
              <button type="button" className="quiet" onClick={() => setDisconnect(false)}>Keep connection</button>
            </div>
          </div>
        ) : <div className="actions">
          <button type="button" className="quiet" disabled={busy} onClick={() => setDisconnect(true)}>Disconnect…</button>
        </div>)}
      </form>
    </details>
    {error && <Problem>{error}</Problem>}
  </li>;
}

function fieldError(field: InputField, value: string) {
  const text = value.trim();
  if (field.required && !text) return "Enter a value for this field.";
  if ([...text].length > field.maxLength) return `Use at most ${field.maxLength} characters.`;
  if (field.kind === "url" && text && !httpUrl(text)) {
    return "Enter an HTTP or HTTPS URL without embedded credentials.";
  }
  return "";
}

function pollPreparation(value: Preparation) {
  if (value.models.some((model) => model.status === "downloading") ||
      value.connections.some((connection) => !connection.connected)) return 2500;
  return !value.canStart && value.models.every((model) => model.status === "ready") ? 5000 : false;
}

export function Prepare({
  api, sha, draft, updateDraft, pending, starting, start,
}: {
  api: Api;
  sha: string;
  draft: Draft;
  updateDraft: (draft: Draft) => void;
  pending: boolean;
  starting: boolean;
  start: (input: string) => Promise<string>;
}) {
  const load = useCallback((signal: AbortSignal) => api<Preparation>(flowPath(sha), { signal }), [api, sha]);
  const resource = useResource(load, pollPreparation);
  const [changing, setChanging] = useState(0);
  const [error, setError] = useState("");
  const [validation, setValidation] = useState(false);
  const markChanging = (change: boolean) => setChanging((count) => count + (change ? 1 : -1));

  useEffect(() => {
    const onFocus = () => resource.refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [resource.refresh]);

  const preparation = resource.value;
  const input = preparation?.flow.setup?.input;
  const fields = input?.fields;
  const ready = !!preparation?.canStart && !resource.loading && !resource.error && !changing;
  const exactInput = () => fields
    ? JSON.stringify(Object.fromEntries(fields.map((field) => [field.key, draft.fields.get(field.key) ?? ""])))
    : draft.text;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!ready || pending || starting) return;
    setError("");
    setValidation(true);
    if (fields?.some((field) => fieldError(field, draft.fields.get(field.key) ?? ""))) {
      setError("Check the highlighted input fields.");
      return;
    }
    if (!fields && input && !draft.text.trim()) {
      setError("Enter text for this flow.");
      return;
    }
    const raw = exactInput();
    if (new TextEncoder().encode(raw).byteLength > 65536) {
      setError("Shorten the input to at most 64 KiB.");
      return;
    }
    setError(await start(raw));
  }

  return <>
    <a className="back" href="#">← Flow library</a>
    {resource.loading && !preparation && <p role="status">Loading flow…</p>}
    {resource.error && <ReadFailure text={resource.error} retry={resource.refresh} />}
    {preparation && <>
      <PageTitle
        title={preparation.flow.title}
        description={preparation.flow.description}
        actions={<a className="button secondary" href={inspectHref(sha)}>Inspect flow</a>}
      />
      <div className="prepare-grid">
        <form className="panel input-panel form-stack" noValidate onSubmit={submit}>
          <h2>{fields ? input?.label || "Flow input" : "Flow input"}</h2>
          {fields ? fields.map((field) => {
            const value = draft.fields.get(field.key) ?? "";
            const problem = validation ? fieldError(field, value) : "";
            const update = (value: string) => updateDraft({ ...draft, fields: new Map(draft.fields).set(field.key, value) });
            return <label key={field.key}>
              <span>{field.label}{field.required && <span className="required" aria-hidden="true"> *</span>}</span>
              {field.kind === "text" && field.maxLength > 250 ? <textarea
                rows={4} value={value} required={field.required} placeholder={field.placeholder}
                aria-invalid={!!problem} aria-describedby={problem ? `input-help-${field.key}` : undefined}
                disabled={starting} onChange={(event) => update(event.target.value)}
              /> : <input
                type="text" inputMode={field.kind === "url" ? "url" : "text"}
                value={value} required={field.required} placeholder={field.placeholder}
                aria-invalid={!!problem} aria-describedby={problem ? `input-help-${field.key}` : undefined}
                disabled={starting} onChange={(event) => update(event.target.value)}
              />}
              {problem && <span id={`input-help-${field.key}`} className="error-text">{problem}</span>}
            </label>;
          }) : <label>
            {input?.label || "Text"}
            <textarea
              rows={8} placeholder={input?.placeholder} value={draft.text} required={!!input}
              disabled={starting} onChange={(event) => updateDraft({ ...draft, text: event.target.value })}
            />
          </label>}
          {error && <Problem>{error}</Problem>}
          <div className="start-area">
            {pending && !starting && <p className="notice">A previous Start needs review. <a href="#history">Open run history</a></p>}
            {!preparation.canStart && preparation.blockers.length > 0 && <div className="muted">
              <p>Before starting</p>
              <ul className="blockers">{preparation.blockers.map((blocker, index) => <li key={index}>{blocker}</li>)}</ul>
            </div>}
            <div className="actions">
              <button disabled={!ready || pending || starting}>{starting ? "Starting…" : "Start run"}</button>
              {ready && !starting && !pending && <span className="muted">Ready to run</span>}
            </div>
          </div>
        </form>
        <div className="panel-stack">
          <Models api={api} sha={sha} models={preparation.models} refresh={resource.refresh} setChanging={markChanging} />
          {preparation.connections.length > 0 && <section className="panel" aria-labelledby="connections-heading">
            <h2 id="connections-heading">Service connections</h2>
            <ul className="requirements">{preparation.connections.map((connection) => <Connection
              key={connection.name} api={api} connection={connection} defaults={preparation.flow.setup?.connections[connection.name]}
              statusConfirmed={!resource.loading && !resource.error} refresh={resource.refresh} setChanging={markChanging}
            />)}</ul>
          </section>}
        </div>
      </div>
    </>}
  </>;
}
