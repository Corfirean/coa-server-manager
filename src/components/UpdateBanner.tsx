import { Download } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n";
import { installNow, useSelfUpdate } from "@/lib/selfUpdate";

/** Small notice shown once a new Manager version is downloaded and waiting to be installed. */
export function UpdateBanner() {
  const t = useT();
  const s = useSelfUpdate();
  if (s.phase !== "ready" && s.phase !== "installing") return null;
  return (
    <div className="fixed bottom-5 right-5 z-50 flex max-w-sm items-start gap-3 rounded-card border border-gold/50 bg-card p-4 shadow-[0_8px_30px_rgb(0_0_0/0.5)]" role="status">
      <Download className="mt-0.5 h-5 w-5 shrink-0 text-gold" aria-hidden />
      <div className="text-sm">
        <p className="font-medium">{t("selfupd.ready", { v: s.version ?? "" })}</p>
        <p className="mt-0.5 text-muted">{s.phase === "installing" ? t("selfupd.installing") : t("selfupd.onClose")}</p>
        {s.phase === "ready" && (
          <Button className="mt-2" size="sm" variant="secondary" onClick={() => void installNow()}>
            {t("selfupd.now")}
          </Button>
        )}
      </div>
    </div>
  );
}
