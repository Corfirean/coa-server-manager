import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useT } from "@/i18n";
import { autoUpdateEnabled, installNow, lookForUpdate, setAutoUpdate, useSelfUpdate } from "@/lib/selfUpdate";

export function AboutCard() {
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
      <h2 className="font-semibold">{t("app.name")}</h2>
      <p className="mt-1 text-sm text-muted">{t("about.version", { v: version || "…" })}</p>

      <label className="mt-3 flex cursor-pointer items-start gap-2 text-sm">
        <input
          type="checkbox"
          className="mt-1"
          checked={auto}
          onChange={(e) => {
            setAuto(e.target.checked);
            setAutoUpdate(e.target.checked);
          }}
        />
        <span>
          {t("about.auto")}
          <span className="block text-xs text-muted">{t("about.autoHint")}</span>
        </span>
      </label>

      <div className="mt-4 flex flex-wrap items-center gap-3">
        <Button
          size="sm"
          disabled={busy}
          onClick={() => {
            setAsked(true);
            void lookForUpdate({ download: false });
          }}
        >
          {s.phase === "checking" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {t("about.check")}
        </Button>
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
      {s.version && s.notes && s.phase !== "current" && <p className="mt-3 whitespace-pre-line text-sm text-muted">{s.notes}</p>}
      {s.phase === "ready" && <p className="mt-3 text-sm text-ok" role="status">{t("selfupd.onClose")}</p>}
      {asked && s.phase === "current" && <p className="mt-3 text-sm text-ok" role="status">{t("about.upToDate")}</p>}
      {s.phase === "error" && (
        <div className="mt-3 text-sm text-bad" role="alert">
          <p>{t("about.checkFailed")}</p>
          {s.error && <p className="selectable mt-1 break-all text-xs text-muted">{s.error}</p>}
        </div>
      )}
    </Card>
  );
}
