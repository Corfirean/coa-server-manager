import { dismissClientJob, startClientUpdate, stopClientJob, syncClient, useClientJob } from "@/lib/clientJob";
import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, Loader2, X } from "lucide-react";
import { api, asUiError, type ClientInfo, type Human, type Performance, type Population, type ServerSummary, type ServiceStatus, type StatusView } from "@/lib/api";
import { cn, formatBytes, formatUptime } from "@/lib/utils";
import { useHuman, useT } from "@/i18n";
import { applyServerUpdate, useServerUpdate } from "@/lib/serverUpdate";
import { checkClient, useClientStatus } from "@/lib/clientUpdate";
import { ClientDialog, type ClientDialogMode } from "@/screens/ClientDialog";
import { RealmlistMenu } from "@/screens/RealmlistMenu";
import { RealmPicker } from "@/screens/RealmPicker";
import { CrashHistoryModal } from "@/screens/CrashHistoryModal";
import { useAutoRestart } from "@/lib/autoRestartPrefs";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

type Action = "starting" | "stopping" | "restarting" | null;
type Failure = { human: Human; technical: string };

/** World loop speed from the server's own timing report, with a simple smooth / busy / lagging verdict. */
function TickRate({ perf }: { perf: Performance }) {
  const t = useT();
  const level = perf.mean_ms <= 50 ? "good" : perf.mean_ms <= 100 ? "busy" : "lag";
  const tone = { good: "text-ok bg-ok/15", busy: "text-warn bg-warn/15", lag: "text-bad bg-bad/15" }[level];
  const bar = { good: "bg-ok", busy: "bg-warn", lag: "bg-bad" }[level];
  const fill = Math.max(4, Math.min(100, Math.round(100 - perf.mean_ms)));
  return (
    <div className="mt-4 rounded-md border border-line bg-black/20 p-3" title={t("overview.tickHint")}>
      <div className="flex items-center justify-between text-sm">
        <span className="text-muted">{t("overview.tick")}</span>
        <span className={`rounded px-2 py-0.5 text-xs font-medium ${tone}`}>{t(level === "good" ? "overview.tickGood" : level === "busy" ? "overview.tickBusy" : "overview.tickLag")}</span>
      </div>
      <div className="mt-1 flex items-baseline gap-2">
        <span className="text-3xl font-semibold tabular-nums">{Math.round(perf.ticks_per_sec)}</span>
        <span className="text-sm text-muted">{t("overview.tickUnit")}</span>
      </div>
      <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-white/10" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={fill} aria-label={t("overview.tick")}>
        <div className={`h-full rounded-full transition-[width] duration-500 ${bar}`} style={{ width: `${fill}%` }} />
      </div>
      <p className="mt-2 text-xs text-muted">{t("overview.tickDetail", { mean: perf.mean_ms, p95: perf.p95_ms, max: perf.max_ms })}</p>
    </div>
  );
}

function Row({ label, s }: { label: string; s: ServiceStatus }) {
  const t = useT();
  const running = s.state === "running";
  const starting = s.state === "starting";
  return (
    <div className="flex items-center justify-between py-2.5">
      <span>{label}</span>
      <span
        className={cn("flex items-center gap-2 text-sm", running ? "text-ok" : starting ? "text-warn" : "text-muted")}
        role="status"
      >
        <span
          aria-hidden
          className={cn(
            "h-2 w-2 rounded-full",
            running ? "bg-ok" : starting ? "animate-[pulse-dot_1.2s_ease-in-out_infinite] bg-warn" : "bg-muted/50",
          )}
        />
        {running ? t("status.running") : starting ? t("status.starting") : t("status.stopped")}
      </span>
    </div>
  );
}

