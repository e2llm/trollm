export interface Bootstrap {
  version: string;
  backend: "llama" | "stub" | "unavailable";
  needsSetup: boolean;
}

export interface InputField {
  key: string;
  label: string;
  placeholder?: string;
  required: boolean;
  kind: "text" | "url";
  maxLength: number;
}

export interface Flow {
  sha256: string;
  id: string;
  title: string;
  description: string;
  version: number;
  setup: {
    input?: { label: string; placeholder?: string; fields?: InputField[] };
    connections: Record<string, { url: string; auth: "bearer" | "oauth" }>;
  } | null;
}

export interface ModelStatus {
  slot: string;
  name: string;
  url: string;
  sha256: string;
  sizeBytes: number;
  downloadedBytes: number;
  status: "missing" | "downloading" | "paused" | "ready" | "error";
  error?: string;
}

export interface ConnectionStatus {
  name: string;
  url: string;
  mode: "unavailable" | "anonymous" | "bearer" | "oauth";
  connected: boolean;
  pending: boolean;
  status: string;
}

export interface Preparation {
  flow: Flow;
  models: ModelStatus[];
  connections: ConnectionStatus[];
  canStart: boolean;
  blockers: string[];
}

export interface Run {
  id: string;
  flowSha256: string;
  phase: "running" | "paused" | "interrupted" | "succeeded" | "failed";
  workerActive: boolean;
  canResume: boolean;
  report?: string;
  error?: string;
  activity?: string;
  lastSeq: number;
}

export interface RunEvent {
  seq: number;
  type: string;
  text: string;
}

export interface RunUpdates {
  run: Run;
  events: RunEvent[];
  hasMore: boolean;
  nextSeq: number;
}

export interface InspectionNode {
  kind: "task" | "foreach" | "end";
  taskRef?: string | null;
  promptSupplement?: string;
  body?: { flowRef: string } | null;
  between?: { flowRef: string } | null;
  outcome?: string | null;
}

export interface Inspection {
  diagnostics: { level: string; message: string; path?: string }[];
  files: { path: string; content: string | null }[];
  plans: {
    id: string;
    entryStep: string;
    nodes: Record<string, InspectionNode>;
  }[];
  registry: {
    tasks: Record<string, { model?: string; prompt?: string; tools?: string[] }>;
  } | null;
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
  ) {
    super(message);
  }
}

export async function request<T>(
  path: string,
  options: RequestInit = {},
  token?: string,
): Promise<T> {
  const headers = new Headers(options.headers);
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const response = await fetch(`/api${path}`, {
    ...options,
    headers,
    cache: "no-store",
    redirect: "error",
  });
  if (!response.ok) {
    const body = await response.json().catch(() => null);
    throw new ApiError(
      body?.error?.message ?? "The desktop service could not complete the request.",
      response.status,
      body?.error?.code ?? "",
    );
  }
  return (await response.json()) as T;
}

export type Api = <T>(path: string, options?: RequestInit) => Promise<T>;

export function json(method: string, body: unknown): RequestInit {
  return {
    method,
    body: JSON.stringify(body),
    headers: { "Content-Type": "application/json" },
  };
}

export const flowPath = (sha: string) => `/flows/${encodeURIComponent(sha)}`;
export const runPath = (id: string) => `/runs/${encodeURIComponent(id)}`;
export const connectionPath = (name: string) => `/connections/${encodeURIComponent(name)}`;

export function message(error: unknown) {
  return error instanceof ApiError
    ? error.message
    : "The desktop service could not be reached. Check that it is running and try again.";
}
