const tokenKey = "trollm.desktop.session";
const startKey = "trollm.desktop.pending-start";

export interface PendingStart {
  requestId: string;
  flowSha256: string;
}

export function readToken() {
  return sessionStorage.getItem(tokenKey) ?? "";
}

export function saveToken(token: string) {
  if (token) sessionStorage.setItem(tokenKey, token);
  else sessionStorage.removeItem(tokenKey);
}

export function readPendingStart(): PendingStart | null {
  const raw = sessionStorage.getItem(startKey);
  if (!raw) return null;
  try {
    const value = JSON.parse(raw);
    return typeof value?.requestId === "string" && typeof value?.flowSha256 === "string"
      ? value
      : null;
  } catch {
    return null;
  }
}

export function savePendingStart(start: PendingStart | null) {
  if (start) sessionStorage.setItem(startKey, JSON.stringify(start));
  else sessionStorage.removeItem(startKey);
}
