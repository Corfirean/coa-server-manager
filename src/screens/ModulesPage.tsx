import { useCallback, useEffect, useState } from "react";
import { ExternalLink, Loader2 } from "lucide-react";
import { api, asUiError, type ModuleSetting, type ModuleView, type UiError } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useI18n } from "@/i18n";

/** The server's optional modules: switch each on or off, open its page on GitHub, change its settings. */
export function ModulesPage({ serverId }: { serverId: string }) {
  const { t, locale } = useI18n();
  const human = useHuman();
  const [modules, setModules] = useState<ModuleView[] | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [changed, setChanged] = useState(false);

  const load = useCallback(async () => {
    try {
      setModules(await api.modulesList(serverId));
    } catch (e) {
      setError(asUiError(e));
    }
  }, [serverId]);

  useEffect(() => {
    void load();
  }, [load]);

  async function toggle(m: ModuleView) {
    setError(null);
    try {
      await api.moduleSetEnabled(serverId, m.id, !m.enabled);
      setChanged(true);
      await load();
    } catch (e) {
      setError(asUiError(e));
    }
  }

  return (
    <div className="max-w-3xl">
      <h1 className="text-2xl font-semibold">{t("mod.title")}</h1>
      <p className="mt-1 text-muted">{t("q.modules")}</p>
      <p className="mt-3 text-sm text-muted">{t("mod.intro")}</p>
      {changed && <p className="mt-3 rounded-md border border-gold/40 bg-gold/5 px-3 py-2 text-sm" role="status">{t("mod.restart")}</p>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}

      <div className="mt-6 space-y-4">
        {modules === null && !error && <p className="text-muted">{t("overview.checking")}</p>}
        {modules?.length === 0 && <Card className="p-6 text-center text-muted">{t("mod.empty")}</Card>}
        {modules?.map((m) => (
          <Card key={m.id} className={cn("p-5", !m.installed && "opacity-60")}>
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                <h2 className="font-semibold">{m.name}</h2>
                <p className="mt-1 text-sm text-muted">{m.description[locale] ?? m.description.en}</p>
              </div>
              {m.installed ? (
                <button
                  role="switch"
                  aria-checked={m.enabled}
                  aria-label={t("mod.switch", { name: m.name })}
                  onClick={() => void toggle(m)}
                  className={cn("relative mt-1 h-6 w-11 shrink-0 cursor-pointer rounded-full transition-colors", m.enabled ? "bg-gold" : "bg-white/15")}
                >
                  <span className={cn("absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all", m.enabled ? "left-[22px]" : "left-0.5")} />
                </button>
              ) : (
                <span className="mt-1 shrink-0 text-xs text-muted">{t("mod.notHere")}</span>
              )}
            </div>
            <div className="mt-3 flex flex-wrap gap-2">
              <Button size="sm" variant="ghost" onClick={() => void api.openLink(m.repo)}>
                <ExternalLink className="h-3.5 w-3.5" aria-hidden /> {t("mod.github")}
              </Button>
              {m.installed && (
                <Button size="sm" variant="ghost" aria-expanded={open === m.id} onClick={() => setOpen(open === m.id ? null : m.id)}>
                  {t("mod.settings")}
                </Button>
              )}
            </div>
            {open === m.id && <ModuleSettings serverId={serverId} module={m} onSaved={() => setChanged(true)} />}
          </Card>
        ))}
      </div>
    </div>
  );
}

function ModuleSettings({ serverId, module, onSaved }: { serverId: string; module: ModuleView; onSaved: () => void }) {
  const { t } = useI18n();
  const human = useHuman();
  const [items, setItems] = useState<ModuleSetting[] | null>(null);
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    void api.moduleSettings(serverId, module.id).then((s) => setItems(s)).catch((e) => setError(asUiError(e)));
  }, [serverId, module.id]);

  async function save() {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const done = await api.moduleSaveSettings(serverId, module.id, edits);
      setEdits({});
      setItems(await api.moduleSettings(serverId, module.id));
      if (done.length) {
        onSaved();
        setNote(t("mod.saved", { n: done.length }));
      }
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  const dirty = Object.keys(edits).length > 0;
  return (
    <div className="mt-4 border-t border-line pt-4">
      {items === null && !error && <Loader2 className="h-4 w-4 animate-spin text-muted" aria-hidden />}
      <ul className="divide-y divide-line">
        {items?.map((s) => (
          <li key={s.key} className="py-2.5">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <label htmlFor={`${module.id}-${s.key}`} className="selectable text-sm font-medium">{s.key}</label>
              <input
                id={`${module.id}-${s.key}`}
                value={edits[s.key] ?? s.value}
                onChange={(e) => setEdits((x) => ({ ...x, [s.key]: e.target.value }))}
                className="selectable w-56 rounded-md border border-line bg-bg px-2.5 py-1.5 text-sm outline-none focus:border-gold"
              />
            </div>
            {s.doc && <p className="mt-1 text-xs text-muted">{s.doc}</p>}
            {s.default !== null && (edits[s.key] ?? s.value) !== s.default && (
              <button className="mt-1 cursor-pointer text-xs text-muted underline hover:text-ink" onClick={() => setEdits((x) => ({ ...x, [s.key]: s.default ?? "" }))}>
                {t("mod.default", { v: s.default })}
              </button>
            )}
          </li>
        ))}
      </ul>
      <div className="mt-3 flex items-center gap-3">
        <Button size="sm" variant="primary" disabled={!dirty || busy} onClick={() => void save()}>
          {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {t("acc.save")}
        </Button>
        {note && <span className="text-sm text-ok" role="status">{note}</span>}
      </div>
      {error && <p className="mt-2 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
    </div>
  );
}
