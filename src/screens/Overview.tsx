import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { api, asUiError, type ClientInfo, type Human, type Population, type ServerSummary, type ServiceStatus, type StatusView } from "@/lib/api";
import { cn, formatUptime } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

type Action = "starting" | "stopping" | "restarting" | null;
type Failure = { human: Human; technical: string };

function Row({ label, s }: { label: string; s: ServiceStatus }) {
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
        {running ? "Running" : starting ? "Starting…" : "Stopped"}
      </span>
    </div>
  );
}

export function Overview({ server, onForget }: { server: ServerSummary; onForget: () => Promise<void> }) {
  const [status, setStatus] = useState<StatusView | null>(null);
  const [action, setAction] = useState<Action>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [showDetails, setShowDetails] = useState(false);
  const [pop, setPop] = useState<Population | null>(null);
  const [client, setClient] = useState<ClientInfo | null>(null);
  const [playing, setPlaying] = useState(false);
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

  if (!status) return <p className="text-muted">Checking your server…</p>;
  if (!status.path_exists) {
    return (
      <div className="max-w-xl">
        <h1 className="text-2xl font-semibold">Server folder not found</h1>
        <p className="mt-2 text-muted">{server.path} is missing or was moved.</p>
        <Button className="mt-4" onClick={() => void onForget()}>
          Remove from Manager (no files are deleted)
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
      ? "Stopping…"
      : "Starting…"
    : running
      ? "Online"
      : anyUp
        ? "Needs attention"
        : "Offline";
  const tone = transitioning ? "text-warn" : running ? "text-ok" : anyUp ? "text-warn" : "text-muted";

  return (
    <div className="max-w-2xl">
      <h1 className="text-2xl font-semibold">{server.name}</h1>
      <p className="selectable mt-0.5 text-xs text-muted">{server.path}</p>

      <Card className="mt-6 p-7">
        <div className="mb-1 text-xs font-medium uppercase tracking-[0.14em] text-muted">Server</div>
        <div className={cn("flex items-center gap-3 text-3xl font-semibold", tone)} role="status" aria-live="polite">
          <span
            aria-hidden
            className={cn("h-3 w-3 rounded-full", running && !transitioning ? "bg-ok" : transitioning || anyUp ? "bg-warn" : "bg-muted/50")}
          />
          {headline}
        </div>

        <div className="mt-5 divide-y divide-line border-y border-line">
          <Row label="Database" s={mysql} />
          <Row label="Auth server" s={auth} />
          <Row label="World server" s={world} />
        </div>

        <dl className="mt-4 grid grid-cols-2 gap-4 text-sm">
          <div>
            <dt className="text-muted">Uptime</dt>
            <dd className="mt-0.5 text-lg">{world.state === "running" ? formatUptime(world.uptime_secs) : "—"}</dd>
          </div>
          <div>
            <dt className="text-muted">Players online</dt>
            <dd className="mt-0.5 text-lg">{pop ? `${pop.players_online}` : "—"}{pop && pop.bots_online > 0 ? <span className="ml-2 text-sm text-muted">+ {pop.bots_online} companions</span> : null}</dd>
          </div>
        </dl>

        <div className="mt-7 flex items-center gap-4">
          {running ? (
            <Button variant="secondary" size="xl" disabled={transitioning} onClick={() => run("stopping")} className="min-w-56">
              {transitioning && <Loader2 className="h-5 w-5 animate-spin" aria-hidden />}
              STOP SERVER
            </Button>
          ) : (
            <Button variant="primary" size="xl" disabled={transitioning} onClick={() => run("starting")} className="min-w-56">
              {transitioning && <Loader2 className="h-5 w-5 animate-spin" aria-hidden />}
              {transitioning ? (action === "stopping" ? "STOPPING…" : "STARTING…") : "START SERVER"}
            </Button>
          )}
          {running && (
            <Button variant="ghost" size="sm" disabled={transitioning} onClick={() => run("restarting")}>
              Restart
            </Button>
          )}
          {anyUp && !running && (
            <Button variant="ghost" size="sm" disabled={transitioning} onClick={() => run("stopping")}>
              Stop
            </Button>
          )}
          {client && (
            <Button variant={running ? "primary" : "secondary"} size="xl" disabled={transitioning || playing} onClick={() => void play()} className="ml-auto min-w-40">
              {playing && <Loader2 className="h-5 w-5 animate-spin" aria-hidden />}
              {running ? "PLAY" : "START & PLAY"}
            </Button>
          )}
        </div>
        {anyUp && !running && !transitioning && (
          <p className="mt-3 text-sm text-muted">Some parts are running. Press Start to bring up the rest, or Stop to shut everything down.</p>
        )}
      </Card>

      {conflict && !anyUp && (
        <Card className="mt-4 border-warn/40 p-4" role="alert">
          <p className="font-medium text-warn">A port this server needs is already in use</p>
          <p className="mt-1 text-sm text-muted">
            Port {conflict.port} is used by another program{conflict.exe ? ` (${conflict.exe})` : ""}.
          </p>
        </Card>
      )}

      {failure && (
        <Card className="mt-4 border-bad/40 p-5" role="alert">
          <p className="font-medium text-bad">{failure.human.title}</p>
          <p className="mt-1 text-sm text-muted">{failure.human.message}</p>
          <div className="mt-3 flex gap-2">
            <Button size="sm" onClick={() => setShowDetails((v) => !v)}>
              {showDetails ? "Hide details" : "View details"}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void navigator.clipboard.writeText(failure.technical)}>
              Copy error
            </Button>
          </div>
          {showDetails && (
            <pre className="mt-3 max-h-56 overflow-auto whitespace-pre-wrap rounded bg-black/40 p-3 text-xs text-muted">
              {failure.technical || "(no output)"}
            </pre>
          )}
        </Card>
      )}
    </div>
  );
}
