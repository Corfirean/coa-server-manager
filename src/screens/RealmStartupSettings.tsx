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
  async function change(realm: "wildcard" | "reborn", enabled: boolean) {
    setBusy(true); setError(null);
    try { setView(await (realm === "wildcard" ? api.realmSimultaneous : api.realmWithReborn)(serverId, enabled)); }
    catch (e) { setError(asUiError(e).technical); }
    finally { setBusy(false); }
  }
  return <Card className="mt-6 p-6" aria-busy={busy}>
    <h2 className="font-semibold">{t("realm.startupTitle")}</h2>
    <label className="mt-3 flex items-center gap-3">
      <input type="checkbox" checked={view?.simultaneous ?? false} disabled={!view || busy || !view.supported || view.active === "reborn"} onChange={e => void change("wildcard", e.target.checked)} />
      <span>{t("realm.simultaneous")}</span>
    </label>
    {view?.simultaneous && <p className="mt-1 text-sm text-muted">{t("realm.secondPort", { port: view.secondary_world_port ?? "—" })}</p>}
    <label className="mt-3 flex items-center gap-3">
      <input type="checkbox" checked={view?.with_reborn ?? false} disabled={!view || busy || !view.reborn_supported || view.active !== "coa"} onChange={e => void change("reborn", e.target.checked)} />
      <span>{t("realm.withReborn")}</span>
    </label>
    {view?.with_reborn && <p className="mt-1 text-sm text-muted">{t("realm.rebornPort", { port: view.reborn_world_port ?? "—" })}</p>}
    {view && view.active !== "coa" && !view.with_reborn && <p className="mt-1 text-sm text-warn">{t("realm.rebornNeedsCoa")}</p>}
    {view?.simultaneous && view.with_reborn && <p className="mt-3 text-sm">{t("realm.allThree")}</p>}
    <details className="mt-4 text-sm text-muted"><summary className="cursor-pointer">{t("settings.startupHelp")}</summary><p className="mt-2 leading-relaxed">{t("realm.simultaneousHint")}</p></details>
    {busy && <p className="mt-2 text-sm" role="status">{t("realm.preparing")}</p>}
    {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
  </Card>;
}
