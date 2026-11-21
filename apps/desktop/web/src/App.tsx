import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { ApiError, json, message, request, type Api, type Bootstrap, type Flow, type Run } from "./api";
import { flowHref, PageTitle, Problem, ReadFailure, runHref, useResource } from "./common";
import { Inspect } from "./Inspect";
import { History, ImportFlow, Library } from "./Library";
import { emptyDraft, Prepare, type Draft } from "./Preparation";
import { RunView } from "./RunView";
import { readPendingStart, readToken, savePendingStart, saveToken, type PendingStart } from "./session";

const knownStartRejections = new Set([
  "FLOW_NOT_FOUND", "FLOW_NOT_READY", "INVALID_INPUT", "INVALID_REQUEST",
  "REQUEST_ID_CONFLICT", "RUN_ACTIVE", "SESSION_REQUIRED", "UNSUPPORTED_PROFILE",
]);
const signInAutocomplete = "current-password";
const loadBootstrap = (signal: AbortSignal) => request<Bootstrap>("/bootstrap", { signal });

function Frame({
  bootstrap, route, signOut, children,
}: {
  bootstrap?: Bootstrap;
  route?: string;
  signOut?: () => void;
  children: ReactNode;
}) {
  const content = useRef<HTMLElement>(null);
  const previous = useRef(route);

  useEffect(() => {
    if (previous.current !== route) {
      content.current?.focus();
      window.scrollTo(0, 0);
      previous.current = route;
    }
  }, [route]);

  return <>
    <a className="skip-link" href="#main" onClick={(event) => {
      event.preventDefault();
      content.current?.focus();
    }}>Skip to content</a>
    <header className="top-bar">
      <div className="shell bar-content">
        {signOut ? <a className="brand" href="#">trollm<span>Desktop</span></a> :
          <div className="brand">trollm<span>Desktop</span></div>}
        {signOut && <nav aria-label="Main navigation">
          <a href="#" aria-current={route === "" ? "page" : undefined}>Flows</a>
          <a href="#history" aria-current={route === "#history" ? "page" : undefined}>History</a>
        </nav>}
        <div className="bar-actions">
          {bootstrap && <div className={`backend ${bootstrap.backend}`}>
            <span className="status-dot" aria-hidden="true" />
            {bootstrap.backend === "llama" ? "Local models" :
              bootstrap.backend === "stub" ? "Simulated models" : "Models unavailable"}
          </div>}
          {signOut && <button className="sign-out quiet" onClick={signOut}>Sign out</button>}
        </div>
      </div>
    </header>
    <main className="shell main" id="main" ref={content} tabIndex={-1}>
      {children}
    </main>
  </>;
}

function SignIn({
  bootstrap, authenticate,
}: {
  bootstrap: Bootstrap;
  authenticate: (token: string) => void;
}) {
  const [passphrase, setPassphrase] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const setup = bootstrap.needsSetup;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setError("");
    if (setup && passphrase !== confirmation) {
      setError("The two passphrases must match.");
      return;
    }
    setBusy(true);
    try {
      const result = await request<{ token: string }>("/session", json("POST", { passphrase }));
      authenticate(result.token);
    } catch (error) {
      setError(message(error));
    } finally {
      setPassphrase("");
      setConfirmation("");
      setBusy(false);
    }
  }

  return <div className="welcome">
    <PageTitle title={setup ? "Set up trollm" : "Unlock trollm"} description={setup
      ? "Create a passphrase to protect saved connection credentials on this computer."
      : "Enter your desktop passphrase to open your flows and saved runs."} />
    <form className="panel form-stack" onSubmit={submit}>
      <label>{setup ? "New passphrase" : "Passphrase"}
        <input type="password" autoComplete={setup ? "new-password" : signInAutocomplete} required
          value={passphrase} onChange={(event) => setPassphrase(event.target.value)} disabled={busy}
          aria-describedby={setup ? "passphrase-hint" : undefined} />
      </label>
      {setup && <p className="subtle" id="passphrase-hint">Use a long, unique passphrase.</p>}
      {setup && <label>Confirm passphrase
        <input type="password" autoComplete="new-password" required value={confirmation}
          onChange={(event) => setConfirmation(event.target.value)} disabled={busy} />
      </label>}
      {error && <Problem>{error}</Problem>}
      <div className="actions">
        <button disabled={busy}>{busy ? "Opening…" : setup ? "Create passphrase" : "Unlock"}</button>
      </div>
    </form>
  </div>;
}

function useHash() {
  const [hash, setHash] = useState(window.location.hash);
  useEffect(() => {
    const update = () => setHash(window.location.hash);
    window.addEventListener("hashchange", update);
    return () => window.removeEventListener("hashchange", update);
  }, []);
  return hash;
}

function readRoute(hash: string) {
  const page = hash.slice(1);
  const [kind, raw] = page.split("/");
  try {
    return { kind, id: raw ? decodeURIComponent(raw) : "" };
  } catch {
    return { kind: "", id: "" };
  }
}

