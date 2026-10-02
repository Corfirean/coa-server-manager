import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { applyServerUpdate, clearServerUpdateResult, useServerUpdate } from "@/lib/serverUpdate";
import { api, asUiError, type UiError, type UpdatePreview, type UpdateTxn } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ClientCard } from "@/screens/ClientCard";
import { DiagnosticsCard } from "@/screens/DiagnosticsCard";
import { AboutCard } from "@/screens/AboutCard";
import { hasKey, useHuman, useI18n, type Key } from "@/i18n";
import { LanguagePicker } from "@/components/LanguagePicker";

function mb(bytes: number) {
  return bytes >= 1 << 20 ? `${(bytes / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

export function SettingsHome({ serverId }: { serverId: string }) {
  const { t } = useI18n();
  const upd = useServerUpdate(serverId);
  const human = useHuman();
  const [preview, setPreview] = useState<UpdatePreview | null>(null);
  const [pending, setPending] = useState<UpdateTxn | null>(null);
  const [localBusy, setBusy] = useState<"check" | "rollback" | null>(null);
  // an update keeps running in the background when this tab is left, so its state is the shared one
  const busy = upd.applying ? "update" : localBusy;
  const progress = upd.applying ? { step: upd.step ?? "Starting", percent: upd.percent } : null;
  const [localError, setError] = useState<UiError | null>(null);
  const error = localError ?? upd.uiError;
  const [localDone, setDone] = useState<string | null>(null);
  const done = upd.result && "committed" in upd.result ? t("upd.updatedTo", { v: upd.result.committed }) : localDone;
  const [choices, setChoices] = useState<Record<string, "keep" | "replace">>({});
  const [advanced, setAdvanced] = useState(false);
  const [source, setSource] = useState("");

  useEffect(() => {
    void api.pendingUpdate(serverId).then(setPending).catch(() => {});
  }, [serverId]);

  // the update finished (possibly while another tab was open): drop the stale preview, show what is left to decide
  useEffect(() => {
    if (!upd.result) return;
    setPreview(null);
    if ("pending" in upd.result) setPending(upd.result.pending);
    else void api.pendingUpdate(serverId).then(setPending).catch(() => {});
  }, [upd.result, serverId]);

  async function check() {
    setBusy("check");
    setError(null);
    setDone(null);
    clearServerUpdateResult(serverId);
    setPreview(null);
    try {
      const p = await api.checkUpdate(serverId, source.trim() || undefined);
      setPreview(p);
      setChoices(Object.fromEntries(p.conflicts.map((c) => [c, "keep" as const])));
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(null);
    }
  }

  async function update() {
    setError(null);
    setDone(null);
    await applyServerUpdate(serverId, { choices, source: source.trim() || undefined });
  }

  async function rollback(txn: UpdateTxn) {
    if (!window.confirm(t("upd.confirmBack"))) return;
    setBusy("rollback");
    setError(null);
    try {
      await api.rollbackUpdate(serverId, txn.id);
      setPending(null);
      setDone(t("upd.wentBack"));
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(null);
    }
  }

  async function browse() {
    const p = await open({ directory: true, multiple: false, title: t("upd.dialogTitle") });
    if (typeof p === "string") setSource(p);
  }

  return (
    <div className="max-w-2xl">
      <h1 className="text-2xl font-semibold">{t("settings.title")}</h1>
      <p className="mt-1 text-muted">{t("q.settings")}</p>

      {pending && (
        <Card className="mt-6 border-warn/40 p-5" role="alert">
          <p className="font-medium text-warn">
            {pending.state === "needs-decision" ? t("upd.pendingBad") : t("upd.pendingUnfinished")}
          </p>
          <p className="mt-1 text-sm text-muted">{pending.message ?? t("upd.safeBack")}</p>
          <p className="mt-1 text-sm text-muted">{t("upd.backNote")}</p>
          <Button className="mt-3" size="sm" disabled={!!busy} onClick={() => void rollback(pending)}>
            {busy === "rollback" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            {pending.from_version ? t("upd.goBackTo", { v: pending.from_version }) : t("upd.goBackBefore")}
          </Button>
        </Card>
      )}

      <Card className="mt-6 p-6">
        <h2 className="font-semibold">{t("settings.language")}</h2>
        <p className="mt-1 text-sm text-muted">{t("settings.languageHint")}</p>
        <LanguagePicker className="mt-3" />
      </Card>

      <ClientCard serverId={serverId} />

      <DiagnosticsCard serverId={serverId} />

      <AboutCard />

      <Card className="mt-6 p-6">
        <h2 className="font-semibold">{t("upd.title")}</h2>
        <p className="mt-1 text-sm text-muted">{t("upd.text")}</p>

        <div className="mt-4 flex items-center gap-3">
          <Button variant="primary" disabled={!!busy} onClick={() => void check()}>
            {busy === "check" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            {t("upd.check")}
          </Button>
          <button onClick={() => setAdvanced((v) => !v)} aria-expanded={advanced} className="flex cursor-pointer items-center gap-1 text-sm text-muted hover:text-ink">
            {advanced ? <ChevronDown className="h-4 w-4" aria-hidden /> : <ChevronRight className="h-4 w-4" aria-hidden />}
            {t("install.advanced")}
          </button>
        </div>
        {advanced && (
          <div className="mt-3 flex gap-2">
            <input aria-label={t("upd.folderLabel")} value={source} onChange={(e) => setSource(e.target.value)} placeholder={t("upd.folderHint")} className="selectable flex-1 rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold" />
            <Button size="sm" onClick={browse}>{t("install.browse")}</Button>
          </div>
        )}

        {done && <p className="mt-4 text-sm text-ok" role="status">{done}</p>}
        {error && (
          <p className="mt-4 text-sm text-bad" role="alert">
            {error.human.code === "unknown" ? error.technical : human(error.human).message}
          </p>
        )}

        {preview && preview.from_version === preview.to_version && preview.items.every((i) => i.action === "skip") && (
          // The same version with no file to change: the database scripts of this package are all in the ledger already.
          <div className="mt-5 border-t border-line pt-4">
            <p className="font-medium text-ok" role="status">{t("upd.current", { v: preview.to_version })}</p>
          </div>
        )}

        {preview && !(preview.from_version === preview.to_version && preview.items.every((i) => i.action === "skip")) && (
          <div className="mt-5 border-t border-line pt-4">
            <p className="font-medium">
              {preview.from_version ? t("upd.availableFrom", { to: preview.to_version, from: preview.from_version }) : t("upd.available", { to: preview.to_version })}
            </p>
            <p className="mt-1 text-sm text-muted">
              {preview.migrations > 0
                ? t("upd.summaryDb", { size: mb(preview.download_bytes), files: preview.items.filter((i) => i.action !== "skip").length, db: preview.migrations })
                : t("upd.summary", { size: mb(preview.download_bytes), files: preview.items.filter((i) => i.action !== "skip").length })}
            </p>
            <ul className="mt-3 max-h-56 divide-y divide-line overflow-auto text-sm">
              {preview.items.filter((i) => i.action !== "skip").map((i) => (
                <li key={i.path} className="py-2">
                  <div className="flex justify-between gap-3">
                    <span className="selectable break-all">{i.path}</span>
                    <span className={i.action === "conflict" ? "shrink-0 text-warn" : "shrink-0 text-muted"}>{t(`upd.act.${i.action}` as Key)}</span>
                  </div>
                  {i.action === "conflict" && (
                    <fieldset className="mt-2 flex flex-wrap gap-4 text-xs text-muted">
                      <legend className="sr-only">{t("upd.conflict")}</legend>
                      <span className="text-warn">{t("upd.conflict")}</span>
                      {(["keep", "replace"] as const).map((c) => (
                        <label key={c} className="flex cursor-pointer items-center gap-1">
                          <input type="radio" name={i.path} checked={choices[i.path] === c} onChange={() => setChoices((x) => ({ ...x, [i.path]: c }))} />
                          {c === "keep" ? t("upd.keep") : t("upd.replace")}
                        </label>
                      ))}
                    </fieldset>
                  )}
                </li>
              ))}
            </ul>
            <Button className="mt-4" variant="primary" disabled={!!busy} onClick={() => void update()}>
              {busy === "update" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
              {t("upd.now")}
            </Button>
            <p className="mt-2 text-xs text-muted">{t("upd.stopNote")}</p>
          </div>
        )}

        {busy === "update" && progress && (
          <div className="mt-5">
            <div className="flex justify-between text-sm">
              <span role="status">{hasKey(`ustep.${progress.step}`) ? t(`ustep.${progress.step}` as Key) : progress.step}</span>
              <span className="text-muted">{progress.percent}%</span>
            </div>
            <div className="mt-2 h-2 overflow-hidden rounded-full bg-white/10" role="progressbar" aria-valuenow={progress.percent} aria-valuemin={0} aria-valuemax={100}>
              <div className="h-full rounded-full bg-gold transition-[width] duration-300" style={{ width: `${progress.percent}%` }} />
            </div>
          </div>
        )}
      </Card>
    </div>
  );
}
