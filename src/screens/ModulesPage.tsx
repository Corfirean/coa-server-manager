import { Fragment, useCallback, useEffect, useRef, useState, type ComponentType, type ReactNode } from "react";
import { Bot, Flame, Gavel, Loader2, Plug, Puzzle, Scaling, Settings2, Snowflake, Sparkles, Swords, type LucideProps } from "lucide-react";
import { api, asUiError, type ModuleSetting, type ModuleView, type UiError } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useI18n, type Key } from "@/i18n";

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

  const load = useCallback(async () => {
    try {
      setModules((await api.modulesList(serverId)).filter((m) => !m.hidden));
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
                  onClick={() => void toggle(m)}
                  className={cn("relative mt-1 h-6 w-11 shrink-0 cursor-pointer rounded-full transition-colors", m.enabled ? "bg-gold" : "bg-white/15")}
                >
                  <span className={cn("absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all", m.enabled ? "left-[22px]" : "left-0.5")} />
                </button>
              )}
              {m.installed && !m.switchable && m.status !== "soon" && m.compatibility !== "unsupported" && <span className="mt-1.5 shrink-0 text-xs text-muted">{t("mod.alwaysOn")}</span>}
              {!m.installed && m.status !== "soon" && <span className="mt-1.5 shrink-0 text-xs text-muted">{t("mod.notHere")}</span>}
            </div>
            <div className="mt-auto flex items-center gap-2">
              <button
                onClick={() => void api.openLink(m.repo)}
                title={t("mod.github")}
                aria-label={`${t("mod.github")}: ${m.name}`}
                className="flex h-8 w-8 cursor-pointer items-center justify-center rounded-md border border-line text-muted transition-colors hover:border-gold/50 hover:text-ink"
              >
                <GithubMark className="h-4 w-4" />
              </button>
              {m.installed && m.status !== "soon" && m.compatibility !== "unsupported" && (
                <Button size="sm" variant="ghost" aria-expanded={open === m.id} onClick={() => setOpen(open === m.id ? null : m.id)}>
                  <Settings2 className="h-3.5 w-3.5" aria-hidden /> {t("mod.settings")}
                </Button>
              )}
            </div>
          </Card>
          {/* The settings open right under the row of the card they belong to, so they are never out of sight below a long grid. */}
          {selected && selectedAt >= 0 && Math.floor(selectedAt / 2) === Math.floor(i / 2) && (i % 2 === 1 || i === modules.length - 1) && (
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
  const { t } = useI18n();
  const human = useHuman();
  const [items, setItems] = useState<ModuleSetting[] | null>(null);
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    setItems(null);
    setEdits({});
    setNote(null);
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
    <div className="mt-3 border-t border-line pt-3">
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
