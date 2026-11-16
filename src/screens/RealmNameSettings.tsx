import { useEffect, useState } from "react";
import { api, asUiError, type RealmName } from "@/lib/api";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n";

const MAX_CHARS = 32;

export function RealmNameSettings({ serverId }: { serverId: string }) {
  const t = useT();
  const [realms, setRealms] = useState<RealmName[] | null>(null);
  const [drafts, setDrafts] = useState<Record<number, string>>({});
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  function show(list: RealmName[]) {
    setRealms(list);
    setDrafts(Object.fromEntries(list.map(r => [r.id, r.name])));
  }

  useEffect(() => {
    let alive = true;
    void api.realmNames(serverId).then(list => { if (alive) show(list); }).catch(e => { if (alive) setError(asUiError(e).technical); });
    return () => { alive = false; };
  }, [serverId]);

  async function rename(id: number, name: string | null) {
    setBusy(true); setError(null); setSaved(null);
    try { show(await api.realmRename(serverId, id, name)); setSaved(id); }
    catch (e) { setError(asUiError(e).technical); }
    finally { setBusy(false); }
  }

  return <Card className="mt-6 p-6" aria-busy={busy}>
    <h2 className="font-semibold">{t("realm.namesTitle")}</h2>
    <p className="mt-1 text-sm text-muted">{t("realm.namesHint")}</p>
    {realms?.map(r => {
      const draft = drafts[r.id] ?? r.name;
      const invalid = draft.trim().length === 0 || draft.trim().length > MAX_CHARS || draft.includes('"');
      return <div key={r.id} className="mt-4">
        <div className="flex flex-wrap items-center gap-2">
          <input
            className="min-w-0 flex-1 rounded border border-line bg-bg px-3 py-1.5 text-sm text-ink outline-none focus:border-gold"
            value={draft}
            maxLength={MAX_CHARS}
            aria-label={t("realm.nameDefault", { name: r.defaultName })}
            disabled={busy}
            onChange={e => setDrafts(d => ({ ...d, [r.id]: e.target.value }))}
          />
          <Button size="sm" disabled={busy || invalid || draft.trim() === r.name} onClick={() => void rename(r.id, draft)}>{t("realm.nameSave")}</Button>
          {r.custom && <Button size="sm" variant="ghost" disabled={busy} onClick={() => void rename(r.id, null)}>{t("realm.nameReset")}</Button>}
        </div>
        <p className="mt-1 text-xs text-muted">{t("realm.nameDefault", { name: r.defaultName })}{saved === r.id ? ` · ${t("realm.nameSaved")}` : ""}</p>
        {draft.includes('"') && <p className="mt-1 text-xs text-warn">{t("realm.nameQuotes")}</p>}
      </div>;
    })}
    {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
  </Card>;
}
