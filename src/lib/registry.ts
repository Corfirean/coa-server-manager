import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type PublishState = "starting" | "waiting_for_realm" | "realm_stopped" | "online" | "retrying" | "rejected" | "unpublishing" | "disabled";

export interface RealmPublishStatus {
  local_id: string;
  realm_id: string | null;
  enabled: boolean;
  state: PublishState;
  display_name: string;
  description: string;
  language: string;
  metadata_revision: number | null;
  last_ok_unix: number | null;
  last_error: string | null;
  retry_in_secs: number | null;
}

export interface RegistryStatus {
  url: string | null;
  realms: RealmPublishStatus[];
}

export const realmRegistry = {
  status: () => invoke<RegistryStatus>("registry_status"),
  setUrl: (url: string | null) => invoke<void>("registry_set_url", { url }),
  publish: (localId: string, displayName: string, description: string, language: string) =>
    invoke<void>("registry_publish", { localId, displayName, description, language }),
  unpublish: (localId: string) => invoke<void>("registry_unpublish", { localId }),
  retry: (localId: string) => invoke<void>("registry_retry", { localId }),
};

/** The publishing state of this Manager's realms, refreshed while a screen shows it. The loop itself runs without the screen. */
export function useRegistryStatus(everyMs = 2000): { status: RegistryStatus | null; refresh: () => Promise<void> } {
  const [status, setStatus] = useState<RegistryStatus | null>(null);
  const busy = useRef(false);
  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try { setStatus(await realmRegistry.status()); } catch { /* the service is not available */ } finally { busy.current = false; }
  }, []);
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), everyMs);
    return () => clearInterval(timer);
  }, [refresh, everyMs]);
  return { status, refresh };
}
