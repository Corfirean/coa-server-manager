import { useState } from "react";
import { api, asUiError, type DatabaseCheck } from "@/lib/api";
import { repairServer, useRepair, clearRepairResult } from "@/lib/repairJob";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useT } from "@/i18n";

export function RepairCard({ serverId }: { serverId: string }) {
  const t = useT();
  const job = useRepair(serverId);
  const [checking, setChecking] = useState(false);
  const [database, setDatabase] = useState<DatabaseCheck[] | null>(null);
  const [files, setFiles] = useState<{ path: string; kind: "missing" | "changed" | string }[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  async function check() {
    clearRepairResult(serverId);
    setChecking(true); setError(null); setDatabase(null); setFiles(null);
    try {
      setFiles(await api.verifyFiles(serverId));
      setDatabase(await api.checkDatabase(serverId));
    } catch (e) { setError(asUiError(e).technical); }
    finally { setChecking(false); }
  }
  const checks = job.result?.database ?? database;
  const busy = checking || job.busy;
  return <Card className="mt-6 p-6" aria-busy={busy}>
    <h2 className="font-semibold">{t("repair.title")}</h2>
    <p className="mt-2 text-sm text-muted">{t("repair.hint")}</p>
    <div className="mt-4 flex flex-wrap gap-2">
      <Button disabled={busy} onClick={() => void check()}>{checking ? t("repair.checking") : t("repair.check")}</Button>
      <Button disabled={busy} onClick={() => void repairServer(serverId)}>{job.busy ? t("repair.running", { n: job.percent }) : t("repair.run")}</Button>
    </div>
    {job.result && <div className="mt-3 text-sm" role="status">
      <p>{t("repair.result", { files: job.result.restored.length, sql: job.result.applied.length })}</p>
      <p className="selectable text-muted">{t("repair.backup", { id: job.result.backup })}</p>
      {job.result.error && <p className="mt-2 text-bad">{job.result.error}</p>}
    </div>}
    {files && !job.result && <p className="mt-3 text-sm">{t("repair.fileCount", { n: files.length })}</p>}
    {files && !job.result && files.length > 0 && (
      <ul className="selectable mt-2 max-h-52 overflow-auto text-sm text-muted">
        {files.slice(0, 200).map((f) => <li key={f.path}><span className={f.kind === "missing" ? "text-bad" : "text-warn"}>{t(f.kind === "missing" ? "repair.fileMissing" : "repair.fileChanged")}</span> {f.path}</li>)}
        {files.length > 200 && <li>…</li>}
      </ul>
    )}
    {checks?.map(c => {
      const pending = c.migrations.filter(m => m.status !== "applied");
      return <div key={c.realm} className="mt-4 border-t border-line pt-3 text-sm">
        <p className="font-medium">{c.realm === "coa" ? "Conquest of Azeroth" : "Wildcard"}</p>
        <p className={c.problems.length || pending.length ? "text-warn" : "text-ok"}>{t("repair.dbCount", { columns: c.problems.length, sql: pending.length })}</p>
        {!c.full_schema && <p className="mt-1 text-muted">{t("repair.limited")}</p>}
        <ul className="mt-2 max-h-52 overflow-auto selectable text-muted">
          {c.problems.map((p,i) => <li key={i}>{p.database}.{p.table}{p.column ? `.${p.column}` : ""}: {p.detail}</li>)}
          {pending.map(m => <li key={m.db + m.id}>{m.db}/{m.id}: {m.status}{m.error ? ` — ${m.error}` : ""}</li>)}
        </ul>
        {c.problems.length > 0 && <p className="mt-2 text-warn">{t("repair.unresolved")}</p>}
      </div>;
    })}
    {(error || job.error) && <p className="mt-3 text-sm text-bad" role="alert">{job.error ?? error}</p>}
  </Card>;
}
