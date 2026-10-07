import { invoke } from "@tauri-apps/api/core";
import type { CharacterView, Note } from "@/lib/portable";

export type JoinStatus = "ready" | "launched" | "needs_link" | "needs_relay" | "needs_client" | "incompatible" | "needs_transfer";

export interface JoinOutcome {
  status: JoinStatus;
  username: string | null;
  account_created: boolean;
  password_reset: boolean;
  route: string | null;
  notes: Note[];
}

export interface AccountView { username: string; kind: "generated" | "linked"; has_password: boolean }
export interface Credentials { username: string; password: string }
export interface RemoteCharacter { name: string; class: number; race: number; level: number; eligible: boolean; reasons: string[]; yours: boolean }

export interface LinkStatus { connected: boolean; last_error: string | null; channels: number; requests: number }
export interface HostControlRealm { local_id: string; realm_id: string; link: LinkStatus }
export interface ControlStatus { secret_store: string; player_id: string | null; hosting: HostControlRealm[]; preferred_username: string }

export interface ControlError { code: string; message: string }

export function asControlError(e: unknown): ControlError {
  if (e && typeof e === "object" && "code" in e && "message" in e) return e as ControlError;
  return { code: "other", message: String(e) };
}

export const control = {
  status: () => invoke<ControlStatus>("control_status"),
  setPreferredUsername: (name: string) => invoke<string>("control_set_preferred_username", { name }),
  setAccess: (localId: string, existingOnly: boolean, route: string | null) => invoke<void>("registry_set_access", { localId, existingOnly, route }),
  join: (realm: string, character: string | null, launch: boolean) => invoke<JoinOutcome>("join_realm", { realm, character, launch }),
  link: (realm: string, login: string, password: string) => invoke<string>("join_link", { realm, login, password }),
  account: (realm: string) => invoke<AccountView | null>("join_account", { realm }),
  credentials: (realm: string) => invoke<Credentials | null>("join_credentials", { realm }),
  forget: (realm: string) => invoke<void>("join_forget", { realm }),
  characters: (realm: string) => invoke<RemoteCharacter[]>("join_characters", { realm }),
  claim: (realm: string, name: string) => invoke<CharacterView>("join_claim", { realm, name }),
};
