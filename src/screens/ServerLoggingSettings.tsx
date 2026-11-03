import { useState } from "react";
import { Card } from "@/components/ui/card";
import { api, asUiError } from "@/lib/api";
import { useT } from "@/i18n";

export function ServerLoggingSettings({ serverId }: { serverId: string }) {
  const t = useT();
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");
  async function apply(preset: "normal" | "errors" | "off") {
    setBusy(true); setSaved(false); setError("");
    try { await api.serverLoggingPreset(serverId, preset); setSaved(true); }
    catch (error) { setError(asUiError(error).technical); }
    finally { setBusy(false); }
  }
  return <Card className="mt-6 p-6">
    <h2 className="font-semibold">{t("serverLogs.title")}</h2>
    <p className="mt-2 text-sm text-muted">{t("serverLogs.description")}</p>
    <div className="mt-4 flex flex-wrap gap-2">
      {(["normal", "errors", "off"] as const).map(preset => <button key={preset} disabled={busy} onClick={() => apply(preset)} className="rounded-md border border-line px-3 py-2 text-sm hover:bg-white/5 disabled:opacity-50">{t(`serverLogs.${preset}`)}</button>)}
    </div>
    {saved && <p role="status" className="mt-3 text-sm text-ok">{t("serverLogs.saved")}</p>}
    {error && <p role="alert" className="mt-3 text-sm text-bad">{error}</p>}
  </Card>;
}
