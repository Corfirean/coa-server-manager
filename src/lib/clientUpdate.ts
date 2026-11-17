import { useSyncExternalStore } from "react";
import { api, type ClientStatus } from "@/lib/api";

/**
 * Whether a game client is linked, for the main screen. The Manager does not download or update the client (the
 * community's official launcher does), so this is a local check only.
 */
interface State {
  status: ClientStatus | null;
  checking: boolean;
}

const EMPTY: State = { status: null, checking: false };
const states = new Map<string, State>();
const listeners = new Set<() => void>();

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
    // keep what we knew
  } finally {
    set(id, { checking: false });
  }
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
