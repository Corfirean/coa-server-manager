import { useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, asUiError, type RepairReport } from "@/lib/api";
type State = { busy: boolean; percent: number; result: RepairReport | null; error: string | null };
const empty: State = { busy: false, percent: 0, result: null, error: null };
const states = new Map<string, State>();
const listeners = new Set<() => void>();
const get = (id: string) => states.get(id) ?? empty;
function set(id: string, change: Partial<State>) { states.set(id, { ...get(id), ...change }); listeners.forEach(fn => fn()); }
export function useRepair(id: string) {
  return useSyncExternalStore(fn => { listeners.add(fn); return () => { listeners.delete(fn); }; }, () => get(id));
}
export function clearRepairResult(id: string) { if (!get(id).busy) set(id, { result: null, error: null }); }
export async function repairServer(id: string) {
  if (get(id).busy) return;
  set(id, { busy: true, percent: 0, result: null, error: null });
  let unlisten: (() => void) | undefined;
  try {
    unlisten = await listen<{ id: string; percent: number }>("repair-progress", ({ payload }) => { if (payload.id === id) set(id, { percent: payload.percent }); });
    const result = await api.repairServer(id);
    set(id, { result, percent: 100 });
  } catch (e) { set(id, { error: asUiError(e).technical }); }
  finally { unlisten?.(); set(id, { busy: false }); }
}
