import { useEffect, useState } from "react";
import { api, asUiError, type RealmProfiles } from "@/lib/api";
import { Card } from "@/components/ui/card";
import { useT } from "@/i18n";

export function RealmStartupSettings({ serverId }: { serverId: string }) {
  const t = useT();
  const [view, setView] = useState<RealmProfiles | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    void api.realmProfiles(serverId).then(v => { if (alive) setView(v); }).catch(e => { if (alive) setError(asUiError(e).technical); });
    return () => { alive = false; };
  }, [serverId]);
  async function change(enabled: boolean) {
    setBusy(true); setError(null);
    try { setView(await api.realmSimultaneous(serverId, enabled)); }
    catch (e) { setError(asUiError(e).technical); }
    finally { setBusy(false); }
  }
  return <Card className="mt-6 p-6" aria-busy={busy}>
    <h2 className="font-semibold">{t("realm.startupTitle")}</h2>
    <label className="mt-3 flex items-center gap-3">
      <input type="checkbox" checked={view?.simultaneous ?? false} disabled={!view || busy || !view.supported} onChange={e => void change(e.target.checked)} />
      <span>{t("realm.simultaneous")}</span>
    </label>
    <p className="mt-2 text-sm text-muted">{t("realm.simultaneousHint")}</p>
    {view?.simultaneous && <p className="mt-2 text-sm text-muted">{t("realm.secondPort", { port: view.secondary_world_port ?? "—" })}</p>}
    {busy && <p className="mt-2 text-sm" role="status">{t("realm.preparing")}</p>}
    {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
  </Card>;
}
