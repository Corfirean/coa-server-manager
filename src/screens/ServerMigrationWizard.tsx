import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, ArrowRight, Check, Database, FolderOpen, Info, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { api, asUiError, type CopyPreview, type ServerSummary, type UiError } from "@/lib/api";
import { useHuman, useT } from "@/i18n";

/**
 * Moving a server to the modular core. A server with the classic core is never converted in place: a new server is
 * installed next to it and its data is copied there (`modular_copy`). This page shows what the old server holds and
 * what the copy takes, then hands over to the same install screen "Manage another server" uses for a modular copy.
 */
export function ServerMigrationWizard(props: {
  servers: ServerSummary[];
  currentName: string;
  onCopy: (path: string) => void;
}) {
  const t = useT();
  const human = useHuman();
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [info, setInfo] = useState<CopyPreview | null>(null);
  const [error, setError] = useState<UiError | null>(null);

  async function inspect(p: string) {
    if (!p.trim()) return;
    setPath(p);
    setError(null);
    setInfo(null);
    setBusy(true);
    try {
      setInfo(await api.modularCopyPreview(p.trim()));
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  async function browse() {
    const picked = await open({ directory: true, multiple: false, title: t("migrate.dialogTitle") });
    if (typeof picked === "string") await inspect(picked);
  }

  const gb = (bytes: number) => (bytes / 1024 ** 3).toFixed(1);
  const count = (n: number | null) => (n === null ? t("migrate.countLater") : String(n));

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-5 px-8 py-8">
      <div>
        <h1 className="text-2xl font-semibold">{t("migrate.title")}</h1>
        <p className="mt-1 text-sm text-muted">{t("migrate.subtitle")}</p>
      </div>

      <Card className="flex flex-col gap-3 p-5">
        <h2 className="font-semibold">{t("migrate.source")}</h2>
        {props.servers.length > 0 && (
          <div className="flex flex-wrap gap-2">
            {props.servers.map((s) => (
              <Button key={s.id} variant="secondary" size="sm" disabled={busy} onClick={() => void inspect(s.path)}>
                {s.name}
              </Button>
            ))}
          </div>
        )}
        <div className="flex gap-2">
          <input
            className="flex-1 rounded-md border border-line bg-bg px-3 py-2 font-mono text-sm outline-none focus:border-gold"
            value={path}
            placeholder="C:\Games\CoA Server"
            onChange={(e) => setPath(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void inspect(path)}
          />
          <Button variant="secondary" onClick={() => void browse()} disabled={busy}>
            <FolderOpen className="mr-2 h-4 w-4" aria-hidden /> {t("migrate.browse")}
          </Button>
          <Button onClick={() => void inspect(path)} disabled={busy || !path.trim()}>
            {busy ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> : t("migrate.inspect")}
          </Button>
        </div>
        <p className="text-xs text-muted">{t("migrate.readOnly")}</p>
      </Card>

      {error && (
        <Card className="border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{human(error.human).title}</p>
          <p className="mt-1 text-sm text-muted">{human(error.human).message}</p>
        </Card>
      )}

      {info && (
        <Card className="flex flex-col gap-4 p-5">
          <div className="flex items-start gap-3">
            {info.classic ? (
              <Check className="mt-0.5 h-5 w-5 text-ok" aria-hidden />
            ) : (
              <Info className="mt-0.5 h-5 w-5 text-warn" aria-hidden />
            )}
            <div>
              <h2 className="font-semibold">
                {info.classic ? t("migrate.classic.title") : info.modular ? t("migrate.modular.title") : t("migrate.none.title")}
              </h2>
              <p className="text-sm text-muted">
                {info.classic ? t("migrate.classic.text") : info.modular ? t("migrate.modular.text") : t("migrate.none.text")}
              </p>
              <p className="selectable mt-1 break-all text-xs text-muted">{info.path}</p>
            </div>
          </div>

          {(info.classic || info.modular) && (
            <dl className="grid grid-cols-2 gap-3 text-sm sm:grid-cols-4">
              <Fact label={t("migrate.fact.version")} value={info.version ?? t("migrate.unknown")} />
              <Fact label={t("migrate.fact.database")} value={t(`migrate.db.${info.database}`)} />
              <Fact label={t("migrate.fact.accounts")} value={count(info.accounts)} />
              <Fact label={t("migrate.fact.characters")} value={count(info.characters)} />
            </dl>
          )}

          {info.classic && info.database === "missing" && (
            <p className="flex items-start gap-2 text-sm text-bad">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden /> {t("migrate.db.missingText")}
            </p>
          )}

          {info.classic && info.database !== "missing" && (
            <>
              <div>
                <h3 className="flex items-center gap-2 text-sm font-semibold">
                  <Database className="h-4 w-4" aria-hidden /> {t("migrate.copied")}
                </h3>
                <ul className="mt-2 list-disc space-y-1 pl-5 text-sm text-muted">
                  <li>
                    {info.schemas.length > 0
                      ? t("migrate.copied.schemas", { list: info.schemas.join(", ") })
                      : t("migrate.copied.allSchemas")}
                  </li>
                  <li>{t("migrate.copied.settings", { n: info.config_files.length })}</li>
                  <li>{t("migrate.copied.world")}</li>
                  <li>{t("migrate.copied.ports")}</li>
                  <li>{t("migrate.copied.modules")}</li>
                  {info.extra_worlds.length > 0 && (
                    <li>{t("migrate.copied.worlds", { list: info.extra_worlds.join(", ") })}</li>
                  )}
                </ul>
              </div>
              <div>
                <h3 className="text-sm font-semibold">{t("migrate.notCopied")}</h3>
                <ul className="mt-2 list-disc space-y-1 pl-5 text-sm text-muted">
                  <li>{t("migrate.notCopied.conf")}</li>
                  <li>{t("migrate.notCopied.merge", { name: props.currentName })}</li>
                </ul>
              </div>
              <div>
                <h3 className="flex items-center gap-2 text-sm font-semibold text-warn">
                  <AlertTriangle className="h-4 w-4" aria-hidden /> {t("migrate.risks")}
                </h3>
                <ul className="mt-2 list-disc space-y-1 pl-5 text-sm text-muted">
                  <li>{t("migrate.risk.oldKept")}</li>
                  <li>{t("migrate.risk.ports")}</li>
                  {info.database === "running" ? (
                    <li>{t("migrate.risk.running")}</li>
                  ) : (
                    <li>{t("migrate.risk.stopped", { gb: gb(info.database_bytes) })}</li>
                  )}
                  <li>{t("migrate.risk.build")}</li>
                </ul>
              </div>
              <div className="flex justify-end">
                <Button onClick={() => props.onCopy(info.path)}>
                  {t("migrate.start")} <ArrowRight className="ml-2 h-4 w-4" aria-hidden />
                </Button>
              </div>
            </>
          )}
        </Card>
      )}
    </div>
  );
}

function Fact(props: { label: string; value: string }) {
  return (
    <div className="rounded-md border border-line p-3">
      <dt className="text-xs text-muted">{props.label}</dt>
      <dd className="mt-1 font-mono text-sm">{props.value}</dd>
    </div>
  );
}
