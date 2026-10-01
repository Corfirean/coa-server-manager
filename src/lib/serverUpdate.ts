import { useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, asUiError, type UiError, type UpdatePreview, type UpdateTxn } from "@/lib/api";

/**
 * Server update status for the main screen and the navigation: checked when the app starts and every five minutes,
 * and applied from the main button when nothing needs a decision (files the owner changed are decided in Settings).
 */
export interface ServerUpdateState {
  preview: UpdatePreview | null;
  available: boolean;
  checking: boolean;
  applying: boolean;
  step: string | null;
  percent: number;
  error: string | null;
  /** The full error of the last update attempt, for the Settings card. */
  uiError: UiError | null;
  /** How the last update ended; kept here so the Settings card can show it after the user left and came back. */
  result: { committed: string } | { pending: UpdateTxn } | null;
}

const EMPTY: ServerUpdateState = { preview: null, available: false, checking: false, applying: false, step: null, percent: 0, error: null, uiError: null, result: null };
const states = new Map<string, ServerUpdateState>();
const listeners = new Set<() => void>();
export const CHECK_EVERY_MS = 5 * 60 * 1000;

const get = (id: string) => states.get(id) ?? EMPTY;
function set(id: string, next: Partial<ServerUpdateState>) {
  states.set(id, { ...get(id), ...next });
  listeners.forEach((l) => l());
}

export async function checkServerUpdate(id: string): Promise<void> {
  const cur = get(id);
  if (cur.applying || cur.checking) return;
  set(id, { checking: true });
  try {
    const p = await api.checkUpdate(id);
    set(id, { preview: p, available: p.to_version !== (p.from_version ?? ""), error: null });
  } catch (e) {
    // offline or no package published: keep what we knew and stay quiet
    set(id, { error: asUiError(e).human.code });
  } finally {
    set(id, { checking: false });
  }
}

/** Check now and then every five minutes. Returns the function that stops it. */
export function startServerUpdatePolling(id: string): () => void {
  void checkServerUpdate(id);
  const t = setInterval(() => void checkServerUpdate(id), CHECK_EVERY_MS);
  return () => clearInterval(t);
}

export function clearServerUpdateResult(id: string) {
  set(id, { result: null, uiError: null });
}

/**
 * Apply an update. Without options this is the main-screen button: the pending update, only when no file needs a
 * decision. With options (the Settings card) the owner has already chosen what to do with each changed file and may
 * point at another package source. The state lives here, not in a screen, so leaving the Settings tab and coming back
 * shows the update still running (and its progress) instead of an "Update now" button the server would refuse.
 */
export async function applyServerUpdate(id: string, opts?: { choices: Record<string, "keep" | "replace">; source?: string }): Promise<boolean> {
  const p = get(id).preview;
  if (get(id).applying) return false;
  if (!opts && (!p || p.conflicts.length > 0)) return false;
  set(id, { applying: true, step: null, percent: 0, error: null, uiError: null, result: null });
  let un: (() => void) | undefined;
  try {
    un = await listen<{ step: string; percent: number }>("update-progress", (e) => set(id, { step: e.payload.step, percent: e.payload.percent }));
  } catch {
    /* progress is optional */
  }
  try {
    const out = await api.applyUpdate(id, opts?.choices ?? {}, opts?.source);
    set(id, { result: out.txn.state === "committed" ? { committed: out.txn.to_version ?? "" } : { pending: out.txn } });
    return true;
  } catch (e) {
    const ui = asUiError(e);
    set(id, { error: ui.human.code, uiError: ui });
    return false;
  } finally {
    un?.();
    set(id, { applying: false });
    void checkServerUpdate(id);
  }
}

export function useServerUpdate(id: string): ServerUpdateState {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => get(id),
  );
}
