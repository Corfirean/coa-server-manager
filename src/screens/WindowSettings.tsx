import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card } from "@/components/ui/card";
import { useI18n } from "@/i18n";
import { asUiError } from "@/lib/api";

export function WindowSettings() {
  const { t } = useI18n();
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { void invoke<boolean>("close_to_tray").then(setEnabled).catch(e => setError(asUiError(e).technical)); }, []);
  async function change(next: boolean) {
    setBusy(true); setError(null);
    try { await invoke("set_close_to_tray", { enabled: next }); setEnabled(next); }
    catch (e) { setError(asUiError(e).technical); }
    finally { setBusy(false); }
  }
  return <Card className="mt-6 p-6">
    <label className="flex items-center gap-3 font-semibold"><input type="checkbox" checked={enabled === true} disabled={enabled === null || busy} onChange={e => void change(e.target.checked)} aria-describedby="close-window-hint" />{t("settings.closeToTray")}</label>
    <p id="close-window-hint" className="mt-2 text-sm text-muted">{t("settings.closeToTrayHint")}</p>
    {error && <p role="alert" className="mt-2 text-sm text-bad">{error}</p>}
  </Card>;
}