export function Overview({ server, companions = true, onForget, onOpenUpdates, onRealmChanged, onReport }: { server: ServerSummary; /** the companions module is on */ companions?: boolean; onForget: () => Promise<void>; onOpenUpdates: () => void; onRealmChanged: () => void; /** open the problem report, which collects the diagnostics */ onReport: () => void }) {
  const t = useT();
  const human = useHuman();
  const [status, setStatus] = useState<StatusView | null>(null);
  // With both worlds started together, each realm gets its own card; the first realm owns the database and auth.
  const [realms, setRealms] = useState<{ first: string; second: string } | null>(null);
  const bothWorlds = !!status?.observed.secondary_world;
  useEffect(() => {
    if (!bothWorlds) { setRealms(null); return; }
    let alive = true;
    void api.realmProfiles(server.id).then((r) => {
      if (!alive) return;
      const name = (m: "coa" | "wildcard") => (m === "coa" ? "Conquest of Azeroth" : "Wildcard");
      setRealms({ first: name(r.active), second: name(r.active === "coa" ? "wildcard" : "coa") });
    }).catch(() => { if (alive) setRealms(null); });
    return () => { alive = false; };
  }, [bothWorlds, server.id]);
  const [action, setAction] = useState<Action>(null);
  const [realmBusy, setRealmBusy] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [showDetails, setShowDetails] = useState(false);
  const [pop, setPop] = useState<Population | null>(null);
  const [client, setClient] = useState<ClientInfo | null | undefined>(undefined);
  const [dialog, setDialog] = useState<ClientDialogMode | null>(null);
  const clientStatus = useClientStatus(server.id).status;
  const [playing, setPlaying] = useState(false);
  const job = useClientJob(server.id);
  const jobBusy = job.phase === "scanning" || job.phase === "working";
  const jobPct = job.step && job.step.total > 0 ? Math.min(100, Math.floor((job.step.done / job.step.total) * 100)) : 0;
  const [keepMine, setKeepMine] = useState(true);
  const [perf, setPerf] = useState<Performance | null>(null);
  const upd = useServerUpdate(server.id);
  const needsDecision = (upd.preview?.conflicts.length ?? 0) > 0;
  const [startBots, setStartBots] = useState<number | null | undefined>(undefined);
  const [autoRestart, setAutoRestart] = useAutoRestart(server.id);
  const autoRestartRef = useRef(autoRestart);
  autoRestartRef.current = autoRestart;

  const [crashCount, setCrashCount] = useState(0);
  const crashCountRef = useRef(0);
  const [showCrashModal, setShowCrashModal] = useState(false);
  const [hasCrashHistory, setHasCrashHistory] = useState(false);
  const [crashLoopPaused, setCrashLoopPaused] = useState(false);
  const [crashRestartNotice, setCrashRestartNotice] = useState<string | null>(null);

  const userStoppedRef = useRef(false);
  const wasWorldRunningRef = useRef(false);
  const actionRef = useRef<Action>(null);
  const crashTimestampsRef = useRef<number[]>([]);
  const pollRef = useRef<() => Promise<void>>(undefined);

  useEffect(() => {
    let active = true;
    void api
      .listCrashes(server.id, 1)
      .then((items) => {
        if (active && items.length > 0) setHasCrashHistory(true);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [server.id]);

  useEffect(() => {
    void api
      .settings(server.id, "bots")
      .then((v) => {
        const on = v.settings.find((s) => s.key === "CoaBots.AutoLoginOnStartup");
        const max = v.settings.find((s) => s.key === "CoaBots.AutoLogin.MaxCount");
        setStartBots(on?.value === true && typeof max?.value === "number" ? max.value : null);
      })
      .catch(() => setStartBots(undefined));
  }, [server.id]);
  const alive = useRef(true);

  useEffect(() => {
    if (job.phase === "done" || job.phase === "current") void api.clientInfo(server.id).then(setClient).catch(() => undefined);
  }, [job.phase, server.id]);

  const run = useCallback(
    async (kind: Exclude<Action, null>) => {
      if (kind === "stopping") {
        userStoppedRef.current = true;
      } else {
        userStoppedRef.current = false;
        setCrashLoopPaused(false);
        setCrashRestartNotice(null);
      }
      setFailure(null);
      setShowDetails(false);
      setAction(kind);
      actionRef.current = kind;
      try {
        if (kind !== "starting") {
          const out = await api.stop(server.id);
          if (!out.ok) throw { human: out.human, technical: out.output };
        }
        if (kind !== "stopping") {
          setAction("starting");
          actionRef.current = "starting";
          const out = await api.start(server.id);
          if (!out.ok) throw { human: out.human, technical: out.output };
        }
      } catch (e) {
        const ui = asUiError(e);
        setFailure({ human: ui.human ?? asUiError(String(e)).human, technical: ui.technical });
      } finally {
        setAction(null);
        actionRef.current = null;
        void pollRef.current?.();
      }
    },
    [server.id],
  );

  const poll = useCallback(async () => {
    try {
      const s = await api.status(server.id);
      if (alive.current) setStatus(s);
      const isWorldRunning = s.observed.world.state === "running";
      if (isWorldRunning) {
        wasWorldRunningRef.current = true;
        userStoppedRef.current = false;
        const p = await api.population(server.id).catch(() => null);
        if (alive.current) setPop(p);
      } else {
        if (alive.current) setPop(null);
        if (
          wasWorldRunningRef.current &&
          !userStoppedRef.current &&
          actionRef.current === null
        ) {
          wasWorldRunningRef.current = false;
          setHasCrashHistory(true);
          const now = Date.now();
          const recent = crashTimestampsRef.current.filter((ts) => now - ts < 180000);
          recent.push(now);
          crashTimestampsRef.current = recent;

          crashCountRef.current += 1;
          const currentCrashCount = crashCountRef.current;
          setCrashCount(currentCrashCount);

          if (recent.length >= 3) {
            setCrashLoopPaused(true);
            setCrashRestartNotice(null);
          } else if (autoRestartRef.current) {
            setCrashRestartNotice(t("crash.restartNotice", { n: currentCrashCount }));
            void run("starting");
          }
        }
      }
    } catch {
      /* transient; next poll retries */
    }
  }, [server.id, t, run]);

  useEffect(() => {
    pollRef.current = poll;
  }, [poll]);

  // The tick rate comes from the server console (one short connection per reading), so it is read less often.
  const worldUp = status?.observed.world.state === "running";
  useEffect(() => {
    if (!worldUp) {
      setPerf(null);
      return;
    }
    let live = true;
    const read = () => void api.performance(server.id).then((p) => live && setPerf(p)).catch(() => {});
    read();
    const iv = setInterval(read, 10000);
    return () => {
      live = false;
      clearInterval(iv);
    };
  }, [worldUp, server.id]);

  useEffect(() => {
    alive.current = true;
    void api.clientInfo(server.id).then(setClient).catch(() => setClient(null));
    void poll();
    const t = setInterval(poll, 2000);
    return () => {
      alive.current = false;
      clearInterval(t);
    };
  }, [poll]);

  async function play() {
    setFailure(null);
    setShowDetails(false);
    setPlaying(true);
    try {
      const out = await api.play(server.id);
      if (!out.ok) throw { human: out.human, technical: out.output };
    } catch (e) {
      const ui = asUiError(e);
      setFailure({ human: ui.human ?? asUiError(String(e)).human, technical: ui.technical });
    } finally {
      setPlaying(false);
      void poll();
    }
  }

  if (!status) return <p className="text-muted">{t("overview.checking")}</p>;
  if (!status.path_exists) {
    return (
      <div className="max-w-xl">
        <h1 className="text-2xl font-semibold">{t("overview.missingTitle")}</h1>
        <p className="mt-2 text-muted">{t("overview.missingText", { path: server.path })}</p>
        <Button className="mt-4" onClick={() => void onForget()}>
          {t("overview.forget")}
        </Button>
      </div>
    );
  }

  const { mysql, auth, world, secondary_world } = status.observed;
  const all = [mysql, auth, world, ...(secondary_world ? [secondary_world] : [])];
  const running = all.every((s) => s.state === "running");
  const anyUp = all.some((s) => s.state !== "stopped");
  const transitioning = action !== null || status.busy || realmBusy;
  const conflict = all.find((s) => s.conflict)?.conflict ?? null;

  const headline = realmBusy ? t("realm.switching") : transitioning
    ? action === "stopping"
      ? t("status.stopping")
      : t("status.starting")
    : running
      ? t("status.online")
      : anyUp
        ? t("status.attention")
        : t("status.offline");
  const tone = transitioning ? "text-warn" : running ? "text-ok" : anyUp ? "text-warn" : "text-muted";

  return (
    <div className="w-full max-w-4xl">
      <h1 className="text-2xl font-semibold">{server.name}</h1>
      <p className="selectable mt-0.5 text-xs text-muted">{server.path}</p>
      <RealmPicker serverId={server.id} running={anyUp} disabled={transitioning || upd.applying} onBusy={setRealmBusy} onChanged={() => { onRealmChanged(); void poll(); }} />

      <Card className="mt-6 p-7">
        <div className="mb-1 text-xs font-medium uppercase tracking-[0.14em] text-muted">{realms ? `${realms.first} · ${t("overview.server")}` : t("overview.server")}</div>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className={cn("flex items-center gap-3 text-3xl font-semibold", tone)} role="status" aria-live="polite">
            <span
              aria-hidden
              className={cn("h-3 w-3 rounded-full", running && !transitioning ? "bg-ok" : transitioning || anyUp ? "bg-warn" : "bg-muted/50")}
            />
            {headline}
          </div>
          {(crashCount > 0 || hasCrashHistory) && (
            <button
              type="button"
              onClick={() => setShowCrashModal(true)}
              className={cn(
                "inline-flex cursor-pointer items-center gap-1.5 rounded-full px-3 py-1 text-xs font-medium transition-colors",
                crashCount > 0
                  ? "border border-bad/40 bg-bad/15 text-bad hover:bg-bad/25"
                  : "border border-line bg-card-2 text-muted hover:text-ink hover:border-gold/40",
              )}
              title={t("crash.historyTitle")}
            >
              <AlertTriangle className="h-3.5 w-3.5" aria-hidden />
              <span>{crashCount > 0 ? t("crash.badge", { n: crashCount }) : t("crash.historyTitle")}</span>
            </button>
          )}
        </div>
        {crashRestartNotice && !crashLoopPaused && (
          <div className="mt-3 flex items-center justify-between rounded-md border border-amber-500/30 bg-amber-500/10 p-3 text-xs text-amber-300">
            <span>{crashRestartNotice}</span>
            <button
              type="button"
              onClick={() => setCrashRestartNotice(null)}
              className="cursor-pointer text-amber-300/70 hover:text-amber-300"
              aria-label={t("btn.close")}
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        )}
        {crashLoopPaused && (
          <div className="mt-3 flex items-center justify-between rounded-md border border-bad/40 bg-bad/15 p-3 text-xs text-bad">
            <span>{t("crash.loopPaused")}</span>
            <button
              type="button"
              onClick={() => setShowCrashModal(true)}
              className="cursor-pointer font-medium underline"
            >
              {t("crash.historyTitle")}
            </button>
          </div>
        )}
        {!transitioning && !running && anyUp && (
          <p className="mt-2 text-sm text-muted">
            {t("overview.reportHint")}{" "}
            <button onClick={onReport} className="cursor-pointer text-gold underline hover:text-ink">{t("overview.reportBtn")}</button>
          </p>
        )}

        <div className="mt-5 divide-y divide-line border-y border-line">
          <Row label={t("overview.database")} s={mysql} />
          <Row label={t("overview.auth")} s={auth} />
          <Row label={t("overview.world")} s={world} />
        </div>

        <dl className="mt-4 grid grid-cols-2 gap-4 text-sm">
          <div>
            <dt className="text-muted">{t("overview.uptime")}</dt>
            <dd className="mt-0.5 text-lg">{world.state === "running" ? formatUptime(world.uptime_secs) : "—"}</dd>
          </div>
          <div>
            <dt className="text-muted">{t("overview.players")}</dt>
            <dd className="mt-0.5 text-lg">{pop ? `${pop.players_online}` : "—"}{companions && pop && pop.bots_online > 0 ? <span className="ml-2 text-sm text-muted">{t("overview.companions", { n: pop.bots_online })}</span> : null}</dd>
          </div>
        </dl>
        {companions && startBots !== undefined && (
          <p className="mt-3 text-xs text-muted">{startBots === null ? t("overview.startBotsOff") : t("overview.startBots", { n: startBots })}</p>
        )}
        {perf && <TickRate perf={perf} />}

        <div className="mt-7 flex flex-wrap items-center gap-x-4 gap-y-3">
          {running ? (
            <Button variant="secondary" size="xl" disabled={transitioning} onClick={() => run("stopping")} className="min-w-56">
              {transitioning && <Loader2 className="h-5 w-5 animate-spin" aria-hidden />}
              {t("btn.stop")}
            </Button>
          ) : upd.available ? (
            <Button
              variant="primary"
              size="xl"
              disabled={transitioning || upd.applying}
              onClick={() => (needsDecision ? onOpenUpdates() : void applyServerUpdate(server.id))}
              className="min-w-56"
            >
              {upd.applying && <Loader2 className="h-5 w-5 animate-spin" aria-hidden />}
              {upd.applying ? t("btn.updating") : needsDecision ? t("overview.openUpdates") : t("btn.update")}
            </Button>
          ) : (
            <Button variant="primary" size="xl" disabled={transitioning} onClick={() => run("starting")} className="min-w-56">
              {transitioning && <Loader2 className="h-5 w-5 animate-spin" aria-hidden />}
              {transitioning ? (action === "stopping" ? t("btn.stoppingCaps") : t("btn.startingCaps")) : t("btn.start")}
            </Button>
          )}
          {!running && upd.available && (
            <Button variant="secondary" size="sm" disabled={transitioning || upd.applying} onClick={() => run("starting")}>
              {t("btn.startWithoutUpdate")}
            </Button>
          )}
          {running && (
            <Button variant="ghost" size="sm" disabled={transitioning || upd.applying} onClick={() => run("restarting")}>
              {t("btn.restart")}
            </Button>
          )}
          {running && upd.available && (
            <Button variant="secondary" size="sm" disabled={transitioning || upd.applying} onClick={() => (needsDecision ? onOpenUpdates() : void applyServerUpdate(server.id))}>
              {upd.applying && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
              {needsDecision ? t("overview.openUpdates") : t("btn.updateTo", { v: upd.preview?.to_version ?? "" })}
            </Button>
          )}
          {anyUp && !running && (
            <Button variant="ghost" size="sm" disabled={transitioning} onClick={() => run("stopping")}>
              {t("btn.stopSmall")}
            </Button>
          )}
          <div className="ml-auto flex max-w-full flex-wrap items-center justify-end gap-3">
          {client && <RealmlistMenu serverId={server.id} />}
          {client && jobBusy ? (
            <div
              className="relative flex h-16 min-w-64 items-center overflow-hidden rounded-md border border-gold/50 bg-card-2"
              role="progressbar"
              aria-valuenow={jobPct}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-label={job.phase === "scanning" ? t("client.job.checking") : t("client.job.updating")}
            >
              <div className="absolute inset-y-0 left-0 bg-gold/35 transition-[width] duration-300" style={{ width: `${jobPct}%` }} />
              <div className="relative z-10 flex min-w-0 flex-1 flex-col px-5 leading-tight">
                <span className="text-sm font-semibold uppercase tracking-wide">{job.phase === "scanning" ? t("client.job.checking") : t("client.job.updating")}</span>
                <span className="truncate text-xs text-muted">
                  {jobPct}%
                  {job.step && job.step.phase === "download" && job.step.bytes_per_sec > 0 ? ` · ${t("client.dl.speed", { speed: formatBytes(job.step.bytes_per_sec) })}` : ""}
                </span>
              </div>
              <button
                type="button"
                onClick={() => stopClientJob(server.id)}
                title={t("client.dl.cancel")}
                aria-label={t("client.dl.cancel")}
                className="relative z-10 flex h-full w-12 cursor-pointer items-center justify-center text-muted hover:text-ink"
              >
                <X className="h-4 w-4" aria-hidden />
              </button>
            </div>
          ) : client && (clientStatus?.update_available || job.phase === "choose") ? (
            <Button variant="primary" size="xl" disabled={playing || job.phase === "choose"} onClick={() => void startClientUpdate(server.id)} className="min-w-40">
              {t("btn.updateClient")}
            </Button>
          ) : client ? (
            <Button variant="primary" size="xl" disabled={realmBusy || playing} onClick={() => void play()} className="min-w-40">
              {playing && <Loader2 className="h-5 w-5 animate-spin" aria-hidden />}
              {t("btn.play")}
            </Button>
          ) : client === null ? (
            <Button variant="secondary" size="xl" onClick={() => setDialog("setup")} className="min-w-40">
              {t("btn.setupClient")}
            </Button>
          ) : null}
          {client && !jobBusy && (clientStatus?.update_available || job.phase === "choose") && (
            <Button variant="secondary" size="sm" disabled={realmBusy || playing || upd.applying} onClick={() => void play()}>
              {t("btn.playWithoutUpdate")}
            </Button>
          )}
          </div>
        </div>
        <div className="mt-6 flex flex-wrap items-center justify-between gap-3 border-t border-line/60 pt-4 text-xs text-muted">
          <label className="flex cursor-pointer items-center gap-2">
            <input
              type="checkbox"
              checked={autoRestart}
              onChange={(e) => setAutoRestart(e.target.checked)}
              className="h-3.5 w-3.5 accent-[#c9a24a]"
            />
            <span className="font-medium text-ink/80">{t("crash.autoRestart")}</span>
            <span className="hidden sm:inline text-muted">— {t("crash.autoRestartDesc")}</span>
          </label>
          {(crashCount > 0 || hasCrashHistory) && (
            <button
              type="button"
              onClick={() => setShowCrashModal(true)}
              className="cursor-pointer text-gold hover:underline"
            >
              {t("crash.historyTitle")}
            </button>
          )}
        </div>
        {anyUp && !running && !transitioning && (
          <p className="mt-3 text-sm text-muted">{t("overview.partial")}</p>
        )}
        {job.phase === "choose" && job.plan && (
          <div className="mt-4 rounded-md border border-warn/40 bg-warn/5 p-3 text-sm">
            <p className="font-medium">{t("client.upd.modifiedTitle", { n: job.plan.items.filter((i) => i.kind === "modified").length })}</p>
            <p className="mt-1 text-muted">{t("client.upd.modifiedText")}</p>
            <label className="mt-3 flex cursor-pointer items-center gap-2">
              <input type="checkbox" checked={keepMine} onChange={(e) => setKeepMine(e.target.checked)} className="h-4 w-4 accent-[#c9a24a]" />
              <span>{t("client.upd.keep")}</span>
            </label>
            <div className="mt-3 flex gap-2">
              <Button size="sm" variant="primary" onClick={() => void syncClient(server.id, keepMine)}>{t("client.upd.start")}</Button>
              <Button size="sm" variant="ghost" onClick={() => dismissClientJob(server.id)}>{t("client.upd.later")}</Button>
            </div>
          </div>
        )}
        {job.phase === "done" && <p className="mt-3 text-sm text-ok" role="status">{t("client.upd.done")}</p>}
        {job.phase === "current" && <p className="mt-3 text-sm text-ok" role="status">{t("client.upd.upToDate", { v: job.plan?.version ?? "" })}</p>}
        {job.phase === "stopped" && (
          <p className="mt-3 flex items-center gap-3 text-sm text-muted" role="status">
            {t("client.dl.cancelled")}
            <Button size="sm" variant="ghost" onClick={() => void startClientUpdate(server.id)}>{t("client.dl.resume")}</Button>
          </p>
        )}
        {job.phase === "failed" && job.error && (
          <p className="mt-3 flex flex-wrap items-center gap-3 text-sm text-bad" role="alert">
            {job.error.human.code === "unknown" ? job.error.technical : human(job.error.human).message}
            <Button size="sm" variant="ghost" onClick={() => void startClientUpdate(server.id)}>{t("client.dl.retry")}</Button>
          </p>
        )}
        {upd.available && upd.preview && (
          <div className="mt-4 rounded-md border border-gold/40 bg-gold/5 p-3 text-sm">
            <p>{t("overview.updateAvail", { to: upd.preview.to_version, from: upd.preview.from_version ?? "?" })}</p>
            {needsDecision && <p className="mt-1 text-muted">{t("overview.updateConflicts")}</p>}
            {upd.applying && (
              <div className="mt-2">
                <p className="text-muted" role="status">{t("overview.updatingNote")}</p>
                <div className="mt-2 flex justify-between text-xs">
                  <span>{upd.step ?? ""}</span>
                  <span className="text-muted">{upd.percent}%</span>
                </div>
                <div className="mt-1 h-1.5 overflow-hidden rounded-full bg-white/10" role="progressbar" aria-valuenow={upd.percent} aria-valuemin={0} aria-valuemax={100}>
                  <div className="h-full rounded-full bg-gold transition-[width] duration-300" style={{ width: `${upd.percent}%` }} />
                </div>
              </div>
            )}
          </div>
        )}
      </Card>

      {secondary_world && (
        <Card className="mt-4 p-7">
          <div className="mb-1 text-xs font-medium uppercase tracking-[0.14em] text-muted">{realms ? `${realms.second} · ${t("overview.server")}` : t("realm.secondWorld")}</div>
          <div
            className={cn("flex items-center gap-3 text-3xl font-semibold", secondary_world.state === "running" ? "text-ok" : secondary_world.state === "starting" || transitioning ? "text-warn" : "text-muted")}
            role="status"
            aria-live="polite"
          >
            <span aria-hidden className={cn("h-3 w-3 rounded-full", secondary_world.state === "running" ? "bg-ok" : secondary_world.state === "starting" || transitioning ? "bg-warn" : "bg-muted/50")} />
            {secondary_world.state === "running" ? t("status.online") : secondary_world.state === "starting" ? t("status.starting") : t("status.offline")}
          </div>
          <div className="mt-5 divide-y divide-line border-y border-line">
            <Row label={t("overview.world")} s={secondary_world} />
          </div>
          <dl className="mt-4 text-sm">
            <dt className="text-muted">{t("overview.uptime")}</dt>
            <dd className="mt-0.5 text-lg">{secondary_world.state === "running" ? formatUptime(secondary_world.uptime_secs) : "—"}</dd>
          </dl>
          <p className="mt-3 text-xs text-muted">{t("realm.bothRunning")}</p>
        </Card>
      )}

      {conflict && !anyUp && (
        <Card className="mt-4 border-warn/40 p-4" role="alert">
          <p className="font-medium text-warn">{t("overview.portTitle")}</p>
          <p className="mt-1 text-sm text-muted">
            {t("overview.portText", { port: conflict.port, exe: conflict.exe ? ` (${conflict.exe})` : "" })}
          </p>
        </Card>
      )}

      {failure && (
        <Card className="mt-4 border-bad/40 p-5" role="alert">
          <p className="font-medium text-bad">{human(failure.human).title}</p>
          <p className="mt-1 text-sm text-muted">{human(failure.human).message}</p>
          <div className="mt-3 flex gap-2">
            <Button size="sm" onClick={() => setShowDetails((v) => !v)}>
              {showDetails ? t("common.hideDetails") : t("common.viewDetails")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void navigator.clipboard.writeText(failure.technical)}>
              {t("common.copyError")}
            </Button>
          </div>
          {showDetails && (
            <pre className="mt-3 max-h-56 overflow-auto whitespace-pre-wrap rounded bg-black/40 p-3 text-xs text-muted">
              {failure.technical || t("common.noOutput")}
            </pre>
          )}
        </Card>
      )}

      {dialog && (
        <ClientDialog
          serverId={server.id}
          mode={dialog}
          onClose={() => setDialog(null)}
          onChanged={() => {
            void api.clientInfo(server.id).then(setClient).catch(() => setClient(null));
            void checkClient(server.id);
          }}
          onPlayAnyway={() => void play()}
        />
      )}

      {showCrashModal && (
        <CrashHistoryModal
          serverId={server.id}
          onClose={() => setShowCrashModal(false)}
        />
      )}
    </div>
  );
}
