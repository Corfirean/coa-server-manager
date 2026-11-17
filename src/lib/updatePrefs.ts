import { useState } from "react";

/**
 * Whether a finished server update starts the server. Off by default: updating a stopped server leaves it stopped (a
 * server that was running is started again, as the update had to stop it).
 */
export type AfterUpdate = "server";

const key = (kind: AfterUpdate, id: string) => `coa-start-after-${kind}-update:${id}`;
const fallback = (_kind: AfterUpdate) => false;

export function startAfterUpdate(kind: AfterUpdate, id: string): boolean {
  try {
    const value = localStorage.getItem(key(kind, id));
    return value === null ? fallback(kind) : value === "1";
  } catch {
    return fallback(kind);
  }
}

export function setStartAfterUpdate(kind: AfterUpdate, id: string, on: boolean) {
  try { localStorage.setItem(key(kind, id), on ? "1" : "0"); } catch { /* storage unavailable: the default applies */ }
}

export function useStartAfterUpdate(kind: AfterUpdate, id: string): [boolean, (on: boolean) => void] {
  const [on, setOn] = useState(() => startAfterUpdate(kind, id));
  return [on, (next) => { setStartAfterUpdate(kind, id, next); setOn(next); }];
}
