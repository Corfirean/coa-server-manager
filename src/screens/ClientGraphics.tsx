import { useEffect, useState } from "react";
import { api, asUiError } from "@/lib/api";
import { checkClient } from "@/lib/clientUpdate";
import { useI18n } from "@/i18n";

export function ClientGraphics({ serverId }: { serverId: string }) {
  const { t } = useI18n();
  const [state, setState] = useState<{ dxvk_disabled: boolean; enhancements_disabled: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { void api.clientGraphics(serverId).then(setState).catch(e => setError(asUiError(e).technical)); }, [serverId]);
  const [clearCache, setClearCache] = useState<boolean | null>(null);
  useEffect(() => { void api.clearGameCacheOnPlay().then(setClearCache).catch(() => setClearCache(true)); }, []);
  async function changeClearCache(on: boolean) {
    setClearCache(on);
    try { await api.setClearGameCacheOnPlay(on); } catch (e) { setClearCache(!on); setError(asUiError(e).technical); }
  }
  async function change(name: string, disabled: boolean) {
    setBusy(true); setError(null);
    try { setState(await api.clientSetGraphics(serverId, name, disabled)); await checkClient(serverId); }
    catch (e) { setError(asUiError(e).technical); }
    finally { setBusy(false); }
  }
  return <div className="mt-5 space-y-3 border-t border-line pt-4">
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={state?.dxvk_disabled ?? false} disabled={!state || busy} onChange={e => void change("d3d9.dll", e.target.checked)} />{t("client.graphics.dxvk")}</label>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={state?.enhancements_disabled ?? false} disabled={!state || busy} onChange={e => void change("dxgi.dll", e.target.checked)} />{t("client.graphics.enhancements")}</label>
    <p className="text-xs text-muted">{t("client.graphics.hint")}</p>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={clearCache ?? true} disabled={clearCache === null} onChange={e => void changeClearCache(e.target.checked)} />{t("client.clearCache")}</label>
    <p className="text-xs text-muted">{t("client.clearCacheHint")}</p>
    {error && <p className="text-sm text-bad" role="alert">{error}</p>}
  </div>;
}
