import { useCallback, useEffect, useState, type ReactNode } from "react";
import { message, type Run } from "./api";

export function useResource<T>(
  load: (signal: AbortSignal) => Promise<T>,
  poll?: (value: T) => number | false,
) {
  const [value, setValue] = useState<T>();
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [revision, setRevision] = useState(0);
  const refresh = useCallback(() => {
    setLoading(true);
    setRevision((current) => current + 1);
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    let timer: number | undefined;

    async function update() {
      try {
        const next = await load(controller.signal);
        if (controller.signal.aborted) return;
        setValue(next);
        setError("");
        const delay = poll?.(next);
        if (delay !== undefined && delay !== false) {
          timer = window.setTimeout(update, delay);
        }
      } catch (error) {
        if (!controller.signal.aborted) setError(message(error));
      } finally {
        if (!controller.signal.aborted) setLoading(false);
      }
    }

    void update();
    return () => {
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [load, poll, revision]);

  return { value, error, loading, refresh };
}

export function Problem({ children }: { children: ReactNode }) {
  return <p className="notice problem" role="alert">{children}</p>;
}

export function ReadFailure({ text, retry }: { text: string; retry: () => void }) {
  return <div className="notice problem">
    <p role="alert">{text}</p>
    <button type="button" className="quiet" onClick={retry}>Refresh</button>
  </div>;
}

export function PageTitle({
  title,
  description,
  actions,
}: {
  title: string;
  description?: string;
  actions?: ReactNode;
}) {
  return (
    <div className="page-title">
      <div>
        <h1>{title}</h1>
        {description && <p>{description}</p>}
      </div>
      {actions && <div className="actions">{actions}</div>}
    </div>
  );
}

export function runLabel(run: Run) {
  switch (run.phase) {
    case "running": return "Running";
    case "paused": return "Paused";
    case "interrupted": return "Interrupted";
    case "succeeded": return "Completed";
    case "failed": return run.error === "Cancelled." ? "Cancelled" : "Failed";
  }
}

export const flowHref = (sha: string) => `#flow/${encodeURIComponent(sha)}`;
export const runHref = (id: string) => `#run/${encodeURIComponent(id)}`;
export const inspectHref = (sha: string) => `#inspect/${encodeURIComponent(sha)}`;
