import { useCallback, useEffect, useMemo, useState } from "react";
import { AlertTriangle, ChevronDown, ChevronRight, RotateCcw } from "lucide-react";
import {
  api,
  asUiError,
  type FieldError,
  type JsonValue,
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
import { CompanionsCard } from "@/screens/CompanionsCard";

const RESTART_LABEL = { runtime: "Applies immediately", world: "Needs a world server restart", full: "Needs a full server restart" } as const;

function fmt(v: JsonValue, s: { type: string; unit?: string | null; options: { value: JsonValue; label: string }[] }): string {
  if (typeof v === "boolean") return v ? "On" : "Off";
  if (s.type === "enum") return s.options.find((o) => o.value === v)?.label ?? String(v);
  return s.unit ? `${v} ${s.unit}` : String(v);
}

function clientCheck(s: SettingView, v: JsonValue): string | null {
  if (s.type === "int" || s.type === "float") {
    if (typeof v !== "number" || Number.isNaN(v)) return "Enter a number";
    if (s.type === "int" && !Number.isInteger(v)) return "Enter a whole number";
    if (s.min !== undefined && v < s.min) return `Must be at least ${s.min}`;
    if (s.max !== undefined && v > s.max) return `Must be at most ${s.max}`;
  }
  return null;
}

function Field(props: { s: SettingView; value: JsonValue; error: string | null; onChange: (v: JsonValue) => void }) {
  const { s, value, error, onChange } = props;
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
        <span className="sr-only">{value === true ? "On" : "Off"}</span>
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
            {o.label}
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
          const t = e.target.value;
          onChange(numeric ? (t.trim() === "" || Number.isNaN(Number(t)) ? (t as unknown as number) : Number(t)) : t);
        }}
      />
      {s.unit && <span className="text-xs text-muted">{s.unit}</span>}
    </div>
  );
}

