import { useState } from "react";
import { Check, Loader2, TriangleAlert, X } from "lucide-react";
import { api, asUiError, type DiagCheck, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { hasKey, useHuman, useI18n, type Key } from "@/i18n";

const ICON = { ok: Check, warn: TriangleAlert, fail: X } as const;
const TONE = { ok: "text-ok", warn: "text-warn", fail: "text-bad" } as const;
const WORD = { ok: "diag.word.ok", warn: "diag.word.warn", fail: "diag.word.fail" } as const;

type T = (k: Key, v?: Record<string, string | number>) => string;

/** The backend writes each check in English; recognise the known ones and show them in the current language. */
function checkText(t: T, c: DiagCheck): { title: string; detail: string } {
  const title = hasKey(`dg.title.${c.id}`) ? t(`dg.title.${c.id}` as Key) : c.title;
  const d = c.detail;
  const rules: [RegExp, (m: RegExpMatchArray) => string][] = [
    [/^All expected server parts were found\.$/, () => t("dg.files.ok")],
    [/^Some server parts are missing\.$/, () => t("dg.files.partial")],
    [/^This is a custom server build; some features are limited\.$/, () => t("dg.files.custom")],
    [/^This folder does not look like a CoA server\.$/, () => t("dg.files.bad")],
    [/^Running$/, () => t("dg.svc.running")],
    [/^Port (\d+) is used by another program \(process (\d+)\)\.$/, (m) => t("dg.svc.port", { port: m[1], pid: m[2] })],
    [/^Starting, not answering yet\.$/, () => t("dg.svc.starting")],
    [/^Not running\.$/, () => t("dg.svc.stopped")],
    [/^Configuration files can be read\.$/, () => t("dg.configs.ok")],
    [/^Cannot read: (.*)$/, (m) => t("dg.configs.bad", { files: m[1] })],
    [/^Unusable values for: (.*)$/, (m) => t("dg.values.bad", { keys: m[1] })],
    [/^(\d+) GB free$/, (m) => t("dg.disk.ok", { gb: m[1] })],
    [/^Only (\d+) GB free; backups and updates need room\.$/, (m) => t("dg.disk.low", { gb: m[1] })],
    [/^The server folder is writable\.$/, () => t("dg.perm.ok")],
    [/^The Manager cannot write to the server folder/, () => t("dg.perm.bad")],
    [/^The saved game folder was not found; choose it again in Settings\.$/, () => t("dg.client.missing")],
    [/^Database and server console are not reachable from the network\.$/, () => t("dg.exposure.ok")],
    [/^Reachable from the network: (.*)\.$/, (m) => t("dg.exposure.bad", { what: m[1] })],
  ];
  for (const [re, f] of rules) {
    const m = d.match(re);
    if (m) return { title, detail: f(m) };
  }
  return { title, detail: d };
}

export function DiagnosticsCard({ serverId }: { serverId: string }) {
  const { t, tn } = useI18n();
  const human = useHuman();
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
      <h2 className="font-semibold">{t("diag.title")}</h2>
      <p className="mt-1 text-sm text-muted">{t("diag.text")}</p>
      <div className="mt-4 flex flex-wrap gap-2">
        <Button variant="primary" disabled={!!busy} onClick={() => void run("diag", async () => setChecks((await api.runDiagnostics(serverId)).checks))}>
          {busy === "diag" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {t("diag.run")}
        </Button>
        <Button disabled={!!busy} onClick={() => void run("verify", async () => setFiles(await api.verifyFiles(serverId)))}>
          {t("diag.verify")}
        </Button>
        <Button variant="ghost" disabled={!!busy} onClick={() => void run("export", async () => setNote(t("diag.saved", { path: await api.exportDiagnostics(serverId) })))}>
          {t("diag.export")}
        </Button>
      </div>

      {checks && (
        <div className="mt-4">
          <p className="font-medium" role="status">{problems === 0 ? t("diag.allGood") : tn("diag.problems", problems)}</p>
          <ul className="mt-2 divide-y divide-line text-sm">
            {checks.map((c) => {
              const Icon = ICON[c.level];
              return (
                <li key={c.id + c.title} className="flex items-start gap-3 py-2">
                  <Icon className={`mt-0.5 h-4 w-4 shrink-0 ${TONE[c.level]}`} aria-hidden />
                  <div>
                    <span className="font-medium">{checkText(t, c).title}</span>
                    <span className="sr-only"> — {t(WORD[c.level])}</span>
                    <p className="text-muted">{checkText(t, c).detail}</p>
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
            <p className="text-ok" role="status">{t("diag.filesOk")}</p>
          ) : (
            <>
              <p className="text-warn">{t("diag.filesDiffer", { n: files.length })}</p>
              <ul className="mt-2 max-h-40 overflow-auto">
                {files.map((f) => (
                  <li key={f.path} className="selectable py-0.5">
                    {f.path} <span className="text-muted">— {f.kind === "missing" ? t("diag.fileMissing") : t("diag.fileChanged")}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      )}
      {note && <p className="mt-3 text-sm text-ok" role="status">{note}</p>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
    </Card>
  );
}
