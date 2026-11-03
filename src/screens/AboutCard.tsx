import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { Loader2, RefreshCw, Monitor } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useT } from "@/i18n";
import { autoUpdateEnabled, installNow, lookForUpdate, setAutoUpdate, useSelfUpdate } from "@/lib/selfUpdate";

export function AboutCard({ onHistory }: { onHistory?: () => void }) {
  const t = useT();
  const s = useSelfUpdate();
  const [version, setVersion] = useState("");
  const [auto, setAuto] = useState(autoUpdateEnabled());
  const [asked, setAsked] = useState(false);

  useEffect(() => {
    void getVersion().then(setVersion).catch(() => {});
  }, []);

  const busy = s.phase === "checking" || s.phase === "downloading" || s.phase === "installing";

  return (
    <Card className="mt-6 p-6">
      <div className="flex items-start justify-between gap-3">
        <div><h2 className="flex items-center gap-2 font-semibold"><Monitor className="h-4 w-4 text-gold" aria-hidden />{t("history.manager")}</h2><p className="mt-2 text-sm text-muted">{t("settings.version", { v: version || "…" })}</p></div>
        <Button size="sm" disabled={busy} aria-label={t("about.check")} title={t("about.check")} onClick={() => { setAsked(true); void lookForUpdate({ download: false }); }}><RefreshCw className={`h-4 w-4 ${s.phase === "checking" ? "animate-spin" : ""}`} aria-hidden /></Button>
      </div>
      <p className="mt-3 text-xs text-muted">{t("about.autoHint")}</p>
      <div className="mt-4 flex flex-wrap items-center gap-3">
        {s.version && s.phase !== "current" && (
          <Button size="sm" variant="primary" disabled={busy} onClick={() => void installNow()}>
            {(s.phase === "downloading" || s.phase === "installing") && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            {t("about.install", { v: s.version })}
          </Button>
        )}
        {s.percent !== null && s.phase === "downloading" && (
          <span className="text-sm text-muted" role="status">
            {s.percent}%
          </span>
        )}
      </div>
      {onHistory ? <button className="mt-3 cursor-pointer text-sm text-gold hover:underline" onClick={onHistory}>{t(s.version && s.phase !== "current" ? "history.open" : "history.title")}</button> : s.version && s.notes && s.phase !== "current" && <p className="mt-3 whitespace-pre-line text-sm text-muted">{s.notes}</p>}
      {s.phase === "ready" && <p className="mt-3 text-sm text-ok" role="status">{t("selfupd.onClose")}</p>}
      {asked && s.phase === "current" && <p className="mt-3 text-sm text-ok" role="status">{t("about.upToDate")}</p>}
      {s.phase === "error" && (
        <div className="mt-3 text-sm text-bad" role="alert">
          <p>{t("about.checkFailed")}</p>
          {s.error && <p className="selectable mt-1 break-all text-xs text-muted">{s.error}</p>}
        </div>
      )}
      <label className="mt-5 flex cursor-pointer items-center justify-between gap-3 border-t border-line pt-4 text-sm" title={t("about.auto")}>
        <span>{t("settings.autoManager")}</span><input type="checkbox" role="switch" aria-description={t("about.auto")} checked={auto} onChange={e => { setAuto(e.target.checked); setAutoUpdate(e.target.checked); }} className="h-4 w-4 accent-[#c9a24a]" />
      </label>
    </Card>
  );
}
