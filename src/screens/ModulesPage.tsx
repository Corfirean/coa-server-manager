import { Fragment, useCallback, useEffect, useRef, useState, type ComponentType, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { Bot, Flame, Gavel, Loader2, Plug, Puzzle, Scaling, Settings2, Snowflake, Sparkles, Swords, Users, type LucideProps } from "lucide-react";
import { api, asUiError, type CustomRacesClientStatus, type CustomRacesProgress, type ModuleSetting, type ModuleView, type UiError } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useI18n, useSchemaText, type Key } from "@/i18n";
import { auctionWeightKey, canonicalModuleValue, moduleEnableKey, moduleField } from "@/lib/moduleSettings";
import { AuctionTypes } from "@/screens/AuctionTypes";

/** The GitHub mark (lucide dropped its brand icons). */
function GithubMark(props: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden className={props.className}>
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.6 7.6 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8z" />
    </svg>
  );
}

/** Icon and colours of a module's square tile, by the name the catalog gives it. */
const TILES: Record<string, { icon: ComponentType<LucideProps>; from: string; to: string }> = {
  bot: { icon: Bot, from: "from-emerald-500", to: "to-teal-700" },
  scaling: { icon: Scaling, from: "from-violet-500", to: "to-indigo-700" },
  plug: { icon: Plug, from: "from-amber-500", to: "to-orange-700" },
  tbc: { icon: Flame, from: "from-lime-500", to: "to-emerald-800" },
  wotlk: { icon: Snowflake, from: "from-sky-400", to: "to-blue-700" },
  gavel: { icon: Gavel, from: "from-yellow-500", to: "to-amber-700" },
  sparkles: { icon: Sparkles, from: "from-fuchsia-500", to: "to-purple-700" },
  swords: { icon: Swords, from: "from-rose-500", to: "to-red-800" },
  races: { icon: Users, from: "from-cyan-500", to: "to-blue-800" },
};
const DEFAULT_TILE = { icon: Puzzle, from: "from-slate-500", to: "to-slate-700" };

const STATUS: Record<string, { label: Key; hint: Key; tone: string }> = {
  soon: { label: "mod.status.soon", hint: "mod.hint.soon", tone: "border-violet-400/50 bg-violet-400/10 text-violet-300" },
  early: { label: "mod.status.early", hint: "mod.hint.early", tone: "border-warn/50 bg-warn/10 text-warn" },
  beta: { label: "mod.status.beta", hint: "mod.hint.beta", tone: "border-sky-400/50 bg-sky-400/10 text-sky-300" },
  release: { label: "mod.status.release", hint: "mod.hint.release", tone: "border-ok/50 bg-ok/10 text-ok" },
};

function SettingsPanel({ className, children }: { className?: string; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, []);
  return (
    <div ref={ref} className={className}>
      <Card className="border-gold/50 p-5">{children}</Card>
    </div>
  );
}

function StatusBadge({ status }: { status: string }) {
  const { t } = useI18n();
  const s = STATUS[status] ?? STATUS.release;
  return (
    <span className={cn("shrink-0 rounded-full border px-2 py-0.5 text-[11px] font-medium uppercase tracking-wide", s.tone)} title={t(s.hint)}>
      {t(s.label)}
    </span>
  );
}

function Tile({ icon, off }: { icon: string; off: boolean }) {
  const tile = TILES[icon] ?? DEFAULT_TILE;
  const Icon = tile.icon;
  return (
    <div
      aria-hidden
      className={cn(
        "flex h-16 w-16 shrink-0 items-center justify-center rounded-2xl bg-gradient-to-br shadow-lg ring-1 ring-white/10 transition",
        tile.from,
        tile.to,
        off && "opacity-40 grayscale",
      )}
    >
      <Icon className="h-8 w-8 text-white drop-shadow" strokeWidth={1.75} />
    </div>
  );
}

