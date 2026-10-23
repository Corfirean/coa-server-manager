import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type PlayStatus =
  | "ready" | "syncing" | "preparing" | "waiting_login" | "playing" | "saving" | "offline"
  | "compat_warning" | "update_required" | "conflict" | "incompatible" | "error";

export interface CopyView {
  realm_id: string;
  realm_name: string;
  status: PlayStatus;
  synced_revision: number;
  behind: boolean;
  projected_level: number | null;
  degraded: boolean;
  updated_at: string;
}

export interface CharacterView {
  id: string;
  name: string;
  class_name: string;
  race: string;
  level: number;
  revision: number;
  updated_at: string;
  active_realm: string | null;
  status: PlayStatus;
  copies: CopyView[];
}

export interface RealmView {
  id: string;
  name: string;
  kind: "installed" | "prepared";
  address: string;
  online: boolean;
  database_ok: boolean;
  level_cap: number | null;
  portable: "ready" | "setup" | "unknown";
  game_account: string | null;
  installed_id: string | null;
}

export interface Note { code: string; params: Record<string, string>; detail: string }
export type Verdict = "compatible" | "degraded" | "incompatible";
export type Step = "prepare" | "arm" | "resume" | "restart" | "resolve" | "blocked" | "offline";

export interface PreflightView {
  verdict: Verdict;
  step: Step;
  projection: [number, number] | null;
  notes: Note[];
  needs_account: boolean;
}

export interface PlayView {
  status: PlayStatus;
  projection: [number, number] | null;
  notes: Note[];
  realm_address: string;
  realm_id: string;
}

export interface LocalCharacterView {
  token: number;
  name: string;
  class_name: string;
  level: number;
  account: string;
  eligible: boolean;
  reasons: string[];
}

export interface HistoryEntry { revision: number; at: string; source_realm: string; kind: "created" | "play" | "other" }
export interface ErrorEntry { at: string; realm: string | null; message: string }

export interface PortableState {
  characters: CharacterView[];
  realms: RealmView[];
  operation: { character_id: string; realm_id: string; status: PlayStatus } | null;
  runtime: { running: boolean; last_tick_secs: number | null; sessions_open: number; errors: ErrorEntry[] };
}

export interface PortableError { code: string; message: string; notes: Note[] }

export function asPortableError(e: unknown): PortableError {
  if (e && typeof e === "object" && "code" in e && "message" in e) return e as PortableError;
  return { code: "other", message: String(e), notes: [] };
}

export type Resolve = "use_canonical" | "detach";

export const portable = {
  state: () => invoke<PortableState>("portable_state"),
  preflight: (character: string, realm: string) => invoke<PreflightView>("portable_preflight", { character, realm }),
  play: (character: string, realm: string, account?: string) => invoke<PlayView>("portable_play", { character, realm, account: account ?? null }),
  localCharacters: (realm: string) => invoke<LocalCharacterView[]>("portable_local_characters", { realm }),
  make: (realm: string, token: number) => invoke<CharacterView>("portable_make", { realm, token }),
  resolve: (character: string, realm: string, action: Resolve) => invoke<void>("portable_resolve", { character, realm, action }),
  history: (character: string) => invoke<HistoryEntry[]>("portable_history", { character }),
  addRealm: (path: string) => invoke<RealmView>("portable_add_realm", { path }),
  removeRealm: (realm: string) => invoke<void>("portable_remove_realm", { realm }),
  diagnostics: () => invoke<unknown>("portable_diagnostics"),
  preflightRemote: (character: string, capabilities: unknown) => invoke<PreflightView>("portable_preflight_remote", { character, capabilities }),
  launch: (realm: string) => invoke<void>("portable_launch", { realm }),
};

/** The state of portable play, refreshed every second or so for as long as a screen shows it. The service itself runs without the screen. */
export function usePortable(everyMs = 1200): { state: PortableState | null; refresh: () => Promise<void> } {
  const [state, setState] = useState<PortableState | null>(null);
  const busy = useRef(false);
  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try { setState(await portable.state()); } catch { /* the service is not available; the screen says so */ } finally { busy.current = false; }
  }, []);
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), everyMs);
    return () => clearInterval(timer);
  }, [refresh, everyMs]);
  return { state, refresh };
}
