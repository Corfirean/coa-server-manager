import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Loader2, ShieldCheck } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, asUiError, type BackupKind, type BackupLocation, type RecoveryPoint, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useI18n, type Key, type Locale } from "@/i18n";

const TRIGGER_KEYS = ["manual", "automatic", "before-update", "before-restore", "before-migration", "before-bots", "before-repair", "before-dangerous-change"];

const DB_KEYS = ["characters", "auth", "world"];

type T = (k: Key, v?: Record<string, string | number>) => string;

function size(bytes: number): string {
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(1)} GB`;
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function when(iso: string, t: T, locale: Locale): string {
  const d = new Date(iso);
  const today = new Date();
  const time = d.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === today.toDateString()) return t("bk.today", { time });
  const y = new Date(today.getTime() - 86400000);
  if (d.toDateString() === y.toDateString()) return t("bk.yesterday", { time });
  return `${d.toLocaleDateString(locale, { day: "numeric", month: "short", year: "numeric" })} ${time}`;
}

export function BackupsPage({ serverId }: { serverId: string }) {
  const { t, locale } = useI18n();
  const human = useHuman();
  const w = (iso: string) => when(iso, t, locale);
  const trigger = (x: string) => (TRIGGER_KEYS.includes(x) ? t(`bk.trigger.${x}` as Key) : x);
  const dbLabel = (x: string) => {
    const [realm, kind] = x.split("-");
    if (kind && DB_KEYS.includes(kind)) return `${realm === "coa" ? "CoA" : "Wildcard"} · ${t(`bk.db.${kind}` as Key)}`;
    return DB_KEYS.includes(x) ? t(`bk.db.${x}` as Key) : x;
  };
  const [points, setPoints] = useState<RecoveryPoint[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [step, setStep] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<RecoveryPoint | null>(null);
  const [running, setRunning] = useState(false);
  const [location, setLocation] = useState<BackupLocation | null>(null);

  const load = useCallback(async () => {
    setPoints(await api.backups(serverId));
    setLocation(await api.backupLocation(serverId));
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
      return t("bk.created");
    });

  const last = points?.[0];

  return (
    <div className="max-w-3xl">
      <h1 className="text-2xl font-semibold">{t("bk.title")}</h1>
      <p className="mt-1 text-muted">{t("q.backups")}</p>

      <Card className="mt-6 p-6">
        <div className="flex items-center gap-3">
          <ShieldCheck className={last ? "h-6 w-6 text-ok" : "h-6 w-6 text-muted"} aria-hidden />
          <div>
            <p className="font-medium">{last ? t("bk.last", { when: w(last.created_at) }) : t("bk.none")}</p>
            <p className="text-sm text-muted">{t("bk.keptNext")}</p>
          </div>
        </div>
        <div className="mt-5 flex flex-wrap items-center gap-3">
          <Button variant="primary" disabled={!!busy} onClick={() => void create("quick")}>
            {busy === "create" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            {t("bk.backUpNow")}
          </Button>
          <Button disabled={!!busy} onClick={() => void create("full")} title={t("bk.fullHint")}>
            {t("bk.full")}
          </Button>
          <Button variant="ghost" disabled={!!busy} onClick={() => void create("config")}>
            {t("bk.settingsOnly")}
          </Button>
          {busy === "create" && <span className="text-sm text-muted" role="status">{step ?? t("bk.working")}</span>}
        </div>
      </Card>

      {location && (
        <Card className="mt-4 p-5">
          <p className="font-medium">{t("bk.folder")}</p>
          <p className="selectable mt-1 break-all text-sm text-muted">{location.path}{location.is_default ? ` (${t("bk.folderDefault")})` : ""}</p>
          <p className="mt-1 text-xs text-muted">{t("bk.folderNote")}</p>
          <div className="mt-3 flex flex-wrap gap-2">
            <Button size="sm" disabled={!!busy} onClick={() => void run("folder", async () => {
              const dir = await open({ directory: true, multiple: false, title: t("bk.folder") });
              if (typeof dir === "string") { await api.setBackupLocation(serverId, dir); return t("bk.folderChanged"); }
            })}>{t("bk.folderChange")}</Button>
            {!location.is_default && (
              <Button size="sm" variant="ghost" disabled={!!busy} onClick={() => void run("folder", async () => { await api.setBackupLocation(serverId, null); return t("bk.folderChanged"); })}>{t("bk.folderReset")}</Button>
            )}
          </div>
        </Card>
      )}

      {notice && <p className="mt-3 text-sm text-ok" role="status">{notice}</p>}
      {error && (
        <Card className="mt-4 border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{error.human.code === "unknown" ? t("bk.failed") : human(error.human).title}</p>
          <p className="mt-1 text-sm text-muted">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>
        </Card>
      )}

      {restoring && (
        <Card className="mt-4 border-gold/40 p-5">
          <p className="font-medium">{t("bk.restoreFrom", { what: trigger(restoring.trigger), when: w(restoring.created_at) })}</p>
          <p className="mt-1 text-sm text-muted">
            {t("bk.restoreNote")}
          </p>
          {running && <p className="mt-2 text-sm text-warn">{t("bk.stopFirst")}</p>}
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
                      return t("bk.settingsRestored");
                    })
                  }
                >
                  {t("bk.restoreSettings")}
                </Button>
              ) : (
                <Button
                  key={c.name}
                  size="sm"
                  disabled={!!busy || running}
                  onClick={() => {
                    if (!window.confirm(t("bk.confirmDb", { db: dbLabel(c.name), when: w(restoring.created_at) }))) return;
                    void run("restore", async () => {
                      const r = await api.restoreDatabase(serverId, restoring.id, c.name);
                      setRestoring(null);
                      return t("bk.dbRestored", { n: r.tables_restored, schema: r.previous_schema });
                    });
                  }}
                >
                  {t("bk.restoreDb", { db: dbLabel(c.name) })}
                </Button>
              ),
            )}
            <Button size="sm" variant="ghost" onClick={() => setRestoring(null)}>
              {t("common.cancel")}
            </Button>
          </div>
        </Card>
      )}

      <div className="mt-6 divide-y divide-line rounded-card border border-line bg-card/60">
        {points === null && <p className="p-4 text-muted">{t("bk.loading")}</p>}
        {points?.length === 0 && <p className="p-4 text-muted">{t("bk.empty")}</p>}
        {points?.map((p) => (
          <div key={p.id} className="flex items-center justify-between gap-4 p-4">
            <div>
              <p className="font-medium">{p.label && p.trigger === "manual" && p.label !== "e2e" ? p.label : trigger(p.trigger)}</p>
              <p className="text-sm text-muted">
                {w(p.created_at)} · {p.realm === "wildcard" ? "Wildcard" : "CoA"} · {t(`bk.kind.${p.kind}` as Key)} · {size(p.components.reduce((a, c) => a + c.bytes, p.mysql_snapshot?.bytes ?? 0))}
              </p>
            </div>
            <div className="flex shrink-0 gap-1">
              <Button size="sm" disabled={!!busy} onClick={() => setRestoring(p)}>
                {t("bk.restore")}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={!!busy}
                onClick={() =>
                  void run("verify", async () => {
                    const v = await api.verifyBackup(serverId, p.id);
                    return v.ok ? t("bk.intact") : t("bk.problem", { details: v.problems.join("; ") });
                  })
                }
              >
                {t("bk.verify")}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={!!busy}
                onClick={() => {
                  if (window.confirm(t("bk.confirmDelete"))) {
                    void run("delete", async () => {
                      await api.deleteBackup(serverId, p.id);
                    });
                  }
                }}
              >
                {t("bk.delete")}
              </Button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
