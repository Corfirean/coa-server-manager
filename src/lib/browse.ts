import { invoke } from "@tauri-apps/api/core";

export interface Rates {
  xp_kill: number | null;
  xp_quest: number | null;
  xp_explore: number | null;
  loot: number | null;
  money: number | null;
  reputation: number | null;
  honor: number | null;
}
export interface ModuleEntry { id: string; version: string | null; enabled: boolean }
export interface Population { players: number; bots: number; capacity: number | null }
export interface AccountProvisioning { automatic: boolean; existing_only: boolean }
export type Ruleset = "coa" | "wildcard";

export interface RealmSummary {
  realm_id: string;
  display_name: string;
  description: string;
  language: string;
  region: string | null;
  ruleset: Ruleset;
  level_cap: number | null;
  rates: Rates;
  modules: ModuleEntry[];
  population: Population;
  account_provisioning: AccountProvisioning;
  manager_version: string;
  capabilities_hash: string;
  listing_hash: string;
  metadata_revision: number;
  online: boolean;
  created_at: number;
  last_seen_at: number;
}

export interface RealmPage { realms: RealmSummary[]; next_cursor: string | null }

export interface RealmDetail {
  realm_id: string;
  public_key: string;
  listing: { display_name: string; description: string; language: string; region: string | null; rates: Rates; modules: ModuleEntry[]; account_provisioning: AccountProvisioning; manager_version: string };
  ruleset: Ruleset;
  level_cap: number | null;
  population: Population;
  capabilities: unknown;
  metadata_revision: number;
  online: boolean;
  last_seen_at: number;
}

export interface ModuleInfo { id: string; name: string; description: Record<string, string>; status: string; icon: string }

export type SortKey = "name" | "players" | "cap" | "created";

export interface BrowseParams {
  q?: string;
  ruleset?: Ruleset;
  cap_min?: number;
  cap_max?: number;
  module?: string;
  players_min?: number;
  status?: "online" | "offline" | "all";
  sort?: SortKey;
  order?: "asc" | "desc";
  cursor?: string;
  limit?: number;
}

export const browse = {
  list: (params: BrowseParams) => invoke<RealmPage>("browse_list", { params }),
  detail: (realmId: string) => invoke<RealmDetail>("browse_detail", { realmId }),
  modules: () => invoke<ModuleInfo[]>("module_catalog"),
};

/** `×2`, `×1.5`; unknown is a dash, never a guess. */
export function rate(v: number | null): string {
  return v === null ? "—" : `×${Number.isInteger(v) ? v : Number(v.toFixed(2))}`;
}
