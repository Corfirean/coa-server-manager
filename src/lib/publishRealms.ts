import { useEffect, useState } from "react";
import { api, type RealmMode } from "@/lib/api";
import type { PublishState, RealmPublishStatus } from "@/lib/registry";

export const REALM_LABEL: Record<RealmMode, string> = { coa: "Conquest of Azeroth", wildcard: "Wildcard", reborn: "Warcraft Reborn" };

/** One realm of this server that can be listed publicly. */
export interface PublishRealm {
  /** The main realm keeps the plain id it was published under; each realm that starts next to it has its own. */
  localId: string;
  mode: RealmMode;
  label: string;
  /** The server name; the realms that start next to the main one get their realm name added. */
  defaultName: string;
}

/** The realms this server runs, main realm first; `null` until they are known. */
export function usePublishRealms(serverId: string, serverName: string): PublishRealm[] | null {
  const [realms, setRealms] = useState<PublishRealm[] | null>(null);
  useEffect(() => {
    let alive = true;
    const build = (active: RealmMode, extra: RealmMode[]) => [
      { localId: `srv-${serverId}`, mode: active, label: REALM_LABEL[active], defaultName: serverName },
      ...extra.map((mode) => ({ localId: `srv-${serverId}:${mode}`, mode, label: REALM_LABEL[mode], defaultName: `${serverName} · ${REALM_LABEL[mode]}` })),
    ];
    void api.realmProfiles(serverId).then((r) => {
      if (!alive) return;
      const extra: RealmMode[] = [];
      if (r.simultaneous) extra.push(r.active === "coa" ? "wildcard" : "coa");
      if (r.with_reborn) extra.push("reborn");
      setRealms(build(r.active, extra));
    }).catch(() => { if (alive) setRealms(build("coa", [])); });
    return () => { alive = false; };
  }, [serverId, serverName]);
  return realms;
}

export function stateTone(state: PublishState | undefined): string {
  if (state === "online") return "text-ok";
  if (state === "rejected") return "text-bad";
  if (state === "retrying" || state === "realm_stopped" || state === "waiting_for_realm") return "text-warn";
  return "text-muted";
}

/** Listed on the public list now (or being removed from it). */
export function isListed(s: RealmPublishStatus | undefined): boolean {
  return !!s && (s.enabled || s.state === "unpublishing");
}
