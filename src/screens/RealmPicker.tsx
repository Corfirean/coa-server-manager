import { useEffect, useState } from "react";
import { Layers, Loader2 } from "lucide-react";
import { api, asUiError, type RealmMode, type RealmProfiles } from "@/lib/api";
import { useT } from "@/i18n";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

export function RealmPicker({ serverId, running, disabled, onChanged, onBusy }: {
  serverId: string; running: boolean; disabled: boolean; onChanged: () => void; onBusy: (busy: boolean) => void;
}) {
  const t = useT();
  const [view, setView] = useState<RealmProfiles | null>(null);
  const [selected, setSelected] = useState<RealmMode>("coa");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    void api.realmProfiles(serverId).then(v => { if (alive) { setView(v); setSelected(v.active); } }).catch(e => { if (alive) setError(asUiError(e).technical); });
    return () => { alive = false; };
  }, [serverId]);

  async function select() {
    setBusy(true); onBusy(true); setError(null);
    try { const next = await api.realmSelect(serverId, selected, running); setView(next); onChanged(); }
    catch (e) {
      setError(asUiError(e).technical);
      // Selection may have committed even if the subsequent server restart failed.
      try { setView(await api.realmProfiles(serverId)); onChanged(); } catch { /* keep the operation error */ }
    }
    finally { setBusy(false); onBusy(false); }
  }

  return <Card className="mt-5 p-5" aria-busy={busy}>
    <div className="flex flex-wrap items-center gap-3">
      <Layers className="h-5 w-5 text-gold" aria-hidden />
      <label htmlFor="realm-profile" className="font-medium">{t("realm.profileLabel")}</label>
      <select id="realm-profile" value={selected} disabled={!view || disabled || busy} onChange={e => setSelected(e.target.value as RealmMode)} className="min-w-52 rounded-md border border-line bg-card-2 px-3 py-2 text-ink">
        <option value="coa">Conquest of Azeroth</option>
        <option value="wildcard" disabled={!view?.supported && !view?.wildcard_created}>Wildcard · Darkmoon</option>
      </select>
      {view && (selected !== view.active || view.recovery_pending) && <Button size="sm" disabled={disabled || busy} onClick={() => void select()}>
        {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
        {busy ? t("realm.switching") : selected === "wildcard" && !view.wildcard_created ? t("realm.create") : running ? t("realm.restart") : t("realm.select")}
      </Button>}
      {view && <span className="ml-auto text-xs text-muted">{t("realm.active", { name: view.active === "coa" ? "CoA" : "Wildcard" })}</span>}
    </div>
    <p className="mt-3 text-sm text-muted">{t("realm.separate")}</p>
    {selected === "wildcard" && <p className="mt-2 text-sm text-warn">{t("realm.modules")}</p>}
    {view && !view.supported && <p className="mt-2 text-sm text-warn">{t("realm.update")}</p>}
    {view?.recovery_pending && <p className="mt-2 text-sm text-warn">{t("realm.recovery")}</p>}
    {error && <p className="mt-3 selectable text-sm text-bad" role="alert">{error}</p>}
  </Card>;
}
