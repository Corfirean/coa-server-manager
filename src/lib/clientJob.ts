import { useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, asUiError, type ClientPlan, type ClientStep, type UiError } from "@/lib/api";
import { checkClient } from "@/lib/clientUpdate";

/**
 * Updating the game client from the main screen, without a window over everything: the "Update client" button turns into
 * a progress bar (as in a game launcher). Only a choice that needs the owner's word (files they changed) is shown, in
 * place, below the buttons. The first-time linking of a client keeps its dialog (ClientDialog).
 */
export type ClientJobPhase = "idle" | "scanning" | "choose" | "working" | "done" | "current" | "stopped" | "failed";

export interface ClientJob {
  phase: ClientJobPhase;
  step: ClientStep | null;
  plan: ClientPlan | null;
  error: UiError | null;
}

const IDLE: ClientJob = { phase: "idle", step: null, plan: null, error: null };
const jobs = new Map<string, ClientJob>();
const listeners = new Set<() => void>();
const cancelled = new Set<string>();

const get = (id: string) => jobs.get(id) ?? IDLE;
function set(id: string, next: Partial<ClientJob>) {
  jobs.set(id, { ...get(id), ...next });
  listeners.forEach((l) => l());
}

let unlisten: (() => void) | null = null;
async function followProgress(id: string) {
  if (unlisten) return;
  try {
    unlisten = await listen<ClientStep>("client-progress", (e) => {
      const phase = get(id).phase;
      if (phase === "scanning" || phase === "working") set(id, { step: e.payload });
    });
  } catch {
    /* no event bridge (a plain browser): the bar just stays at zero */
  }
}

function fail(id: string, e: unknown) {
  if (cancelled.has(id)) set(id, { phase: "stopped", step: null });
  else set(id, { phase: "failed", step: null, error: asUiError(e) });
}

async function refresh(id: string) {
  await checkClient(id);
}

/** Look at what differs from the published client; update at once when nothing needs a decision. */
export async function startClientUpdate(id: string): Promise<void> {
  const now = get(id).phase;
  if (now === "scanning" || now === "working") return;
  cancelled.delete(id);
  set(id, { phase: "scanning", step: null, plan: null, error: null });
  void followProgress(id);
  try {
    const plan = await api.clientPlan(id);
    if (plan.items.length === 0) {
      set(id, { phase: "current", plan, step: null });
      void refresh(id);
      setTimeout(() => {
        if (get(id).phase === "current") set(id, { phase: "idle" });
      }, 5000);
      return;
    }
    if (plan.items.some((i) => i.kind === "modified")) {
      set(id, { phase: "choose", plan, step: null });
      return;
    }
    await syncClient(id, true);
  } catch (e) {
    fail(id, e);
  }
}

/** Download what is missing or changed; `keepModified` keeps the files the owner changed. */
export async function syncClient(id: string, keepModified: boolean): Promise<void> {
  cancelled.delete(id);
  set(id, { phase: "working", step: null, error: null });
  void followProgress(id);
  try {
    await api.clientSync(id, keepModified);
    set(id, { phase: "done", step: null });
    void refresh(id);
    setTimeout(() => {
      if (get(id).phase === "done") set(id, { phase: "idle" });
    }, 6000);
  } catch (e) {
    fail(id, e);
  }
}

export function stopClientJob(id: string) {
  cancelled.add(id);
  void api.clientCancel().catch(() => undefined);
}

export function dismissClientJob(id: string) {
  set(id, { phase: "idle", error: null, plan: null, step: null });
}

export function useClientJob(id: string): ClientJob {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => get(id),
  );
}
