import { useState } from "react";
import { Check, Loader2, TriangleAlert, X } from "lucide-react";
import { api, asUiError, type DiagCheck, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

const ICON = { ok: Check, warn: TriangleAlert, fail: X } as const;
const TONE = { ok: "text-ok", warn: "text-warn", fail: "text-bad" } as const;
const WORD = { ok: "OK", warn: "Attention", fail: "Problem" } as const;

export function DiagnosticsCard({ serverId }: { serverId: string }) {
  const [checks, setChecks] = useState<DiagCheck[] | null>(null);
  const [files, setFiles] = useState<{ path: string; kind: string }[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);

  async function run(label: string, fn: () => Promise<void>) {
    setBusy(label);
    setError(null);
    setNote(null);
    try {
      await fn();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(null);
    }
  }

  const problems = checks?.filter((c) => c.level !== "ok").length ?? 0;

  return (
    <Card className="mt-6 p-6">
      <h2 className="font-semibold">Diagnostics</h2>
      <p className="mt-1 text-sm text-muted">Looks for common problems. Nothing is changed.</p>
      <div className="mt-4 flex flex-wrap gap-2">
        <Button variant="primary" disabled={!!busy} onClick={() => void run("diag", async () => setChecks((await api.runDiagnostics(serverId)).checks))}>
          {busy === "diag" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          Run diagnostics
        </Button>
        <Button disabled={!!busy} onClick={() => void run("verify", async () => setFiles(await api.verifyFiles(serverId)))}>
          Check server files
        </Button>
        <Button variant="ghost" disabled={!!busy} onClick={() => void run("export", async () => setNote(`Saved for bug reports: ${await api.exportDiagnostics(serverId)}`))}>
          Export diagnostic package
        </Button>
      </div>

      {checks && (
        <div className="mt-4">
          <p className="font-medium" role="status">{problems === 0 ? "Everything looks good." : `${problems} thing${problems === 1 ? "" : "s"} to look at`}</p>
          <ul className="mt-2 divide-y divide-line text-sm">
            {checks.map((c) => {
              const Icon = ICON[c.level];
              return (
                <li key={c.id + c.title} className="flex items-start gap-3 py-2">
                  <Icon className={`mt-0.5 h-4 w-4 shrink-0 ${TONE[c.level]}`} aria-hidden />
                  <div>
                    <span className="font-medium">{c.title}</span>
                    <span className="sr-only"> — {WORD[c.level]}</span>
                    <p className="text-muted">{c.detail}</p>
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {files && (
        <div className="mt-4 border-t border-line pt-3 text-sm">
          {files.length === 0 ? (
            <p className="text-ok" role="status">All program files are intact.</p>
          ) : (
            <>
              <p className="text-warn">{files.length} program file(s) differ from what was installed. Your own files are never listed here. "Check for updates" restores missing ones.</p>
              <ul className="mt-2 max-h-40 overflow-auto">
                {files.map((f) => (
                  <li key={f.path} className="selectable py-0.5">
                    {f.path} <span className="text-muted">— {f.kind === "missing" ? "missing" : "changed"}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      )}
      {note && <p className="mt-3 text-sm text-ok" role="status">{note}</p>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : error.human.message}</p>}
    </Card>
  );
}