function Desktop({
  bootstrap, api, signOut,
}: {
  bootstrap: Bootstrap;
  api: Api;
  signOut: () => void;
}) {
  const hash = useHash();
  const route = readRoute(hash);
  const loadFlows = useCallback((signal: AbortSignal) => api<{ flows: Flow[] }>("/flows", { signal }), [api]);
  const library = useResource(loadFlows);
  const [drafts, setDrafts] = useState(new Map<string, Draft>());
  const [pending, setPending] = useState(readPendingStart);
  const pendingRef = useRef(pending);
  const [starting, setStarting] = useState(false);

  useEffect(() => {
    if (pendingRef.current && window.location.hash !== "#history") {
      window.location.replace("#history");
    }
  }, []);

  function recordPending(next: PendingStart | null) {
    savePendingStart(next);
    pendingRef.current = next;
    setPending(next);
  }

  async function start(flowSha256: string, input: string) {
    if (pendingRef.current) return "Review the earlier start in run history.";
    let newRequest: PendingStart;
    try {
      newRequest = { requestId: crypto.randomUUID(), flowSha256 };
      recordPending(newRequest);
    } catch {
      return "The browser could not keep track of a new run request. Check that browser storage is available.";
    }
    setStarting(true);
    try {
      const run = await api<Pick<Run, "id" | "phase">>("/runs", {
        ...json("POST", { ...newRequest, input }),
        signal: AbortSignal.timeout(30_000),
      });
      if (!run?.id) throw new Error("No run ID");
      recordPending(null);
      setDrafts((old) => {
        const updated = new Map(old);
        updated.delete(flowSha256);
        return updated;
      });
      window.location.hash = runHref(run.id);
      return "";
    } catch (error) {
      if (error instanceof ApiError && knownStartRejections.has(error.code)) {
        recordPending(null);
        return error.message;
      }
      window.location.hash = "history";
      return "The start could not be confirmed. Review run history.";
    } finally {
      setStarting(false);
    }
  }

  const flows = library.value?.flows ?? [];
  const title = (sha: string) => flows.find((flow) => flow.sha256 === sha)?.title ?? "Saved run";
  let body: ReactNode;
  if (route.kind === "history") {
    body = <History api={api} flows={flows} pending={starting ? null : pending} clearPending={() => recordPending(null)} />;
  } else if (route.kind === "flow" && route.id) {
    body = <Prepare key={route.id} api={api} sha={route.id} draft={drafts.get(route.id) ?? emptyDraft}
      updateDraft={(draft) => setDrafts((old) => new Map(old).set(route.id, draft))}
      pending={!!pending} starting={starting} start={(input) => start(route.id, input)} />;
  } else if (route.kind === "inspect" && route.id) {
    body = <Inspect key={route.id} api={api} sha={route.id} title={title(route.id)} />;
  } else if (route.kind === "run" && route.id) {
    body = <RunView key={route.id} api={api} id={route.id} title={title} />;
  } else if (route.kind === "import") {
    body = <ImportFlow api={api} installed={(flow) => {
      library.refresh();
      window.location.hash = flowHref(flow.sha256);
    }} />;
  } else {
    body = <>
      {library.loading && !library.value && <p role="status">Loading flows…</p>}
      {library.error && <ReadFailure text={library.error} retry={library.refresh} />}
      {library.value && <Library flows={flows} />}
    </>;
  }

  return <Frame bootstrap={bootstrap} route={hash} signOut={signOut}>
    {starting && route.kind !== "flow" && <p role="status" className="notice">Sending the start request…</p>}
    {body}
  </Frame>;
}

function Session() {
  const bootstrap = useResource(loadBootstrap);
  const [token, setToken] = useState(readToken);
  const [sessionError, setSessionError] = useState("");

  const authenticate = useCallback((token: string) => {
    saveToken(token);
    setToken(token);
    setSessionError("");
  }, []);
  const api: Api = useCallback(async function authenticated<T>(path: string, options?: RequestInit) {
    try {
      return await request<T>(path, options, token);
    } catch (error) {
      if (error instanceof ApiError && error.status === 401 && readToken() === token) {
        authenticate("");
        setSessionError("Your session ended. Unlock trollm again to continue.");
      }
      throw error;
    }
  }, [authenticate, token]);

  async function signOut() {
    authenticate("");
    try {
      await request("/session", { method: "DELETE" }, token);
    } catch {
      setSessionError("This browser signed out. The desktop service did not confirm the sign-out request.");
    }
  }

  if (!bootstrap.value) {
    return <Frame>{bootstrap.error ? <ReadFailure text={bootstrap.error} retry={bootstrap.refresh} /> :
      <p role="status">Connecting to trollm…</p>}</Frame>;
  }
  if (token) return <Desktop bootstrap={bootstrap.value} api={api} signOut={() => void signOut()} />;
  return <Frame bootstrap={bootstrap.value}>
    {sessionError && <Problem>{sessionError}</Problem>}
    <SignIn bootstrap={bootstrap.value} authenticate={(token) => {
      authenticate(token);
      bootstrap.refresh();
    }} />
  </Frame>;
}

export function App() {
  const oauth = new URLSearchParams(window.location.search).get("oauth");
  if (oauth === "connected" || oauth === "failed") {
    return <Frame><div className="welcome"><PageTitle
      title={oauth === "connected" ? "Connection saved" : "Sign-in could not be completed"}
      description={oauth === "connected" ? "Return to the trollm tab to continue, then close this tab." :
        "Return to the trollm tab and request a new sign-in."}
    /></div></Frame>;
  }
  return <Session />;
}
