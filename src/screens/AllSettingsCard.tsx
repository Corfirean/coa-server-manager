import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Loader2, Search } from "lucide-react";
import { api, asUiError, type AllSetting, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";

/** How many matches are drawn at once; the rest is reached by searching. */
const SHOWN = 40;

/** Every documented setting of the world server, searchable. For people who want more than the curated list above. */
export function AllSettingsCard({ serverId }: { serverId: string }) {
  const t = useT();
  const human = useHuman();
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<AllSetting[] | null>(null);
  const [query, setQuery] = useState("");
  const [onlyChanged, setOnlyChanged] = useState(false);
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    if (!open || items) return;
    void api.allSettings(serverId).then(setItems).catch((e) => setError(asUiError(e)));
  }, [open, items, serverId]);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (items ?? []).filter((s) => (!onlyChanged || s.changed) && (!q || s.key.toLowerCase().includes(q) || s.doc.toLowerCase().includes(q)));
  }, [items, query, onlyChanged]);

  const dirty = Object.keys(edits);

  async function save() {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const done = await api.allSettingsSave(serverId, edits);
      setEdits({});
      setItems(await api.allSettings(serverId));
      setNote(done.length ? t("all.saved") : t("set.nothingChanged"));
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className="mt-8 p-5">
      <button onClick={() => setOpen((v) => !v)} aria-expanded={open} className="flex w-full cursor-pointer items-center gap-2 text-left">
        {open ? <ChevronDown className="h-4 w-4" aria-hidden /> : <ChevronRight className="h-4 w-4" aria-hidden />}
        <span className="font-semibold">{t("all.title")}</span>
      </button>
      <p className="mt-2 text-sm text-muted">{t("all.intro")}</p>

      {open && (
        <div className="mt-4">
          {items === null && !error && <Loader2 className="h-4 w-4 animate-spin text-muted" aria-hidden />}
          {items && (
            <>
              <div className="flex flex-wrap items-center gap-3">
                <label className="relative block min-w-[16rem] flex-1">
                  <Search className="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-muted" aria-hidden />
                  <input
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    placeholder={t("all.search")}
                    aria-label={t("all.search")}
                    className="selectable w-full rounded-md border border-line bg-bg py-2 pl-8 pr-3 text-sm outline-none focus:border-gold"
                  />
                </label>
                <label className="flex cursor-pointer items-center gap-2 text-sm">
                  <input type="checkbox" checked={onlyChanged} onChange={(e) => setOnlyChanged(e.target.checked)} />
                  {t("all.onlyChanged")}
                </label>
              </div>
              <p className="mt-2 text-xs text-muted">{t("all.count", { shown: Math.min(matches.length, SHOWN), total: matches.length })}</p>

              {matches.length === 0 && <p className="mt-4 text-sm text-muted">{t("all.none")}</p>}
              <ul className="mt-2 divide-y divide-line">
                {matches.slice(0, SHOWN).map((s) => {
                  const value = edits[s.key] ?? s.value;
                  return (
                    <li key={s.key} className="py-3">
                      <div className="flex flex-wrap items-center justify-between gap-3">
                        <label htmlFor={`all-${s.key}`} className="selectable text-sm font-medium">
                          {s.key}
                          {s.changed && <span className="ml-2 rounded bg-gold/15 px-1.5 py-0.5 text-xs text-gold">{t("all.changed")}</span>}
                        </label>
                        <input
                          id={`all-${s.key}`}
                          value={value}
                          onChange={(e) => {
                            const v = e.target.value;
                            setEdits((x) => {
                              const n = { ...x };
                              if (v === s.value) delete n[s.key];
                              else n[s.key] = v;
                              return n;
                            });
                          }}
                          className="selectable w-56 rounded-md border border-line bg-bg px-2.5 py-1.5 text-sm outline-none focus:border-gold"
                        />
                      </div>
                      <p className="selectable mt-1 text-xs text-muted">{s.doc}</p>
                      {value !== s.default && (
                        <button
                          className="mt-1 cursor-pointer text-xs text-muted underline hover:text-ink"
                          onClick={() =>
                            setEdits((x) => {
                              const n = { ...x };
                              if (s.default === s.value) delete n[s.key];
                              else n[s.key] = s.default;
                              return n;
                            })
                          }
                        >
                          {t("all.useDefault", { v: s.default })}
                        </button>
                      )}
                    </li>
                  );
                })}
              </ul>

              <div className="mt-4 flex items-center gap-3">
                <Button size="sm" variant="primary" disabled={dirty.length === 0 || busy} onClick={() => void save()}>
                  {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
                  {t("all.save", { n: dirty.length })}
                </Button>
                {dirty.length > 0 && (
                  <Button size="sm" variant="ghost" onClick={() => setEdits({})}>
                    {t("set.discard")}
                  </Button>
                )}
                {note && <span className="text-sm text-ok" role="status">{note}</span>}
              </div>
            </>
          )}
          {error && <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
        </div>
      )}
    </Card>
  );
}
