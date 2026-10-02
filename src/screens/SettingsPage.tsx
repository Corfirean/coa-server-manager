import { useCallback, useEffect, useMemo, useState } from "react";
import { AlertTriangle, ChevronDown, ChevronRight, RotateCcw, Search } from "lucide-react";
import {
  api,
  asUiError,
  type AllSetting,
  type FieldError,
  type JsonValue,
  type ModuleSetting,
  type PresetInfo,
  type PresetPreview,
  type SaveReport,
  type Scope,
  type SettingsView,
  type SettingView,
  type UiError,
} from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { CollectionRows, RawRows, collectionsOf } from "@/screens/ExtraRows";
import { CompanionsCard } from "@/screens/CompanionsCard";
import { useHuman, useI18n, useSchemaText, useT, type Key } from "@/i18n";

type T = (k: Key, v?: Record<string, string | number>) => string;

const RESTART_LABEL = { runtime: "set.restart.runtime", world: "set.restart.world", full: "set.restart.full" } as const;

type SX = ReturnType<typeof useSchemaText>;

function fmt(t: T, sx: SX, v: JsonValue, s: { key: string; type: string; unit?: string | null; options: { value: JsonValue; label: string }[] }): string {
  if (typeof v === "boolean") return v ? t("set.on") : t("set.off");
  if (s.type === "enum") {
    const o = s.options.find((x) => x.value === v);
    return o ? sx.option(s.key, o) : String(v);
  }
  return s.unit ? `${v} ${sx.unit(s.unit)}` : String(v);
}

function clientCheck(t: T, s: SettingView, v: JsonValue): string | null {
  if (s.type === "int" || s.type === "float") {
    if (typeof v !== "number" || Number.isNaN(v)) return t("set.enterNumber");
    if (s.type === "int" && !Number.isInteger(v)) return t("set.enterInt");
    if (s.min !== undefined && v < s.min) return t("set.atLeast", { n: s.min });
    if (s.max !== undefined && v > s.max) return t("set.atMost", { n: s.max });
  }
  return null;
}

function Field(props: { s: SettingView; value: JsonValue; error: string | null; onChange: (v: JsonValue) => void }) {
  const { s, value, error, onChange } = props;
  const t = useT();
  const sx = useSchemaText();
  const id = `f-${s.key}`;
  const base = "rounded-md border bg-bg px-3 py-2 text-sm outline-none focus:border-gold";
  if (s.type === "bool") {
    return (
      <button
        id={id}
        role="switch"
        aria-checked={value === true}
        onClick={() => onChange(!(value === true))}
        className={cn("relative h-7 w-14 cursor-pointer rounded-full transition-colors", value === true ? "bg-gold" : "bg-white/15")}
      >
        <span className={cn("absolute left-1 top-1 h-5 w-5 rounded-full bg-white transition-transform", value === true && "translate-x-7")} />
        <span className="sr-only">{value === true ? t("set.on") : t("set.off")}</span>
      </button>
    );
  }
  if (s.type === "enum") {
    return (
      <select
        id={id}
        className={cn(base, "border-line")}
        value={String(value)}
        onChange={(e) => onChange(s.options.find((o) => String(o.value) === e.target.value)!.value)}
      >
        {s.options.map((o) => (
          <option key={String(o.value)} value={String(o.value)}>
            {sx.option(s.key, o)}
          </option>
        ))}
      </select>
    );
  }
  const numeric = s.type === "int" || s.type === "float";
  return (
    <div className="flex items-center gap-2">
      <input
        id={id}
        className={cn(base, numeric ? "w-32 text-right" : "w-72", error ? "border-bad" : "border-line")}
        value={String(value)}
        inputMode={numeric ? "decimal" : undefined}
        aria-invalid={!!error}
        onChange={(e) => {
          const raw = e.target.value;
          onChange(numeric ? (raw.trim() === "" || Number.isNaN(Number(raw)) ? (raw as unknown as number) : Number(raw)) : raw);
        }}
      />
      {s.unit && <span className="text-xs text-muted">{sx.unit(s.unit)}</span>}
    </div>
  );
}

