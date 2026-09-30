import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Loader2, ShieldCheck } from "lucide-react";
import { api, asUiError, type BackupKind, type RecoveryPoint, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

const TRIGGER: Record<string, string> = {
  manual: "Manual backup",
  automatic: "Automatic backup",
  "before-update": "Before update",
  "before-restore": "Safety copy before a restore",
  "before-migration": "Before database update",
  "before-bots": "Before installing companions",
  "before-repair": "Before repair",
  "before-dangerous-change": "Before a risky change",
};

const KIND: Record<BackupKind, string> = {
  quick: "Characters, accounts and settings",
  full: "Everything, including the world database",
  config: "Settings only",
  database: "Characters and accounts",
};

const DB_LABEL: Record<string, string> = { characters: "Characters", auth: "Accounts", world: "World data" };

function size(bytes: number): string {
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(1)} GB`;
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function when(iso: string): string {
  const d = new Date(iso);
  const today = new Date();
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === today.toDateString()) return `Today ${time}`;
  const y = new Date(today.getTime() - 86400000);
  if (d.toDateString() === y.toDateString()) return `Yesterday ${time}`;
  return `${d.toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" })} ${time}`;
}

export function BackupsPage({ serverId }: { serverId: string }) {
  const [points, setPoints] = useState<RecoveryPoint[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [step, setStep] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<RecoveryPoint | null>(null);
  const [running, setRunning] = useState(false);

  const load = useCallback(async () => {
    setPoints(await api.backups(serverId));
    const s = await api.status(serverId);
    setRunning(s.observed.world.state !== "stopped" || s.observed.auth.state !== "stopped");
  }, [serverId]);

  useEffect(() => {
    void load().catch((e) => setError(asUiError(e)));
    const un = listen<string>("backup-progress", (e) => setStep(e.payload));
    return () => {
      void un.then((f) => f());
    };
  }, [load]);

  async function run(label: string, fn: () => Promise<string | void>) {
    setBusy(label);
    setError(null);
    setNotice(null);
    setStep(null);
    try {
      const msg = await fn();
      if (msg) setNotice(msg);
      await load();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(null);
      setStep(null);
    }
  }

  const create = (kind: BackupKind) =>
    run("create", async () => {
      await api.createBackup(serverId, kind);
      return "Backup created.";
    });

  const last = points?.[0];

  return (
    <div className="max-w-3xl">
      <h1 className="text-2xl font-semibold">Backups</h1>
      <p className="mt-1 text-muted">Is my data safe?</p>

      <Card className="mt-6 p-6">
        <div className="flex items-center gap-3">
          <ShieldCheck className={last ? "h-6 w-6 text-ok" : "h-6 w-6 text-muted"} aria-hidden />
          <div>
            <p className="font-medium">{last ? `Last backup: ${when(last.created_at)}` : "No backups yet"}</p>
            <p className="text-sm text-muted">Backups are kept next to your server and never overwrite your live data.</p>
          </div>
        </div>
        <div className="mt-5 flex flex-wrap items-center gap-3">
          <Button variant="primary" disabled={!!busy} onClick={() => void create("quick")}>
            {busy === "create" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            Back up now
          </Button>
          <Button disabled={!!busy} onClick={() => void create("full")} title="Also saves the large world database">
            Full backup
          </Button>
          <Button variant="ghost" disabled={!!busy} onClick={() => void create("config")}>
            Settings only
          </Button>
          {busy === "create" && <span className="text-sm text-muted" role="status">{step ?? "Working…"}</span>}
        </div>
      </Card>

      {notice && <p className="mt-3 text-sm text-ok" role="status">{notice}</p>}
      {error && (
        <Card className="mt-4 border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{error.human.code === "unknown" ? "That did not work" : error.human.title}</p>
          <p className="mt-1 text-sm text-muted">{error.human.code === "unknown" ? error.technical : error.human.message}</p>
        </Card>
      )}

      {restoring && (
        <Card className="mt-4 border-gold/40 p-5">
          <p className="font-medium">Restore from {TRIGGER[restoring.trigger] ?? restoring.trigger}, {when(restoring.created_at)}</p>
          <p className="mt-1 text-sm text-muted">
            Nothing is deleted. Your current data is saved first, and a replaced database is kept alongside under a new name.
          </p>
          {running && <p className="mt-2 text-sm text-warn">Stop the server first — databases can only be restored while it is stopped.</p>}
          <div className="mt-4 flex flex-wrap gap-2">
            {restoring.components.map((c) =>
              c.name === "configs" ? (
                <Button
                  key={c.name}
                  size="sm"
                  disabled={!!busy}
                  onClick={() =>
                    void run("restore", async () => {
                      await api.restoreConfigs(serverId, restoring.id);
                      setRestoring(null);
                      return "Settings restored. A safety copy of the previous settings was saved.";
                    })
                  }
                >
                  Restore settings
                </Button>
              ) : (
                <Button
                  key={c.name}
                  size="sm"
                  disabled={!!busy || running}
                  onClick={() => {
                    if (!window.confirm(`Restore the ${DB_LABEL[c.name] ?? c.name} database from this backup? Progress made since ${when(restoring.created_at)} will be set aside (not deleted).`)) return;
                    void run("restore", async () => {
                      const r = await api.restoreDatabase(serverId, restoring.id, c.name);
                      setRestoring(null);
                      return `Restored ${r.tables_restored} tables. The previous data is kept as ${r.previous_schema}.`;
                    });
                  }}
                >
                  Restore {DB_LABEL[c.name]?.toLowerCase() ?? c.name}
                </Button>
              ),
            )}
            <Button size="sm" variant="ghost" onClick={() => setRestoring(null)}>
              Cancel
            </Button>
          </div>
        </Card>
      )}

      <div className="mt-6 divide-y divide-line rounded-card border border-line bg-card/60">
        {points === null && <p className="p-4 text-muted">Loading…</p>}
        {points?.length === 0 && <p className="p-4 text-muted">Your backups will appear here.</p>}
        {points?.map((p) => (
          <div key={p.id} className="flex items-center justify-between gap-4 p-4">
            <div>
              <p className="font-medium">{p.label && p.trigger === "manual" && p.label !== "e2e" ? p.label : (TRIGGER[p.trigger] ?? p.trigger)}</p>
              <p className="text-sm text-muted">
                {when(p.created_at)} · {KIND[p.kind]} · {size(p.components.reduce((a, c) => a + c.bytes, 0))}
              </p>
            </div>
            <div className="flex shrink-0 gap-1">
              <Button size="sm" disabled={!!busy} onClick={() => setRestoring(p)}>
                Restore
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={!!busy}
                onClick={() =>
                  void run("verify", async () => {
                    const v = await api.verifyBackup(serverId, p.id);
                    return v.ok ? "This backup is intact." : `Problem found: ${v.problems.join("; ")}`;
                  })
                }
              >
                Verify
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={!!busy}
                onClick={() => {
                  if (window.confirm("Delete this backup? Only this backup is removed; your server is not affected.")) {
                    void run("delete", async () => {
                      await api.deleteBackup(serverId, p.id);
                    });
                  }
                }}
              >
                Delete
              </Button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
