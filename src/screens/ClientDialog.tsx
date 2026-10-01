import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { Check, Loader2 } from "lucide-react";
import { api, asUiError, type ClientDownloadCheck, type ClientPlan, type ClientStep, type UiError } from "@/lib/api";
import { formatBytes } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";

/**
 * - setup:  no client linked yet: pick an existing folder or download the client.
 * - update: a client the Manager keeps up to date has a newer version.
 * - track:  an existing client the Manager has not looked at yet: compare it once, then keep it up to date.
 */
export type ClientDialogMode = "setup" | "update" | "track";
type Phase = "choose" | "confirm" | "scanning" | "preview" | "working" | "done" | "stopped" | "failed";

export function ClientDialog(props: {
  serverId: string;
  mode: ClientDialogMode;
  onClose: () => void;
  /** The linked client or its version changed; the caller refreshes what it shows. */
  onChanged: () => void;
  /** Update mode only: start the game without updating. */
  onPlayAnyway?: () => void;
}) {
  const t = useT();
  const human = useHuman();
  const { serverId, mode } = props;
  const [phase, setPhase] = useState<Phase>(mode === "setup" ? "choose" : "scanning");
  const [step, setStep] = useState<ClientStep | null>(null);
  const [parent, setParent] = useState<string | null>(null);
  const [check, setCheck] = useState<ClientDownloadCheck | null>(null);
  const [plan, setPlan] = useState<ClientPlan | null>(null);
  const [keep, setKeep] = useState(true);
  const [error, setError] = useState<UiError | null>(null);
  const cancelled = useRef(false);
  const working = phase === "scanning" || phase === "working";

  useEffect(() => {
    let un: (() => void) | undefined;
    let gone = false;
    listen<ClientStep>("client-progress", (e) => setStep(e.payload))
      .then((u) => (gone ? u() : (un = u)))
      .catch(() => undefined);
    return () => {
      gone = true;
      un?.();
    };
  }, []);

  useEffect(() => {
    if (mode !== "setup") void startPlan();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !working) close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  function close() {
    props.onChanged();
    props.onClose();
  }

  function fail(e: unknown) {
    if (cancelled.current) {
      setPhase("stopped");
    } else {
      setError(asUiError(e));
      setPhase("failed");
    }
  }

  async function useExisting() {
    const picked = await open({ directory: true, multiple: false, title: t("client.dialogTitle") });
    if (typeof picked !== "string") return;
    setError(null);
    try {
      await api.setClient(serverId, picked);
      setPhase("done");
    } catch (e) {
      setError(asUiError(e));
      setPhase("failed");
    }
  }

  async function pickParent() {
    const picked = await open({ directory: true, multiple: false, title: t("client.dl.pickParent") });
    if (typeof picked !== "string") return;
    setError(null);
    try {
      setCheck(await api.clientDownloadCheck(picked));
      setParent(picked);
      setPhase("confirm");
    } catch (e) {
      setError(asUiError(e));
      setPhase("failed");
    }
  }

  async function startDownload() {
    if (!parent) return;
    cancelled.current = false;
    setStep(null);
    setPhase("working");
    try {
      await api.clientDownload(serverId, parent);
      setPhase("done");
    } catch (e) {
      fail(e);
    }
  }

  async function startPlan() {
    cancelled.current = false;
    setStep(null);
    setError(null);
    setPhase("scanning");
    try {
      const p = await api.clientPlan(serverId);
      setPlan(p);
      setKeep(p.items.some((i) => i.kind === "modified"));
      setPhase("preview");
    } catch (e) {
      fail(e);
    }
  }

  async function startSync() {
    cancelled.current = false;
    setStep(null);
    setPhase("working");
    try {
      await api.clientSync(serverId, keep);
      setPhase("done");
    } catch (e) {
      fail(e);
    }
  }

  function stop() {
    cancelled.current = true;
    void api.clientCancel();
  }

  const modified = plan?.items.filter((i) => i.kind === "modified") ?? [];
  const pct = step && step.total > 0 ? Math.min(100, Math.floor((step.done / step.total) * 100)) : 0;
  const title = mode === "setup" ? t("client.setup.title") : mode === "update" ? t("client.upd.title") : t("client.track.title");

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6" role="presentation">
      <Card className="max-h-full w-full max-w-lg overflow-y-auto p-6 shadow-2xl" role="dialog" aria-modal="true" aria-label={title}>
        <h2 className="text-lg font-semibold">{title}</h2>

        {phase === "choose" && (
          <>
            <p className="mt-1 text-sm text-muted">{t("client.setup.intro")}</p>
            <div className="mt-4 grid gap-3">
              <button type="button" onClick={() => void useExisting()} className="cursor-pointer rounded-md border border-line bg-card-2 p-4 text-left transition-colors hover:border-gold/50">
                <span className="block font-medium">{t("client.setup.have")}</span>
                <span className="mt-1 block text-sm text-muted">{t("client.setup.haveHint")}</span>
              </button>
              <button type="button" onClick={() => void pickParent()} className="cursor-pointer rounded-md border border-line bg-card-2 p-4 text-left transition-colors hover:border-gold/50">
                <span className="block font-medium">{t("client.setup.download")}</span>
                <span className="mt-1 block text-sm text-muted">{t("client.setup.downloadHint")}</span>
              </button>
            </div>
            <div className="mt-5 flex justify-end">
              <Button variant="ghost" onClick={close}>{t("btn.close")}</Button>
            </div>
          </>
        )}

        {phase === "confirm" && check && (
          <>
            <p className="mt-1 text-sm text-muted">{t("client.dl.confirmText", { v: check.version, size: formatBytes(check.needed_bytes) })}</p>
            <p className="selectable mt-3 break-all rounded-md bg-black/30 px-3 py-2 text-sm">{check.dest}</p>
            <p className={check.free_bytes < check.needed_bytes + 512 * 1024 * 1024 ? "mt-3 text-sm text-bad" : "mt-3 text-sm text-muted"}>
              {check.free_bytes < check.needed_bytes + 512 * 1024 * 1024
                ? t("client.dl.notEnough", { need: formatBytes(check.needed_bytes), free: formatBytes(check.free_bytes) })
                : t("client.dl.free", { free: formatBytes(check.free_bytes) })}
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <Button variant="ghost" onClick={() => void pickParent()}>{t("client.dl.otherFolder")}</Button>
              <Button variant="primary" disabled={check.free_bytes < check.needed_bytes + 512 * 1024 * 1024} onClick={() => void startDownload()}>
                {t("client.dl.start")}
              </Button>
            </div>
          </>
        )}

        {(phase === "scanning" || phase === "working") && (
          <div className="mt-4" role="status">
            <div className="flex items-center gap-2 text-sm">
              <Loader2 className="h-4 w-4 animate-spin text-gold" aria-hidden />
              <span>{phase === "scanning" || step?.phase === "scan" ? t("client.dl.scanning") : t("client.dl.downloading")}</span>
              <span className="ml-auto text-muted">{pct}%</span>
            </div>
            <div className="mt-2 h-2 overflow-hidden rounded-full bg-white/10" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
              <div className="h-full rounded-full bg-gold transition-[width] duration-300" style={{ width: `${pct}%` }} />
            </div>
            <div className="mt-2 flex justify-between gap-4 text-xs text-muted">
              <span className="min-w-0 truncate">{step?.file ?? ""}</span>
              <span className="shrink-0">
                {step ? t("client.dl.progress", { done: formatBytes(step.done), total: formatBytes(step.total) }) : ""}
                {step && step.phase === "download" && step.bytes_per_sec > 0 ? ` · ${t("client.dl.speed", { speed: formatBytes(step.bytes_per_sec) })}` : ""}
              </span>
            </div>
            <div className="mt-5 flex justify-end">
              <Button variant="ghost" onClick={stop}>{t("client.dl.cancel")}</Button>
            </div>
          </div>
        )}

        {phase === "preview" && plan && (
          <>
            {plan.items.length === 0 ? (
              <p className="mt-2 text-sm">{t("client.upd.upToDate", { v: plan.version })}</p>
            ) : (
              <>
                {mode === "update" && <p className="mt-2 text-sm">{t("client.upd.available", { v: plan.version })}</p>}
                <p className="mt-1 text-sm text-muted">{t("client.upd.summary", { n: plan.items.length, size: formatBytes(plan.download_bytes) })}</p>
              </>
            )}
            {mode === "track" && <p className="mt-3 text-sm text-muted">{t("client.track.text")}</p>}
            {modified.length > 0 && (
              <div className="mt-4 rounded-md border border-warn/40 bg-warn/5 p-3 text-sm">
                <p className="font-medium">{t("client.upd.modifiedTitle", { n: modified.length })}</p>
                <p className="mt-1 text-muted">{t("client.upd.modifiedText")}</p>
                <ul className="selectable mt-2 space-y-0.5 text-xs text-muted">
                  {modified.slice(0, 8).map((i) => (
                    <li key={i.path} className="truncate">{i.path}</li>
                  ))}
                  {modified.length > 8 && <li>{t("client.upd.more", { n: modified.length - 8 })}</li>}
                </ul>
                <label className="mt-3 flex cursor-pointer items-center gap-2">
                  <input type="checkbox" checked={keep} onChange={(e) => setKeep(e.target.checked)} className="h-4 w-4 accent-[#c9a24a]" />
                  <span>{t("client.upd.keep")}</span>
                </label>
              </div>
            )}
            <div className="mt-5 flex flex-wrap justify-end gap-2">
              {mode === "update" && props.onPlayAnyway && (
                <Button variant="ghost" onClick={() => { close(); props.onPlayAnyway?.(); }}>{t("client.upd.playAnyway")}</Button>
              )}
              <Button variant="ghost" onClick={close}>{t("client.upd.later")}</Button>
              <Button variant="primary" onClick={() => void startSync()}>
                {mode === "track" ? (plan.items.length === 0 ? t("client.track.start") : t("client.track.startUpdate")) : t("client.upd.start")}
              </Button>
            </div>
          </>
        )}

        {phase === "done" && (
          <>
            <div className="mt-4 flex items-center gap-3">
              <span className="flex h-9 w-9 items-center justify-center rounded-full bg-ok/15 text-ok"><Check className="h-5 w-5" aria-hidden /></span>
              <div>
                <p className="font-medium">{mode === "setup" ? t("client.dl.done") : t("client.upd.done")}</p>
                <p className="text-sm text-muted">{mode === "setup" ? t("client.dl.doneText") : t("client.upd.doneText")}</p>
              </div>
            </div>
            <div className="mt-5 flex justify-end">
              <Button variant="primary" onClick={close}>{t("btn.close")}</Button>
            </div>
          </>
        )}

        {phase === "stopped" && (
          <>
            <p className="mt-3 text-sm text-muted">{t("client.dl.cancelled")}</p>
            <div className="mt-5 flex justify-end gap-2">
              <Button variant="ghost" onClick={close}>{t("btn.close")}</Button>
              {mode === "setup" ? (
                parent && <Button variant="primary" onClick={() => void startDownload()}>{t("client.dl.resume")}</Button>
              ) : (
                <Button variant="primary" onClick={() => void startPlan()}>{t("client.dl.resume")}</Button>
              )}
            </div>
          </>
        )}

        {phase === "failed" && error && (
          <>
            <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>
            <div className="mt-5 flex justify-end gap-2">
              <Button variant="ghost" onClick={close}>{t("btn.close")}</Button>
              {mode === "setup" ? <Button onClick={() => { setError(null); setPhase("choose"); }}>{t("client.dl.back")}</Button> : <Button onClick={() => void startPlan()}>{t("client.dl.retry")}</Button>}
            </div>
          </>
        )}
      </Card>
    </div>
  );
}