/** The server's optional modules: square cards with a switch, the module's status, a link to GitHub and its settings. */
export function ModulesPage({ serverId, onChanged }: { serverId: string; onChanged?: () => void }) {
  const { t, locale } = useI18n();
  const human = useHuman();
  const [modules, setModules] = useState<ModuleView[] | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [changed, setChanged] = useState(false);
  const [racesPatchStatus, setRacesPatchStatus] = useState<CustomRacesClientStatus | null>(null);
  const [racesInstalling, setRacesInstalling] = useState(false);
  const [racesProgress, setRacesProgress] = useState<CustomRacesProgress | null>(null);

  const loadRacesStatus = useCallback(async () => {
    try {
      const st = await api.customRacesStatus(serverId);
      setRacesPatchStatus(st);
    } catch {
      // ignore
    }
  }, [serverId]);

  const load = useCallback(async () => {
    try {
      setModules((await api.modulesList(serverId)).filter((m) => !m.hidden));
      await loadRacesStatus();
    } catch (e) {
      setError(asUiError(e));
    }
  }, [serverId, loadRacesStatus]);

  useEffect(() => {
    void load();
    const un = listen<CustomRacesProgress>("custom-races-progress", (e) => {
      if (e.payload.id === serverId) {
        setRacesProgress(e.payload);
      }
    });
    return () => {
      void un.then((f) => f());
    };
  }, [load, serverId]);

  async function installClientPatch() {
    setRacesInstalling(true);
    setError(null);
    try {
      await api.customRacesInstall(serverId);
      await loadRacesStatus();
      await load();
      onChanged?.();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setRacesInstalling(false);
      setRacesProgress(null);
    }
  }

  async function toggle(m: ModuleView) {
    setError(null);
    try {
      await api.moduleSetEnabled(serverId, m.id, !m.enabled);
      setChanged(true);
      await load();
      if (m.id === "custom-races") {
        await loadRacesStatus();
      }
      onChanged?.();
    } catch (e) {
      setError(asUiError(e));
    }
  }

  const selected = modules?.find((m) => m.id === open) ?? null;
  const selectedAt = modules?.findIndex((m) => m.id === open) ?? -1;

  return (
    <div className="max-w-4xl">
      <h1 className="text-2xl font-semibold">{t("mod.title")}</h1>
      <p className="mt-1 text-muted">{t("q.modules")}</p>
      <p className="mt-3 text-sm text-muted">{t("mod.intro")}</p>

      {changed && <p className="mt-4 rounded-md border border-gold/40 bg-gold/5 px-3 py-2 text-sm" role="status">{t("mod.restart")}</p>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}

      <div className="mt-6 grid gap-4 sm:grid-cols-2">
        {modules === null && !error && <p className="text-muted">{t("overview.checking")}</p>}
        {modules?.length === 0 && <Card className="p-6 text-center text-muted sm:col-span-2">{t("mod.empty")}</Card>}
        {modules?.map((m, i) => (
          <Fragment key={m.id}>
          <Card className={cn("flex flex-col gap-4 p-5 transition-colors", (!m.installed || m.status === "soon") && "opacity-60", open === m.id && "border-gold/50")} aria-label={m.status === "soon" ? `${m.name}: ${t("mod.status.soon")}` : undefined}>
            <div className="flex items-start gap-4">
              <Tile icon={m.icon} off={!m.enabled || !m.installed || m.status === "soon"} />
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  <h2 className="font-semibold leading-tight">{m.name}</h2>
                  {m.version && <span className="ml-2 text-xs font-normal text-muted">{m.version}</span>}
                  <StatusBadge status={m.status} />
                </div>
                <p className="mt-1.5 line-clamp-2 text-sm text-muted">{m.description[locale] ?? m.description.en}</p>
                {m.compatibility === "unsupported" && <p className="mt-2 text-xs text-warn">{t("realm.unsupported")}</p>}
                {m.compatibility === "experimental" && <p className="mt-2 text-xs text-warn">{t("realm.experimental")}</p>}
              </div>
              {m.installed && m.switchable && (
                <button
                  role="switch"
                  aria-checked={m.enabled}
                  aria-label={t("mod.switch", { name: m.name })}
                  disabled={m.id === "custom-races" && racesInstalling}
                  onClick={() => void toggle(m)}
                  className={cn("relative mt-1 h-6 w-11 shrink-0 cursor-pointer rounded-full transition-colors", m.enabled ? "bg-gold" : "bg-white/15", (m.id === "custom-races" && racesInstalling) && "opacity-50 cursor-not-allowed")}
                >
                  <span className={cn("absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all", m.enabled ? "left-[22px]" : "left-0.5")} />
                </button>
              )}
              {m.installed && !m.switchable && m.status !== "soon" && m.compatibility !== "unsupported" && <span className="mt-1.5 shrink-0 text-xs text-muted">{t("mod.alwaysOn")}</span>}
              {!m.installed && m.status !== "soon" && <span className="mt-1.5 shrink-0 text-xs text-muted">{t("mod.notHere")}</span>}
            </div>
            {m.id === "custom-races" && (
              <div className="w-full">
                {racesInstalling && racesProgress ? (
                  <div className="rounded-lg border border-gold/30 bg-gold/5 p-3">
                    <div className="flex items-center justify-between gap-3 text-xs">
                      <span className="flex min-w-0 items-center gap-2 font-medium text-ink">
                        <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-gold" />
                        <span className="truncate">{racesProgress.step}</span>
                      </span>
                      <span className="shrink-0 font-mono text-xs font-semibold text-gold">
                        {Math.round(racesProgress.percent * 100)}%
                      </span>
                    </div>
                    <div className="mt-2.5 h-2 w-full overflow-hidden rounded-full bg-white/10" role="progressbar" aria-valuenow={Math.round(racesProgress.percent * 100)} aria-valuemin={0} aria-valuemax={100}>
                      <div
                        className="h-full rounded-full bg-gold transition-all duration-300"
                        style={{ width: `${Math.round(racesProgress.percent * 100)}%` }}
                      />
                    </div>
                  </div>
                ) : (
                  <div className="flex flex-wrap items-center justify-between gap-2 border-t border-line/40 pt-3 text-xs">
                    <div className="flex items-center gap-2">
                      {racesPatchStatus?.state === "enabled" && (
                        <span className="inline-flex items-center gap-1.5 rounded-full bg-ok/10 px-2.5 py-1 text-xs text-ok border border-ok/30">
                          <span className="h-1.5 w-1.5 rounded-full bg-ok" />
                          {locale === "ru" ? "Патч клиента: Включен" : "Client Patch: Active"}
                        </span>
                      )}
                      {racesPatchStatus?.state === "disabled" && (
                        <span className="inline-flex items-center gap-1.5 rounded-full bg-white/5 px-2.5 py-1 text-xs text-muted border border-white/15" title="MPQ files disabled with .disabled suffix">
                          <span className="h-1.5 w-1.5 rounded-full bg-muted" />
                          {locale === "ru" ? "Патч клиента: Отключен (.disabled)" : "Client Patch: Disabled (.disabled)"}
                        </span>
                      )}
                      {racesPatchStatus?.state === "not-installed" && (
                        <span className="inline-flex items-center gap-1.5 rounded-full bg-warn/10 px-2.5 py-1 text-xs text-warn border border-warn/30">
                          <span className="h-1.5 w-1.5 rounded-full bg-warn" />
                          {locale === "ru" ? "Патч клиента: Не установлен" : "Client Patch: Not Installed"}
                        </span>
                      )}
                      {racesPatchStatus?.state === "partially-installed" && (
                        <span className="inline-flex items-center gap-1.5 rounded-full bg-warn/10 px-2.5 py-1 text-xs text-warn border border-warn/30">
                          <span className="h-1.5 w-1.5 rounded-full bg-warn" />
                          {locale === "ru" ? `Патч клиента: ${racesPatchStatus.installedMpqs}/${racesPatchStatus.totalMpqs} MPQ` : `Client Patch: ${racesPatchStatus.installedMpqs}/${racesPatchStatus.totalMpqs} MPQ`}
                        </span>
                      )}
                    </div>
                    {(!racesPatchStatus || racesPatchStatus.state === "not-installed" || racesPatchStatus.state === "partially-installed") && (
                      <Button
                        size="sm"
                        variant="secondary"
                        className="h-7 px-3 text-xs border border-gold/40 hover:border-gold"
                        disabled={racesInstalling}
                        onClick={() => void installClientPatch()}
                      >
                        {racesInstalling && <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />}
                        {locale === "ru" ? "Установить патч клиента" : "Install Client Patch"}
                      </Button>
                    )}
                  </div>
                )}
              </div>
            )}
            <div className="mt-auto flex items-center gap-2">
              <button
                onClick={() => void api.openLink(m.repo)}
                title={t("mod.github")}
                aria-label={`${t("mod.github")}: ${m.name}`}
                className="flex h-8 w-8 cursor-pointer items-center justify-center rounded-md border border-line text-muted transition-colors hover:border-gold/50 hover:text-ink"
              >
                <GithubMark className="h-4 w-4" />
              </button>
              {!m.page && m.installed && m.has_settings && m.status !== "soon" && m.compatibility !== "unsupported" && (
                <Button size="sm" variant="ghost" aria-expanded={open === m.id} onClick={() => setOpen(open === m.id ? null : m.id)}>
                  <Settings2 className="h-3.5 w-3.5" aria-hidden /> {t("mod.settings")}
                </Button>
              )}
            </div>
          </Card>
          {/* The settings open right under the row of the card they belong to, so they are never out of sight below a long grid. */}
          {selected?.has_settings && !selected.page && selectedAt >= 0 && Math.floor(selectedAt / 2) === Math.floor(i / 2) && (i % 2 === 1 || i === modules.length - 1) && (
            <SettingsPanel key={selected.id} className="sm:col-span-2">
              <h2 className="font-semibold">{t("mod.settingsOf", { name: selected.name })}</h2>
              <ModuleSettings serverId={serverId} module={selected} onSaved={() => setChanged(true)} />
            </SettingsPanel>
          )}
          </Fragment>
        ))}
      </div>
    </div>
  );
}

