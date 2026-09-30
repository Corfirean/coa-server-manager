import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, Check, Minus } from "lucide-react";
import { api, asUiError, type Classification, type ScanReport, type ServerSummary, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

const HEADLINE: Record<Classification, { title: string; tone: string; text: string }> = {
  healthy: { title: "Existing server detected", tone: "text-ok", text: "Everything needed to run this server was found." },
  partial: {
    title: "Server partly found",
    tone: "text-warn",
    text: "Some parts are missing. You can still add it; nothing will be fixed automatically.",
  },
  "unknown-custom": {
    title: "Custom server detected",
    tone: "text-warn",
    text: "This build is not recognised. Some update and install features will be disabled.",
  },
  incompatible: {
    title: "This is not a server folder",
    tone: "text-bad",
    text: "Choose the folder that contains Core, Data and mysql (or your worldserver.exe).",
  },
};

export function ImportServer(props: {
  canCancel: boolean;
  onCancel: () => void;
  onAdded: (s: ServerSummary) => void | Promise<void>;
}) {
  const [report, setReport] = useState<ScanReport | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [busy, setBusy] = useState(false);

  async function choose() {
    setError(null);
    const picked = await open({ directory: true, multiple: false, title: "Choose your server folder" });
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
        <h1 className="text-2xl font-semibold">Add an existing server</h1>
        <p className="mt-1 text-muted">Pick the folder your server runs from. It will only be read.</p>
      </div>

      <div className="flex gap-3">
        <Button variant="secondary" onClick={choose} disabled={busy}>
          {report ? "Choose another folder" : "Choose folder…"}
        </Button>
        {props.canCancel && (
          <Button variant="ghost" onClick={props.onCancel}>
            Cancel
          </Button>
        )}
      </div>

      {busy && !report && <p className="text-muted">Looking at your server…</p>}

      {error && (
        <Card className="border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{error.human.title}</p>
          <p className="mt-1 text-sm text-muted">{error.human.message}</p>
          <details className="mt-2 text-xs text-muted">
            <summary className="cursor-pointer">Show technical details</summary>
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
              <h2 className="font-semibold">{head.title}</h2>
              <p className="text-sm text-muted">{head.text}</p>
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
                      <Check className="h-4 w-4" aria-hidden /> Detected
                    </span>
                  ) : (
                    <span className="flex items-center gap-1">
                      <Minus className="h-4 w-4" aria-hidden /> {it.key === "companions" ? "Not installed" : "Not found"}
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
            <p className="text-sm text-muted">No files will be changed during import.</p>
            <Button
              variant="primary"
              onClick={add}
              disabled={busy || report.classification === "incompatible"}
            >
              Add server
            </Button>
          </div>
        </Card>
      )}
    </main>
  );
}
