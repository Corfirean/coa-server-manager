import { useEffect, useState } from "react";
import { api, asUiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useT } from "@/i18n";

/** Takes a server out of the Manager's list. Nothing on disk is deleted, so the folder can be added again later. */
export function RemoveServerCard({ serverId, path, onForget }: { serverId: string; path: string; onForget: () => Promise<void> }) {
  const t = useT();
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let active = true;
    void api.status(serverId).then((s) => {
      if (!active) return;
      const all = [s.observed.mysql, s.observed.auth, s.observed.world, ...(s.observed.secondary_world ? [s.observed.secondary_world] : [])];
      setRunning(all.some((x) => x.state === "running" || x.state === "starting" || x.state === "stopping"));
    }).catch(() => { /* an unreadable status must not hide the button */ });
    return () => { active = false; };
  }, [serverId]);

  async function remove() {
    if (!window.confirm(t("remove.confirm", { path }))) return;
    setBusy(true); setError(null);
    try { await onForget(); }
    catch (e) { setError(asUiError(e).technical); setBusy(false); }
  }

  return <Card className="mt-6 p-6">
    <h2 className="font-semibold">{t("remove.title")}</h2>
    <p className="mt-2 text-sm text-muted">{t("remove.text")}</p>
    <p className="selectable mt-1 break-all text-sm text-muted">{path}</p>
    <Button className="mt-4" disabled={busy || running} onClick={() => void remove()}>{t("remove.button")}</Button>
    {running && <p className="mt-2 text-sm text-warn">{t("remove.stopFirst")}</p>}
    {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
  </Card>;
}
