import { useEffect, useMemo, useState } from "react";
import { Loader2 } from "lucide-react";
import { api, asUiError, type ReportContext, type UiError } from "@/lib/api";
import { MAX_LINK, issueUrl, reportBody } from "@/lib/report";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";

const field = "mt-1 block w-full rounded-md border border-line bg-bg px-3 py-2.5 text-[15px] outline-none focus:border-gold";

/** A problem report that becomes a prefilled GitHub issue. Nothing is sent from here: the person reviews and submits it on GitHub. */
export function ReportPage({ serverId }: { serverId: string }) {
  const t = useT();
  const human = useHuman();
  const [ctx, setCtx] = useState<ReportContext | null>(null);
  const [title, setTitle] = useState("");
  const [what, setWhat] = useState("");
  const [expected, setExpected] = useState("");
  const [steps, setSteps] = useState("");
  const [file, setFile] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [showPreview, setShowPreview] = useState(false);

  useEffect(() => {
    void api.reportContext(serverId).then(setCtx).catch(() => setCtx(null));
  }, [serverId]);

  const body = useMemo(
    () =>
      reportBody({
        managerVersion: ctx?.manager_version ?? "",
        windows: ctx?.windows ?? "",
        installKind: ctx?.install_kind ?? "new",
        serverVersion: ctx?.server_version ?? null,
        what,
        expected,
        steps,
        diagnostics: file !== null,
      }),
    [ctx, what, expected, steps, file],
  );
  const issueTitle = `[Bug] ${title.trim()}`;
  const ready = title.trim().length > 0 && what.trim().length > 0;

  async function makeFile() {
    setBusy(true);
    setError(null);
    try {
      setFile(await api.exportDiagnostics(serverId));
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  async function openIssue() {
    setError(null);
    setNote(null);
    try {
      const full = issueUrl(issueTitle, body);
      if (full.length <= MAX_LINK) {
        await api.openLink(full);
        setNote(t("rep.opened"));
      } else {
        await navigator.clipboard.writeText(body);
        await api.openLink(issueUrl(issueTitle, "Paste the report text here (it was copied to the clipboard by CoA Server Manager)."));
        setNote(t("rep.openedLong"));
      }
    } catch (e) {
      setError(asUiError(e));
    }
  }

  async function copy() {
    await navigator.clipboard.writeText(`${issueTitle}\n\n${body}`);
    setNote(t("rep.copied"));
  }

  return (
    <div className="max-w-2xl">
      <h1 className="text-2xl font-semibold">{t("rep.title")}</h1>
      <p className="mt-1 text-muted">{t("q.report")}</p>
      <p className="mt-3 text-sm text-muted">{t("rep.intro")}</p>

      <Card className="mt-6 p-6">
        <h2 className="font-semibold">{t("rep.autoTitle")}</h2>
        <dl className="mt-3 grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-sm">
          <dt className="text-muted">{t("rep.manager")}</dt>
          <dd>{ctx?.manager_version ?? "…"}</dd>
          <dt className="text-muted">{t("rep.windows")}</dt>
          <dd>{ctx ? ctx.windows || "—" : "…"}</dd>
          <dt className="text-muted">{t("rep.kind")}</dt>
          <dd>{ctx ? (ctx.install_kind === "new" ? t("rep.kindNew") : t("rep.kindImported")) : "…"}</dd>
          {ctx?.server_version && (
            <>
              <dt className="text-muted">{t("rep.server")}</dt>
              <dd>{ctx.server_version}</dd>
            </>
          )}
        </dl>
      </Card>

      <Card className="mt-6 p-6">
        <div>
          <label htmlFor="rep-title" className="text-sm text-muted">{t("rep.summary")}</label>
          <input id="rep-title" className={field} value={title} maxLength={100} onChange={(e) => setTitle(e.target.value)} placeholder={t("rep.summaryHint")} />
        </div>
        <div className="mt-4">
          <label htmlFor="rep-what" className="text-sm text-muted">{t("rep.what")}</label>
          <textarea id="rep-what" rows={4} className={field} value={what} onChange={(e) => setWhat(e.target.value)} placeholder={t("rep.whatHint")} />
        </div>
        <div className="mt-4">
          <label htmlFor="rep-exp" className="text-sm text-muted">{t("rep.expected")}</label>
          <textarea id="rep-exp" rows={2} className={field} value={expected} onChange={(e) => setExpected(e.target.value)} placeholder={t("rep.expectedHint")} />
        </div>
        <div className="mt-4">
          <label htmlFor="rep-steps" className="text-sm text-muted">{t("rep.steps")}</label>
          <textarea id="rep-steps" rows={4} className={field} value={steps} onChange={(e) => setSteps(e.target.value)} placeholder={"1.\n2.\n3."} />
        </div>
      </Card>

      <Card className="mt-6 p-6">
        <h2 className="font-semibold">{t("rep.attachTitle")}</h2>
        <p className="mt-1 text-sm text-muted">{t("rep.attachText")}</p>
        <Button className="mt-3" size="sm" disabled={busy} onClick={() => void makeFile()}>
          {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {t("rep.makeFile")}
        </Button>
        {file && <p className="selectable mt-3 break-all text-sm text-ok" role="status">{t("rep.fileSaved", { path: file })}</p>}
        <p className="mt-4 text-xs text-warn">{t("rep.privacy")}</p>
      </Card>

      <div className="mt-6 flex flex-wrap items-center gap-3">
        <Button variant="primary" disabled={!ready} onClick={() => void openIssue()}>{t("rep.open")}</Button>
        <Button variant="ghost" disabled={!ready} onClick={() => void copy()}>{t("rep.copy")}</Button>
        <button onClick={() => setShowPreview((v) => !v)} aria-expanded={showPreview} className="cursor-pointer text-sm text-muted underline hover:text-ink">
          {t("rep.preview")}
        </button>
      </div>
      {!ready && <p className="mt-2 text-xs text-muted">{t("rep.needFields")}</p>}
      <p className="mt-2 text-xs text-muted">{t("rep.githubNote")}</p>
      {note && <p className="mt-3 text-sm text-ok" role="status">{note}</p>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
      {showPreview && <pre className="selectable mt-3 max-h-80 overflow-auto whitespace-pre-wrap rounded bg-black/40 p-3 text-xs text-muted">{`${issueTitle}\n\n${body}`}</pre>}
    </div>
  );
}
