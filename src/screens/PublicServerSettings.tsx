import { useEffect, useState } from "react";
import { Card } from "@/components/ui/card";
import { useI18n } from "@/i18n";
import { browse } from "@/lib/browse";
import { asUiError } from "@/lib/api";

export function PublicServerSettings() {
  const { t } = useI18n();
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { void browse.enabled().then(setEnabled).catch(e => setError(asUiError(e).technical)); }, []);
  async function change(next: boolean) {
    setBusy(true); setError(null);
    try { await browse.setEnabled(next); setEnabled(next); }
    catch (e) { setError(asUiError(e).technical); }
    finally { setBusy(false); }
  }
  return <Card className="mt-6 p-6">
    <label className="flex items-center gap-3 font-semibold"><input type="checkbox" checked={enabled === true} disabled={enabled === null || busy} onChange={e => void change(e.target.checked)} aria-describedby="public-server-hint" />{t("settings.publicServers")}</label>
    <p id="public-server-hint" className="mt-2 text-sm text-muted">{t("settings.publicServersHint")}</p>
    {error && <p role="alert" className="mt-2 text-sm text-bad">{error}</p>}
  </Card>;
}