function ModuleSettings({ serverId, module, onSaved }: { serverId: string; module: ModuleView; onSaved: () => void }) {
  const { t, locale } = useI18n();
  const sx = useSchemaText();
  const human = useHuman();
  const [advanced, setAdvanced] = useState(false);
  const [items, setItems] = useState<ModuleSetting[] | null>(null);
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});

  useEffect(() => {
    let cancelled = false;
    setItems(null);
    setEdits({});
    setNote(null);
    setError(null);
    setFieldErrors({});
    void api.moduleSettings(serverId, module.id).then((s) => { if (!cancelled) setItems(s); }).catch((e) => { if (!cancelled) setError(asUiError(e)); });
    return () => { cancelled = true; };
  }, [serverId, module.id]);

  async function save() {
    const errors: Record<string, string> = {};
    for (const [key, value] of Object.entries(edits)) {
      const field = moduleField(key, locale, items?.find(item => item.key === key)?.field);
      if (field && ["int", "float", "range"].includes(field.type)) {
        const number = Number(value);
        const message = !value.trim() || !Number.isFinite(number) ? t("set.enterNumber")
          : field.type === "int" && !Number.isInteger(number) ? t("set.enterInt")
          : field.min !== undefined && number < field.min ? t("set.atLeast", { n: field.min })
          : field.max !== undefined && number > field.max ? t("set.atMost", { n: field.max }) : null;
        if (message) errors[key] = message;
      }
    }
    setFieldErrors(errors);
    if (Object.keys(errors).length) return;
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
  const edit = (key: string, value: string) => setEdits(previous => {
    const next = { ...previous };
    if (value === items?.find(item => item.key === key)?.value) delete next[key];
    else next[key] = value;
    return next;
  });
  const available = items?.filter(s => s.key !== moduleEnableKey(module.id)) ?? [];
  const advancedCount = available.filter(s => !auctionWeightKey(s.key) && (!moduleField(s.key, locale, s.field) || moduleField(s.key, locale, s.field)?.advanced)).length;
  const visible = available.filter(s => {
    if (module.id === "ah-bot" && auctionWeightKey(s.key)) return false;
    const field = moduleField(s.key, locale, s.field);
    if (!advanced && !query.trim() && (!field || field.advanced)) return false;
    const title = field ? (s.field ? field.title : sx.title({ key: s.key, title: field.title })) : s.key;
    const description = field ? (s.field ? field.description : sx.description({ key: s.key, description: field.description })) : s.doc;
    return `${title} ${description} ${s.key}`.toLocaleLowerCase(locale).includes(query.trim().toLocaleLowerCase(locale));
  });
  return (
    <div className="mt-3 border-t border-line pt-3">
      {items === null && !error && <Loader2 className="h-4 w-4 animate-spin text-muted" aria-hidden />}
      {module.id === "npc-enchanter" && (
        <div className="mb-4 space-y-2 text-sm text-muted">
          <p>{t("mod.enchanter.add")}</p>
          <p>{t("mod.enchanter.remove")}</p>
        </div>
      )}
      {module.id === "ah-bot" && (
        <div className="mb-4 space-y-2 text-sm text-muted">
          <p>{t("mod.ahbot.setup")}</p>
        </div>
      )}
      {available.length >= 10 && <input value={query} onChange={e => setQuery(e.target.value)} aria-label={t("set.search")} placeholder={t("set.search")} className="mb-3 w-full rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold" />}
      {advancedCount > 0 && <label className="mb-3 flex items-center gap-2 text-sm text-muted"><input type="checkbox" checked={advanced} onChange={e => setAdvanced(e.target.checked)} />{t("set.advanced", { n: advancedCount })}</label>}
      {module.id === "ah-bot" && <AuctionTypes items={available} edits={edits} busy={busy} query={query} errors={fieldErrors} onChange={changes => setEdits(previous => {
        const next = { ...previous };
        for (const [key, value] of Object.entries(changes)) {
          if (value === items?.find(item => item.key === key)?.value) delete next[key];
          else next[key] = value;
        }
        return next;
      })} />}
      {items && !visible.length && !available.some(s => module.id === "ah-bot" && auctionWeightKey(s.key)) && <p className="py-3 text-sm text-muted">{t(available.length ? "all.none" : "mod.noSettings")}</p>}
      <ul className="divide-y divide-line">
        {visible.map((s) => {
          const field = moduleField(s.key, locale, s.field);
          const title = field ? (s.field ? field.title : sx.title({ key: s.key, title: field.title })) : s.key;
          const description = field ? (s.field ? field.description : sx.description({ key: s.key, description: field.description })) : s.doc;
          const value = edits[s.key] ?? s.value;
          const normalized = canonicalModuleValue(s.key, value);
          const options = field?.options ?? [];
          const bool = field?.type === "bool" || /^(true|false)$/i.test(normalized) || (/^(0|1)$/.test(normalized) && /(?:\.Enable(?:d)?$|Enable\/Disable)/i.test(s.key + " " + s.doc));
          const on = /^(1|true|yes|on)$/i.test(normalized);
          const id = `${module.id}-${s.key}`;
          const controlClass = "selectable w-56 rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold";
          return (
          <li key={s.key} className="py-2.5">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="selectable text-sm font-medium"><label htmlFor={id}>{title}</label>{s.key in edits && <span className="ml-2 text-xs text-gold">{t("set.changed")}</span>}</div>
              {bool ? <button id={id} role="switch" aria-checked={on} disabled={busy} onClick={() => edit(s.key, /^(true|false)$/i.test(canonicalModuleValue(s.key, s.value)) ? String(!on) : on ? "0" : "1")} className={cn("relative h-7 w-14 cursor-pointer rounded-full", on ? "bg-gold" : "bg-white/15")}><span className={cn("absolute left-1 top-1 h-5 w-5 rounded-full bg-white transition-transform", on && "translate-x-7")} /><span className="sr-only">{t(on ? "set.on" : "set.off")}</span></button>
                : options.length ? <select id={id} value={normalized} disabled={busy} onChange={e => edit(s.key, e.target.value)} className={controlClass}>{!options.some(o => String(o.value) === normalized) && <option value={normalized}>{normalized}</option>}{options.map(o => <option key={String(o.value)} value={String(o.value)}>{sx.option(s.key, o)}</option>)}</select>
                : field?.type === "range" ? <div className="flex w-56 items-center gap-3"><input id={id} type="range" min={field.min} max={field.max} step={field.step} value={Number(normalized)} disabled={busy} onChange={e => edit(s.key, e.target.value)} aria-valuetext={`${Math.round(Number(normalized) * 100)}%`} className="min-w-0 flex-1 accent-gold" /><output htmlFor={id} className="w-12 text-right text-sm">{Math.round(Number(normalized) * 100)}%</output></div>
                : <input id={id} type={field?.type === "int" || field?.type === "float" ? "number" : "text"} min={field?.min} max={field?.max} step={field?.type === "float" ? "any" : 1} value={value} disabled={busy} onChange={e => edit(s.key, e.target.value)} className={controlClass} />}
            </div>
            {description && <p className="mt-1 text-sm text-muted">{description}</p>}
            {fieldErrors[s.key] && <p className="mt-1 text-sm text-bad" role="alert">{fieldErrors[s.key]}</p>}
            {advanced && <p className="mt-1 selectable text-xs text-muted/70">{s.key}</p>}
            {s.default !== null && (edits[s.key] ?? s.value) !== s.default && (
              <button disabled={busy} className="mt-1 cursor-pointer text-xs text-muted underline hover:text-ink" onClick={() => edit(s.key, s.default ?? "")}>
                {t("mod.default", { v: s.default })}
              </button>
            )}
          </li>
        ); })}
      </ul>
      {available.length > 0 && <div className="mt-3 flex items-center gap-3">
        <Button size="sm" variant="primary" disabled={!dirty || busy} onClick={() => void save()}>
          {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {t("acc.save")}
        </Button>
        {dirty && <Button size="sm" variant="ghost" disabled={busy} onClick={() => { setEdits({}); setFieldErrors({}); setNote(null); }}>{t("set.discard")}</Button>}
        {note && <span className="text-sm text-ok" role="status">{note}</span>}
      </div>}
      {error && <p className="mt-2 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
    </div>
  );
}
