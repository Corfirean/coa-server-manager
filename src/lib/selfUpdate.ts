import { useSyncExternalStore } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/**
 * Manager self-update: look for a new version in the background, download it quietly, and install it when the window is
 * closed (or right away when asked). Servers are separate processes, so updating the Manager never touches them.
 */
export type Phase = "idle" | "checking" | "downloading" | "ready" | "installing" | "error" | "current";
export interface SelfUpdateState {
  phase: Phase;
  version?: string;
  notes?: string;
  percent: number | null;
  /** Raw error text, kept for the details line (the UI shows a friendly message). */
  error?: string;
}

const AUTO_KEY = "coa-auto-update";
let state: SelfUpdateState = { phase: "idle", percent: null };
let update: Update | null = null;
const listeners = new Set<() => void>();

function set(next: Partial<SelfUpdateState>) {
  state = { ...state, ...next };
  listeners.forEach((l) => l());
}

export function autoUpdateEnabled(): boolean {
  try {
    return localStorage.getItem(AUTO_KEY) !== "off";
  } catch {
    return true;
  }
}

export function setAutoUpdate(on: boolean) {
  try {
    localStorage.setItem(AUTO_KEY, on ? "on" : "off");
  } catch {
    /* ignore */
  }
  listeners.forEach((l) => l());
}

/** Look for a new version. With `download` it also fetches the installer so it is ready to install on exit. */
export async function lookForUpdate(opts: { download: boolean }): Promise<void> {
  if (state.phase === "checking" || state.phase === "downloading" || state.phase === "installing") return;
  set({ phase: "checking", error: undefined, percent: null });
  try {
    const u = await check();
    if (!u) {
      update = null;
      set({ phase: "current", version: undefined });
      return;
    }
    update = u;
    set({ version: u.version, notes: u.body ?? undefined });
    if (!opts.download) {
      set({ phase: "idle" });
      return;
    }
    await downloadPending();
  } catch (e) {
    set({ phase: "error", error: String(e) });
  }
}

async function downloadPending(): Promise<void> {
  if (!update) return;
  set({ phase: "downloading", percent: 0 });
  let total = 0;
  let done = 0;
  try {
    await update.download((e) => {
      if (e.event === "Started") total = e.data.contentLength ?? 0;
      if (e.event === "Progress") {
        done += e.data.chunkLength;
        if (total) set({ percent: Math.round((done / total) * 100) });
      }
    });
    set({ phase: "ready", percent: 100 });
  } catch (e) {
    set({ phase: "error", error: String(e), percent: null });
  }
}

/** Download (if needed) and install now. The installer ends this process; `relaunch` is the fallback. */
export async function installNow(): Promise<void> {
  if (!update) return;
  try {
    if (state.phase !== "ready") await downloadPending();
    if (state.phase !== "ready") return;
    set({ phase: "installing" });
    await update.install();
    await relaunch();
  } catch (e) {
    set({ phase: "error", error: String(e) });
  }
}

/** Install a downloaded update when the user closes the window. Call once at start-up. */
export function installOnClose(): () => void {
  let stop: (() => void) | undefined;
  try {
    const win = getCurrentWindow();
    void win
      .onCloseRequested(async (event) => {
        if (state.phase !== "ready" || !update || !autoUpdateEnabled()) return;
        event.preventDefault();
        try {
          set({ phase: "installing" });
          await update.install();
        } catch (e) {
          set({ phase: "error", error: String(e) });
          await win.destroy();
        }
      })
      .then((un) => {
        stop = un;
      })
      .catch(() => {});
  } catch {
    /* not running inside the desktop shell (browser preview) */
  }
  return () => stop?.();
}

export function useSelfUpdate(): SelfUpdateState {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => state,
  );
}
