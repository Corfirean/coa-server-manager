import { useEffect, useState } from "react";
import { api, asUiError, type RaceAvailability, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { useHuman, useI18n } from "@/i18n";
import type { UnsavedChangesGuard } from "@/lib/unsavedGuard";

export function RaceAvailabilitySettings({ serverId, blocked, onSaved, onRegisterGuard }: {
  serverId: string;
  blocked: boolean;
  onSaved: () => void;
  onRegisterGuard: (guard: UnsavedChangesGuard | null) => void;
}) {
  const { t } = useI18n();
  const human = useHuman();
  const [data, setData] = useState<RaceAvailability | null>(null);
  const [disabled, setDisabled] = useState<number[]>([]);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const canonical = (ids: number[]) => [...ids].sort((a, b) => a - b).join(",");
  const dirty = !!data && canonical(disabled) !== canonical(data.disabledRaceIds);
  const locked = blocked || busy || !data?.serverConfigSupported;

  useEffect(() => {
    let active = true;
    setData(null);
    setDisabled([]);
    setError(null);
    setSaved(false);
    void api.raceAvailability(serverId).then(result => {
      if (active) { setData(result); setDisabled(result.disabledRaceIds); }
    }).catch(reason => { if (active) setError(asUiError(reason)); });
    return () => { active = false; };
  }, [serverId]);

  async function save(): Promise<boolean> {
    if (!data || locked) return false;
    setBusy(true);
    setError(null);
    try {
      const result = await api.raceAvailabilitySave(serverId, data.catalogSha256, data.realm, disabled);
      setData(result);
      setDisabled(result.disabledRaceIds);
      setSaved(true);
      onSaved();
      return true;
    } catch (reason) {
      setError(asUiError(reason));
      return false;
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    onRegisterGuard(dirty ? {
      isDirty: () => dirty,
      save,
      discard: () => { setDisabled(data?.disabledRaceIds ?? []); setSaved(false); },
    } : null);
    return () => onRegisterGuard(null);
  }, [dirty, disabled, data, busy, blocked, serverId, onRegisterGuard]);

  return (
    <section className="mt-3 space-y-3 border-t border-line/40 pt-3" aria-label={t("races.available")}>
      <h3 className="text-sm font-semibold">{t("races.available")}</h3>
      {error && <p role="alert" className="text-xs text-bad">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
      {data && <>
        <p className="text-xs text-muted">{t("races.count", { allowed: data.races.length - disabled.length, disabled: disabled.length })}</p>
        {!data.serverConfigSupported && <p className="text-xs text-warn">{t("races.updateServer")}</p>}
        {data.catalogNeedsReview && <p className="text-xs text-warn">{t("races.reviewCatalog")}</p>}
        {data.unrecognizedRaceIds.length > 0 && <p className="text-xs text-warn">{t("races.unknownIds", { ids: data.unrecognizedRaceIds.join(", ") })}</p>}
        <input value={query} onChange={event => setQuery(event.target.value)} placeholder={t("set.search")}
          aria-label={t("set.search")} className="w-full rounded border border-line bg-bg px-2 py-1 text-sm" />
        <div className="grid gap-3 sm:grid-cols-2">
          {(["horde", "alliance", null] as const).map(faction => {
            const races = data.races.filter(race => race.faction === faction);
            if (faction === null && !races.length) return null;
            const title = t(faction === "horde" ? "races.horde" : faction === "alliance" ? "races.alliance" : "races.available");
            return (
              <div key={faction ?? "other"} className={faction === null ? "sm:col-span-2" : undefined}>
                <div className="max-h-52 overflow-auto rounded-md border border-line/60">
                  <table className="w-full text-left text-xs" aria-label={title}>
                    <thead className="sticky top-0 bg-bg text-muted">
                      <tr><th scope="col" className="w-12 px-3 py-2 font-medium">ID</th>
                        <th scope="col" className="px-3 py-2 font-medium text-ink">{title}</th>
                        <th scope="col" className="w-10 px-3 py-2"><span className="sr-only">{t("races.allowCreation")}</span></th></tr>
                    </thead>
                    <tbody className="divide-y divide-line/40">
                      {races.filter(race => `${race.name} ${race.id}`.toLowerCase().includes(query.trim().toLowerCase())).map(race => (
                        <tr key={race.id} className="hover:bg-line/20">
                          <td className="px-3 py-1.5 tabular-nums text-muted">{race.id}</td>
                          <td className="px-3 py-1.5">{race.name}</td>
                          <td className="px-3 py-1.5 text-right">
                            <input type="checkbox" checked={!disabled.includes(race.id)} disabled={locked}
                              aria-label={`${race.name} (${race.id}): ${t("races.allowCreation")}`}
                              onChange={event => { setSaved(false); setDisabled(previous => event.target.checked ? previous.filter(id => id !== race.id) : [...previous, race.id]); }} />
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            );
          })}
        </div>
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="ghost" disabled={locked} onClick={() => { setDisabled([]); setSaved(false); }}>{t("races.allowAll")}</Button>
          <Button size="sm" variant="primary" disabled={locked || (!dirty && !data.catalogNeedsReview)} onClick={() => void save()}>{t("set.save")}</Button>
          <Button size="sm" variant="ghost" disabled={busy || !dirty} onClick={() => { setDisabled(data.disabledRaceIds); setSaved(false); }}>{t("set.discard")}</Button>
        </div>
        <p className="text-xs text-muted">{t(saved ? "races.saved" : "races.restart")}</p>
        <p className="text-xs text-muted">{t("races.clientPending")}</p>
        <p className="text-xs text-muted">{t("races.existing")}</p>
      </>}
    </section>
  );
}
