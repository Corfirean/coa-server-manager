import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, Check, Minus } from "lucide-react";
import { api, asUiError, type Classification, type ScanReport, type ServerSummary, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT, type Key } from "@/i18n";

const HEADLINE: Record<Classification, { title: Key; tone: string; text: Key }> = {
  healthy: { title: "import.healthy.title", tone: "text-ok", text: "import.healthy.text" },
  partial: { title: "import.partial.title", tone: "text-warn", text: "import.partial.text" },
  "unknown-custom": { title: "import.custom.title", tone: "text-warn", text: "import.custom.text" },
  incompatible: { title: "import.incompatible.title", tone: "text-bad", text: "import.incompatible.text" },
};

export function ImportServer(props: {
  canCancel: boolean;
  onCancel: () => void;
  onAdded: (s: ServerSummary) => void | Promise<void>;
}) {
  const t = useT();
  const human = useHuman();
  const [report, setReport] = useState<ScanReport | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [busy, setBusy] = useState(false);

  async function choose() {
    setError(null);
    const picked = await open({ directory: true, multiple: false, title: t("import.dialogTitle") });
    if (typeof picked !== "string") return;
    setBusy(true);
    setReport(null);
    try {
      setReport(await api.scan(picked));
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  async function add() {
    if (!report) return;
    setBusy(true);
    try {
      await props.onAdded(await api.add(report.path));
    } catch (e) {
      setError(asUiError(e));
      setBusy(false);
    }
  }

  const head = report ? HEADLINE[report.classification] : null;

  return (
    <main className="mx-auto flex h-full max-w-2xl flex-col justify-center gap-5 px-8 py-10">
      <div>
        <h1 className="text-2xl font-semibold">{t("import.title")}</h1>
        <p className="mt-1 text-muted">{t("import.subtitle")}</p>
      </div>

      <div className="flex gap-3">
        <Button variant="secondary" onClick={choose} disabled={busy}>
          {report ? t("import.chooseAnother") : t("import.choose")}
        </Button>
        {props.canCancel && (
          <Button variant="ghost" onClick={props.onCancel}>
            {t("common.cancel")}
          </Button>
        )}
      </div>

      {busy && !report && <p className="text-muted">{t("import.looking")}</p>}

      {error && (
        <Card className="border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{human(error.human).title}</p>
          <p className="mt-1 text-sm text-muted">{human(error.human).message}</p>
          <details className="mt-2 text-xs text-muted">
            <summary className="cursor-pointer">{t("import.techDetails")}</summary>
            <pre className="mt-1 whitespace-pre-wrap">{error.technical}</pre>
          </details>
        </Card>
      )}

      {report && head && (
        <Card className="p-5">
          <div className="flex items-start gap-3">
            {report.classification === "healthy" ? (
              <Check className={`mt-0.5 h-5 w-5 ${head.tone}`} aria-hidden />
            ) : (
              <AlertTriangle className={`mt-0.5 h-5 w-5 ${head.tone}`} aria-hidden />
            )}
            <div>
              <h2 className="font-semibold">{t(head.title)}</h2>
              <p className="text-sm text-muted">{t(head.text)}</p>
              <p className="selectable mt-1 break-all text-xs text-muted">{report.path}</p>
            </div>
          </div>

          <ul className="mt-4 divide-y divide-line text-sm">
            {report.items.map((it) => (
              <li key={it.key} className="flex items-center justify-between py-2">
                <span>{it.label}</span>
                <span className="flex items-center gap-2 text-muted">
                  {it.detail && <span className="text-xs">{it.detail}</span>}
                  {it.status === "found" ? (
                    <span className="flex items-center gap-1 text-ok">
                      <Check className="h-4 w-4" aria-hidden /> {t("import.detected")}
                    </span>
                  ) : (
                    <span className="flex items-center gap-1">
                      <Minus className="h-4 w-4" aria-hidden /> {it.key === "companions" ? t("import.notInstalled") : t("import.notFound")}
                    </span>
                  )}
                </span>
              </li>
            ))}
          </ul>

          {report.notes.map((n) => (
            <p key={n} className="mt-3 text-sm text-warn">
              {n}
            </p>
          ))}

          <div className="mt-5 flex items-center justify-between border-t border-line pt-4">
            <p className="text-sm text-muted">{t("import.noChanges")}</p>
            <Button
              variant="primary"
              onClick={add}
              disabled={busy || report.classification === "incompatible"}
            >
              {t("import.add")}
            </Button>
          </div>
        </Card>
      )}
    </main>
  );
}