function Row(props: { s: SettingView; value: JsonValue; error: string | null; onChange: (v: JsonValue) => void; onReset: () => void }) {
  const { s, value, error } = props;
  const t = useT();
  const sx = useSchemaText();
  const changed = value !== s.value;
  return (
    <div className="grid grid-cols-[1fr_auto] items-start gap-x-6 gap-y-1 py-3.5">
      <div>
        <label htmlFor={`f-${s.key}`} className="flex flex-wrap items-center gap-2 font-medium">
          {sx.title(s)}
          {s.dangerous && (
            <span className="inline-flex items-center gap-1 rounded bg-warn/15 px-1.5 py-0.5 text-[11px] font-normal text-warn">
              <AlertTriangle className="h-3 w-3" aria-hidden /> {t("set.careful")}
            </span>
          )}
          {changed && <span className="rounded bg-gold/15 px-1.5 py-0.5 text-[11px] font-normal text-gold">{t("set.changed")}</span>}
        </label>
        <p className="mt-0.5 max-w-xl text-sm text-muted">{sx.description(s)}</p>
        <p className="mt-1 text-xs text-muted/80">
          {t("set.default", { v: fmt(t, sx, s.default, s) })}
          {s.min !== undefined && s.max !== undefined && s.type !== "bool" ? ` · ${t("set.range", { min: s.min, max: s.max })}` : ""} · {t(RESTART_LABEL[s.restartRequired])}
        </p>
        {s.problem && <p className="mt-1 text-xs text-warn">{t("set.badValue", { problem: s.problem })}</p>}
        {s.drift && <p className="mt-1 text-xs text-warn">{t("set.drift")}</p>}
        {error && (
          <p role="alert" className="mt-1 text-xs text-bad">
            {error}
          </p>
        )}
      </div>
      <div className="flex items-center gap-2 pt-0.5">
        <Field s={s} value={value} error={error} onChange={props.onChange} />
        <button
          title={t("set.reset")}
          aria-label={t("set.resetItem", { name: sx.title(s) })}
          onClick={props.onReset}
          disabled={value === s.default}
          className="cursor-pointer rounded p-1.5 text-muted hover:bg-white/5 hover:text-ink disabled:opacity-25"
        >
          <RotateCcw className="h-4 w-4" aria-hidden />
        </button>
      </div>
    </div>
  );
}

