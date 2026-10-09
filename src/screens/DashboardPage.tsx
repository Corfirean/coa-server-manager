import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Loader2 } from "lucide-react";
import { api, asUiError, type DashboardStatus, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";

/** SQUID's bot dashboard: installed from the release that matches the bots, started on a free port, shown here. */
export function DashboardPage({ serverId }: { serverId: string }) {
  const t = useT();
  const human = useHuman();
  const [status, setStatus] = useState<DashboardStatus | null>(null);
  const [busy, setBusy] = useState<"install" | "start" | "stop" | null>(null);
  const [step, setStep] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);

  const load = useCallback(async () => {
    try { setStatus(await api.dashboardStatus(serverId)); } catch (e) { setError(asUiError(e)); }
  }, [serverId]);

  useEffect(() => {
    void load();
    const un = listen<string>("dashboard-progress", (e) => setStep(e.payload));
    return () => { void un.then((f) => f()); };
  }, [load]);

  async function run(kind: "install" | "start" | "stop", fn: () => Promise<DashboardStatus>) {
    setBusy(kind); setError(null); setStep(null);
    try { setStatus(await fn()); } catch (e) { setError(asUiError(e)); await load(); }
    finally { setBusy(null); setStep(null); }
  }

  const install = () => run("install", () => api.dashboardInstall(serverId));

  return (
    <div className="flex h-full max-w-6xl flex-col">
      <h1 className="text-2xl font-semibold">{t("dash.title")}</h1>
      <p className="mt-1 text-muted">{t("dash.intro")}</p>

      {!status && !error && <p className="mt-6 text-muted">{t("overview.checking")}</p>}

      {status && (
        <Card className="mt-5 p-5">
          {!status.installed ? (
            <>
              <p className="font-medium">{t("dash.notInstalled")}</p>
              <p className="mt-1 text-sm text-muted">{status.squid_tag ? t("dash.willInstall", { v: status.squid_tag }) : t("dash.noSquid")}</p>
              <Button className="mt-4" variant="primary" disabled={!!busy || !status.squid_tag} onClick={() => void install()}>
                {busy === "install" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
                {t("dash.install")}
              </Button>
            </>
          ) : (
            <>
              <p className="text-sm text-muted">
                {t("dash.versions", { dash: status.tag ?? "?", bots: status.squid_tag ?? "?" })}
                {" · "}
                <span className={status.running ? "text-ok" : "text-muted"}>{status.running ? t("dash.running") : t("dash.stopped")}</span>
              </p>
              {!status.matches && status.squid_tag && (
                <p className="mt-2 text-sm text-warn">{t("dash.mismatch", { bots: status.squid_tag })}</p>
              )}
              <div className="mt-4 flex flex-wrap items-center gap-2">
                {status.running ? (
                  <>
                    <Button size="sm" variant="secondary" disabled={!!busy} onClick={() => void run("stop", () => api.dashboardStop(serverId))}>
                      {busy === "stop" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
                      {t("dash.stop")}
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => void api.dashboardOpen(serverId).catch((e) => setError(asUiError(e)))}>{t("dash.openBrowser")}</Button>
                  </>
                ) : (
                  <Button size="sm" variant="primary" disabled={!!busy} onClick={() => void run("start", () => api.dashboardStart(serverId))}>
                    {busy === "start" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
                    {t("dash.start")}
                  </Button>
                )}
                {!status.matches && status.squid_tag && (
                  <Button size="sm" disabled={!!busy} onClick={() => void install()}>
                    {busy === "install" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
                    {t("dash.update")}
                  </Button>
                )}
              </div>
            </>
          )}
          {busy === "install" && <p className="mt-3 text-sm text-muted" role="status">{step ?? t("bk.working")}</p>}
        </Card>
      )}

      {error && (
        <Card className="mt-4 border-bad/40 p-4" role="alert">
          <p className="selectable whitespace-pre-wrap break-words text-sm text-bad">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>
          <button className="mt-1 cursor-pointer text-xs text-muted underline" onClick={() => void navigator.clipboard.writeText(error.technical)}>{t("common.copyError")}</button>
        </Card>
      )}

      {status?.running && (
        <iframe
          key={status.url}
          title={t("dash.title")}
          src={status.url}
          className="mt-4 min-h-[480px] w-full flex-1 rounded-card border border-line bg-black/30"
        />
      )}
    </div>
  );
}
