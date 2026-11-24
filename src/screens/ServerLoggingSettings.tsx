import { useEffect, useState } from "react";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { api, asUiError, type CleanLogsReport, type LogSettings, type ServerLoggingView } from "@/lib/api";
import { formatBytes } from "@/lib/utils";
import { useT } from "@/i18n";

const DEFAULT_LOG_SETTINGS: LogSettings = { autoCleanup: false, maxSizeMb: 500 };

export function ServerLoggingSettings({ serverId }: { serverId: string }) {
  const t = useT();
  const [view, setView] = useState<ServerLoggingView | null>(null);
  const [draft, setDraft] = useState<LogSettings>(DEFAULT_LOG_SETTINGS);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [autoSaved, setAutoSaved] = useState(false);
  const [cleanReport, setCleanReport] = useState<CleanLogsReport | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let alive = true;
    void api.serverLoggingState(serverId)
      .then(v => {
        if (!alive) return;
        setView(v);
        setDraft(v.settings);
      })
      .catch(err => {
        if (alive) setError(asUiError(err).technical);
      });
    return () => { alive = false; };
  }, [serverId]);

  async function refreshState() {
    const v = await api.serverLoggingState(serverId);
    setView(v);
    setDraft(v.settings);
  }

  async function apply(preset: "normal" | "errors" | "off") {
    setBusy(true); setSaved(false); setAutoSaved(false); setCleanReport(null); setError("");
    try {
      await api.serverLoggingPreset(serverId, preset);
      await refreshState();
      setSaved(true);
    } catch (err) {
      setError(asUiError(err).technical);
    } finally {
      setBusy(false);
    }
  }

  async function saveAutoCleanup() {
    setBusy(true); setSaved(false); setAutoSaved(false); setCleanReport(null); setError("");
    try {
      const v = await api.serverLoggingSettingsSave(serverId, draft);
      setView(v);
      setDraft(v.settings);
      setAutoSaved(true);
    } catch (err) {
      setError(asUiError(err).technical);
    } finally {
      setBusy(false);
    }
  }

  async function cleanNow() {
    setBusy(true); setSaved(false); setAutoSaved(false); setCleanReport(null); setError("");
    try {
      const report = await api.serverLogsClean(serverId);
      setCleanReport(report);
      await refreshState();
    } catch (err) {
      setError(asUiError(err).technical);
    } finally {
      setBusy(false);
    }
  }

  const changed = !!view && JSON.stringify(view.settings) !== JSON.stringify(draft);
  const sizeOk = Number.isInteger(draft.maxSizeMb) && draft.maxSizeMb >= 10 && draft.maxSizeMb <= 100000;
  const logBytes = view?.logBytes ?? 0;
  const input = "rounded border border-line bg-bg px-2 py-1 text-sm text-ink outline-none focus:border-gold";

  return <Card className="mt-6 p-6" aria-busy={busy}>
    <h2 className="font-semibold">{t("serverLogs.title")}</h2>
    <p className="mt-2 text-sm text-muted">{t("serverLogs.description")}</p>
    <div className="mt-4 flex flex-wrap gap-2">
      {(["normal", "errors", "off"] as const).map(preset => {
        const active = view?.preset === preset;
        return <button
          key={preset}
          disabled={busy}
          aria-pressed={active}
          onClick={() => void apply(preset)}
          className={`rounded-md border px-3 py-2 text-sm disabled:opacity-50 ${active ? "border-gold bg-gold/10 text-gold" : "border-line hover:bg-white/5"}`}
        >
          {t(`serverLogs.${preset}`)}
        </button>;
      })}
    </div>
    {saved && <p role="status" className="mt-3 text-sm text-ok">{t("serverLogs.saved")}</p>}

    <div className="mt-5 border-t border-line pt-4">
      <label className="flex cursor-pointer items-center gap-3">
        <input
          type="checkbox"
          className="h-4 w-4 accent-[#c9a24a]"
          checked={draft.autoCleanup}
          disabled={busy || !view}
          onChange={e => { setDraft(d => ({ ...d, autoCleanup: e.target.checked })); setAutoSaved(false); }}
        />
        <span className="text-sm font-medium">{t("serverLogs.autoClean")}</span>
      </label>
      <div className="mt-3 flex flex-wrap items-center gap-3 text-sm">
        <label className="flex items-center gap-2">
          <span>{t("serverLogs.maxSize")}</span>
          <input
            type="number"
            min={10}
            max={100000}
            step={10}
            className={`${input} w-24`}
            value={draft.maxSizeMb}
            disabled={busy || !draft.autoCleanup}
            onChange={e => { setDraft(d => ({ ...d, maxSizeMb: Number(e.target.value) })); setAutoSaved(false); }}
          />
          <span className="text-muted">MB</span>
        </label>
        <Button size="sm" disabled={busy || !changed || (draft.autoCleanup && !sizeOk)} onClick={() => void saveAutoCleanup()}>
          {t("sched.save")}
        </Button>
        {autoSaved && <span role="status" className="text-xs text-ok">{t("sched.saved")}</span>}
      </div>
      <p className="mt-2 text-xs text-muted">{t("serverLogs.autoCleanHint")}</p>
    </div>

    <div className="mt-5 flex flex-wrap items-center justify-between gap-3 border-t border-line pt-4">
      <span className="text-sm text-muted">
        {t("serverLogs.currentSize", { size: formatBytes(logBytes) })}
      </span>
      <Button size="sm" disabled={busy || !view || logBytes === 0} onClick={() => void cleanNow()}>
        {t("serverLogs.cleanNow", { size: formatBytes(logBytes) })}
      </Button>
    </div>

    {cleanReport && (
      <p role="status" className="mt-3 text-sm text-ok">
        {t("serverLogs.cleaned", { freed: formatBytes(cleanReport.freedBytes) })}
        {cleanReport.skippedFiles > 0 && ` ${t("serverLogs.skippedInUse", { remaining: formatBytes(cleanReport.remainingBytes) })}`}
      </p>
    )}
    {error && <p role="alert" className="mt-3 text-sm text-bad">{error}</p>}
  </Card>;
}