export function SettingsPage(props: { serverId: string; scope: Scope; title: string; question: string }) {
  const { serverId, scope } = props;
  const { t, tn } = useI18n();
  const sx = useSchemaText();
  const human = useHuman();
  const [view, setView] = useState<SettingsView | null>(null);
  const [draft, setDraft] = useState<Record<string, JsonValue>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [fatal, setFatal] = useState<UiError | null>(null);
  const [saveErr, setSaveErr] = useState<UiError | null>(null);
  const [saved, setSaved] = useState<SaveReport | null>(null);
  const [presets, setPresets] = useState<PresetInfo[]>([]);
  const [preview, setPreview] = useState<PresetPreview | null>(null);
  const [category, setCategory] = useState<string>("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [busy, setBusy] = useState(false);
  const [running, setRunning] = useState(false);
  // Beside the curated settings the server page offers the collection switches and every other documented setting. They are
  // searched together and saved with the rest, so the page has one search box, one set of tabs and one save bar.
  const [query, setQuery] = useState("");
  const [raw, setRaw] = useState<AllSetting[] | null>(null);
  const [rawEdits, setRawEdits] = useState<Record<string, string>>({});
  const [onlyChanged, setOnlyChanged] = useState(false);
  const [cols, setCols] = useState<ModuleSetting[]>([]);
  const [colEdits, setColEdits] = useState<Record<string, string>>({});

  const load = useCallback(async () => {
    try {
      const v = await api.settings(serverId, scope);
      setView(v);
      setDraft({});
      setErrors({});
      setCategory((c) => c || v.categories.find((x) => v.settings.some((s) => s.category === x.id && !s.advanced))?.id || v.categories[0].id);
      setFatal(null);
    } catch (e) {
      setFatal(asUiError(e));
    }
  }, [serverId, scope]);

  useEffect(() => {
    setView(null);
    setSaved(null);
    setCategory("");
    void load();
    void api.presets(scope).then(setPresets);
    void api.status(serverId).then((s) => setRunning(s.observed.world.state === "running"));
    setQuery("");
    setRaw(null);
    setRawEdits({});
    setColEdits({});
    setCols([]);
    if (scope === "server") void api.moduleSettings(serverId, "client-compat").then(setCols).catch(() => setCols([]));
  }, [load, scope, serverId]);

  const needRaw = scope === "server" && (category === "@all" || query.trim() !== "");
  useEffect(() => {
    if (needRaw && !raw) void api.allSettings(serverId).then(setRaw).catch(() => setRaw([]));
  }, [needRaw, raw, serverId]);

  const valueOf = (s: SettingView): JsonValue => (s.key in draft ? draft[s.key] : s.value);
  const dirtyKeys = useMemo(() => Object.keys(draft).filter((k) => view && draft[k] !== view.settings.find((s) => s.key === k)?.value), [draft, view]);
  const editRaw = (key: string, value: string | null) =>
    setRawEdits((e) => {
      const n = { ...e };
      if (value === null) delete n[key];
      else n[key] = value;
      return n;
    });
  const editCol = (key: string, value: string | null) =>
    setColEdits((e) => {
      const n = { ...e };
      if (value === null) delete n[key];
      else n[key] = value;
      return n;
    });
  const pending = dirtyKeys.length + Object.keys(rawEdits).length + Object.keys(colEdits).length;

  if (fatal) {
    return (
      <div className="max-w-xl">
        <h1 className="text-2xl font-semibold">{props.title}</h1>
        <Card className="mt-6 p-5">
          <p className="font-medium">{fatal.human.code === "unknown" ? t("set.nothingYet") : human(fatal.human).title}</p>
          <p className="mt-1 text-sm text-muted">{fatal.human.code === "unknown" ? fatal.technical : human(fatal.human).message}</p>
        </Card>
      </div>
    );
  }
  if (!view) return <p className="text-muted">{t("set.loading")}</p>;

  const inCategory = view.settings.filter((s) => s.category === category);
  const basic = inCategory.filter((s) => !s.advanced);
  const advanced = inCategory.filter((s) => s.advanced);
  const advancedCount = (id: string) => view.settings.filter((s) => s.category === id && s.advanced).length;
  const basicCount = (id: string) => view.settings.filter((s) => s.category === id && !s.advanced).length;

  const SHOWN = 40; // the long list of other settings is drawn in part; searching narrows it
  const q = query.trim().toLowerCase();
  const searching = q !== "";
  const hit = (...parts: string[]) => parts.some((x) => x.toLowerCase().includes(q));
  const curatedKeys = new Set(view.settings.map((s) => s.key));
  const collections = collectionsOf(cols);
  const curatedHits = searching ? view.settings.filter((s) => hit(s.key, sx.title(s), sx.description(s))) : [];
  const collectionHits = searching ? collections.filter((c) => hit(c.item.key, t(c.label), t(c.text))) : [];
  const rawHits = (raw ?? []).filter(
    (s) => !curatedKeys.has(s.key) && (!onlyChanged || s.changed || s.key in rawEdits) && (!searching || hit(s.key, s.doc)),
  );
  const rowOf = (s: SettingView) => (
    <Row
      key={s.key}
      s={s}
      value={valueOf(s)}
      error={errors[s.key] ?? null}
      onChange={(v) => setDraft((d) => ({ ...d, [s.key]: v }))}
      onReset={() => setDraft((d) => ({ ...d, [s.key]: s.default }))}
    />
  );
  const tabs = [
    ...view.categories.filter((c) => basicCount(c.id) + advancedCount(c.id) > 0).map((c) => ({ id: c.id, label: sx.category(scope, c) })),
    ...(collections.length ? [{ id: "@collections", label: t("set.tab.collections") }] : []),
    ...(scope === "server" ? [{ id: "@all", label: t("set.tab.all") }] : []),
  ];

  async function save(changes: Record<string, JsonValue>): Promise<boolean> {
    const local: Record<string, string> = {};
    for (const [k, v] of Object.entries(changes)) {
      const s = view!.settings.find((x) => x.key === k)!;
      const m = clientCheck(t, s, v);
      if (m) local[k] = m;
    }
    setErrors(local);
    if (Object.keys(local).length) return false;
    const dangerous = Object.keys(changes).filter((k) => view!.settings.find((s) => s.key === k)?.dangerous);
    if (dangerous.length && !window.confirm(t("set.confirmCareful", { n: dangerous.length }))) return false;
    setBusy(true);
    setSaveErr(null);
    try {
      const report = await api.save(serverId, scope, changes);
      setSaved(report);
      setPreview(null);
      await load();
      void api.status(serverId).then((s) => setRunning(s.observed.world.state === "running"));
      return true;
    } catch (e) {
      const ui = asUiError(e);
      setSaveErr(ui);
      const map: Record<string, string> = {};
      (ui.fields ?? ([] as FieldError[])).forEach((f) => (map[f.key] = f.message));
      setErrors(map);
      return false;
    } finally {
      setBusy(false);
    }
  }

  /** The save bar saves all three kinds of edits: the curated settings, the collection switches and the other documented ones. */
  async function saveEverything() {
    if (dirtyKeys.length && !(await save(Object.fromEntries(dirtyKeys.map((k) => [k, draft[k]]))))) return;
    const extra = [...Object.keys(rawEdits), ...Object.keys(colEdits)];
    if (!extra.length) return;
    setBusy(true);
    setSaveErr(null);
    try {
      if (Object.keys(rawEdits).length) {
        await api.allSettingsSave(serverId, rawEdits);
        setRawEdits({});
        setRaw(await api.allSettings(serverId));
      }
      if (Object.keys(colEdits).length) {
        await api.moduleSaveSettings(serverId, "client-compat", colEdits);
        setColEdits({});
        setCols(await api.moduleSettings(serverId, "client-compat"));
      }
      const more = extra.map((key) => ({ key, title: key, restart: "world" as const, dangerous: false }));
      setSaved((prev) => ({ changed: [...(prev?.changed ?? []), ...more], restart: prev?.restart ?? "world", snapshot: prev?.snapshot ?? null }));
      void api.status(serverId).then((st) => setRunning(st.observed.world.state === "running"));
    } catch (e) {
      setSaveErr(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  async function openPreset(id: string) {
    setSaved(null);
    setPreview(await api.previewPreset(serverId, scope, id));
  }

  async function restartNow() {
    setBusy(true);
    try {
      await api.stop(serverId);
      await api.start(serverId);
      setSaved(null);
      setRunning(true);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="max-w-3xl pb-24">
      <h1 className="text-2xl font-semibold">{props.title}</h1>
      <p className="mt-1 text-muted">{props.question}</p>

      {scope === "bots" && <CompanionsCard serverId={serverId} />}

      {view.drift_keys.length > 0 && (
        <Card className="mt-5 border-warn/40 p-4" role="status">
          <p className="font-medium text-warn">{t("set.driftTitle", { n: view.drift_keys.length })}</p>
          <p className="mt-1 text-sm text-muted">
            {t("set.driftText")}{" "}
            <span className="selectable">{view.drift_keys.slice(0, 6).join(", ")}</span>
            {view.drift_keys.length > 6 ? "…" : ""}
          </p>
        </Card>
      )}

      <div className="mt-5 flex flex-wrap items-center gap-2">
        <span className="text-sm text-muted">{t("set.presets")}</span>
        {presets.map((p) => (
          <Button key={p.id} size="sm" title={sx.preset(p).description} onClick={() => void openPreset(p.id)}>
            {sx.preset(p).title}
          </Button>
        ))}
        <Button size="sm" variant="ghost" onClick={() => void openPreset("defaults")}>
          {t("set.restoreDefaults")}
        </Button>
      </div>

      {preview && (
        <Card className="mt-4 border-gold/40 p-5">
          <p className="font-medium">{sx.preset({ id: preview.id, title: preview.title, description: preview.description }).title}</p>
          <p className="text-sm text-muted">{sx.preset({ id: preview.id, title: preview.title, description: preview.description }).description}</p>
          {preview.changes.length === 0 ? (
            <p className="mt-3 text-sm">{t("set.matches")}</p>
          ) : (
            <>
              <p className="mt-3 text-sm">
                {tn("set.willChange", preview.changes.length)}
              </p>
              <ul className="mt-2 max-h-56 divide-y divide-line overflow-auto text-sm">
                {preview.changes.map((c) => {
                  const s = view.settings.find((x) => x.key === c.key)!;
                  return (
                    <li key={c.key} className="flex justify-between gap-4 py-1.5">
                      <span>{sx.title({ key: c.key, title: c.title })}</span>
                      <span className="text-muted">
                        {fmt(t, sx, c.from, s)} → <span className="text-ink">{fmt(t, sx, c.to, s)}</span>
                      </span>
                    </li>
                  );
                })}
              </ul>
            </>
          )}
          <div className="mt-4 flex gap-2">
            {preview.changes.length > 0 && (
              <Button
                variant="primary"
                size="sm"
                disabled={busy}
                onClick={() => void save(Object.fromEntries(preview.changes.map((c) => [c.key, c.to])))}
              >
                {t("set.apply")}
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={() => setPreview(null)}>
              {t("common.cancel")}
            </Button>
          </div>
        </Card>
      )}

      {saved && (
        <Card className="mt-4 border-ok/40 p-4" role="status">
          {saved.changed.length === 0 ? (
            <p>{t("set.nothingChanged")}</p>
          ) : (
            <>
              <p className="font-medium text-ok">{t("set.saved")}</p>
              <p className="mt-1 text-sm text-muted">
                {running && saved.restart
                  ? tn("set.restartRequired", saved.changed.length)
                  : t("set.appliesNextStart")}
              </p>
              {running && saved.restart && (
                <div className="mt-3 flex gap-2">
                  <Button size="sm" variant="primary" disabled={busy} onClick={() => void restartNow()}>
                    {t("set.restartNow")}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setSaved(null)}>
                    {t("set.later")}
                  </Button>
                </div>
              )}
            </>
          )}
        </Card>
      )}

      {saveErr && (
        <Card className="mt-4 border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{human(saveErr.human).title}</p>
          <p className="mt-1 text-sm text-muted">{t("set.notSaved")}</p>
        </Card>
      )}

      <Card className="mt-6 p-0">
        <div className="border-b border-line px-5 pb-0 pt-4">
          <label className="relative block">
            <Search className="pointer-events-none absolute left-3 top-2.5 h-4 w-4 text-muted" aria-hidden />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t(scope === "server" ? "set.searchAll" : "set.search")}
              aria-label={t(scope === "server" ? "set.searchAll" : "set.search")}
              className="selectable w-full rounded-md border border-line bg-bg py-2 pl-9 pr-3 text-sm outline-none focus:border-gold"
            />
          </label>
          <div className="mt-3 flex flex-wrap gap-1" role="tablist">
            {tabs.map((c) => (
              <button
                key={c.id}
                role="tab"
                aria-selected={!searching && category === c.id}
                onClick={() => {
                  setQuery("");
                  setCategory(c.id);
                }}
                className={cn(
                  "-mb-px cursor-pointer border-b-2 px-3 py-2 text-sm",
                  !searching && category === c.id ? "border-gold text-ink" : "border-transparent text-muted hover:text-ink",
                )}
              >
                {c.label}
              </button>
            ))}
          </div>
        </div>

        <div className="px-5 pb-5 pt-2">
          {searching ? (
            <>
              {curatedHits.length + collectionHits.length + rawHits.length === 0 && <p className="py-4 text-sm text-muted">{t("all.none")}</p>}
              <div className="divide-y divide-line">{curatedHits.map(rowOf)}</div>
              {collectionHits.length > 0 && <CollectionRows items={collectionHits} edits={colEdits} onChange={editCol} />}
              <RawRows items={rawHits.slice(0, SHOWN)} edits={rawEdits} onChange={editRaw} />
              {rawHits.length > SHOWN && <p className="pt-3 text-xs text-muted">{t("all.count", { shown: SHOWN, total: rawHits.length })}</p>}
            </>
          ) : category === "@collections" ? (
            <>
              <p className="py-3 text-sm text-muted">{t("col.intro")}</p>
              <CollectionRows items={collections} edits={colEdits} onChange={editCol} />
            </>
          ) : category === "@all" ? (
            <>
              <p className="py-3 text-sm text-muted">{t("all.intro")}</p>
              <label className="flex cursor-pointer items-center gap-2 pb-2 text-sm">
                <input type="checkbox" checked={onlyChanged} onChange={(e) => setOnlyChanged(e.target.checked)} />
                {t("all.onlyChanged")}
              </label>
              {raw === null ? <p className="py-3 text-sm text-muted">{t("set.loading")}</p> : <RawRows items={rawHits.slice(0, SHOWN)} edits={rawEdits} onChange={editRaw} />}
              {rawHits.length > SHOWN && <p className="pt-3 text-xs text-muted">{t("all.count", { shown: SHOWN, total: rawHits.length })}</p>}
            </>
          ) : (
            <>
              <div className="divide-y divide-line">{basic.map(rowOf)}</div>

              {advanced.length > 0 && (
                <div className="mt-2">
                  {/* A tab whose settings are all advanced has nothing else to show, so its settings are listed directly. */}
                  {basic.length > 0 && (
                    <button
                      onClick={() => setShowAdvanced((v) => !v)}
                      aria-expanded={showAdvanced}
                      className="flex cursor-pointer items-center gap-1 py-2 text-sm text-muted hover:text-ink"
                    >
                      {showAdvanced ? <ChevronDown className="h-4 w-4" aria-hidden /> : <ChevronRight className="h-4 w-4" aria-hidden />}
                      {t("set.advanced", { n: advanced.length })}
                    </button>
                  )}
                  {(showAdvanced || basic.length === 0) && <div className="divide-y divide-line">{advanced.map(rowOf)}</div>}
                </div>
              )}
            </>
          )}
        </div>
      </Card>

      {view.unknown_keys > 0 && (
        <p className="mt-6 text-xs text-muted">{t("set.unknownKept", { n: view.unknown_keys })}</p>
      )}

      {pending > 0 && (
        <div className="fixed bottom-0 left-60 right-0 flex items-center justify-between border-t border-line bg-[#0b0c0e]/95 px-10 py-3">
          <span className="text-sm text-muted">{tn("set.unsaved", pending)}</span>
          <div className="flex gap-2">
            <Button variant="ghost" size="sm" onClick={() => { setDraft({}); setErrors({}); setRawEdits({}); setColEdits({}); }}>
              {t("set.discard")}
            </Button>
            <Button variant="primary" size="sm" disabled={busy} onClick={() => void saveEverything()}>
              {t("set.save")}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
