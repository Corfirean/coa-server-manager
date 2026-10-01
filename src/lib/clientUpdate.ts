import { useSyncExternalStore } from "react";
import { api, type ClientStatus } from "@/lib/api";

/**
 * Game client status for the main screen: which client is linked and, for a client the Manager keeps up to date,
 * whether a newer one has been published. Checked at start and every five minutes (one small request; no game file is read).
 */
interface State {
  status: ClientStatus | null;
  checking: boolean;
}

const EMPTY: State = { status: null, checking: false };
const states = new Map<string, State>();
const listeners = new Set<() => void>();
const CHECK_EVERY_MS = 5 * 60 * 1000;

const get = (id: string) => states.get(id) ?? EMPTY;
function set(id: string, next: Partial<State>) {
  states.set(id, { ...get(id), ...next });
  listeners.forEach((l) => l());
}

export async function checkClient(id: string): Promise<void> {
  if (get(id).checking) return;
  set(id, { checking: true });
  try {
    set(id, { status: await api.clientStatus(id) });
  } catch {
    // offline: keep what we knew
  } finally {
    set(id, { checking: false });
  }
}

/** Check now and then every five minutes. Returns the function that stops it. */
export function startClientPolling(id: string): () => void {
  void checkClient(id);
  const t = setInterval(() => void checkClient(id), CHECK_EVERY_MS);
  return () => clearInterval(t);
}

export function useClientStatus(id: string): State {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => get(id),
  );
}