function Row(props: { s: SettingView; value: JsonValue; error: string | null; onChange: (v: JsonValue) => void; onReset: () => void }) {
  const { s, value, error } = props;
  const changed = value !== s.value;
  return (
    <div className="grid grid-cols-[1fr_auto] items-start gap-x-6 gap-y-1 py-3.5">
      <div>
        <label htmlFor={`f-${s.key}`} className="flex flex-wrap items-center gap-2 font-medium">
          {s.title}
          {s.dangerous && (
            <span className="inline-flex items-center gap-1 rounded bg-warn/15 px-1.5 py-0.5 text-[11px] font-normal text-warn">
              <AlertTriangle className="h-3 w-3" aria-hidden /> Careful
            </span>
          )}
          {changed && <span className="rounded bg-gold/15 px-1.5 py-0.5 text-[11px] font-normal text-gold">Changed</span>}
        </label>
        <p className="mt-0.5 max-w-xl text-sm text-muted">{s.description}</p>
        <p className="mt-1 text-xs text-muted/80">
          Default: {fmt(s.default, s)}
          {s.min !== undefined && s.max !== undefined && s.type !== "bool" ? ` · Range ${s.min}–${s.max}` : ""} · {RESTART_LABEL[s.restartRequired]}
        </p>
        {s.problem && <p className="mt-1 text-xs text-warn">The file has an unusable value ({s.problem}); showing the default.</p>}
        {s.drift && <p className="mt-1 text-xs text-warn">Edited by hand in the generated file — the launcher will reset it at next start unless you save it here.</p>}
        {error && (
          <p role="alert" className="mt-1 text-xs text-bad">
            {error}
          </p>
        )}
      </div>
      <div className="flex items-center gap-2 pt-0.5">
        <Field s={s} value={value} error={error} onChange={props.onChange} />
        <button
          title="Reset to default"
          aria-label={`Reset ${s.title} to default`}
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
    void load();
    void api.presets(scope).then(setPresets);
    void api.status(serverId).then((s) => setRunning(s.observed.world.state === "running"));
  }, [load, scope, serverId]);

  const valueOf = (s: SettingView): JsonValue => (s.key in draft ? draft[s.key] : s.value);
  const dirtyKeys = useMemo(() => Object.keys(draft).filter((k) => view && draft[k] !== view.settings.find((s) => s.key === k)?.value), [draft, view]);

  if (fatal) {
    return (
      <div className="max-w-xl">
        <h1 className="text-2xl font-semibold">{props.title}</h1>
        <Card className="mt-6 p-5">
          <p className="font-medium">{fatal.human.code === "unknown" ? "Nothing to configure yet" : fatal.human.title}</p>
          <p className="mt-1 text-sm text-muted">{fatal.human.code === "unknown" ? fatal.technical : fatal.human.message}</p>
        </Card>
      </div>
    );
  }
  if (!view) return <p className="text-muted">Loading settings…</p>;

  const inCategory = view.settings.filter((s) => s.category === category);
  const basic = inCategory.filter((s) => !s.advanced);
  const advanced = inCategory.filter((s) => s.advanced);
  const advancedCount = (id: string) => view.settings.filter((s) => s.category === id && s.advanced).length;
  const basicCount = (id: string) => view.settings.filter((s) => s.category === id && !s.advanced).length;

  async function save(changes: Record<string, JsonValue>) {
    const local: Record<string, string> = {};
    for (const [k, v] of Object.entries(changes)) {
      const s = view!.settings.find((x) => x.key === k)!;
      const m = clientCheck(s, v);
      if (m) local[k] = m;
    }
    setErrors(local);
    if (Object.keys(local).length) return;
    const dangerous = Object.keys(changes).filter((k) => view!.settings.find((s) => s.key === k)?.dangerous);
    if (dangerous.length && !window.confirm(`You are changing ${dangerous.length} setting(s) marked Careful. A snapshot of the current configuration is saved first, so this can be undone. Continue?`)) return;
    setBusy(true);
    setSaveErr(null);
    try {
      const report = await api.save(serverId, scope, changes);
      setSaved(report);
      setPreview(null);
      await load();
      void api.status(serverId).then((s) => setRunning(s.observed.world.state === "running"));
    } catch (e) {
      const ui = asUiError(e);
      setSaveErr(ui);
      const map: Record<string, string> = {};
      (ui.fields ?? ([] as FieldError[])).forEach((f) => (map[f.key] = f.message));
      setErrors(map);
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
          <p className="font-medium text-warn">{view.drift_keys.length} setting(s) were edited by hand</p>
          <p className="mt-1 text-sm text-muted">
            The server launcher rebuilds worldserver.conf from its template every time it starts, so these manual edits will be lost:{" "}
            <span className="selectable">{view.drift_keys.slice(0, 6).join(", ")}</span>
            {view.drift_keys.length > 6 ? "…" : ""}
          </p>
        </Card>
      )}

      <div className="mt-5 flex flex-wrap items-center gap-2">
        <span className="text-sm text-muted">Presets:</span>
        {presets.map((p) => (
          <Button key={p.id} size="sm" title={p.description} onClick={() => void openPreset(p.id)}>
            {p.title}
          </Button>
        ))}
        <Button size="sm" variant="ghost" onClick={() => void openPreset("defaults")}>
          Restore recommended defaults
        </Button>
      </div>

      {preview && (
        <Card className="mt-4 border-gold/40 p-5">
          <p className="font-medium">{preview.title}</p>
          <p className="text-sm text-muted">{preview.description}</p>
          {preview.changes.length === 0 ? (
            <p className="mt-3 text-sm">Nothing to change — your settings already match.</p>
          ) : (
            <>
              <p className="mt-3 text-sm">
                This will change <b>{preview.changes.length}</b> setting{preview.changes.length === 1 ? "" : "s"}:
              </p>
              <ul className="mt-2 max-h-56 divide-y divide-line overflow-auto text-sm">
                {preview.changes.map((c) => {
                  const s = view.settings.find((x) => x.key === c.key)!;
                  return (
                    <li key={c.key} className="flex justify-between gap-4 py-1.5">
                      <span>{c.title}</span>
                      <span className="text-muted">
                        {fmt(c.from, s)} → <span className="text-ink">{fmt(c.to, s)}</span>
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
                Apply
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={() => setPreview(null)}>
              Cancel
            </Button>
          </div>
        </Card>
      )}

      {saved && (
        <Card className="mt-4 border-ok/40 p-4" role="status">
          {saved.changed.length === 0 ? (
            <p>Nothing changed.</p>
          ) : (
            <>
              <p className="font-medium text-ok">Saved.</p>
              <p className="mt-1 text-sm text-muted">
                {running && saved.restart
                  ? `Restart required to apply ${saved.changed.length} setting${saved.changed.length === 1 ? "" : "s"}.`
                  : "The changes will apply the next time the server starts."}
              </p>
              {running && saved.restart && (
                <div className="mt-3 flex gap-2">
                  <Button size="sm" variant="primary" disabled={busy} onClick={() => void restartNow()}>
                    Restart now
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setSaved(null)}>
                    Later
                  </Button>
                </div>
              )}
            </>
          )}
        </Card>
      )}

      {saveErr && (
        <Card className="mt-4 border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{saveErr.human.title}</p>
          <p className="mt-1 text-sm text-muted">Nothing was saved. Fix the highlighted settings and try again.</p>
        </Card>
      )}

      <div className="mt-6 flex flex-wrap gap-1 border-b border-line" role="tablist">
        {view.categories
          .filter((c) => basicCount(c.id) + advancedCount(c.id) > 0)
          .map((c) => (
            <button
              key={c.id}
              role="tab"
              aria-selected={category === c.id}
              onClick={() => setCategory(c.id)}
              className={cn(
                "-mb-px cursor-pointer border-b-2 px-3 py-2 text-sm",
                category === c.id ? "border-gold text-ink" : "border-transparent text-muted hover:text-ink",
              )}
            >
              {c.title}
            </button>
          ))}
      </div>

      <div className="divide-y divide-line">
        {basic.map((s) => (
          <Row
            key={s.key}
            s={s}
            value={valueOf(s)}
            error={errors[s.key] ?? null}
            onChange={(v) => setDraft((d) => ({ ...d, [s.key]: v }))}
            onReset={() => setDraft((d) => ({ ...d, [s.key]: s.default }))}
          />
        ))}
      </div>

      {advanced.length > 0 && (
        <div className="mt-4">
          <button
            onClick={() => setShowAdvanced((v) => !v)}
            aria-expanded={showAdvanced}
            className="flex cursor-pointer items-center gap-1 text-sm text-muted hover:text-ink"
          >
            {showAdvanced ? <ChevronDown className="h-4 w-4" aria-hidden /> : <ChevronRight className="h-4 w-4" aria-hidden />}
            Advanced settings ({advanced.length})
          </button>
          {showAdvanced && (
            <div className="divide-y divide-line">
              {advanced.map((s) => (
                <Row
                  key={s.key}
                  s={s}
                  value={valueOf(s)}
                  error={errors[s.key] ?? null}
                  onChange={(v) => setDraft((d) => ({ ...d, [s.key]: v }))}
                  onReset={() => setDraft((d) => ({ ...d, [s.key]: s.default }))}
                />
              ))}
            </div>
          )}
        </div>
      )}

      {view.unknown_keys > 0 && (
        <p className="mt-6 text-xs text-muted">{view.unknown_keys} other setting(s) in the configuration file are kept exactly as they are.</p>
      )}

      {dirtyKeys.length > 0 && (
        <div className="fixed bottom-0 left-60 right-0 flex items-center justify-between border-t border-line bg-[#0b0c0e]/95 px-10 py-3">
          <span className="text-sm text-muted">{dirtyKeys.length} unsaved change{dirtyKeys.length === 1 ? "" : "s"}</span>
          <div className="flex gap-2">
            <Button variant="ghost" size="sm" onClick={() => { setDraft({}); setErrors({}); }}>
              Discard
            </Button>
            <Button variant="primary" size="sm" disabled={busy} onClick={() => void save(Object.fromEntries(dirtyKeys.map((k) => [k, draft[k]])))}>
              Save changes
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
