import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { api, asUiError, type ClientInfo, type Human, type Performance, type Population, type ServerSummary, type ServiceStatus, type StatusView } from "@/lib/api";
import { cn, formatUptime } from "@/lib/utils";
import { useHuman, useT } from "@/i18n";
import { applyServerUpdate, useServerUpdate } from "@/lib/serverUpdate";
import { checkClient, useClientStatus } from "@/lib/clientUpdate";
import { ClientDialog, type ClientDialogMode } from "@/screens/ClientDialog";
import { RealmlistMenu } from "@/screens/RealmlistMenu";
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

export function Overview({ server, onForget, onOpenUpdates }: { server: ServerSummary; onForget: () => Promise<void>; onOpenUpdates: () => void }) {
  const t = useT();
  const human = useHuman();
  const [status, setStatus] = useState<StatusView | null>(null);
  const [action, setAction] = useState<Action>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [showDetails, setShowDetails] = useState(false);
  const [pop, setPop] = useState<Population | null>(null);
  const [client, setClient] = useState<ClientInfo | null | undefined>(undefined);
  const [dialog, setDialog] = useState<ClientDialogMode | null>(null);
  const clientStatus = useClientStatus(server.id).status;
  const [playing, setPlaying] = useState(false);
  const [perf, setPerf] = useState<Performance | null>(null);
  const upd = useServerUpdate(server.id);
  const needsDecision = (upd.preview?.conflicts.length ?? 0) > 0;
  const [startBots, setStartBots] = useState<number | null | undefined>(undefined);

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

  const poll = useCallback(async () => {
    try {
      const s = await api.status(server.id);
      if (alive.current) setStatus(s);
      if (s.observed.world.state === "running") {
        const p = await api.population(server.id).catch(() => null);
        if (alive.current) setPop(p);
      } else if (alive.current) setPop(null);
    } catch {
      /* transient; next poll retries */
    }
  }, [server.id]);

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

  async function run(kind: Exclude<Action, null>) {
    setFailure(null);
    setShowDetails(false);
    setAction(kind);
    try {
      if (kind !== "starting") {
        const out = await api.stop(server.id);
        if (!out.ok) throw { human: out.human, technical: out.output };
      }
      if (kind !== "stopping") {
        setAction("starting");
        const out = await api.start(server.id);
        if (!out.ok) throw { human: out.human, technical: out.output };
      }
    } catch (e) {
      const ui = asUiError(e);
      setFailure({ human: ui.human ?? asUiError(String(e)).human, technical: ui.technical });
    } finally {
      setAction(null);
      void poll();
    }
  }

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

  const { mysql, auth, world } = status.observed;
  const all = [mysql, auth, world];
  const running = all.every((s) => s.state === "running");
  const anyUp = all.some((s) => s.state !== "stopped");
  const transitioning = action !== null || status.busy;
  const conflict = all.find((s) => s.conflict)?.conflict ?? null;

  const headline = transitioning
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
    <div className="max-w-2xl">
      <h1 className="text-2xl font-semibold">{server.name}</h1>
      <p className="selectable mt-0.5 text-xs text-muted">{server.path}</p>

      <Card className="mt-6 p-7">
        <div className="mb-1 text-xs font-medium uppercase tracking-[0.14em] text-muted">{t("overview.server")}</div>
        <div className={cn("flex items-center gap-3 text-3xl font-semibold", tone)} role="status" aria-live="polite">
          <span
            aria-hidden
            className={cn("h-3 w-3 rounded-full", running && !transitioning ? "bg-ok" : transitioning || anyUp ? "bg-warn" : "bg-muted/50")}
          />
          {headline}
        </div>

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
            <dd className="mt-0.5 text-lg">{pop ? `${pop.players_online}` : "—"}{pop && pop.bots_online > 0 ? <span className="ml-2 text-sm text-muted">{t("overview.companions", { n: pop.bots_online })}</span> : null}</dd>
          </div>
        </dl>
        {startBots !== undefined && (
          <p className="mt-3 text-xs text-muted">{startBots === null ? t("overview.startBotsOff") : t("overview.startBots", { n: startBots })}</p>
        )}
        {perf && <TickRate perf={perf} />}

        <div className="mt-7 flex items-center gap-4">
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
          {client && <RealmlistMenu serverId={server.id} className="ml-auto" />}
          {client && clientStatus?.update_available ? (
            <Button variant="primary" size="xl" disabled={playing} onClick={() => setDialog("update")} className="min-w-40">
              {t("btn.updateClient")}
            </Button>
          ) : client ? (
            <Button variant={running ? "primary" : "secondary"} size="xl" disabled={transitioning || playing} onClick={() => void play()} className="min-w-40">
              {playing && <Loader2 className="h-5 w-5 animate-spin" aria-hidden />}
              {running ? t("btn.play") : t("btn.startPlay")}
            </Button>
          ) : client === null ? (
            <Button variant="secondary" size="xl" onClick={() => setDialog("setup")} className="ml-auto min-w-40">
              {t("btn.setupClient")}
            </Button>
          ) : null}
        </div>
        {anyUp && !running && !transitioning && (
          <p className="mt-3 text-sm text-muted">{t("overview.partial")}</p>
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
    </div>
  );
}
