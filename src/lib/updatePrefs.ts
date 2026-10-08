import { useState } from "react";

/**
 * What to do when an update has finished: leave the server running (the default, as the update proves the new build
 * starts) or stop it again, and whether to open the game after a client update (off by default).
 */
export type AfterUpdate = "server" | "client";

const key = (kind: AfterUpdate, id: string) => `coa-start-after-${kind}-update:${id}`;
const fallback = (kind: AfterUpdate) => kind === "server";

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
